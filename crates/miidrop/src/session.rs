//! The Mii Maker exchange between two joined nodes: ready messages, a container and a verdict.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::container;
use crate::keys::Key;
use crate::link::{Link, LinkEvent};
use crate::mii::{self, Record};
use crate::prnn::{self, ACK, Header, RST, SendState, Sender};
use crate::radio::Radio;
use crate::uds::{BROADCAST_NODE, NodeInfo};
use crate::{Error, uds};

pub const READY: u32 = 0;
pub const RECEIVING: u32 = 1;
pub const ACCEPTED: u32 = 2;
pub const REJECTED: u32 = 5;
/// Second word a 3DS writes into its acceptance message.
pub const ACCEPTED_WORD: u32 = 0x50;

const GIVE_UP_MS: u64 = 30_000;
const MAX_WAIT: Duration = Duration::from_millis(20);
const SEND_RESTARTS: u32 = 3;

/// Channel a node opens bulk connections on; its control channel is two higher.
pub fn bulk_channel(node: u16) -> u16 {
    node
}

pub fn control_channel(node: u16) -> u16 {
    node + 2
}

pub fn control_message(kind: u32, word: u32) -> [u8; 8] {
    let mut m = [0u8; 8];
    m[..4].copy_from_slice(&kind.to_le_bytes());
    m[4..].copy_from_slice(&word.to_le_bytes());
    m
}

pub fn parse_control(b: &[u8]) -> Option<(u32, u32)> {
    let b: &[u8; 8] = b.get(..8)?.try_into().ok()?;
    Some((
        u32::from_le_bytes(b[..4].try_into().unwrap()),
        u32::from_le_bytes(b[4..].try_into().unwrap()),
    ))
}

#[derive(Debug)]
pub struct Received {
    pub container: Vec<u8>,
    pub record: Result<Record, Error>,
    /// Whether the peer was told the Mii was accepted.
    pub accepted: bool,
}

#[derive(Debug)]
pub enum Event {
    PeerReady,
    /// The peer accepted the connection carrying our Mii.
    PeerReceiving,
    /// The peer's verdict on our Mii.
    Verdict(bool),
    /// The peer started sending a Mii.
    Incoming,
    Received(Received),
    Nodes(Vec<NodeInfo>),
    /// A message could not be delivered.
    Failed(String),
    Left,
}

#[derive(Default)]
struct Inbound {
    rx: prnn::Receiver,
    data: Vec<u8>,
}

struct Bulk {
    sender: Sender,
    data: Vec<u8>,
    restarts: u32,
}

/// A joined network as the exchange sees it.
pub trait Transport {
    fn node_id(&self) -> u16;
    fn send(&mut self, channel: u16, dst: u16, payload: &[u8]) -> Result<(), Error>;
    fn recv(&mut self, timeout: Duration) -> Result<Option<LinkEvent>, Error>;
    /// Leaves or ends the network.
    fn close(&mut self);
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn node_id(&self) -> u16 {
        (**self).node_id()
    }
    fn send(&mut self, channel: u16, dst: u16, payload: &[u8]) -> Result<(), Error> {
        (**self).send(channel, dst, payload)
    }
    fn recv(&mut self, timeout: Duration) -> Result<Option<LinkEvent>, Error> {
        (**self).recv(timeout)
    }
    fn close(&mut self) {
        (**self).close()
    }
}

impl<R: Radio> Transport for Link<R> {
    fn node_id(&self) -> u16 {
        self.node_id
    }
    fn send(&mut self, channel: u16, dst: u16, payload: &[u8]) -> Result<(), Error> {
        Link::send(self, channel, dst, payload)
    }
    fn recv(&mut self, timeout: Duration) -> Result<Option<LinkEvent>, Error> {
        Link::recv(self, timeout)
    }
    fn close(&mut self) {
        Link::close(self)
    }
}

