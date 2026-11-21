//! Hosting a UDS network: beacons, admitting one client and framing data for it.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Error;
use crate::keys::Key;
use crate::link::{self, LinkEvent};
use crate::radio::Radio;
use crate::session::Transport;
use crate::uds::eapol::{self, JoinReply, Message};
use crate::uds::key::session_key;
use crate::uds::securedata::{
    CHANNEL_KEEPALIVE, CHANNEL_NODE_LIST, ETHERTYPE_EAPOL, ETHERTYPE_SECURE, Packet, parse_llc,
};
use crate::uds::{App, BROADCAST_NODE, HOST_NODE, NetworkInfo, NodeInfo};
use crate::wifi::ccmp;
use crate::wifi::frame::{
    self, ASSOC_REQUEST, AUTH, BROADCAST, DEAUTH, DISASSOC, FC_FROM_DS, FC_TO_DS, HEADER_LEN,
    Header, Mac, TYPE_DATA, TYPE_MGMT,
};

const BEACON_EVERY: Duration = Duration::from_micros(102_400);
/// A client that stays silent this long is considered gone.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const CLIENT_NODE: u16 = 2;
const UNICAST_DURATION: u16 = 0x2c;
const REASON_HOST_LEAVING: u16 = 2;
const WAITING: &[u8; 8] = b"C:1\0\0\0\0\0";
const FULL: &[u8; 8] = b"C:0\0\0\0\0\0";

struct Client {
    mac: Mac,
    aid: u16,
    node: Option<NodeInfo>,
    last_pn: Option<u64>,
    heard: Instant,
}

pub struct HostLink<R: Radio> {
    radio: R,
    key: Key,
    /// Encrypts the beacon's node list; without it the list cannot be read by scanners.
    beacon_key: Key,
    pub network: NetworkInfo,
    me: NodeInfo,
    client: Option<Client>,
    pn: u64,
    uds_seq: u16,
    frame_seq: u16,
    epoch: Instant,
    next_beacon: Instant,
    pub keepalives: u32,
}

