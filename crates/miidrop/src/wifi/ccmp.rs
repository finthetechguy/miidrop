//! CCMP protection for three-address, non-QoS data frames.

use aes::Aes128;
use ccm::Ccm;
use ccm::aead::generic_array::GenericArray;
use ccm::aead::{AeadInPlace, KeyInit};
use ccm::consts::{U8, U13};

use super::frame::{FC_PROTECTED, HEADER_LEN, Header};
use crate::Error;
use crate::keys::Key;

pub const CCMP_HEADER_LEN: usize = 8;
pub const MIC_LEN: usize = 8;

type FrameCcm = Ccm<Aes128, U8, U13>;

fn nonce(addr2: &[u8], pn: u64) -> [u8; 13] {
    let mut n = [0u8; 13];
    n[1..7].copy_from_slice(addr2);
    n[7..].copy_from_slice(&pn.to_be_bytes()[2..]);
    n
}

fn aad(header: &[u8]) -> [u8; 22] {
    let mut a = [0u8; 22];
    // Retry, power management and more-data are mutable in transit and excluded.
    a[0] = header[0] & 0x8f;
    a[1] = (header[1] & 0xc7) | (FC_PROTECTED >> 8) as u8;
    a[2..20].copy_from_slice(&header[4..22]);
    a[20] = header[22] & 0x0f;
    a
}

fn ccmp_header(pn: u64) -> [u8; CCMP_HEADER_LEN] {
    let p = pn.to_le_bytes();
    [p[0], p[1], 0, 0x20, p[2], p[3], p[4], p[5]]
}

fn check_header(header: &Header) -> Result<(), Error> {
    if header.ftype() != super::frame::TYPE_DATA || header.subtype() & 0x8 != 0 {
        return Err(Error::Format(
            "only non-QoS data frames are supported".into(),
        ));
    }
    if header.fc & 0x0300 == 0x0300 {
        return Err(Error::Format(
            "four-address frames are not supported".into(),
        ));
    }
    Ok(())
}

/// Builds a protected data frame from a header and plaintext body.
pub fn encrypt(key: &Key, header: &Header, pn: u64, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    check_header(header)?;
    let mut h = *header;
    h.fc |= FC_PROTECTED;
    let mut out = Vec::with_capacity(HEADER_LEN + CCMP_HEADER_LEN + plaintext.len() + MIC_LEN);
    h.write(&mut out);
    out.extend_from_slice(&ccmp_header(pn));
    let body_at = out.len();
    out.extend_from_slice(plaintext);
    let a = aad(&out[..HEADER_LEN]);
    let tag = FrameCcm::new(key.into())
        .encrypt_in_place_detached(&nonce(&h.addr2, pn).into(), &a, &mut out[body_at..])
        .map_err(|_| Error::Format("frame too long".into()))?;
    out.extend_from_slice(&tag);
    Ok(out)
}

/// Authenticates and decrypts a protected data frame, returning its packet number and body.
pub fn decrypt(key: &Key, frame: &[u8]) -> Result<(u64, Vec<u8>), Error> {
    let header = Header::parse(frame)?;
    check_header(&header)?;
    if !header.protected() || frame.len() < HEADER_LEN + CCMP_HEADER_LEN + MIC_LEN {
        return Err(Error::Format("not a CCMP frame".into()));
    }
    let c = &frame[HEADER_LEN..HEADER_LEN + CCMP_HEADER_LEN];
    if c[3] & 0x20 == 0 {
        return Err(Error::Format("CCMP extended IV bit missing".into()));
    }
    let pn = u64::from_le_bytes([c[0], c[1], c[4], c[5], c[6], c[7], 0, 0]);
    let body_end = frame.len() - MIC_LEN;
    let mut body = frame[HEADER_LEN + CCMP_HEADER_LEN..body_end].to_vec();
    FrameCcm::new(key.into())
        .decrypt_in_place_detached(
            &nonce(&header.addr2, pn).into(),
            &aad(frame),
            &mut body,
            GenericArray::from_slice(&frame[body_end..]),
        )
        .map_err(|_| Error::Authentication)?;
    Ok((pn, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::frame::{FC_TO_DS, TYPE_DATA};

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn round_trip_and_tamper() {
        let key = [3u8; 16];
        let h = Header::new(TYPE_DATA, 0, FC_TO_DS, [1; 6], [2; 6], [1; 6]);
        let f = encrypt(&key, &h, 7, b"hello world").unwrap();
        assert_eq!(&f[24..32], &[7, 0, 0, 0x20, 0, 0, 0, 0]);
        assert_eq!(decrypt(&key, &f).unwrap(), (7, b"hello world".to_vec()));

        let mut retried = f.clone();
        retried[1] |= 0x08;
        assert!(decrypt(&key, &retried).is_ok());
        let mut bad = f.clone();
        bad[40] ^= 1;
        assert!(matches!(decrypt(&key, &bad), Err(Error::Authentication)));
    }

    #[test]
    fn ieee_test_vector() {
        let key: Key = hex("c97c1f67ce371185514a8a19f2bdd52f").try_into().unwrap();
        let h = Header::parse(&hex("0848c32c0fd2e128a57c5030f1844408abaea5b8fcba8033")).unwrap();
        let pt = hex("f8ba1a55d02f85ae967bb62fb6cda8eb7e78a050");
        let f = encrypt(&key, &h, 0xb5039776e70c, &pt).unwrap();
        assert_eq!(&f[24..32], &hex("0ce70020769703b5")[..]);
        assert_eq!(
            &f[32..],
            &hex("f3d0a2fe9a3dbf2342a643e43246e80c3c04d0197845ce0b16f97623")[..]
        );
    }
}
