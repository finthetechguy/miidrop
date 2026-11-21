//! A simulated host console on a lossy shared medium, for testing the client end to end.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::Error;
use crate::keys::Key;
use crate::link::{self, Link, LinkEvent};
use crate::mii::{self, Record};
use crate::radio::{Radio, Received};
use crate::session::{Event, Session, Transport};
use crate::uds::eapol::{self, JoinReply, Message};
use crate::uds::key::session_key;
use crate::uds::securedata::{
    CHANNEL_KEEPALIVE, CHANNEL_NODE_LIST, ETHERTYPE_EAPOL, ETHERTYPE_SECURE, Packet, parse_llc,
};
use crate::uds::{BROADCAST_NODE, HOST_NODE, MII_MAKER, NetworkInfo, NodeInfo};
use crate::wifi::ccmp;
use crate::wifi::frame::{
    self, ASSOC_REQUEST, AUTH, BROADCAST, DEAUTH, FC_FROM_DS, Header, Mac, TYPE_DATA, TYPE_MGMT,
};
use crate::wifi::radiotap::RxInfo;

const HOST: Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
const CLIENT: Mac = [0x02, 0x66, 0x77, 0x88, 0x99, 0xaa];
const BASE: Key = [0x2d; 16];
const RECORD_KEY: Key = [0x31; 16];

/// One end of a simulated radio link that drops a share of broadcast frames.
pub struct AirRadio {
    mac: Mac,
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    loss: u32,
    rng: u64,
}

impl AirRadio {
    fn dropped(&mut self, frame: &[u8]) -> bool {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        frame[4..10] == BROADCAST && ((self.rng >> 33) % 100) < self.loss as u64
    }
}

/// Two connected radios; `loss` is the percentage of broadcast frames lost each way.
pub fn air(loss: u32, seed: u64) -> (AirRadio, AirRadio) {
    let (a_tx, b_rx) = mpsc::channel();
    let (b_tx, a_rx) = mpsc::channel();
    let end = |mac, tx, rx, rng| AirRadio {
        mac,
        tx,
        rx,
        loss,
        rng,
    };
    (
        end(HOST, a_tx, a_rx, seed),
        end(CLIENT, b_tx, b_rx, seed ^ 0x9e37_79b9),
    )
}

impl Radio for AirRadio {
    fn mac(&self) -> Mac {
        self.mac
    }
    fn set_channel(&mut self, _: u8) -> std::io::Result<()> {
        Ok(())
    }
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        if !self.dropped(frame) {
            let _ = self.tx.send(frame.to_vec());
        }
        Ok(())
    }
    fn recv(&mut self, timeout: Duration) -> std::io::Result<Option<Received>> {
        Ok(self.rx.recv_timeout(timeout).ok().map(|frame| Received {
            info: RxInfo {
                frequency: Some(2437),
                signal: Some(-40),
                bad_fcs: false,
            },
            frame,
        }))
    }
}

pub fn network() -> NetworkInfo {
    NetworkInfo {
        host: HOST,
        channel: 6,
        comm_id: MII_MAKER.comm_id,
        id8: MII_MAKER.id8,
        attributes: 0x8000,
        network_id: 0x1234_5678,
        total_nodes: 1,
        max_nodes: 2,
        updates: 2,
        appdata: b"C:1\0\0\0\0\0".to_vec(),
        rates: frame::RATES.to_vec(),
        encrypted_nodes: vec![],
    }
}

/// The host side of a network, answering joins the way a waiting console does.
pub struct SimHost {
    radio: AirRadio,
    key: Key,
    nodes: Vec<NodeInfo>,
    client: Option<Mac>,
    pn: u64,
    uds_seq: u16,
    pub keepalives: u32,
}

impl SimHost {
    pub fn new(radio: AirRadio) -> Self {
        let n = network();
        let mut me = NodeInfo::new(0x1111, "Console");
        me.node_id = HOST_NODE;
        SimHost {
            radio,
            key: session_key(&BASE, &MII_MAKER, n.network_id, &n.host),
            nodes: vec![me],
            client: None,
            pn: 0,
            uds_seq: 0,
            keepalives: 0,
        }
    }

