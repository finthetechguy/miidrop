//! The reliable stream Mii Maker runs over UDS, one short one-way connection per message.

pub const FRAME_SIZE: usize = 1464;
pub const HEADER_SIZE: usize = 24;
pub const MAX_DATA: usize = FRAME_SIZE - HEADER_SIZE;
pub const WINDOW: u16 = 0x0800;

pub const ACK: u16 = 0x01;
pub const RST: u16 = 0x02;
pub const SYN: u16 = 0x08;
pub const FIN: u16 = 0x10;

pub const SYN_TIMEOUT_MS: u64 = 200;
pub const DATA_TIMEOUT_MS: u64 = 50;

const MAGIC: &[u8; 4] = b"PRNN";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub seq: u32,
    pub ack: u32,
    pub flags: u16,
    pub len: u32,
    pub window: u16,
}

impl Header {
    pub fn ack(ack: u32) -> Self {
        Header {
            ack,
            flags: ACK,
            window: WINDOW,
            ..Default::default()
        }
    }

    pub fn rst() -> Self {
        Header {
            flags: RST,
            ..Default::default()
        }
    }
}

/// Builds a full-size frame; the unused tail is zero.
pub fn build(h: &Header, data: &[u8]) -> Vec<u8> {
    debug_assert!(data.len() == h.len as usize && data.len() <= MAX_DATA);
    let mut out = vec![0u8; FRAME_SIZE];
    out[..4].copy_from_slice(MAGIC);
    out[4..8].copy_from_slice(&h.seq.to_le_bytes());
    out[8..12].copy_from_slice(&h.ack.to_le_bytes());
    out[0xe..0x10].copy_from_slice(&h.flags.to_le_bytes());
    out[0x10..0x14].copy_from_slice(&h.len.to_le_bytes());
    out[0x14..0x16].copy_from_slice(&h.window.to_le_bytes());
    out[HEADER_SIZE..HEADER_SIZE + data.len()].copy_from_slice(data);
    out
}

pub fn parse(buf: &[u8]) -> Option<(Header, &[u8])> {
    if buf.len() < HEADER_SIZE || &buf[..4] != MAGIC {
        return None;
    }
    let u32_at = |at: usize| u32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
    let h = Header {
        seq: u32_at(4),
        ack: u32_at(8),
        flags: u16::from_le_bytes([buf[0xe], buf[0xf]]),
        len: u32_at(0x10),
        window: u16::from_le_bytes([buf[0x14], buf[0x15]]),
    };
    let len = h.len as usize;
    if len > MAX_DATA || len > buf.len() - HEADER_SIZE {
        return None;
    }
    Some((h, &buf[HEADER_SIZE..HEADER_SIZE + len]))
}

/// What the receiving side should do with one incoming frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RxEvent<'a> {
    pub ack: Option<u32>,
    pub data: Option<&'a [u8]>,
    pub opened: bool,
    pub closed: bool,
    pub reset: bool,
}

/// Receiving side of the connections the other node opens on one channel.
#[derive(Clone, Debug, Default)]
pub struct Receiver {
    open: bool,
    seen: bool,
    next: u32,
}

