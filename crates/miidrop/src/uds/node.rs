//! Node information exchanged when joining and carried in beacons.

use crate::Error;

pub const NODE_INFO_LEN: usize = 0x28;
/// Node info as stored in a beacon: console ID, name and node ID.
pub const BEACON_ENTRY_LEN: usize = 0x1e;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeInfo {
    pub console_id: u64,
    pub name: [u16; 10],
    pub node_id: u16,
}

impl NodeInfo {
    pub fn new(console_id: u64, name: &str) -> Self {
        let mut units = [0u16; 10];
        for (slot, u) in units.iter_mut().zip(name.encode_utf16()) {
            *slot = u;
        }
        NodeInfo {
            console_id,
            name: units,
            node_id: 0,
        }
    }

    pub fn name(&self) -> String {
        let end = self.name.iter().position(|&u| u == 0).unwrap_or(10);
        String::from_utf16_lossy(&self.name[..end])
    }

    fn write_identity(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.console_id.to_be_bytes());
        for u in self.name {
            out.extend_from_slice(&u.to_be_bytes());
        }
    }

    fn parse_identity(b: &[u8]) -> Self {
        let mut name = [0u16; 10];
        for (i, u) in name.iter_mut().enumerate() {
            *u = u16::from_be_bytes([b[8 + 2 * i], b[9 + 2 * i]]);
        }
        NodeInfo {
            console_id: u64::from_be_bytes(b[..8].try_into().unwrap()),
            name,
            node_id: 0,
        }
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        self.write_identity(out);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&self.node_id.to_be_bytes());
        out.extend_from_slice(&[0; 6]);
    }

    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        if b.len() < NODE_INFO_LEN {
            return Err(Error::Format("node info too short".into()));
        }
        let mut n = Self::parse_identity(b);
        n.node_id = u16::from_be_bytes([b[0x20], b[0x21]]);
        Ok(n)
    }

    pub fn write_beacon_entry(&self, out: &mut Vec<u8>) {
        self.write_identity(out);
        out.extend_from_slice(&self.node_id.to_be_bytes());
    }

    pub fn parse_beacon_entry(b: &[u8]) -> Self {
        let mut n = Self::parse_identity(b);
        n.node_id = u16::from_be_bytes([b[0x1c], b[0x1d]]);
        n
    }
}

/// Parses a fixed-size table of node infos, keeping only occupied entries.
pub fn parse_table(b: &[u8]) -> Result<Vec<NodeInfo>, Error> {
    let mut out = vec![];
    for chunk in b.chunks(NODE_INFO_LEN) {
        let n = NodeInfo::parse(chunk)?;
        if n.node_id != 0 {
            out.push(n);
        }
    }
    Ok(out)
}

pub fn write_table(nodes: &[NodeInfo], out: &mut Vec<u8>) {
    for i in 0..super::MAX_NODES {
        nodes.get(i).cloned().unwrap_or_default().write(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut n = NodeInfo::new(0x0102_0304_0506_0708, "Fin");
        n.node_id = 2;
        let mut b = vec![];
        n.write(&mut b);
        assert_eq!(b.len(), NODE_INFO_LEN);
        assert_eq!(&b[8..14], &[0, b'F', 0, b'i', 0, b'n']);
        assert_eq!(NodeInfo::parse(&b).unwrap(), n);
        let mut e = vec![];
        n.write_beacon_entry(&mut e);
        assert_eq!(e.len(), BEACON_ENTRY_LEN);
        assert_eq!(NodeInfo::parse_beacon_entry(&e), n);
        assert_eq!(n.name(), "Fin");
    }
}
