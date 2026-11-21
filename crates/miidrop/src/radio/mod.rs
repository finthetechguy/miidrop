//! Sending and receiving raw 802.11 frames.

use std::io;
use std::time::Duration;

use crate::wifi::frame::Mac;
use crate::wifi::radiotap::RxInfo;

#[cfg(target_os = "linux")]
pub mod linux;
pub mod pcap;

/// A received frame without radiotap header or FCS.
#[derive(Clone, Debug)]
pub struct Received {
    pub info: RxInfo,
    pub frame: Vec<u8>,
}

pub trait Radio {
    /// Address the radio acknowledges frames for.
    fn mac(&self) -> Mac;
    fn set_channel(&mut self, channel: u8) -> io::Result<()>;
    /// Injects a frame given without FCS.
    fn send(&mut self, frame: &[u8]) -> io::Result<()>;
    /// Waits up to `timeout` for the next frame.
    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Received>>;
}

impl<R: Radio + ?Sized> Radio for &mut R {
    fn mac(&self) -> Mac {
        (**self).mac()
    }
    fn set_channel(&mut self, channel: u8) -> io::Result<()> {
        (**self).set_channel(channel)
    }
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        (**self).send(frame)
    }
    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Received>> {
        (**self).recv(timeout)
    }
}
