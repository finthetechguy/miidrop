//! UDS network information carried in beacons.

use aes::Aes128;
use ctr::cipher::{KeyIvInit, StreamCipher};
use md5::{Digest, Md5};
use sha1::Sha1;

use super::node::{BEACON_ENTRY_LEN, NodeInfo};
use super::{App, NINTENDO_OUI};
use crate::keys::Key;
use crate::wifi::frame::{
    self, BEACON, BROADCAST, CAPABILITY, Header, IE_DS, IE_ERP, IE_EXT_RATES, IE_RATES, IE_SSID,
    IE_TIM, IE_VENDOR, Mac, TYPE_MGMT,
};

type Ctr = ctr::Ctr128BE<Aes128>;

const TYPE_INFO: u8 = 0x15;
const TYPE_NODES: u8 = 0x18;
const TYPE_NODES_MORE: u8 = 0x19;
const INFO_LEN: usize = 0x34;
const HASH_AT: usize = 0x1f;
const FIRST_NODES_LEN: usize = 0xfa;
const IE_COUNTRY: u8 = 7;
/// Beacon interval in time units of 1024 microseconds.
pub const BEACON_INTERVAL_TU: u16 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkInfo {
    pub host: Mac,
    pub channel: u8,
    pub comm_id: u32,
    pub id8: u8,
    pub attributes: u16,
    pub network_id: u32,
    pub total_nodes: u8,
    pub max_nodes: u8,
    /// Counts changes to the advertised information.
    pub updates: u8,
    pub appdata: Vec<u8>,
    /// Supported rates the host advertises.
    pub rates: Vec<u8>,
    /// Node list as sent, still encrypted.
    pub encrypted_nodes: Vec<u8>,
}

impl NetworkInfo {
    pub fn matches(&self, app: &App) -> bool {
        self.comm_id == app.comm_id && self.id8 == app.id8
    }

    /// Parses a beacon frame, returning None for anything that is not a UDS host.
    pub fn from_beacon(frame: &[u8]) -> Option<Self> {
        let header = Header::parse(frame).ok()?;
        if !header.is(TYPE_MGMT, BEACON) || frame.len() < 36 {
            return None;
        }
        let mut channel = 0;
        let mut rates = Vec::new();
        let mut info = None;
        let mut nodes = Vec::new();
        for (id, data) in frame::elements(&frame[36..]) {
            if id == IE_DS && data.len() == 1 {
                channel = data[0];
            }
            if id == IE_RATES || id == IE_EXT_RATES {
                rates.extend_from_slice(data);
            }
            if id != IE_VENDOR || data.len() < 4 || data[..3] != NINTENDO_OUI {
                continue;
            }
            match data[3] {
                TYPE_INFO => info = Some(data),
                TYPE_NODES | TYPE_NODES_MORE => nodes.extend_from_slice(&data[4..]),
                _ => {}
            }
        }
        let d = info?;
        if d.len() < INFO_LEN || d.len() < INFO_LEN + d[0x33] as usize {
            return None;
        }
        Some(NetworkInfo {
            host: header.addr2,
            channel,
            comm_id: u32::from_be_bytes(d[4..8].try_into().unwrap()),
            id8: d[8],
            attributes: u16::from_be_bytes([d[0xa], d[0xb]]),
            network_id: u32::from_be_bytes(d[0xc..0x10].try_into().unwrap()),
            total_nodes: d[0x10],
            max_nodes: d[0x11],
            updates: d[9],
            appdata: d[INFO_LEN..INFO_LEN + d[0x33] as usize].to_vec(),
            rates,
            encrypted_nodes: nodes,
        })
    }

    fn node_ctr(&self) -> [u8; 16] {
        let mut ctr = [0u8; 16];
        ctr[..6].copy_from_slice(&self.host);
        ctr[6..10].copy_from_slice(&self.comm_id.to_le_bytes());
        ctr[10] = self.id8;
        ctr[12..].copy_from_slice(&self.network_id.to_le_bytes());
        ctr
    }

    /// Decrypts the beacon's node list, checking its digest.
    pub fn nodes(&self, beacon_key: &Key) -> Option<Vec<NodeInfo>> {
        let len = 0x12 + BEACON_ENTRY_LEN * self.max_nodes as usize;
        if self.encrypted_nodes.len() < len {
            return None;
        }
        let mut plain = self.encrypted_nodes[..len].to_vec();
        Ctr::new(beacon_key.into(), &self.node_ctr().into()).apply_keystream(&mut plain);
        if Md5::digest(&plain[0x10..])[..] != plain[..0x10] {
            return None;
        }
        let entries = plain[0x12..]
            .chunks(BEACON_ENTRY_LEN)
            .take(self.total_nodes as usize)
            .map(NodeInfo::parse_beacon_entry)
            .collect();
        Some(entries)
    }

