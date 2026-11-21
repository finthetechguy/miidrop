//! Session key derivation for a network's data frames.

use aes::Aes128;
use ctr::cipher::{KeyIvInit, StreamCipher};
use md5::{Digest, Md5};

use super::App;
use crate::keys::Key;
use crate::wifi::frame::Mac;

type Ctr = ctr::Ctr128BE<Aes128>;

pub fn session_key(base: &Key, app: &App, network_id: u32, host: &Mac) -> Key {
    let mut iv = Md5::new();
    iv.update(app.comm_id.to_le_bytes());
    iv.update(network_id.to_le_bytes());
    iv.update(host);
    iv.update((app.id8 as u16).to_le_bytes());
    let iv: [u8; 16] = iv.finalize().into();
    let mut key: [u8; 16] = Md5::digest(app.passphrase).into();
    Ctr::new(base.into(), &iv.into()).apply_keystream(&mut key);
    key
}