    /// Serves joins until a client is admitted.
    pub fn accept(&mut self, timeout: Duration) -> Result<(), Error> {
        let until = Instant::now() + timeout;
        while self.client.is_none() {
            if Instant::now() >= until {
                return Err(Error::Timeout("no client joined".into()));
            }
            self.recv(Duration::from_millis(20))?;
        }
        Ok(())
    }

    fn send_body(&mut self, to: Mac, body: &[u8]) -> Result<(), Error> {
        let h = Header::new(TYPE_DATA, 0, FC_FROM_DS, to, HOST, HOST);
        let f = ccmp::encrypt(&self.key, &h, self.pn, body)?;
        self.pn += 1;
        self.radio.send(&f)?;
        Ok(())
    }

    fn handle(&mut self, f: &[u8]) -> Result<Option<LinkEvent>, Error> {
        let h = Header::parse(f)?;
        if h.addr1 != HOST && h.addr1 != BROADCAST {
            return Ok(None);
        }
        if h.is(TYPE_MGMT, AUTH) {
            self.radio.send(&frame::auth(HOST, h.addr2, HOST, 2, 0))?;
        } else if h.is(TYPE_MGMT, ASSOC_REQUEST) {
            let n = network();
            let r = frame::assoc_response(HOST, h.addr2, 1, n.network_id, n.channel);
            self.radio.send(&r)?;
        } else if h.is(TYPE_MGMT, DEAUTH) && Some(h.addr2) == self.client {
            self.client = None;
            self.nodes.truncate(1);
            return Ok(Some(LinkEvent::Left));
        } else if h.ftype() == TYPE_DATA {
            let (_, body) = ccmp::decrypt(&self.key, f)?;
            match parse_llc(&body)? {
                (ETHERTYPE_EAPOL, rest) => {
                    if let Message::Request(r) = eapol::parse(rest)? {
                        let mut node = r.node;
                        node.node_id = 2;
                        self.nodes.truncate(1);
                        self.nodes.push(node);
                        self.client = Some(h.addr2);
                        let list = link::node_list(&self.nodes, 2);
                        Transport::send(self, CHANNEL_NODE_LIST, BROADCAST_NODE, &list)?;
                        let reply = JoinReply {
                            node_id: 2,
                            client: h.addr2,
                            total_nodes: 2,
                            max_nodes: 2,
                            nodes: self.nodes.clone(),
                        };
                        self.send_body(h.addr2, &reply.to_body())?;
                    }
                }
                (ETHERTYPE_SECURE, rest) => {
                    let p = Packet::parse(rest)?;
                    if p.channel == CHANNEL_KEEPALIVE {
                        self.keepalives += 1;
                    } else {
                        return Ok(Some(LinkEvent::Packet(p)));
                    }
                }
                _ => {}
            }
        }
        Ok(None)
    }
}

impl Transport for SimHost {
    fn node_id(&self) -> u16 {
        HOST_NODE
    }

    fn send(&mut self, channel: u16, dst: u16, payload: &[u8]) -> Result<(), Error> {
        let p = Packet {
            channel,
            seq: self.uds_seq,
            dst,
            src: HOST_NODE,
            payload: payload.to_vec(),
        };
        self.uds_seq = self.uds_seq.wrapping_add(1);
        let to = if dst == BROADCAST_NODE {
            BROADCAST
        } else {
            self.client.unwrap_or(BROADCAST)
        };
        self.send_body(to, &p.to_body())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<LinkEvent>, Error> {
        let until = Instant::now() + timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let Some(rx) = self.radio.recv(left)? else {
                return Ok(None);
            };
            if let Some(e) = self.handle(&rx.frame)? {
                return Ok(Some(e));
            }
            if left.is_zero() {
                return Ok(None);
            }
        }
    }
}

