//! The 802.11 layer.

pub mod ccmp;
pub mod frame;
pub mod radiotap;

/// Channel number for a 2.4 GHz frequency.
pub fn channel_of(frequency: u16) -> Option<u8> {
    match frequency {
        2484 => Some(14),
        2412..=2472 if (frequency - 2407) % 5 == 0 => Some(((frequency - 2407) / 5) as u8),
        _ => None,
    }
}

pub fn frequency_of(channel: u8) -> Option<u16> {
    match channel {
        1..=13 => Some(2407 + 5 * channel as u16),
        14 => Some(2484),
        _ => None,
    }
}
