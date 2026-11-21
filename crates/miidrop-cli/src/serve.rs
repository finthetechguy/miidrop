//! A helper for the desktop app: commands arrive as JSON lines on stdin, events leave on stdout.

use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

use miidrop::host::HostLink;
use miidrop::keys::Keys;
use miidrop::link::Link;
use miidrop::mii::Record;
use miidrop::radio::Radio;
use miidrop::scan;
use miidrop::session::{Event, Session, Transport};
use miidrop::uds::{MII_MAKER, NetworkInfo, NodeInfo};
use miidrop::wifi::frame::Mac;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{LinuxRadio, describe, found_json, hex, mac_text, parse_mac};

const POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// The key file's text, read by the app so this process never opens user paths.
    Keys {
        text: String,
    },
    Scan,
    Join {
        host: Option<String>,
        #[serde(default = "default_name")]
        name: String,
    },
    Host {
        #[serde(default = "default_channel")]
        channel: u8,
        #[serde(default = "default_name")]
        name: String,
    },
    /// A plain record in hex, sent once both sides are ready.
    Send {
        record: String,
    },
    Stop,
    Quit,
}

fn default_name() -> String {
    "miidrop".into()
}

fn default_channel() -> u8 {
    6
}

pub fn parse_command(line: &str) -> Result<Command, String> {
    serde_json::from_str(line).map_err(|e| format!("bad command: {e}"))
}

fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn error(message: impl std::fmt::Display) {
    emit(json!({ "event": "error", "message": message.to_string() }));
}

fn state(s: &str) {
    emit(json!({ "event": "state", "state": s }));
}

fn decode_record(text: &str) -> Result<Record, String> {
    let bytes: Vec<u8> = (0..text.len() / 2)
        .map(|i| u8::from_str_radix(text.get(2 * i..2 * i + 2).unwrap_or(""), 16))
        .collect::<Result<_, _>>()
        .map_err(|_| "record is not hex".to_string())?;
    match bytes.len() {
        miidrop::mii::RECORD_SIZE => Record::from_bytes(&bytes),
        miidrop::mii::STORED_SIZE => Record::from_stored(&bytes),
        n => return Err(format!("record must be 92 or 96 bytes, got {n}")),
    }
    .map_err(|e| e.to_string())
}

struct Helper {
    commands: Receiver<Command>,
    cancel: Arc<AtomicBool>,
    keys: Option<Keys>,
    /// Quit arrived while an operation was being cancelled.
    pending_quit: bool,
}

enum Next {
    Continue,
    Quit,
}

impl Helper {
    fn keys(&self) -> Result<&Keys, String> {
        self.keys.as_ref().ok_or_else(|| "no keys loaded".into())
    }

    /// Takes the next command if one is waiting, handling stop and quit for long operations.
    fn interrupted(&self) -> Option<Next> {
        match self.commands.try_recv() {
            Ok(Command::Quit) | Err(TryRecvError::Disconnected) => Some(Next::Quit),
            Ok(Command::Stop) => Some(Next::Continue),
            Ok(other) => {
                error(format!("busy, ignored {other:?}"));
                None
            }
            Err(TryRecvError::Empty) => None,
        }
    }

    fn scan(&mut self, radio: &mut LinuxRadio) -> Next {
        state("scanning");
        let beacon = self.keys.as_ref().and_then(|k| k.beacon);
        loop {
            let r = scan::scan(radio, &MII_MAKER, 1, beacon.as_ref(), &self.cancel, |f| {
                emit(json!({ "event": "found", "console": found_json(f) }))
            });
            if let Err(e) = r {
                error(e);
                return Next::Continue;
            }
            self.cancel.store(false, Ordering::Relaxed);
            if let Some(n) = self.interrupted() {
                return n;
            }
        }
    }

    fn me(radio: &LinuxRadio, name: &str) -> Result<NodeInfo, String> {
        if name.encode_utf16().count() > 10 {
            return Err("name must be at most 10 characters".into());
        }
        let m = radio.mac();
        let id = u64::from_be_bytes([0, 0, m[0], m[1], m[2], m[3], m[4], m[5]]);
        Ok(NodeInfo::new(id, name))
    }

