//! Finding hosts of an application by listening for their beacons on each channel.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::keys::Key;
use crate::radio::Radio;
use crate::uds::{App, NetworkInfo};

/// Channels a host may pick, the usual ones first.
pub const CHANNELS: [u8; 13] = [1, 6, 11, 2, 3, 4, 5, 7, 8, 9, 10, 12, 13];
/// Slightly longer than one beacon interval.
pub const DWELL: Duration = Duration::from_millis(120);

#[derive(Clone, Debug)]
pub struct Found {
    pub network: NetworkInfo,
    pub signal: Option<i8>,
    /// Node names from the beacon, when the beacon key is available.
    pub names: Option<Vec<String>>,
}

pub fn scan(
    radio: &mut dyn Radio,
    app: &App,
    rounds: u32,
    beacon_key: Option<&Key>,
    stop: &AtomicBool,
    mut on_found: impl FnMut(&Found),
) -> io::Result<Vec<Found>> {
    let mut found: Vec<Found> = Vec::new();
    'outer: for _ in 0..rounds {
        for &channel in &CHANNELS {
            if stop.load(Ordering::Relaxed) {
                break 'outer;
            }
            radio.set_channel(channel)?;
            let until = Instant::now() + DWELL;
            while let Some(rx) = radio.recv(until.saturating_duration_since(Instant::now()))? {
                let Some(network) = NetworkInfo::from_beacon(&rx.frame) else {
                    continue;
                };
                if !network.matches(app) {
                    continue;
                }
                let names = beacon_key
                    .and_then(|k| network.nodes(k))
                    .map(|n| n.iter().map(|n| n.name()).collect());
                let entry = Found {
                    network,
                    signal: rx.info.signal,
                    names,
                };
                match found
                    .iter_mut()
                    .find(|f| f.network.host == entry.network.host)
                {
                    Some(f) if f.network == entry.network => f.signal = entry.signal,
                    Some(f) => {
                        *f = entry;
                        on_found(f);
                    }
                    None => {
                        on_found(&entry);
                        found.push(entry);
                    }
                }
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::Received;
    use crate::uds::MII_MAKER;
    use crate::wifi::frame::{self, BEACON, BROADCAST, Header, Mac, TYPE_MGMT};

    struct FakeRadio {
        channel: u8,
        beacon: Vec<u8>,
        pending: bool,
    }

    impl Radio for FakeRadio {
        fn mac(&self) -> Mac {
            [2; 6]
        }
        fn set_channel(&mut self, channel: u8) -> io::Result<()> {
            self.channel = channel;
            self.pending = true;
            Ok(())
        }
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> io::Result<Option<Received>> {
            if self.channel == 6 && std::mem::take(&mut self.pending) {
                return Ok(Some(Received {
                    info: Default::default(),
                    frame: self.beacon.clone(),
                }));
            }
            Ok(None)
        }
    }

    #[test]
    fn finds_host_once() {
        let network = NetworkInfo {
            host: [2, 0, 0, 0, 0, 1],
            channel: 6,
            comm_id: MII_MAKER.comm_id,
            id8: MII_MAKER.id8,
            attributes: 0x8000,
            network_id: 7,
            total_nodes: 1,
            max_nodes: 2,
            updates: 2,
            appdata: b"C:1\0\0\0\0\0".to_vec(),
            rates: frame::RATES.to_vec(),
            encrypted_nodes: vec![],
        };
        let mut beacon = Vec::new();
        Header::new(TYPE_MGMT, BEACON, 0, BROADCAST, network.host, network.host).write(&mut beacon);
        beacon.extend_from_slice(&[0; 8]);
        beacon.extend_from_slice(&[100, 0, 0x31, 0x04]);
        frame::push_element(&mut beacon, frame::IE_RATES, &frame::RATES);
        frame::push_element(&mut beacon, frame::IE_DS, &[6]);
        beacon.extend_from_slice(&network.beacon_elements(&[0; 16], &[]));

        let mut radio = FakeRadio {
            channel: 0,
            beacon,
            pending: false,
        };
        let mut reports = 0;
        let found = scan(
            &mut radio,
            &MII_MAKER,
            2,
            None,
            &AtomicBool::new(false),
            |_| reports += 1,
        )
        .unwrap();
        assert_eq!(reports, 1);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].network.network_id, 7);
    }
}
