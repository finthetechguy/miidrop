//! Loads keys from a `name=hex` key file without ever formatting or logging them.

use std::fmt;
use std::path::Path;

use crate::Error;

pub type Key = [u8; 16];

const SCRAMBLER: u128 = 0x1FF9_E9AA_C5FE_0408_0245_91DC_5D52_768A;

/// Derives a normal key from a key pair the way the console's key scrambler does.
pub fn scramble(key_x: &Key, key_y: &Key) -> Key {
    let x = u128::from_be_bytes(*key_x);
    let y = u128::from_be_bytes(*key_y);
    (x.rotate_left(2) ^ y)
        .wrapping_add(SCRAMBLER)
        .rotate_left(87)
        .to_be_bytes()
}

/// The keys miidrop needs.
#[derive(Clone)]
pub struct Keys {
    /// Base key for the local wireless session key.
    pub session: Key,
    /// Key that protects Mii records inside a container.
    pub record: Key,
    /// Key for the encrypted node list in beacons, needed only for hosting.
    pub beacon: Option<Key>,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keys")
            .field("beacon", &self.beacon.is_some())
            .finish_non_exhaustive()
    }
}

impl Keys {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::Keys(e.to_string()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        let get = |name: &str| -> Result<Option<Key>, Error> {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                if k.trim().eq_ignore_ascii_case(name) {
                    return parse_key(v.trim())
                        .map(Some)
                        .ok_or_else(|| Error::Keys(format!("{name} is not 16 hex bytes")));
                }
            }
            Ok(None)
        };
        let session =
            get("slot0x2DKeyN")?.ok_or_else(|| Error::Keys("slot0x2DKeyN missing".into()))?;
        let record = match get("slot0x31KeyN")? {
            Some(k) => k,
            None => match (get("slot0x31KeyX")?, get("slot0x31KeyY")?) {
                (Some(x), Some(y)) => scramble(&x, &y),
                _ => return Err(Error::Keys("slot0x31KeyX/KeyY missing".into())),
            },
        };
        let beacon = get("nwmBeaconKey")?;
        Ok(Keys {
            session,
            record,
            beacon,
        })
    }
}

fn parse_key(hex: &str) -> Option<Key> {
    let bytes = hex.as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in bytes.chunks(2).enumerate() {
        let s = std::str::from_utf8(pair).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scramble_is_rotations_and_add() {
        let x = [0u8; 16];
        let y = [0u8; 16];
        assert_eq!(scramble(&x, &y), SCRAMBLER.rotate_left(87).to_be_bytes());
    }

    #[test]
    fn parse_prefers_normal_key_and_reports_missing() {
        let text = "slot0x2DKeyN=000102030405060708090a0b0c0d0e0f\nslot0x31KeyN=ffffffffffffffffffffffffffffffff\n";
        let k = Keys::parse(text).unwrap();
        assert_eq!(k.session[15], 0x0f);
        assert_eq!(k.record, [0xff; 16]);
        assert!(k.beacon.is_none());
        assert!(Keys::parse("slot0x31KeyN=00").is_err());
    }

    #[test]
    fn debug_hides_keys() {
        let text = "slot0x2DKeyN=11111111111111111111111111111111\nslot0x31KeyN=11111111111111111111111111111111\n";
        let s = format!("{:?}", Keys::parse(text).unwrap());
        assert!(!s.contains("17") && !s.contains("11, "));
    }
}
