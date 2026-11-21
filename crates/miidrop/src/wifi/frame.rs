//! 802.11 MAC headers, information elements and the management frames UDS uses.

use crate::Error;

pub type Mac = [u8; 6];

pub const BROADCAST: Mac = [0xff; 6];
pub const HEADER_LEN: usize = 24;

pub const TYPE_MGMT: u8 = 0;
pub const TYPE_DATA: u8 = 2;

pub const ASSOC_REQUEST: u8 = 0;
pub const ASSOC_RESPONSE: u8 = 1;
pub const PROBE_REQUEST: u8 = 4;
pub const BEACON: u8 = 8;
pub const DISASSOC: u8 = 10;
pub const AUTH: u8 = 11;
pub const DEAUTH: u8 = 12;

pub const FC_TO_DS: u16 = 0x0100;
pub const FC_FROM_DS: u16 = 0x0200;
pub const FC_RETRY: u16 = 0x0800;
pub const FC_PROTECTED: u16 = 0x4000;

/// Capability bits every UDS node advertises.
pub const CAPABILITY: u16 = 0x0431;

pub const IE_SSID: u8 = 0;
pub const IE_RATES: u8 = 1;
pub const IE_DS: u8 = 3;
pub const IE_TIM: u8 = 5;
pub const IE_ERP: u8 = 42;
pub const IE_EXT_RATES: u8 = 50;
pub const IE_VENDOR: u8 = 221;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub fc: u16,
    pub duration: u16,
    pub addr1: Mac,
    pub addr2: Mac,
    pub addr3: Mac,
    pub seq_ctrl: u16,
}

impl Header {
    pub fn new(ftype: u8, subtype: u8, flags: u16, addr1: Mac, addr2: Mac, addr3: Mac) -> Self {
        let fc = ((subtype as u16) << 4) | ((ftype as u16) << 2) | flags;
        Header {
            fc,
            duration: 0,
            addr1,
            addr2,
            addr3,
            seq_ctrl: 0,
        }
    }

    pub fn parse(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() < HEADER_LEN {
            return Err(Error::Format("802.11 frame too short".into()));
        }
        let mac = |at: usize| -> Mac { frame[at..at + 6].try_into().unwrap() };
        Ok(Header {
            fc: u16::from_le_bytes([frame[0], frame[1]]),
            duration: u16::from_le_bytes([frame[2], frame[3]]),
            addr1: mac(4),
            addr2: mac(10),
            addr3: mac(16),
            seq_ctrl: u16::from_le_bytes([frame[22], frame[23]]),
        })
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.fc.to_le_bytes());
        out.extend_from_slice(&self.duration.to_le_bytes());
        out.extend_from_slice(&self.addr1);
        out.extend_from_slice(&self.addr2);
        out.extend_from_slice(&self.addr3);
        out.extend_from_slice(&self.seq_ctrl.to_le_bytes());
    }

    pub fn ftype(&self) -> u8 {
        ((self.fc >> 2) & 3) as u8
    }

    pub fn subtype(&self) -> u8 {
        ((self.fc >> 4) & 0xf) as u8
    }

    pub fn is(&self, ftype: u8, subtype: u8) -> bool {
        self.fc & 3 == 0 && self.ftype() == ftype && self.subtype() == subtype
    }

    pub fn protected(&self) -> bool {
        self.fc & FC_PROTECTED != 0
    }

    pub fn sequence(&self) -> u16 {
        self.seq_ctrl >> 4
    }

    pub fn set_sequence(&mut self, seq: u16) {
        self.seq_ctrl = (seq & 0x0fff) << 4;
    }

    /// Address of the BSS, wherever the distribution bits put it.
    pub fn bssid(&self) -> Mac {
        match self.fc & (FC_TO_DS | FC_FROM_DS) {
            FC_TO_DS => self.addr1,
            FC_FROM_DS => self.addr2,
            _ => self.addr3,
        }
    }
}

/// Iterates over complete information elements, stopping at the first truncated one.
pub fn elements(mut body: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    std::iter::from_fn(move || {
        if body.len() < 2 || body.len() < 2 + body[1] as usize {
            return None;
        }
        let (id, len) = (body[0], body[1] as usize);
        let data = &body[2..2 + len];
        body = &body[2 + len..];
        Some((id, data))
    })
}

pub fn push_element(out: &mut Vec<u8>, id: u8, data: &[u8]) {
    debug_assert!(data.len() <= 255);
    out.push(id);
    out.push(data.len() as u8);
    out.extend_from_slice(data);
}

/// Rates a UDS host advertises, all marked basic.
pub const RATES: [u8; 7] = [0x96, 0x98, 0xa4, 0xb0, 0xc8, 0xe0, 0xec];
const CLIENT_LOW_RATES: [u8; 5] = [0x02, 0x04, 0x0b, 0x0c, 0x12];

pub fn auth(from: Mac, to: Mac, bssid: Mac, transaction: u16, status: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(30);
    Header::new(TYPE_MGMT, AUTH, 0, to, from, bssid).write(&mut out);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&transaction.to_le_bytes());
    out.extend_from_slice(&status.to_le_bytes());
    out
}