    /// Scans until a matching console with room is waiting; None if interrupted.
    fn find(
        &mut self,
        radio: &mut LinuxRadio,
        keys: &Keys,
        want: Option<Mac>,
    ) -> Result<Option<NetworkInfo>, String> {
        state("searching");
        loop {
            let found = scan::scan(
                &mut *radio,
                &MII_MAKER,
                1,
                keys.beacon.as_ref(),
                &self.cancel,
                |_| {},
            )
            .map_err(|e| e.to_string())?;
            self.cancel.store(false, Ordering::Relaxed);
            if let Some(f) = found.into_iter().find(|f| {
                let n = &f.network;
                want.is_none_or(|m| n.host == m)
                    && n.appdata.starts_with(b"C:1")
                    && n.total_nodes < n.max_nodes
            }) {
                return Ok(Some(f.network));
            }
            if let Some(n) = self.interrupted() {
                self.pending_quit = matches!(n, Next::Quit);
                return Ok(None);
            }
        }
    }

    fn join(&mut self, radio: &mut LinuxRadio, host: Option<String>, name: &str) -> Next {
        let prepared = (|| -> Result<_, String> {
            let keys = self.keys()?.clone();
            let want: Option<Mac> = host.as_deref().map(parse_mac).transpose()?;
            Ok((keys, want, Self::me(radio, name)?))
        })();
        let (keys, want, me) = match prepared {
            Ok(p) => p,
            Err(e) => {
                error(e);
                return Next::Continue;
            }
        };
        let network = match self.find(radio, &keys, want) {
            Ok(Some(n)) => n,
            Ok(None) => return self.after_interrupt(),
            Err(e) => {
                error(e);
                return Next::Continue;
            }
        };
        emit(
            json!({ "event": "joining", "host": mac_text(&network.host), "channel": network.channel }),
        );
        match Link::join(&mut *radio, &network, &keys.session, &MII_MAKER, me) {
            Ok(link) => {
                let peer = link.host_name().unwrap_or_default();
                self.session(Box::new(link), &keys, "client", &peer)
            }
            Err(e) => {
                error(e);
                Next::Continue
            }
        }
    }

    fn host(&mut self, radio: &mut LinuxRadio, channel: u8, name: &str) -> Next {
        let keys = match self.keys() {
            Ok(k) => k.clone(),
            Err(e) => {
                error(e);
                return Next::Continue;
            }
        };
        if !(1..=13).contains(&channel) {
            error("channel must be 1 to 13");
            return Next::Continue;
        }
        let me = match Self::me(radio, name) {
            Ok(m) => m,
            Err(e) => {
                error(e);
                return Next::Continue;
            }
        };
        let mut link = match HostLink::start(
            &mut *radio,
            &MII_MAKER,
            &keys.session,
            keys.beacon,
            me,
            channel,
        ) {
            Ok(l) => l,
            Err(e) => {
                error(e);
                return Next::Continue;
            }
        };
        emit(
            json!({ "event": "hosting", "channel": channel, "beacon_key": keys.beacon.is_some() }),
        );
        state("hosting");
        let peer = loop {
            match link.wait_client(Duration::from_millis(500)) {
                Ok(n) => break n.name(),
                Err(miidrop::Error::Timeout(_)) => {}
                Err(e) => {
                    error(e);
                    link.close();
                    return Next::Continue;
                }
            }
            if let Some(n) = self.interrupted() {
                link.close();
                state("idle");
                return n;
            }
        };
        self.session(Box::new(link), &keys, "host", &peer)
    }

    fn after_interrupt(&mut self) -> Next {
        state("idle");
        if std::mem::take(&mut self.pending_quit) {
            Next::Quit
        } else {
            Next::Continue
        }
    }

