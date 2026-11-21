//! Reading classic pcap files of 802.11 traffic.

use crate::Error;

pub const LINKTYPE_IEEE802_11: u32 = 105;
pub const LINKTYPE_RADIOTAP: u32 = 127;

pub struct Packet<'a> {
    pub timestamp_us: u64,
    pub data: &'a [u8],
}

pub struct Pcap<'a> {
    pub linktype: u32,
    data: &'a [u8],
    at: usize,
    big_endian: bool,
    nanos: bool,
}

impl<'a> Pcap<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() < 24 {
            return Err(Error::Format("pcap too short".into()));
        }
        let magic = u32::from_le_bytes(data[..4].try_into().unwrap());
        let (big_endian, nanos) = match magic {
            0xa1b2_c3d4 => (false, false),
            0xa1b2_3c4d => (false, true),
            0xd4c3_b2a1 => (true, false),
            0x4d3c_b2a1 => (true, true),
            _ => return Err(Error::Format("not a pcap file".into())),
        };
        let mut p = Pcap {
            linktype: 0,
            data,
            at: 24,
            big_endian,
            nanos,
        };
        p.linktype = p.u32(20) & 0x0fff_ffff;
        Ok(p)
    }

    fn u32(&self, at: usize) -> u32 {
        let b: [u8; 4] = self.data[at..at + 4].try_into().unwrap();
        if self.big_endian {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        }
    }
}

impl<'a> Iterator for Pcap<'a> {
    type Item = Result<Packet<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.at + 16 > self.data.len() {
            return None;
        }
        let secs = self.u32(self.at) as u64;
        let frac = self.u32(self.at + 4) as u64;
        let len = self.u32(self.at + 8) as usize;
        let start = self.at + 16;
        if start + len > self.data.len() {
            self.at = self.data.len();
            return Some(Err(Error::Format("truncated pcap record".into())));
        }
        self.at = start + len;
        let timestamp_us = secs * 1_000_000 + if self.nanos { frac / 1000 } else { frac };
        Some(Ok(Packet {
            timestamp_us,
            data: &self.data[start..start + len],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_records() {
        let mut f = vec![];
        f.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        f.extend_from_slice(&[2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0]);
        f.extend_from_slice(&127u32.to_le_bytes());
        for (s, payload) in [(1u32, &b"ab"[..]), (2, &b"c"[..])] {
            f.extend_from_slice(&s.to_le_bytes());
            f.extend_from_slice(&5u32.to_le_bytes());
            f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            f.extend_from_slice(payload);
        }
        let p = Pcap::new(&f).unwrap();
        assert_eq!(p.linktype, LINKTYPE_RADIOTAP);
        let packets: Vec<_> = p
            .map(|r| r.unwrap())
            .map(|p| (p.timestamp_us, p.data.to_vec()))
            .collect();
        assert_eq!(
            packets,
            vec![(1_000_005, b"ab".to_vec()), (2_000_005, b"c".to_vec())]
        );
    }
}
