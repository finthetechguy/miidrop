//! Mii Maker local transfer over UDS.

pub mod container;
pub mod keys;
pub mod mii;
pub mod prnn;
pub mod radio;
pub mod uds;
pub mod wifi;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("key file: {0}")]
    Keys(String),
    #[error("{0}")]
    Format(String),
    #[error("authentication failed")]
    Authentication,
}