    fn session(
        &mut self,
        link: Box<dyn Transport + '_>,
        keys: &Keys,
        role: &str,
        peer: &str,
    ) -> Next {
        emit(json!({ "event": "connected", "role": role, "peer": peer }));
        state("connected");
        let mut s = Session::new(link, keys.record);
        let next = loop {
            match self.commands.try_recv() {
                Ok(Command::Send { record }) => match decode_record(&record) {
                    Ok(_) if !s.ready() => error("the other console is not ready yet"),
                    Ok(r) => {
                        let wrapped = miidrop::mii::wrap(&keys.record, &r.for_sending());
                        let c = miidrop::container::build(&wrapped, miidrop::container::LENGTH_3DS);
                        match s.send_container(&c) {
                            Ok(()) => emit(json!({ "event": "sending", "name": r.name() })),
                            Err(e) => error(e),
                        }
                    }
                    Err(e) => error(e),
                },
                Ok(Command::Stop) => break Next::Continue,
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => break Next::Quit,
                Ok(other) => error(format!("busy, ignored {other:?}")),
                Err(TryRecvError::Empty) => {}
            }
            let event = match s.next_event(POLL) {
                Ok(e) => e,
                Err(e) => {
                    error(e);
                    break Next::Continue;
                }
            };
            match event {
                None | Some(Event::Nodes(_)) => {}
                Some(Event::PeerReady) => s.answer_ready(),
                Some(Event::Ready) => {
                    emit(json!({ "event": "ready" }));
                    state("ready");
                }
                Some(Event::PeerReceiving) => emit(json!({ "event": "peer_receiving" })),
                Some(Event::Verdict(accepted)) => {
                    emit(json!({ "event": "verdict", "accepted": accepted }))
                }
                Some(Event::Incoming) => emit(json!({ "event": "incoming" })),
                Some(Event::Received(r)) => {
                    let (record, info, err) = match &r.record {
                        Ok(rec) => (Some(hex(rec.as_bytes())), Some(describe(rec)), None),
                        Err(e) => (None, None, Some(e.to_string())),
                    };
                    emit(json!({
                        "event": "received", "accepted": r.accepted,
                        "record": record, "info": info, "error": err,
                    }));
                }
                Some(Event::Failed(why)) => error(why),
                Some(Event::Left) => {
                    emit(json!({ "event": "left" }));
                    break Next::Continue;
                }
            }
        };
        let _ = s.linger(Duration::from_millis(500));
        s.into_link().close();
        state("idle");
        next
    }
}

pub fn run(interface: &str, force: bool) -> Result<(), String> {
    let (tx, commands) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let reader_cancel = cancel.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match parse_command(&line) {
                Ok(c) => {
                    if matches!(c, Command::Stop | Command::Quit) {
                        reader_cancel.store(true, Ordering::Relaxed);
                    }
                    if tx.send(c).is_err() {
                        break;
                    }
                }
                Err(e) => error(e),
            }
        }
        reader_cancel.store(true, Ordering::Relaxed);
    });

    let mut radio = crate::open_radio(interface, force)?;
    emit(json!({
        "event": "hello", "version": env!("CARGO_PKG_VERSION"),
        "mac": mac_text(&radio.mac()), "active_monitor": radio.capabilities.active_monitor,
    }));
    state("idle");
    let mut helper = Helper {
        commands,
        cancel,
        keys: None,
        pending_quit: false,
    };
    loop {
        let cmd = match helper.commands.recv_timeout(Duration::from_secs(1)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        helper.cancel.store(false, Ordering::Relaxed);
        let next = match cmd {
            Command::Keys { text } => {
                match Keys::parse(&text) {
                    Ok(k) => {
                        emit(json!({ "event": "keys", "beacon": k.beacon.is_some() }));
                        helper.keys = Some(k);
                    }
                    Err(e) => error(e),
                }
                Next::Continue
            }
            Command::Scan => helper.scan(&mut radio),
            Command::Join { host, name } => helper.join(&mut radio, host, &name),
            Command::Host { channel, name } => helper.host(&mut radio, channel, &name),
            Command::Send { .. } => {
                error("not connected");
                Next::Continue
            }
            Command::Stop => Next::Continue,
            Command::Quit => Next::Quit,
        };
        if matches!(next, Next::Quit) {
            break;
        }
        state("idle");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse_command(r#"{"cmd":"scan"}"#).unwrap(), Command::Scan);
        assert_eq!(
            parse_command(r#"{"cmd":"host"}"#).unwrap(),
            Command::Host {
                channel: 6,
                name: "miidrop".into()
            }
        );
        assert_eq!(
            parse_command(r#"{"cmd":"join","host":"02:00:00:00:00:01","name":"PC"}"#).unwrap(),
            Command::Join {
                host: Some("02:00:00:00:00:01".into()),
                name: "PC".into()
            }
        );
        assert!(parse_command(r#"{"cmd":"format_disk"}"#).is_err());
        assert!(parse_command("not json").is_err());
    }

    #[test]
    fn decodes_records() {
        assert!(decode_record(&"00".repeat(96)).is_ok());
        assert!(decode_record(&"00".repeat(92)).is_ok());
        assert!(decode_record(&"00".repeat(95)).is_err());
        assert!(decode_record("zz").is_err());
    }
}
