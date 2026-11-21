//! AES-CCM record protection, with the tag taken over the body zero-padded to a whole block.

use aes::Aes128;
use ccm::Ccm;
use ccm::aead::generic_array::GenericArray;
use ccm::aead::{AeadInPlace, KeyInit};
use ccm::consts::{U12, U16};

use super::{RECORD_SIZE, Record};
use crate::Error;
use crate::keys::Key;

pub const WRAPPED_SIZE: usize = RECORD_SIZE + 16;

const NONCE_AT: usize = 0xc;
const NONCE_LEN: usize = 8;
const BODY_LEN: usize = RECORD_SIZE - NONCE_LEN;
const PADDED_LEN: usize = BODY_LEN.div_ceil(16) * 16;

type RecordCcm = Ccm<Aes128, U16, U12>;

fn nonce(bytes: &[u8]) -> GenericArray<u8, U12> {
    let mut n = [0u8; 12];
    n[..NONCE_LEN].copy_from_slice(bytes);
    n.into()
}

/// Runs CCM over the body padded to a whole block; returns the transformed body and tag.
fn seal(key: &Key, nonce_bytes: &[u8], body: &[u8; BODY_LEN]) -> ([u8; BODY_LEN], [u8; 16]) {
    let mut buf = [0u8; PADDED_LEN];
    buf[..BODY_LEN].copy_from_slice(body);
    let tag = RecordCcm::new(key.into())
        .encrypt_in_place_detached(&nonce(nonce_bytes), &[], &mut buf)
        .expect("record fits CCM limits");
    (buf[..BODY_LEN].try_into().unwrap(), tag.into())
}

pub fn wrap(key: &Key, record: &Record) -> [u8; WRAPPED_SIZE] {
    let r = record.as_bytes();
    let id = &r[NONCE_AT..NONCE_AT + NONCE_LEN];
    let mut body = [0u8; BODY_LEN];
    body[..NONCE_AT].copy_from_slice(&r[..NONCE_AT]);
    body[NONCE_AT..].copy_from_slice(&r[NONCE_AT + NONCE_LEN..]);
    let (sealed, tag) = seal(key, id, &body);
    let mut out = [0u8; WRAPPED_SIZE];
    out[..NONCE_LEN].copy_from_slice(id);
    out[NONCE_LEN..NONCE_LEN + BODY_LEN].copy_from_slice(&sealed);
    out[NONCE_LEN + BODY_LEN..].copy_from_slice(&tag);
    out
}

pub fn unwrap(key: &Key, wrapped: &[u8]) -> Result<Record, Error> {
    if wrapped.len() != WRAPPED_SIZE {
        return Err(Error::Format(format!(
            "wrapped record must be {WRAPPED_SIZE} bytes"
        )));
    }
    let id = &wrapped[..NONCE_LEN];
    let sealed: [u8; BODY_LEN] = wrapped[NONCE_LEN..NONCE_LEN + BODY_LEN].try_into().unwrap();
    // The cipher is a counter mode, so sealing the ciphertext yields the plaintext.
    let (body, _) = seal(key, id, &sealed);
    let (_, expected) = seal(key, id, &body);
    let diff = expected
        .iter()
        .zip(&wrapped[NONCE_LEN + BODY_LEN..])
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if diff != 0 {
        return Err(Error::Authentication);
    }
    let mut r = [0u8; RECORD_SIZE];
    r[..NONCE_AT].copy_from_slice(&body[..NONCE_AT]);
    r[NONCE_AT..NONCE_AT + NONCE_LEN].copy_from_slice(id);
    r[NONCE_AT + NONCE_LEN..].copy_from_slice(&body[NONCE_AT..]);
    Ok(Record(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mii::record::tests::sample;

    #[test]
    fn round_trip_and_layout() {
        let key = [7u8; 16];
        let r = sample();
        let w = wrap(&key, &r);
        assert_eq!(&w[..8], &r.as_bytes()[0xc..0x14]);
        assert_eq!(unwrap(&key, &w).unwrap(), r);
    }

    #[test]
    fn tampering_fails() {
        let key = [7u8; 16];
        let mut w = wrap(&key, &sample());
        w[20] ^= 1;
        assert!(matches!(unwrap(&key, &w), Err(Error::Authentication)));
        assert!(unwrap(&[8u8; 16], &wrap(&key, &sample())).is_err());
    }

    #[test]
    fn known_vector_with_real_key() {
        let Some(path) = std::env::var_os("MIIDROP_KEYS") else {
            return;
        };
        let keys = crate::keys::Keys::load(std::path::Path::new(&path)).unwrap();
        let w = wrap(&keys.record, &sample());
        let hex: String = w.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, EXPECTED);
    }

    const EXPECTED: &str = "90001234020000000abbf91e90656f367c0ee531a8a9e23e5aca900e34f44241e2b93c565d80eae44768eef41e36f75287abee5d7b4ce08ad9fd3e1f755739fac04ac3ab7f74449b8c677be06c67b60135feb810a467bb442e9c490ac799ba1434c8e4fe62675f140cff585344964665";
}
