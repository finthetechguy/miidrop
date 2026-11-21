//! Membership of a UDS network as a client: joining, framing and keepalives.

use std::time::{Duration, Instant};

use crate::Error;
use crate::keys::Key;
use crate::radio::Radio;
use crate::uds::eapol::{self, JoinRequest, Message};
use crate::uds::key::session_key;
use crate::uds::securedata::{
    CHANNEL_KEEPALIVE, CHANNEL_NODE_LIST, ETHERTYPE_EAPOL, ETHERTYPE_SECURE, Packet, parse_llc,
};
use crate::uds::{App, BROADCAST_NODE, HOST_NODE, NetworkInfo, NodeInfo, node};
use crate::wifi::ccmp;
use crate::wifi::frame::{
    self, ASSOC_RESPONSE, AUTH, BROADCAST, DEAUTH, DISASSOC, FC_FROM_DS, FC_TO_DS, HEADER_LEN,
    Header, Mac, TYPE_DATA, TYPE_MGMT,
};

pub const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(1100);
const STEP_TIMEOUT: Duration = Duration::from_millis(300);
const ATTEMPTS: u32 = 5;
const REASON_LEAVING: u16 = 3;
/// Duration field consoles put in unicast frames, in microseconds.
const UNICAST_DURATION: u16 = 0x2c;

#[derive(Debug, PartialEq, Eq)]
pub enum LinkEvent {
    Packet(Packet),
    /// The host sent a new node list.
    Nodes(Vec<NodeInfo>),
    /// The host ended the session or dropped this node.
    Left,
}

pub struct Link<R: Radio> {
    radio: R,
    key: Key,
    me: Mac,
    pub network: NetworkInfo,
    pub node_id: u16,
    pub nodes: Vec<NodeInfo>,
    pn: u64,
    host_pn: Option<u64>,
    frame_seq: u16,
    uds_seq: u16,
    next_keepalive: Instant,
    joined: bool,
}

impl<R: Radio> Link<R> {
    /// Authenticates, associates and asks the host to admit `me`.
    pub fn join(
        mut radio: R,
        network: &NetworkInfo,
        base: &Key,
        app: &App,
        me: NodeInfo,
    ) -> Result<Self, Error> {
        radio.set_channel(network.channel)?;
        let mut link = Link {
            key: session_key(base, app, network.network_id, &network.host),
            me: radio.mac(),
            radio,
            network: network.clone(),
            node_id: 0,
            nodes: vec![],
            pn: 0,
            host_pn: None,
            frame_seq: 0,
            uds_seq: 0,
            next_keepalive: Instant::now(),
            joined: false,
        };
        let host = network.host;

        let auth = frame::auth(link.me, host, host, 1, 0);
        link.exchange(&auth, "authentication", |f| {
            let h = Header::parse(f).ok()?;
            let body = &f[HEADER_LEN..];
            if !h.is(TYPE_MGMT, AUTH) || body.len() < 6 {
                return None;
            }
            let transaction = u16::from_le_bytes([body[2], body[3]]);
            let status = u16::from_le_bytes([body[4], body[5]]);
            (transaction == 2).then_some(status)
        })
        .and_then(|status| refused("authentication", status))?;

        let assoc = frame::assoc_request(link.me, host, network.network_id, &network.rates);
        let aid = link
            .exchange(&assoc, "association", |f| {
                let h = Header::parse(f).ok()?;
                if !h.is(TYPE_MGMT, ASSOC_RESPONSE) {
                    return None;
                }
                frame::parse_assoc_response(&f[HEADER_LEN..]).ok()
            })
            .and_then(|(status, aid)| refused("association", status).map(|_| aid))?;

        let request = JoinRequest { aid, node: me }.to_body();
        let reply = link.exchange_data(&request, |body| match parse_llc(body) {
            Ok((ETHERTYPE_EAPOL, rest)) => match eapol::parse(rest) {
                Ok(Message::Reply(r)) => Some(r),
                _ => None,
            },
            _ => None,
        })?;
        link.node_id = reply.node_id;
        link.nodes = reply.nodes;
        link.joined = true;
        link.next_keepalive = Instant::now() + KEEPALIVE_INTERVAL;
        Ok(link)
    }

