//! Mii Maker local transfer over UDS.

pub mod container;
pub mod host;
pub mod keys;
pub mod link;
pub mod mii;
pub mod prnn;
pub mod radio;
pub mod scan;
pub mod session;
#[cfg(test)]
mod sim;
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
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Timeout(String),
    #[error("{0}")]
    Session(String),
    #[error("cancelled")]
    Cancelled,
}
