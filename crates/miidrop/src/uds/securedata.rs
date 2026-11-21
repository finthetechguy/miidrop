//! LLC/SNAP encapsulation and the UDS header on data frames.

use crate::Error;

pub const ETHERTYPE_EAPOL: u16 = 0x888e;
pub const ETHERTYPE_SECURE: u16 = 0x876d;
pub const LLC_LEN: usize = 8;
pub const HEADER_LEN: usize = 14;

pub const CHANNEL_NODE_LIST: u16 = 0x101;
pub const CHANNEL_KEEPALIVE: u16 = 0x103;

pub fn llc(ethertype: u16) -> [u8; LLC_LEN] {
    let e = ethertype.to_be_bytes();
    [0xaa, 0xaa, 0x03, 0, 0, 0, e[0], e[1]]
}

/// Splits a decrypted body into its ethertype and payload.
pub fn parse_llc(body: &[u8]) -> Result<(u16, &[u8]), Error> {
    if body.len() < LLC_LEN || body[..6] != [0xaa, 0xaa, 0x03, 0, 0, 0] {
        return Err(Error::Format("missing LLC/SNAP header".into()));
    }
    Ok((u16::from_be_bytes([body[6], body[7]]), &body[LLC_LEN..]))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub channel: u16,
    pub seq: u16,
    pub dst: u16,
    pub src: u16,
    pub payload: Vec<u8>,
}

impl Packet {
    /// Returns the full frame body, LLC header included.
    pub fn to_body(&self) -> Vec<u8> {
        let size = (HEADER_LEN + self.payload.len()) as u16;
        let mut out = Vec::with_capacity(LLC_LEN + size as usize);
        out.extend_from_slice(&llc(ETHERTYPE_SECURE));
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(size - 4).to_be_bytes());
        out.extend_from_slice(&self.channel.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.dst.to_be_bytes());
        out.extend_from_slice(&self.src.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Parses the bytes after the LLC header.
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        if b.len() < HEADER_LEN {
            return Err(Error::Format("UDS header too short".into()));
        }
        let word = |at: usize| u16::from_be_bytes([b[at], b[at + 1]]);
        if word(0) as usize != b.len() || word(4) as usize + 4 != b.len() {
            return Err(Error::Format("UDS header sizes do not match the frame".into()));
        }
        Ok(Packet {
            channel: word(6),
            seq: word(8),
            dst: word(10),
            src: word(12),
            payload: b[HEADER_LEN..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_layout() {
        let p = Packet {
            channel: CHANNEL_KEEPALIVE,
            seq: 0xba,
            dst: 1,
            src: 2,
            payload: vec![0],
        };
        let body = p.to_body();
        assert_eq!(
            body,
            [
                0xaa, 0xaa, 3, 0, 0, 0, 0x87, 0x6d, 0, 0x0f, 0, 1, 0, 0x0b, 1, 3, 0, 0xba, 0, 1, 0,
                2, 0
            ]
        );
        let (ethertype, rest) = parse_llc(&body).unwrap();
        assert_eq!(ethertype, ETHERTYPE_SECURE);
        assert_eq!(Packet::parse(rest).unwrap(), p);
        assert!(Packet::parse(&rest[..rest.len() - 1]).is_err());
    }
}