    /// Builds a complete beacon frame, without FCS.
    pub fn beacon_frame(
        &self,
        beacon_key: &Key,
        nodes: &[NodeInfo],
        tsf: u64,
        seq: u16,
    ) -> Vec<u8> {
        let mut h = Header::new(TYPE_MGMT, BEACON, 0, BROADCAST, self.host, self.host);
        h.set_sequence(seq);
        let mut out = Vec::with_capacity(256);
        h.write(&mut out);
        out.extend_from_slice(&tsf.to_le_bytes());
        out.extend_from_slice(&BEACON_INTERVAL_TU.to_le_bytes());
        out.extend_from_slice(&CAPABILITY.to_le_bytes());
        frame::push_element(&mut out, IE_SSID, &[0; 8]);
        frame::push_element(&mut out, IE_RATES, &self.rates);
        frame::push_element(&mut out, IE_DS, &[self.channel]);
        frame::push_element(&mut out, IE_TIM, &[0, 1, 1, 0]);
        frame::push_element(&mut out, IE_COUNTRY, &[b'J', b'P', 0, 1, 13, 20]);
        frame::push_element(&mut out, IE_ERP, &[0]);
        out.extend_from_slice(&self.beacon_elements(beacon_key, nodes));
        out
    }

    /// Builds the vendor elements a host puts in its beacon.
    pub fn beacon_elements(&self, beacon_key: &Key, nodes: &[NodeInfo]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut first = NINTENDO_OUI.to_vec();
        first.extend_from_slice(&[0x14, 0x0a, 0x00, 0x00]);
        frame::push_element(&mut out, IE_VENDOR, &first);

        let mut info = NINTENDO_OUI.to_vec();
        info.push(TYPE_INFO);
        info.extend_from_slice(&self.comm_id.to_be_bytes());
        info.push(self.id8);
        info.push(self.updates);
        info.extend_from_slice(&self.attributes.to_be_bytes());
        info.extend_from_slice(&self.network_id.to_be_bytes());
        info.push(self.total_nodes);
        info.push(self.max_nodes);
        info.extend_from_slice(&[0x00, 0x01]);
        info.resize(INFO_LEN, 0);
        info[0x33] = self.appdata.len() as u8;
        info.extend_from_slice(&self.appdata);
        let hash = Sha1::digest(&info);
        info[HASH_AT..HASH_AT + 20].copy_from_slice(&hash);
        frame::push_element(&mut out, IE_VENDOR, &info);

        let mut plain = vec![0u8; 0x12];
        for i in 0..self.max_nodes as usize {
            match nodes.get(i) {
                Some(n) => n.write_beacon_entry(&mut plain),
                None => plain.extend_from_slice(&[0; BEACON_ENTRY_LEN]),
            }
        }
        let digest = Md5::digest(&plain[0x10..]);
        plain[..0x10].copy_from_slice(&digest);
        Ctr::new(beacon_key.into(), &self.node_ctr().into()).apply_keystream(&mut plain);
        for (i, chunk) in plain.chunks(FIRST_NODES_LEN).enumerate() {
            let mut tag = NINTENDO_OUI.to_vec();
            tag.push(if i == 0 { TYPE_NODES } else { TYPE_NODES_MORE });
            tag.extend_from_slice(chunk);
            frame::push_element(&mut out, IE_VENDOR, &tag);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uds::MII_MAKER;
    use crate::wifi::frame::{BROADCAST, push_element};

    fn network() -> NetworkInfo {
        NetworkInfo {
            host: [0x02, 1, 2, 3, 4, 5],
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

    #[test]
    fn beacon_round_trip() {
        let key = [9u8; 16];
        let n = network();
        let mut host = NodeInfo::new(42, "Host");
        host.node_id = 1;
        let mut f = Vec::new();
        Header::new(TYPE_MGMT, BEACON, 0, BROADCAST, n.host, n.host).write(&mut f);
        f.extend_from_slice(&[0; 8]);
        f.extend_from_slice(&[100, 0, 0x31, 0x04]);
        push_element(&mut f, frame::IE_RATES, &frame::RATES);
        push_element(&mut f, IE_DS, &[6]);
        f.extend_from_slice(&n.beacon_elements(&key, std::slice::from_ref(&host)));

        let p = NetworkInfo::from_beacon(&f).unwrap();
        assert!(p.matches(&MII_MAKER));
        assert_eq!(
            NetworkInfo {
                encrypted_nodes: vec![],
                ..p.clone()
            },
            n
        );
        assert_eq!(p.nodes(&key).unwrap(), vec![host]);
        assert!(p.nodes(&[8u8; 16]).is_none());
    }

    #[test]
    fn info_hash_covers_tag() {
        let e = network().beacon_elements(&[0; 16], &[]);
        let info = frame::elements(&e).nth(1).unwrap().1.to_vec();
        let mut zeroed = info.clone();
        zeroed[HASH_AT..HASH_AT + 20].fill(0);
        assert_eq!(&info[HASH_AT..HASH_AT + 20], &Sha1::digest(&zeroed)[..]);
    }
}