impl Receiver {
    pub fn handle<'a>(&mut self, h: &Header, payload: &'a [u8]) -> RxEvent<'a> {
        let mut ev = RxEvent::default();
        if h.flags & RST != 0 {
            self.open = false;
            ev.reset = true;
            return ev;
        }
        if h.flags == SYN {
            let next = h.seq.wrapping_add(1);
            ev.opened = !self.open || self.next != next;
            self.open = true;
            self.seen = true;
            self.next = next;
            ev.ack = Some(next);
            return ev;
        }
        if !self.seen {
            return ev;
        }
        let end = h.seq.wrapping_add(h.len);
        if h.flags == 0 && h.len > 0 {
            if self.open && h.seq == self.next {
                ev.data = Some(payload);
                self.next = end;
            }
            // Re-acknowledge data already received so a sender that lost the ACK can move on.
            let old = self.next.wrapping_sub(end) < 1 << 31;
            if self.open || old {
                ev.ack = Some(self.next);
            }
        } else if h.flags == FIN {
            if self.open && h.seq == self.next {
                self.next = self.next.wrapping_add(1);
                self.open = false;
                ev.closed = true;
                ev.ack = Some(self.next);
            } else if self.open || h.seq.wrapping_add(1) == self.next {
                ev.ack = Some(self.next);
            }
        }
        ev
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendState {
    Active,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
struct Segment {
    flags: u16,
    seq: u32,
    data: Vec<u8>,
    sent_at: Option<u64>,
}

impl Segment {
    fn end(&self) -> u32 {
        let used = if self.flags & (SYN | FIN) != 0 {
            1
        } else {
            self.data.len() as u32
        };
        self.seq.wrapping_add(used)
    }

    fn timeout(&self) -> u64 {
        if self.flags == SYN {
            SYN_TIMEOUT_MS
        } else {
            DATA_TIMEOUT_MS
        }
    }
}

/// Sending side of one connection: SYN alone, then every data segment and the FIN back to back.
#[derive(Clone, Debug)]
pub struct Sender {
    segments: Vec<Segment>,
    unacked: usize,
    give_up_at: u64,
    state: SendState,
    pub transmissions: u32,
}

impl Sender {
    /// Prepares a connection carrying `chunks` as separate data segments.
    pub fn new(isn: u32, chunks: &[&[u8]], now: u64, give_up_after: u64) -> Option<Self> {
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        if total > WINDOW as usize || chunks.iter().any(|c| c.is_empty() || c.len() > MAX_DATA) {
            return None;
        }
        let mut segments = vec![Segment {
            flags: SYN,
            seq: isn,
            data: vec![],
            sent_at: None,
        }];
        let mut seq = isn.wrapping_add(1);
        for c in chunks {
            segments.push(Segment {
                flags: 0,
                seq,
                data: c.to_vec(),
                sent_at: None,
            });
            seq = seq.wrapping_add(c.len() as u32);
        }
        segments.push(Segment {
            flags: FIN,
            seq,
            data: vec![],
            sent_at: None,
        });
        Some(Sender {
            segments,
            unacked: 0,
            give_up_at: now + give_up_after,
            state: SendState::Active,
            transmissions: 0,
        })
    }

    pub fn state(&self) -> SendState {
        self.state
    }

    /// Returns the next frame that is due, if any; call until it returns None.
    pub fn poll(&mut self, now: u64) -> Option<(Header, Vec<u8>)> {
        if self.state != SendState::Active {
            return None;
        }
        if now >= self.give_up_at {
            self.state = SendState::Failed;
            return None;
        }
        let last = if self.segments[self.unacked].flags == SYN {
            self.unacked
        } else {
            self.segments.len() - 1
        };
        for s in &mut self.segments[self.unacked..=last] {
            if s.sent_at.is_some_and(|t| now < t + s.timeout()) {
                continue;
            }
            s.sent_at = Some(now);
            self.transmissions += 1;
            let h = Header {
                seq: s.seq,
                flags: s.flags,
                len: s.data.len() as u32,
                ..Default::default()
            };
            return Some((h, s.data.clone()));
        }
        None
    }

    /// Applies a cumulative ACK; returns true if it retired anything.
    pub fn on_ack(&mut self, ack: u32) -> bool {
        if self.state != SendState::Active {
            return false;
        }
        let hit = self.segments[self.unacked..]
            .iter()
            .take_while(|s| s.sent_at.is_some())
            .position(|s| s.end() == ack);
        let Some(i) = hit else { return false };
        self.unacked += i + 1;
        if self.unacked == self.segments.len() {
            self.state = SendState::Done;
        }
        true
    }

    /// The earliest time a frame may become due, for sleeping until then.
    pub fn next_due(&self) -> Option<u64> {
        if self.state != SendState::Active {
            return None;
        }
        let due = self.segments[self.unacked..]
            .iter()
            .take(if self.segments[self.unacked].flags == SYN {
                1
            } else {
                usize::MAX
            })
            .map(|s| s.sent_at.map_or(0, |t| t + s.timeout()))
            .min()?;
        Some(due.min(self.give_up_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let h = Header {
            seq: 0x50f854,
            len: 3,
            ..Default::default()
        };
        let f = build(&h, b"abc");
        assert_eq!(f.len(), FRAME_SIZE);
        assert_eq!(&f[..8], b"PRNN\x54\xf8\x50\x00");
        assert_eq!(parse(&f), Some((h, &b"abc"[..])));
        assert_eq!(
            &build(&Header::ack(9), &[])[0xe..0x16],
            &[1, 0, 0, 0, 0, 0, 0, 8]
        );
        let mut bad = f.clone();
        bad[0x10] = 0xff;
        bad[0x11] = 0xff;
        assert!(parse(&bad).is_none());
    }

    /// Runs a sender against a receiver, dropping frames whose index `drop` selects.
    fn exchange(
        isn: u32,
        chunks: &[&[u8]],
        mut drop: impl FnMut(usize, bool) -> bool,
    ) -> (Vec<u8>, Sender, u64) {
        let mut tx = Sender::new(isn, chunks, 0, 30_000).unwrap();
        let mut rx = Receiver::default();
        let mut got = vec![];
        let mut n = 0;
        let mut now = 0;
        while tx.state() == SendState::Active && now < 60_000 {
            while let Some((h, data)) = tx.poll(now) {
                n += 1;
                if drop(n, true) {
                    continue;
                }
                let frame = build(&h, &data);
                let (ph, payload) = parse(&frame).unwrap();
                let ev = rx.handle(&ph, payload);
                if let Some(d) = ev.data {
                    got.extend_from_slice(d);
                }
                if let Some(a) = ev.ack {
                    n += 1;
                    if !drop(n, false) {
                        tx.on_ack(a);
                    }
                }
            }
            now += 5;
        }
        (got, tx, now)
    }

    #[test]
    fn clean_transfer() {
        let container = [7u8; 512];
        let (got, tx, _) = exchange(1000, &[&container, &[1; 32]], |_, _| false);
        assert_eq!(tx.state(), SendState::Done);
        assert_eq!(got.len(), 544);
        assert_eq!(tx.transmissions, 4);
    }

    #[test]
    fn lossy_transfer_and_wrap() {
        let data: Vec<u8> = (0..512u32).map(|i| i as u8).collect();
        for seed in 1..50u32 {
            let mut state = seed;
            let lossy = |_, _| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (state >> 16) % 10 < 3
            };
            let (got, tx, _) = exchange(u32::MAX - 3, &[&data, &[9; 32]], lossy);
            assert_eq!(tx.state(), SendState::Done, "seed {seed}");
            assert_eq!(&got[..512], &data[..]);
            assert_eq!(got.len(), 544);
        }
    }

    #[test]
    fn lost_ack_of_last_data_does_not_block_fin() {
        // Frames: SYN, its ACK, data, ACK, follow-up, its ACK (lost), FIN, ACK.
        let (got, tx, _) = exchange(5, &[&[1; 512], &[2; 32]], |n, ack| ack && n == 6);
        assert_eq!(tx.state(), SendState::Done);
        assert_eq!(got.len(), 544);
    }

    #[test]
    fn syn_retransmits_every_200ms_and_gives_up() {
        let mut tx = Sender::new(1, &[b"x"], 0, 1000).unwrap();
        let mut times = vec![];
        for now in (0..1100).step_by(10) {
            while let Some((h, _)) = tx.poll(now) {
                assert_eq!(h.flags, SYN);
                times.push(now);
            }
        }
        assert_eq!(times, vec![0, 200, 400, 600, 800]);
        assert_eq!(tx.state(), SendState::Failed);
    }

    #[test]
    fn receiver_reacks_duplicate_fin_and_ignores_out_of_order() {
        let mut rx = Receiver::default();
        let syn = Header {
            seq: 10,
            flags: SYN,
            ..Default::default()
        };
        assert_eq!(rx.handle(&syn, &[]).ack, Some(11));
        let late = Header {
            seq: 20,
            len: 2,
            ..Default::default()
        };
        let ev = rx.handle(&late, b"zz");
        assert_eq!((ev.data, ev.ack), (None, Some(11)));
        let early_fin = Header {
            seq: 22,
            flags: FIN,
            ..Default::default()
        };
        assert!(!rx.handle(&early_fin, &[]).closed);
        let data = Header {
            seq: 11,
            len: 2,
            ..Default::default()
        };
        assert_eq!(rx.handle(&data, b"ab").data, Some(&b"ab"[..]));
        let fin = Header {
            seq: 13,
            flags: FIN,
            ..Default::default()
        };
        let ev = rx.handle(&fin, &[]);
        assert!(ev.closed);
        assert_eq!(ev.ack, Some(14));
        assert_eq!(rx.handle(&fin, &[]).ack, Some(14));
        assert!(rx.handle(&Header::rst(), &[]).reset);
    }
}