    pub fn mac(&self) -> Mac {
        self.me
    }

    pub fn host_name(&self) -> Option<String> {
        self.nodes
            .iter()
            .find(|n| n.node_id == HOST_NODE)
            .map(|n| n.name())
    }

    fn next_frame_seq(&mut self) -> u16 {
        self.frame_seq = (self.frame_seq + 1) & 0x0fff;
        self.frame_seq
    }

    fn send_mgmt(&mut self, f: &[u8]) -> Result<(), Error> {
        let mut f = f.to_vec();
        let seq = self.next_frame_seq();
        f[2..4].copy_from_slice(&UNICAST_DURATION.to_le_bytes());
        f[22..24].copy_from_slice(&(seq << 4).to_le_bytes());
        self.radio.send(&f)?;
        Ok(())
    }

    /// Sends a management frame until the host answers with a frame `answer` accepts.
    fn exchange<T>(
        &mut self,
        f: &[u8],
        what: &str,
        mut answer: impl FnMut(&[u8]) -> Option<T>,
    ) -> Result<T, Error> {
        for _ in 0..ATTEMPTS {
            self.send_mgmt(f)?;
            let until = Instant::now() + STEP_TIMEOUT;
            while let Some(rx) = self
                .radio
                .recv(until.saturating_duration_since(Instant::now()))?
            {
                let Ok(h) = Header::parse(&rx.frame) else {
                    continue;
                };
                if h.addr2 != self.network.host || h.addr1 != self.me {
                    continue;
                }
                if h.is(TYPE_MGMT, DEAUTH) || h.is(TYPE_MGMT, DISASSOC) {
                    return Err(Error::Session(format!("host refused {what}")));
                }
                if let Some(v) = answer(&rx.frame) {
                    return Ok(v);
                }
            }
        }
        Err(Error::Timeout(format!("no answer to {what}")))
    }