impl<R: Radio> HostLink<R> {
    /// Starts beaconing a two-node network for `app` on `channel`.
    pub fn start(
        mut radio: R,
        app: &App,
        base: &Key,
        beacon_key: Option<Key>,
        mut me: NodeInfo,
        channel: u8,
    ) -> Result<Self, Error> {
        radio.set_channel(channel)?;
        let host = radio.mac();
        let network_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x1234_5678, |d| d.subsec_nanos() ^ d.as_secs() as u32);
        let network = NetworkInfo {
            host,
            channel,
            comm_id: app.comm_id,
            id8: app.id8,
            attributes: 0x8000,
            network_id,
            total_nodes: 1,
            max_nodes: 2,
            updates: 2,
            appdata: WAITING.to_vec(),
            rates: frame::RATES.to_vec(),
            encrypted_nodes: vec![],
        };
        me.node_id = HOST_NODE;
        let now = Instant::now();
        Ok(HostLink {
            key: session_key(base, app, network_id, &host),
            beacon_key: beacon_key.unwrap_or_default(),
            radio,
            network,
            me,
            client: None,
            pn: 0,
            uds_seq: 0,
            frame_seq: 0,
            epoch: now,
            next_beacon: now,
            keepalives: 0,
        })
    }

    pub fn mac(&self) -> Mac {
        self.network.host
    }

    /// The joined client's node info, once it has joined.
    pub fn client(&self) -> Option<&NodeInfo> {
        self.client.as_ref().and_then(|c| c.node.as_ref())
    }

    fn nodes(&self) -> Vec<NodeInfo> {
        let mut nodes = vec![self.me.clone()];
        nodes.extend(self.client().cloned());
        nodes
    }

    fn next_frame_seq(&mut self) -> u16 {
        self.frame_seq = (self.frame_seq + 1) & 0x0fff;
        self.frame_seq
    }

    fn send_mgmt(&mut self, f: &[u8]) -> Result<(), Error> {
        let mut f = f.to_vec();
        let seq = self.next_frame_seq();
        if f[4..10] != BROADCAST {
            f[2..4].copy_from_slice(&UNICAST_DURATION.to_le_bytes());
        }
        f[22..24].copy_from_slice(&(seq << 4).to_le_bytes());
        self.radio.send(&f)?;
        Ok(())
    }

    fn beacon_due(&mut self) -> Result<(), Error> {
        let now = Instant::now();
        if now < self.next_beacon {
            return Ok(());
        }
        let tsf = self.epoch.elapsed().as_micros() as u64;
        let seq = self.next_frame_seq();
        let f = self
            .network
            .beacon_frame(&self.beacon_key, &self.nodes(), tsf, seq);
        self.radio.send(&f)?;
        self.next_beacon += BEACON_EVERY;
        if self.next_beacon < now {
            self.next_beacon = now + BEACON_EVERY;
        }
        Ok(())
    }

    fn send_body(&mut self, to: Mac, body: &[u8]) -> Result<(), Error> {
        let host = self.network.host;
        let mut h = Header::new(TYPE_DATA, 0, FC_FROM_DS, to, host, host);
        if to != BROADCAST {
            h.duration = UNICAST_DURATION;
        }
        let seq = self.next_frame_seq();
        h.set_sequence(seq);
        let f = ccmp::encrypt(&self.key, &h, self.pn, body)?;
        self.pn += 1;
        self.radio.send(&f)?;
        Ok(())
    }

    fn advertise(&mut self) -> Result<(), Error> {
        let joined = self.client().is_some();
        self.network.total_nodes = 1 + joined as u8;
        self.network.appdata = if joined { FULL } else { WAITING }.to_vec();
        self.network.updates = self.network.updates.wrapping_add(2);
        let list = link::node_list(&self.nodes(), self.network.max_nodes);
        Transport::send(self, CHANNEL_NODE_LIST, BROADCAST_NODE, &list)
    }

    fn drop_client(&mut self) -> Result<Option<LinkEvent>, Error> {
        let had_node = self.client.take().is_some_and(|c| c.node.is_some());
        if had_node {
            self.advertise()?;
            return Ok(Some(LinkEvent::Left));
        }
        Ok(None)
    }

    fn handle(&mut self, f: &[u8]) -> Result<Option<LinkEvent>, Error> {
        let Ok(h) = Header::parse(f) else {
            return Ok(None);
        };
        let host = self.network.host;
        if h.addr1 != host && h.addr1 != BROADCAST {
            return Ok(None);
        }
        let from = h.addr2;
        let known = self.client.as_ref().is_some_and(|c| c.mac == from);
        let free = self.client.is_none() || known;
        if h.is(TYPE_MGMT, AUTH) && h.addr1 == host && free {
            let body = &f[HEADER_LEN..];
            if body.len() >= 6 && u16::from_le_bytes([body[2], body[3]]) == 1 {
                self.send_mgmt(&frame::auth(host, from, host, 2, 0))?;
            }
        } else if h.is(TYPE_MGMT, ASSOC_REQUEST) && h.addr1 == host && free {
            let aid = 1;
            let r = frame::assoc_response(
                host,
                from,
                aid,
                self.network.network_id,
                self.network.channel,
            );
            self.send_mgmt(&r)?;
            if !known {
                self.client = Some(Client {
                    mac: from,
                    aid,
                    node: None,
                    last_pn: None,
                    heard: Instant::now(),
                });
            }
        } else if (h.is(TYPE_MGMT, DEAUTH) || h.is(TYPE_MGMT, DISASSOC)) && known {
            return self.drop_client();
        } else if h.ftype() == TYPE_DATA && known && h.bssid() == host {
            if h.fc & FC_FROM_DS != 0 {
                return Ok(None);
            }
            let Ok((pn, body)) = ccmp::decrypt(&self.key, f) else {
                return Ok(None);
            };
            let c = self.client.as_mut().unwrap();
            if c.last_pn.is_some_and(|last| pn <= last) {
                return Ok(None);
            }
            c.last_pn = Some(pn);
            c.heard = Instant::now();
            return self.on_data(from, h.fc & FC_TO_DS != 0, &body);
        }
        Ok(None)
    }

    fn on_data(
        &mut self,
        from: Mac,
        _unicast: bool,
        body: &[u8],
    ) -> Result<Option<LinkEvent>, Error> {
        match parse_llc(body) {
            Ok((ETHERTYPE_EAPOL, rest)) => {
                let Ok(Message::Request(r)) = eapol::parse(rest) else {
                    return Ok(None);
                };
                let c = self.client.as_mut().unwrap();
                if r.aid != c.aid {
                    return Ok(None);
                }
                let first = c.node.is_none();
                let mut node = r.node;
                node.node_id = CLIENT_NODE;
                c.node = Some(node);
                if first {
                    self.advertise()?;
                }
                let reply = JoinReply {
                    node_id: CLIENT_NODE,
                    client: from,
                    total_nodes: self.network.total_nodes,
                    max_nodes: self.network.max_nodes,
                    nodes: self.nodes(),
                };
                self.send_body(from, &reply.to_body())?;
                if first {
                    return Ok(Some(LinkEvent::Nodes(self.nodes())));
                }
            }
            Ok((ETHERTYPE_SECURE, rest)) => {
                let Ok(p) = Packet::parse(rest) else {
                    return Ok(None);
                };
                let joined = self.client().is_some();
                self.keepalives += (p.channel == CHANNEL_KEEPALIVE) as u32;
                if joined && p.channel != CHANNEL_KEEPALIVE && p.src == CLIENT_NODE {
                    return Ok(Some(LinkEvent::Packet(p)));
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// Beacons and serves joins until a client has joined.
    pub fn wait_client(&mut self, timeout: Duration) -> Result<NodeInfo, Error> {
        let until = Instant::now() + timeout;
        loop {
            if let Some(n) = self.client() {
                return Ok(n.clone());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout("no console joined".into()));
            }
            Transport::recv(self, left.min(Duration::from_millis(200)))?;
        }
    }

    /// Ends the network and hands the radio back.
    pub fn close(mut self) -> R {
        Transport::close(&mut self);
        self.radio
    }
}

impl<R: Radio> Transport for HostLink<R> {
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
        let to = match (&self.client, dst) {
            (Some(c), CLIENT_NODE) => c.mac,
            _ => BROADCAST,
        };
        self.send_body(to, &p.to_body())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<LinkEvent>, Error> {
        let until = Instant::now() + timeout;
        loop {
            self.beacon_due()?;
            if self
                .client
                .as_ref()
                .is_some_and(|c| c.heard.elapsed() > CLIENT_TIMEOUT)
            {
                if let Some(e) = self.drop_client()? {
                    return Ok(Some(e));
                }
            }
            let now = Instant::now();
            if now >= until {
                return Ok(None);
            }
            let wait = until.min(self.next_beacon).saturating_duration_since(now);
            let Some(rx) = self.radio.recv(wait)? else {
                continue;
            };
            if let Some(e) = self.handle(&rx.frame)? {
                return Ok(Some(e));
            }
        }
    }

    fn close(&mut self) {
        let host = self.network.host;
        for _ in 0..3 {
            let _ = self.send_mgmt(&frame::deauth(host, BROADCAST, host, REASON_HOST_LEAVING));
        }
        self.client = None;
    }
}
