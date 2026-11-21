//! Nintendo local wireless (UDS) framing on top of 802.11.

pub mod beacon;
pub mod eapol;
pub mod key;
pub mod node;
pub mod securedata;

pub use beacon::NetworkInfo;
pub use node::NodeInfo;

/// Parameters that identify one application's networks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    pub comm_id: u32,
    pub id8: u8,
    pub passphrase: &'static [u8],
}

pub const MII_MAKER: App = App {
    comm_id: 0x0002_0710,
    id8: 1,
    passphrase: b"EDIT_Mii_Studio",
};

pub const NINTENDO_OUI: [u8; 3] = [0x00, 0x1f, 0x32];
pub const BROADCAST_NODE: u16 = 0xffff;
pub const HOST_NODE: u16 = 1;
pub const MAX_NODES: usize = 16;