    /// Sends a protected frame body until the host answers with one `answer` accepts.
    fn exchange_data<T>(
        &mut self,
        body: &[u8],
        mut answer: impl FnMut(&[u8]) -> Option<T>,
    ) -> Result<T, Error> {
        for _ in 0..ATTEMPTS {
            self.send_body(body, false)?;
            let until = Instant::now() + STEP_TIMEOUT;
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                let Some(body) = self.recv_body(left)? else {
                    break;
                };
                if let Some(v) = answer(&body) {
                    return Ok(v);
                }
            }
        }
        Err(Error::Timeout("no answer to the join request".into()))
    }

    fn send_body(&mut self, body: &[u8], broadcast: bool) -> Result<(), Error> {
        let host = self.network.host;
        let mut h = if broadcast {
            Header::new(TYPE_DATA, 0, 0, BROADCAST, self.me, host)
        } else {
            let mut h = Header::new(TYPE_DATA, 0, FC_TO_DS, host, self.me, host);
            h.duration = UNICAST_DURATION;
            h
        };
        let seq = self.next_frame_seq();
        h.set_sequence(seq);
        let f = ccmp::encrypt(&self.key, &h, self.pn, body)?;
        self.pn += 1;
        self.radio.send(&f)?;
        Ok(())
    }

    /// Sends a UDS packet to one node or, with BROADCAST_NODE, to all of them.
    pub fn send(&mut self, channel: u16, dst: u16, payload: &[u8]) -> Result<(), Error> {
        let p = Packet {
            channel,
            seq: self.uds_seq,
            dst,
            src: self.node_id,
            payload: payload.to_vec(),
        };
        self.uds_seq = self.uds_seq.wrapping_add(1);
        self.send_body(&p.to_body(), dst == BROADCAST_NODE)
    }

    /// Waits for the next authenticated body from the host, returning None on timeout.
    fn recv_body(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, Error> {
        let until = Instant::now() + timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let Some(rx) = self.radio.recv(left)? else {
                return Ok(None);
            };
            let Ok(h) = Header::parse(&rx.frame) else {
                continue;
            };
            if h.addr2 != self.network.host || (h.addr1 != self.me && h.addr1 != BROADCAST) {
                continue;
            }
            if self.joined && (h.is(TYPE_MGMT, DEAUTH) || h.is(TYPE_MGMT, DISASSOC)) {
                self.joined = false;
                return Ok(Some(vec![]));
            }
            if h.ftype() != TYPE_DATA || h.fc & (FC_TO_DS | FC_FROM_DS) != FC_FROM_DS {
                continue;
            }
            let Ok((pn, body)) = ccmp::decrypt(&self.key, &rx.frame) else {
                continue;
            };
            // Retries repeat the packet number.
            if self.host_pn.is_some_and(|last| pn <= last) {
                continue;
            }
            self.host_pn = Some(pn);
            return Ok(Some(body));
        }
    }

    fn keepalive_due(&mut self) -> Result<(), Error> {
        if self.joined && Instant::now() >= self.next_keepalive {
            self.send(CHANNEL_KEEPALIVE, HOST_NODE, &[0])?;
            self.next_keepalive = Instant::now() + KEEPALIVE_INTERVAL;
        }
        Ok(())
    }

    /// Waits up to `timeout` for the next event, sending keepalives meanwhile.
    pub fn recv(&mut self, timeout: Duration) -> Result<Option<LinkEvent>, Error> {
        let until = Instant::now() + timeout;
        loop {
            if !self.joined {
                return Ok(Some(LinkEvent::Left));
            }
            self.keepalive_due()?;
            let now = Instant::now();
            if now >= until {
                return Ok(None);
            }
            let wait = until
                .min(self.next_keepalive)
                .saturating_duration_since(now);
            let Some(body) = self.recv_body(wait)? else {
                continue;
            };
            if !self.joined {
                return Ok(Some(LinkEvent::Left));
            }
            let Ok((ETHERTYPE_SECURE, rest)) = parse_llc(&body) else {
                continue;
            };
            let Ok(p) = Packet::parse(rest) else {
                continue;
            };
            if p.channel == CHANNEL_NODE_LIST {
                let Some(nodes) = parse_node_list(&p.payload) else {
                    continue;
                };
                if !nodes.iter().any(|n| n.node_id == self.node_id) {
                    self.joined = false;
                    return Ok(Some(LinkEvent::Left));
                }
                self.nodes = nodes.clone();
                return Ok(Some(LinkEvent::Nodes(nodes)));
            }
            if p.dst == BROADCAST_NODE || p.dst == self.node_id {
                return Ok(Some(LinkEvent::Packet(p)));
            }
        }
    }

    /// Tells the host this node is leaving and hands the radio back.
    pub fn leave(mut self) -> R {
        if self.joined {
            let host = self.network.host;
            let f = frame::deauth(self.me, host, host, REASON_LEAVING);
            let _ = self.send_mgmt(&f);
        }
        self.radio
    }
}

fn refused(what: &str, status: u16) -> Result<(), Error> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Session(format!(
            "host refused {what} (status {status})"
        )))
    }
}

/// Parses the node list body the host broadcasts.
pub fn parse_node_list(b: &[u8]) -> Option<Vec<NodeInfo>> {
    let total = *b.get(2)? as usize;
    let mut nodes = node::parse_table(b.get(8..)?).ok()?;
    nodes.truncate(total);
    Some(nodes)
}

/// Builds the node list body a host broadcasts.
pub fn node_list(nodes: &[NodeInfo], max_nodes: u8) -> Vec<u8> {
    let mut out = vec![0, 0, nodes.len() as u8, max_nodes, 0, 0, 0, 0];
    node::write_table(nodes, &mut out);
    out
}
