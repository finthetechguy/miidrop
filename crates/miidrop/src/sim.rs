//! A simulated host console on a lossy shared medium, for testing the client end to end.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::host::HostLink;
use crate::keys::Key;
use crate::link::Link;
use crate::mii::{self, Record};
use crate::radio::{Radio, Received};
use crate::session::{Event, Session};
use crate::uds::{MII_MAKER, NetworkInfo, NodeInfo};
use crate::wifi::frame::{BROADCAST, Mac};
use crate::wifi::radiotap::RxInfo;

const HOST: Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
const CLIENT: Mac = [0x02, 0x66, 0x77, 0x88, 0x99, 0xaa];
const BASE: Key = [0x2d; 16];
const RECORD_KEY: Key = [0x31; 16];
const BEACON_KEY: Key = [0x4b; 16];

/// One end of a simulated radio link that drops a share of broadcast frames.
pub struct AirRadio {
    mac: Mac,
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    loss: u32,
    rng: u64,
}

impl AirRadio {
    fn dropped(&mut self, frame: &[u8]) -> bool {
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        frame[4..10] == BROADCAST && ((self.rng >> 33) % 100) < self.loss as u64
    }
}

/// Two connected radios; `loss` is the percentage of broadcast frames lost each way.
pub fn air(loss: u32, seed: u64) -> (AirRadio, AirRadio) {
    let (a_tx, b_rx) = mpsc::channel();
    let (b_tx, a_rx) = mpsc::channel();
    let end = |mac, tx, rx, rng| AirRadio {
        mac,
        tx,
        rx,
        loss,
        rng,
    };
    (
        end(HOST, a_tx, a_rx, seed),
        end(CLIENT, b_tx, b_rx, seed ^ 0x9e37_79b9),
    )
}

impl Radio for AirRadio {
    fn mac(&self) -> Mac {
        self.mac
    }
    fn set_channel(&mut self, _: u8) -> std::io::Result<()> {
        Ok(())
    }
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        if !self.dropped(frame) {
            let _ = self.tx.send(frame.to_vec());
        }
        Ok(())
    }
    fn recv(&mut self, timeout: Duration) -> std::io::Result<Option<Received>> {
        Ok(self.rx.recv_timeout(timeout).ok().map(|frame| Received {
            info: RxInfo {
                frequency: Some(2437),
                signal: Some(-40),
                bad_fcs: false,
            },
            frame,
        }))
    }
}

/// What the host side saw.
pub struct Outcome<C> {
    pub host: C,
    pub client_left: bool,
    pub keepalives: u32,
}

const STEP: Duration = Duration::from_secs(10);

/// Listens until a beacon from a matching network arrives.
fn find_network(radio: &mut AirRadio) -> NetworkInfo {
    let until = Instant::now() + STEP;
    while Instant::now() < until {
        if let Some(rx) = radio.recv(Duration::from_millis(50)).unwrap() {
            if let Some(n) = NetworkInfo::from_beacon(&rx.frame) {
                assert!(n.matches(&MII_MAKER));
                return n;
            }
        }
    }
    panic!("no beacon");
}

/// Runs a host behaviour against a client behaviour over a link losing `loss` percent.
pub fn run<C: Send + 'static, P>(
    loss: u32,
    seed: u64,
    host: impl FnOnce(&mut Session<HostLink<AirRadio>>) -> C + Send + 'static,
    client: impl FnOnce(&mut Session<Link<AirRadio>>) -> P,
) -> (Outcome<C>, P) {
    let (host_radio, mut client_radio) = air(loss, seed);
    let handle = thread::spawn(move || {
        let me = NodeInfo::new(0x1111, "Console");
        let mut link =
            HostLink::start(host_radio, &MII_MAKER, &BASE, Some(BEACON_KEY), me, 6).unwrap();
        let joined = link.wait_client(STEP).unwrap();
        assert_eq!(joined.name(), "PC");
        let mut s = Session::new(link, RECORD_KEY);
        s.send_control(crate::session::READY);
        s.wait_ready(STEP).unwrap();
        let result = host(&mut s);
        let until = Instant::now() + STEP;
        let mut left = false;
        while !left && Instant::now() < until {
            left = matches!(
                s.next_event(Duration::from_millis(50)),
                Ok(Some(Event::Left))
            );
        }
        Outcome {
            host: result,
            client_left: left,
            keepalives: s.into_link().keepalives,
        }
    });
    let network = find_network(&mut client_radio);
    assert_eq!(network.appdata, b"C:1\0\0\0\0\0");
    let names: Vec<String> = network
        .nodes(&BEACON_KEY)
        .unwrap()
        .iter()
        .map(|n| n.name())
        .collect();
    assert_eq!(names, ["Console"]);
    let me = NodeInfo::new(0x2222, "PC");
    let link = Link::join(client_radio, &network, &BASE, &MII_MAKER, me).unwrap();
    assert_eq!(link.node_id, 2);
    assert_eq!(link.host_name().as_deref(), Some("Console"));
    let mut s = Session::new(link, RECORD_KEY);
    s.wait_ready(STEP).unwrap();
    let p = client(&mut s);
    s.linger(Duration::from_millis(500)).unwrap();
    s.into_link().leave();
    (handle.join().unwrap(), p)
}

pub fn record_key() -> Key {
    RECORD_KEY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container;

    fn sample() -> Record {
        mii::sample()
    }

    #[test]
    fn client_sends() {
        for (loss, seed) in [(0, 1), (25, 2), (25, 3)] {
            let (out, verdict) = run(
                loss,
                seed,
                |s| s.receive_mii(STEP).unwrap(),
                |s| s.send_mii(&sample(), STEP).unwrap(),
            );
            assert!(verdict, "loss {loss}");
            let got = out.host;
            assert!(got.accepted);
            assert_eq!(got.record.unwrap(), sample().for_sending());
            assert!(out.client_left);
        }
    }

    #[test]
    fn client_receives() {
        for (loss, seed) in [(0, 4), (25, 5)] {
            let (out, got) = run(
                loss,
                seed,
                |s| s.send_mii(&sample(), STEP).unwrap(),
                |s| s.receive_mii(STEP).unwrap(),
            );
            assert!(out.host, "loss {loss}");
            assert!(got.accepted);
            assert_eq!(got.container.len(), container::SEGMENT_SIZE);
            assert_eq!(got.record.unwrap(), sample().for_sending());
        }
    }

    #[test]
    fn bad_record_is_rejected() {
        let (out, verdict) = run(
            0,
            6,
            |s| s.receive_mii(STEP).unwrap(),
            |s| {
                let mut bytes = *sample().for_sending().as_bytes();
                bytes[0x5f] ^= 1;
                let bad = Record::from_bytes(&bytes).unwrap();
                let wrapped = mii::wrap(&record_key(), &bad);
                let c = container::build(&wrapped, container::LENGTH_3DS);
                s.send_raw(&c, STEP).unwrap()
            },
        );
        assert!(!verdict);
        assert!(!out.host.accepted);
    }

    #[test]
    fn keepalives_flow() {
        let (out, ()) = run(
            0,
            7,
            |_| (),
            |s| {
                while s.next_event(Duration::from_millis(1300)).unwrap().is_some() {}
            },
        );
        assert!(out.keepalives >= 1);
        assert!(out.client_left);
    }
}
