//! The 512-byte container a wrapped Mii travels in.

use crate::Error;
use crate::mii::WRAPPED_SIZE;

pub const SEGMENT_SIZE: usize = 512;
const HEADER: usize = 0x18;
const RECORD_AT: usize = 4 + HEADER;
const MAGIC: &[u8; 4] = b"CFPB";

/// Length word a 3DS sender writes.
pub const LENGTH_3DS: u32 = 0xa0;
/// Length word a Wii U sender writes.
pub const LENGTH_WII_U: u32 = 0xf0;

pub fn build(wrapped: &[u8; WRAPPED_SIZE], length_word: u32) -> [u8; SEGMENT_SIZE] {
    let mut out = [0u8; SEGMENT_SIZE];
    out[..4].copy_from_slice(&length_word.to_le_bytes());
    out[4..8].copy_from_slice(MAGIC);
    out[8] = 1;
    out[0x18..0x1a].copy_from_slice(&1u16.to_le_bytes());
    out[RECORD_AT..RECORD_AT + WRAPPED_SIZE].copy_from_slice(wrapped);
    out
}

/// Returns the first populated wrapped record.
pub fn parse(data: &[u8]) -> Result<[u8; WRAPPED_SIZE], Error> {
    if data.len() < RECORD_AT + WRAPPED_SIZE {
        return Err(Error::Format("container too short".into()));
    }
    let h = &data[4..];
    if &h[..4] != MAGIC {
        return Err(Error::Format("not a CFPB container".into()));
    }
    let count = u16::from_le_bytes([h[0x14], h[0x15]]) as usize;
    let bitmap = &h[4..0x14];
    let available = (data.len() - RECORD_AT) / WRAPPED_SIZE;
    let index = (0..count.min(available).min(128))
        .find(|&i| bitmap[i / 8] & (1 << (i % 8)) != 0)
        .ok_or_else(|| Error::Format("container holds no record".into()))?;
    let at = RECORD_AT + index * WRAPPED_SIZE;
    Ok(data[at..at + WRAPPED_SIZE].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_then_parse() {
        let wrapped = [0x5a; WRAPPED_SIZE];
        let c = build(&wrapped, LENGTH_3DS);
        assert_eq!(
            &c[..12],
            &[0xa0, 0, 0, 0, b'C', b'F', b'P', b'B', 1, 0, 0, 0]
        );
        assert_eq!(&c[0x18..0x1c], &[1, 0, 0, 0]);
        assert!(c[RECORD_AT + WRAPPED_SIZE..].iter().all(|&b| b == 0));
        assert_eq!(parse(&c).unwrap(), wrapped);
    }

    #[test]
    fn rejects_empty_and_foreign() {
        let mut c = build(&[1; WRAPPED_SIZE], LENGTH_WII_U);
        c[8] = 0;
        assert!(parse(&c).is_err());
        c[4] = b'X';
        assert!(parse(&c).is_err());
    }
}