/// What the simulated console saw.
pub struct Outcome<C> {
    pub console: C,
    pub client_left: bool,
    pub keepalives: u32,
}

const STEP: Duration = Duration::from_secs(10);

/// Runs a console behaviour against a client behaviour over a link losing `loss` percent.
pub fn run<C: Send + 'static, P>(
    loss: u32,
    seed: u64,
    console: impl FnOnce(&mut Session<SimHost>) -> C + Send + 'static,
    client: impl FnOnce(&mut Session<Link<AirRadio>>) -> P,
) -> (Outcome<C>, P) {
    let (host_radio, client_radio) = air(loss, seed);
    let handle = thread::spawn(move || {
        let mut host = SimHost::new(host_radio);
        host.accept(STEP).unwrap();
        let mut s = Session::new(host, RECORD_KEY);
        s.send_control(crate::session::READY);
        s.wait_ready(STEP).unwrap();
        let result = console(&mut s);
        let until = Instant::now() + STEP;
        let mut left = false;
        while !left && Instant::now() < until {
            left = matches!(
                s.next_event(Duration::from_millis(50)),
                Ok(Some(Event::Left))
            );
        }
        Outcome {
            console: result,
            client_left: left,
            keepalives: s.into_link().keepalives,
        }
    });
    let mut me = NodeInfo::new(0x2222, "PC");
    me.node_id = 0;
    let link = Link::join(client_radio, &network(), &BASE, &MII_MAKER, me).unwrap();
    assert_eq!(link.node_id, 2);
    assert_eq!(link.host_name().as_deref(), Some("Console"));
    let mut s = Session::new(link, RECORD_KEY);
    s.wait_ready(STEP).unwrap();
    let p = client(&mut s);
    s.linger(Duration::from_millis(500)).unwrap();
    s.into_link().leave();
    (handle.join().unwrap(), p)
}

pub fn record_key() -> Key {
    RECORD_KEY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container;

    fn sample() -> Record {
        mii::sample()
    }

    #[test]
    fn client_sends() {
        for (loss, seed) in [(0, 1), (25, 2), (25, 3)] {
            let (out, verdict) = run(
                loss,
                seed,
                |s| s.receive_mii(STEP).unwrap(),
                |s| s.send_mii(&sample(), STEP).unwrap(),
            );
            assert!(verdict, "loss {loss}");
            let got = out.console;
            assert!(got.accepted);
            assert_eq!(got.record.unwrap(), sample().for_sending());
            assert!(out.client_left);
        }
    }

    #[test]
    fn client_receives() {
        for (loss, seed) in [(0, 4), (25, 5)] {
            let (out, got) = run(
                loss,
                seed,
                |s| s.send_mii(&sample(), STEP).unwrap(),
                |s| s.receive_mii(STEP).unwrap(),
            );
            assert!(out.console, "loss {loss}");
            assert!(got.accepted);
            assert_eq!(got.container.len(), container::SEGMENT_SIZE);
            assert_eq!(got.record.unwrap(), sample().for_sending());
        }
    }

    #[test]
    fn bad_record_is_rejected() {
        let (out, verdict) = run(
            0,
            6,
            |s| s.receive_mii(STEP).unwrap(),
            |s| {
                let mut bytes = *sample().for_sending().as_bytes();
                bytes[0x5f] ^= 1;
                let bad = Record::from_bytes(&bytes).unwrap();
                let wrapped = mii::wrap(&record_key(), &bad);
                let c = container::build(&wrapped, container::LENGTH_3DS);
                s.send_raw(&c, STEP).unwrap()
            },
        );
        assert!(!verdict);
        assert!(!out.console.accepted);
    }

    #[test]
    fn keepalives_flow() {
        let (out, ()) = run(
            0,
            7,
            |_| (),
            |s| {
                while s.next_event(Duration::from_millis(1300)).unwrap().is_some() {}
            },
        );
        assert!(out.keepalives >= 1);
        assert!(out.client_left);
    }
}