pub struct Session<T: Transport> {
    link: T,
    record_key: Key,
    epoch: Instant,
    isn_base: u32,
    peer: u16,
    peer_bulk: Inbound,
    peer_control: Inbound,
    control_queue: VecDeque<[u8; 8]>,
    control: Option<Sender>,
    bulk: Option<Bulk>,
    /// Second word of the next control message, carried over like the console does.
    word: u32,
    events: VecDeque<Event>,
    ready_sent: bool,
    peer_ready: bool,
    cancel: Option<Arc<AtomicBool>>,
}

impl<T: Transport> Session<T> {
    pub fn new(link: T, record_key: Key) -> Self {
        let peer = if link.node_id() == uds::HOST_NODE {
            2
        } else {
            uds::HOST_NODE
        };
        let isn_base = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u32);
        Session {
            link,
            record_key,
            epoch: Instant::now(),
            isn_base,
            peer,
            peer_bulk: Inbound::default(),
            peer_control: Inbound::default(),
            control_queue: VecDeque::new(),
            control: None,
            bulk: None,
            word: 0,
            events: VecDeque::new(),
            ready_sent: false,
            peer_ready: false,
            cancel: None,
        }
    }

    /// Makes waits fail with `Error::Cancelled` once `flag` is set.
    pub fn set_cancel(&mut self, flag: Arc<AtomicBool>) {
        self.cancel = Some(flag);
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    pub fn link(&self) -> &T {
        &self.link
    }

    pub fn into_link(self) -> T {
        self.link
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn isn(&self) -> u32 {
        self.isn_base.wrapping_add(self.now_ms() as u32)
    }

    fn me(&self) -> u16 {
        self.link.node_id()
    }

    /// Queues a control message for the peer.
    pub fn send_control(&mut self, kind: u32) {
        if kind == ACCEPTED {
            self.word = ACCEPTED_WORD;
        }
        self.ready_sent |= kind == READY;
        self.control_queue
            .push_back(control_message(kind, self.word));
    }

    /// Starts sending a 512-byte container on this node's bulk channel.
    pub fn send_container(&mut self, data: &[u8]) -> Result<(), Error> {
        let sender = Sender::new(self.isn(), &[data], self.now_ms(), GIVE_UP_MS)
            .ok_or_else(|| Error::Format("container does not fit one connection".into()))?;
        self.bulk = Some(Bulk {
            sender,
            data: data.to_vec(),
            restarts: 0,
        });
        Ok(())
    }

    /// Whether any of this node's messages are still unacknowledged.
    pub fn busy(&self) -> bool {
        self.control.is_some() || !self.control_queue.is_empty() || self.bulk.is_some()
    }

    fn send_prnn(&mut self, channel: u16, h: &Header, data: &[u8]) -> Result<(), Error> {
        self.link
            .send(channel, BROADCAST_NODE, &prnn::build(h, data))
    }

    /// Sends what is due and returns the earliest time something else will be.
    fn transmit(&mut self) -> Result<Option<u64>, Error> {
        let now = self.now_ms();
        if self.control.is_none() {
            if let Some(m) = self.control_queue.pop_front() {
                self.control = Sender::new(self.isn(), &[&m], now, GIVE_UP_MS);
            }
        }
        let control = control_channel(self.me());
        if let Some(mut s) = self.control.take() {
            while let Some((h, d)) = s.poll(now) {
                self.send_prnn(control, &h, &d)?;
            }
            match s.state() {
                SendState::Active => self.control = Some(s),
                SendState::Done => {}
                SendState::Failed => self
                    .events
                    .push_back(Event::Failed("control message not acknowledged".into())),
            }
        }
        let bulk = bulk_channel(self.me());
        if let Some(mut b) = self.bulk.take() {
            while let Some((h, d)) = b.sender.poll(now) {
                self.send_prnn(bulk, &h, &d)?;
            }
            match b.sender.state() {
                SendState::Active => self.bulk = Some(b),
                SendState::Done => {}
                SendState::Failed => self
                    .events
                    .push_back(Event::Failed("Mii not acknowledged".into())),
            }
        }
        let due = self
            .control
            .iter()
            .chain(self.bulk.iter().map(|b| &b.sender))
            .filter_map(|s| s.next_due())
            .min();
        Ok(
            if self.control.is_none() && !self.control_queue.is_empty() {
                Some(now)
            } else {
                due
            },
        )
    }

    fn handle_inbound(&mut self, control: bool, h: &Header, payload: &[u8]) -> Result<(), Error> {
        let channel = if control {
            control_channel(self.peer)
        } else {
            bulk_channel(self.peer)
        };
        let inbound = if control {
            &mut self.peer_control
        } else {
            &mut self.peer_bulk
        };
        let ev = inbound.rx.handle(h, payload);
        if ev.opened {
            inbound.data.clear();
        }
        if let Some(d) = ev.data {
            inbound.data.extend_from_slice(d);
        }
        let message = ev.closed.then(|| std::mem::take(&mut inbound.data));
        if let Some(ack) = ev.ack {
            self.send_prnn(channel, &Header::ack(ack), &[])?;
        }
        if ev.opened && !control {
            self.send_control(RECEIVING);
            self.events.push_back(Event::Incoming);
        }
        match message {
            Some(m) if control => self.on_control(&m),
            Some(m) => self.on_container(m),
            None => {}
        }
        Ok(())
    }

    fn on_control(&mut self, m: &[u8]) {
        let Some((kind, _)) = parse_control(m) else {
            return;
        };
        let e = match kind {
            READY => {
                self.peer_ready = true;
                Event::PeerReady
            }
            RECEIVING => Event::PeerReceiving,
            ACCEPTED => Event::Verdict(true),
            REJECTED => Event::Verdict(false),
            _ => return,
        };
        self.events.push_back(e);
    }

    fn on_container(&mut self, data: Vec<u8>) {
        let record = container::parse(&data).and_then(|w| mii::unwrap(&self.record_key, &w));
        let accepted = record
            .as_ref()
            .is_ok_and(|r| r.crc_valid() && r.validate().is_empty());
        self.send_control(if accepted { ACCEPTED } else { REJECTED });
        self.events.push_back(Event::Received(Received {
            container: data,
            record,
            accepted,
        }));
    }

    fn on_ours(&mut self, channel: u16, h: &Header) {
        if h.flags & RST != 0 {
            if channel == bulk_channel(self.me()) {
                self.restart_bulk();
            }
            return;
        }
        if h.flags & ACK == 0 {
            return;
        }
        if channel == control_channel(self.me()) {
            if let Some(s) = &mut self.control {
                s.on_ack(h.ack);
            }
        } else if let Some(b) = &mut self.bulk {
            b.sender.on_ack(h.ack);
        }
    }

    /// Reopens the bulk connection after the peer reset its end.
    fn restart_bulk(&mut self) {
        let Some(b) = self.bulk.take() else { return };
        if b.restarts >= SEND_RESTARTS {
            self.events
                .push_back(Event::Failed("peer reset the connection".into()));
            return;
        }
        if let Some(sender) = Sender::new(self.isn(), &[&b.data], self.now_ms(), GIVE_UP_MS) {
            self.bulk = Some(Bulk {
                sender,
                data: b.data,
                restarts: b.restarts + 1,
            });
        }
    }

    fn handle(&mut self, e: LinkEvent) -> Result<(), Error> {
        let p = match e {
            LinkEvent::Packet(p) => p,
            LinkEvent::Nodes(n) => {
                self.events.push_back(Event::Nodes(n));
                return Ok(());
            }
            LinkEvent::Left => {
                self.events.push_back(Event::Left);
                return Ok(());
            }
        };
        if p.src != self.peer {
            return Ok(());
        }
        let Some((h, payload)) = prnn::parse(&p.payload) else {
            return Ok(());
        };
        let me = self.me();
        match p.channel {
            c if c == bulk_channel(self.peer) => self.handle_inbound(false, &h, payload)?,
            c if c == control_channel(self.peer) => self.handle_inbound(true, &h, payload)?,
            c if c == bulk_channel(me) || c == control_channel(me) => self.on_ours(c, &h),
            _ => {}
        }
        Ok(())
    }

    /// Runs the exchange until the next event or `timeout`.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>, Error> {
        let until = Instant::now() + timeout;
        loop {
            let due = self.transmit()?;
            if let Some(e) = self.events.pop_front() {
                return Ok(Some(e));
            }
            let now = Instant::now();
            if now >= until {
                return Ok(None);
            }
            let mut wait = (until - now).min(MAX_WAIT);
            if let Some(due) = due {
                wait = wait.min(Duration::from_millis(due.saturating_sub(self.now_ms())));
            }
            if let Some(e) = self.link.recv(wait)? {
                self.handle(e)?;
            }
        }
    }

    /// Waits for `want` to accept an event, failing on a lost link or timeout.
    fn wait_for<V>(
        &mut self,
        timeout: Duration,
        what: &str,
        mut want: impl FnMut(Event) -> Option<V>,
    ) -> Result<V, Error> {
        let until = Instant::now() + timeout;
        loop {
            if self.cancelled() {
                return Err(Error::Cancelled);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout(format!("gave up waiting for {what}")));
            }
            match self.next_event(left.min(Duration::from_millis(200)))? {
                Some(Event::Left) => return Err(Error::Session("the other console left".into())),
                Some(Event::Failed(why)) => return Err(Error::Session(why)),
                Some(e) => {
                    if let Some(v) = want(e) {
                        return Ok(v);
                    }
                }
                None => {}
            }
        }
    }

    /// Waits for the peer's ready message, then sends this node's unless it already went.
    pub fn wait_ready(&mut self, timeout: Duration) -> Result<(), Error> {
        if !self.peer_ready {
            self.wait_for(timeout, "the other console", |e| {
                matches!(e, Event::PeerReady).then_some(())
            })?;
        }
        if !self.ready_sent {
            self.send_control(READY);
        }
        self.flush(Duration::from_secs(10))?;
        // Clears any half-open sender on the channel this node receives on.
        let ch = bulk_channel(self.peer);
        self.send_prnn(ch, &Header::rst(), &[])
    }

    /// Keeps answering the peer for `d`, so retransmitted segments still get acknowledged.
    pub fn linger(&mut self, d: Duration) -> Result<(), Error> {
        let until = Instant::now() + d;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            if let Some(Event::Left) = self.next_event(left)? {
                break;
            }
        }
        Ok(())
    }

    /// Waits until everything queued has been acknowledged, keeping other events for later.
    pub fn flush(&mut self, timeout: Duration) -> Result<(), Error> {
        let until = Instant::now() + timeout;
        let mut kept = Vec::new();
        let result = loop {
            if !self.busy() {
                break Ok(());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break Err(Error::Timeout("messages not acknowledged".into()));
            }
            match self.next_event(left.min(MAX_WAIT)) {
                Err(e) => break Err(e),
                Ok(Some(Event::Left)) => {
                    break Err(Error::Session("the other console left".into()));
                }
                Ok(Some(Event::Failed(why))) => break Err(Error::Session(why)),
                Ok(Some(e)) => kept.push(e),
                Ok(None) => {}
            }
        };
        for e in kept.into_iter().rev() {
            self.events.push_front(e);
        }
        result
    }

    /// Sends a Mii and returns the peer's verdict.
    pub fn send_mii(&mut self, record: &Record, timeout: Duration) -> Result<bool, Error> {
        let wrapped = mii::wrap(&self.record_key, &record.for_sending());
        self.send_raw(&container::build(&wrapped, container::LENGTH_3DS), timeout)
    }

    /// Sends a prepared container and returns the peer's verdict.
    pub fn send_raw(&mut self, container: &[u8], timeout: Duration) -> Result<bool, Error> {
        self.send_container(container)?;
        let mut receiving = false;
        self.wait_for(timeout, "the other console's answer", |e| match e {
            Event::PeerReceiving => {
                receiving = true;
                None
            }
            Event::Verdict(v) if receiving => Some(v),
            _ => None,
        })
    }

    /// Waits for the peer to send a Mii; the verdict has been sent when this returns.
    pub fn receive_mii(&mut self, timeout: Duration) -> Result<Received, Error> {
        let r = self.wait_for(timeout, "a Mii", |e| match e {
            Event::Received(r) => Some(r),
            _ => None,
        })?;
        self.flush(Duration::from_secs(10))?;
        Ok(r)
    }
}
