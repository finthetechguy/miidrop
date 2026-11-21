//! Mii records and the protection applied to them in transit.

mod record;
mod wrap;

pub use record::{FIELDS, Field, Issue, RECORD_SIZE, Record, STORED_SIZE, crc16};
pub use wrap::{WRAPPED_SIZE, unwrap, wrap};