/// The SSID a client associates with is the network ID in upper-case hex.
pub fn network_ssid(network_id: u32) -> [u8; 8] {
    format!("{network_id:08X}").as_bytes().try_into().unwrap()
}

/// Builds an association request offering the low rates plus the host's own.
pub fn assoc_request(from: Mac, bssid: Mac, network_id: u32, host_rates: &[u8]) -> Vec<u8> {
    let mut rates = CLIENT_LOW_RATES.to_vec();
    rates.extend_from_slice(host_rates);
    let (main, ext) = rates.split_at(rates.len().min(8));
    let mut out = Vec::with_capacity(64);
    Header::new(TYPE_MGMT, ASSOC_REQUEST, 0, bssid, from, bssid).write(&mut out);
    out.extend_from_slice(&CAPABILITY.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    push_element(&mut out, IE_SSID, &network_ssid(network_id));
    push_element(&mut out, IE_RATES, main);
    if !ext.is_empty() {
        push_element(&mut out, IE_EXT_RATES, ext);
    }
    out
}

pub fn assoc_response(bssid: Mac, to: Mac, aid: u16, network_id: u32, channel: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    Header::new(TYPE_MGMT, ASSOC_RESPONSE, 0, to, bssid, bssid).write(&mut out);
    out.extend_from_slice(&CAPABILITY.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(aid | 0xc000).to_le_bytes());
    push_element(&mut out, IE_SSID, &network_ssid(network_id));
    push_element(&mut out, IE_RATES, &RATES);
    push_element(&mut out, IE_DS, &[channel]);
    push_element(&mut out, IE_ERP, &[0]);
    out
}

pub fn deauth(from: Mac, to: Mac, bssid: Mac, reason: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(26);
    Header::new(TYPE_MGMT, DEAUTH, 0, to, from, bssid).write(&mut out);
    out.extend_from_slice(&reason.to_le_bytes());
    out
}

/// Status and association ID from an association response body.
pub fn parse_assoc_response(body: &[u8]) -> Result<(u16, u16), Error> {
    if body.len() < 6 {
        return Err(Error::Format("association response too short".into()));
    }
    let status = u16::from_le_bytes([body[2], body[3]]);
    let aid = u16::from_le_bytes([body[4], body[5]]) & 0x3fff;
    Ok((status, aid))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Mac = [0x02, 0, 0, 0, 0, 0xa];
    const B: Mac = [0x02, 0, 0, 0, 0, 0xb];

    #[test]
    fn header_round_trip() {
        let mut h = Header::new(TYPE_DATA, 0, FC_PROTECTED | FC_FROM_DS, BROADCAST, B, B);
        h.set_sequence(0x123);
        let mut out = Vec::new();
        h.write(&mut out);
        assert_eq!(&out[..2], &[0x08, 0x42]);
        let p = Header::parse(&out).unwrap();
        assert_eq!(p, h);
        assert!(p.is(TYPE_DATA, 0) && p.protected());
        assert_eq!((p.sequence(), p.bssid()), (0x123, B));
    }

    #[test]
    fn assoc_request_layout() {
        let f = assoc_request(A, B, 0xbf3b_fa3e, &RATES);
        assert_eq!(&f[..2], &[0, 0]);
        assert_eq!(
            &f[24..],
            &[
                0x31, 0x04, 0x01, 0x00, 0x00, 0x08, b'B', b'F', b'3', b'B', b'F', b'A', b'3', b'E',
                0x01, 0x08, 0x02, 0x04, 0x0b, 0x0c, 0x12, 0x96, 0x98, 0xa4, 0x32, 0x04, 0xb0, 0xc8,
                0xe0, 0xec
            ]
        );
    }

    #[test]
    fn assoc_response_parses() {
        let f = assoc_response(B, A, 1, 0x20f7_debf, 6);
        assert_eq!(
            &f[24..],
            &[
                0x31, 0x04, 0, 0, 0x01, 0xc0, 0, 8, b'2', b'0', b'F', b'7', b'D', b'E', b'B', b'F',
                1, 7, 0x96, 0x98, 0xa4, 0xb0, 0xc8, 0xe0, 0xec, 3, 1, 6, 0x2a, 1, 0
            ]
        );
        assert_eq!(parse_assoc_response(&f[24..]).unwrap(), (0, 1));
    }

    #[test]
    fn assoc_request_follows_host_rates() {
        let f = assoc_request(A, B, 1, &[0x96, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c]);
        let ies: Vec<_> = elements(&f[28..]).collect();
        assert_eq!(ies[1].1, &[0x02, 0x04, 0x0b, 0x0c, 0x12, 0x96, 0x18, 0x24]);
        assert_eq!(ies[2].1, &[0x30, 0x48, 0x60, 0x6c]);
    }

    #[test]
    fn truncated_elements_stop() {
        let ies: Vec<_> = elements(&[0, 1, 9, 3, 5, 1]).collect();
        assert_eq!(ies, vec![(0, &[9u8][..])]);
    }
}
