//! Radiotap headers on received and injected frames.

use crate::Error;

const FLAGS: u32 = 1 << 1;
const RATE: u32 = 1 << 2;
const CHANNEL: u32 = 1 << 3;
const SIGNAL: u32 = 1 << 5;
const TX_FLAGS: u32 = 1 << 15;
const EXT: u32 = 1 << 31;

const FLAG_FCS: u8 = 0x10;
const FLAG_BAD_FCS: u8 = 0x40;
const TX_NO_ACK: u16 = 0x0008;

/// What miidrop uses from a received frame's radiotap header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxInfo {
    pub frequency: Option<u16>,
    pub signal: Option<i8>,
    pub bad_fcs: bool,
}

/// Splits a captured packet into radiotap information and the 802.11 frame without FCS.
pub fn parse(packet: &[u8]) -> Result<(RxInfo, &[u8]), Error> {
    if packet.len() < 8 || packet[0] != 0 {
        return Err(Error::Format("not a radiotap packet".into()));
    }
    let len = u16::from_le_bytes([packet[2], packet[3]]) as usize;
    if len < 8 || len > packet.len() {
        return Err(Error::Format("bad radiotap length".into()));
    }
    let mut words = vec![];
    let mut at = 4;
    loop {
        if at + 4 > len {
            return Err(Error::Format("truncated radiotap presence".into()));
        }
        let w = u32::from_le_bytes(packet[at..at + 4].try_into().unwrap());
        words.push(w);
        at += 4;
        if w & EXT == 0 {
            break;
        }
    }
    let present = words[0];
    let mut info = RxInfo::default();
    let mut flags = 0u8;
    // Fields follow in bit order, each aligned to its own size.
    const FIELDS: [(usize, usize); 6] = [(8, 8), (1, 1), (1, 1), (2, 4), (2, 2), (1, 1)];
    for (bit, &(align, size)) in FIELDS.iter().enumerate() {
        if present & (1 << bit) == 0 {
            continue;
        }
        at = at.next_multiple_of(align);
        if at + size > len {
            return Err(Error::Format("truncated radiotap field".into()));
        }
        let field = &packet[at..at + size];
        match 1u32 << bit {
            FLAGS => flags = field[0],
            CHANNEL => info.frequency = Some(u16::from_le_bytes([field[0], field[1]])),
            SIGNAL => info.signal = Some(field[0] as i8),
            _ => {}
        }
        at += size;
    }
    info.bad_fcs = flags & FLAG_BAD_FCS != 0;
    let mut frame = &packet[len..];
    if flags & FLAG_FCS != 0 {
        if frame.len() < 4 {
            return Err(Error::Format("frame shorter than its FCS".into()));
        }
        frame = &frame[..frame.len() - 4];
    }
    Ok((info, frame))
}

/// Prefixes a frame for injection at a fixed rate, without waiting for an ACK on group frames.
pub fn build(frame: &[u8], rate_500kbps: u8, no_ack: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + frame.len());
    out.extend_from_slice(&[0, 0, 12, 0]);
    out.extend_from_slice(&(RATE | TX_FLAGS).to_le_bytes());
    out.push(rate_500kbps);
    out.push(0);
    out.extend_from_slice(&(if no_ack { TX_NO_ACK } else { 0 }).to_le_bytes());
    out.extend_from_slice(frame);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_captured_header() {
        // Present words with TSFT, flags, rate, channel and signal, plus an extension word.
        let mut p = vec![0, 0, 32, 0];
        p.extend_from_slice(&0x8000_002fu32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&[0; 12]);
        p.extend_from_slice(&[FLAG_FCS, 0x0c, 0x85, 0x09, 0xc0, 0x00, 0xd0]);
        p.push(0);
        p.extend_from_slice(b"frame");
        p.extend_from_slice(&[1, 2, 3, 4]);
        let (info, frame) = parse(&p).unwrap();
        assert_eq!(
            info,
            RxInfo {
                frequency: Some(2437),
                signal: Some(-48),
                bad_fcs: false
            }
        );
        assert_eq!(frame, b"frame");
    }

    #[test]
    fn build_then_parse() {
        let p = build(b"xyz", 4, true);
        assert_eq!(&p[8..12], &[4, 0, 8, 0]);
        assert_eq!(parse(&p).unwrap().1, b"xyz");
    }
}
