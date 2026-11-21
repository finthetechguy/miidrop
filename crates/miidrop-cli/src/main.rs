use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Parser, Subcommand};
use miidrop::container;
use miidrop::keys::Keys;
use miidrop::mii::{self, FIELDS, RECORD_SIZE, Record, STORED_SIZE, WRAPPED_SIZE};
use miidrop::radio::Radio;
use miidrop::scan;
use miidrop::uds::MII_MAKER;
use serde_json::{Value, json};

mod decode;

#[derive(Parser)]
#[command(
    name = "miidrop",
    version,
    about = "Send and receive Miis over local wireless"
)]
struct Cli {
    /// Key file; defaults to $MIIDROP_KEYS.
    #[arg(long, global = true)]
    keys: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a record, wrapped record or container as JSON.
    Info { file: PathBuf },
    /// Protect a record for sending, optionally inside a container.
    Wrap {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        container: bool,
        /// Use the length word a Wii U writes.
        #[arg(long, requires = "container")]
        wii_u: bool,
    },
    /// Recover the record from a wrapped record or container.
    Unwrap { input: PathBuf, output: PathBuf },
    /// Decrypt the Mii Maker sessions in a capture and print each message as JSON.
    Decode { pcap: PathBuf },
    /// List consoles waiting in Mii Maker.
    Scan {
        /// Wireless interface to take over while scanning.
        #[arg(short, long)]
        interface: String,
        /// Passes over all channels.
        #[arg(long, default_value_t = 3)]
        rounds: u32,
        /// Take over the interface even if it is connected to a network.
        #[arg(long)]
        force: bool,
        /// Report how many frames each channel delivered.
        #[arg(short, long)]
        verbose: bool,
    },
}

static STOP: AtomicBool = AtomicBool::new(false);

/// Wraps a radio to report frames received per channel.
struct Counting<R: Radio> {
    inner: R,
    channel: u8,
    frames: u32,
    verbose: bool,
}

impl<R: Radio> Counting<R> {
    fn report(&mut self) {
        if self.verbose && self.channel != 0 {
            eprintln!("channel {:2}: {} frames", self.channel, self.frames);
        }
        self.frames = 0;
    }
}

impl<R: Radio> Radio for Counting<R> {
    fn mac(&self) -> miidrop::wifi::frame::Mac {
        self.inner.mac()
    }
    fn set_channel(&mut self, channel: u8) -> std::io::Result<()> {
        self.report();
        self.channel = channel;
        self.inner.set_channel(channel)
    }
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        self.inner.send(frame)
    }
    fn recv(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<Option<miidrop::radio::Received>> {
        let r = self.inner.recv(timeout)?;
        self.frames += r.is_some() as u32;
        Ok(r)
    }
}

impl<R: Radio> Drop for Counting<R> {
    fn drop(&mut self) {
        self.report();
    }
}

fn mac_text(m: &[u8]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn found_json(f: &scan::Found) -> Value {
    let n = &f.network;
    json!({
        "host": mac_text(&n.host),
        "channel": n.channel,
        "network_id": format!("{:08x}", n.network_id),
        "nodes": [n.total_nodes, n.max_nodes],
        "available": n.appdata.starts_with(b"C:1"),
        "signal": f.signal,
        "names": f.names,
    })
}

#[cfg(target_os = "linux")]
fn open_radio(interface: &str, force: bool) -> Result<miidrop::radio::linux::LinuxRadio, String> {
    let radio = miidrop::radio::linux::LinuxRadio::open(interface, force).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied
            || e.to_string().contains("Operation not permitted")
        {
            format!("{e} (needs root or CAP_NET_ADMIN and CAP_NET_RAW)")
        } else {
            e.to_string()
        }
    })?;
    if !radio.capabilities.active_monitor {
        eprintln!(
            "miidrop: {interface} cannot acknowledge frames in monitor mode; scanning works, connecting will not"
        );
    }
    Ok(radio)
}

fn load_keys(path: &Option<PathBuf>) -> Result<Keys, String> {
    let path = path
        .clone()
        .or_else(|| std::env::var_os("MIIDROP_KEYS").map(PathBuf::from))
        .ok_or("no key file: pass --keys or set MIIDROP_KEYS")?;
    Keys::load(&path).map_err(|e| e.to_string())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn describe(r: &Record) -> Value {
    let (month, day) = r.birthday();
    let fields: serde_json::Map<String, Value> = FIELDS
        .iter()
        .map(|f| (f.name.to_string(), json!(r.get(f))))
        .collect();
    json!({
        "name": r.name(),
        "creator": r.creator_name(),
        "mii_id": format!("{:08x}", r.mii_id()),
        "system_id": hex(&r.system_id()),
        "creator_mac": hex(&r.creator_mac()),
        "version": r.version(),
        "birthday": [month, day],
        "copying_allowed": r.copying_allowed(),
        "sharing_disabled": r.sharing_disabled(),
        "favourite": r.favourite(),
        "crc_valid": r.crc_valid(),
        "issues": r.validate().iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        "fields": fields,
    })
}

/// Reads any supported form and returns the plain record.
fn read_record(path: &Path, keys: &Option<PathBuf>) -> Result<Record, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match data.len() {
        RECORD_SIZE => Record::from_bytes(&data).map_err(|e| e.to_string()),
        STORED_SIZE => Record::from_stored(&data).map_err(|e| e.to_string()),
        WRAPPED_SIZE => mii::unwrap(&load_keys(keys)?.record, &data).map_err(|e| e.to_string()),
        _ => {
            let wrapped = container::parse(&data).map_err(|e| e.to_string())?;
            mii::unwrap(&load_keys(keys)?.record, &wrapped).map_err(|e| e.to_string())
        }
    }
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Info { file } => {
            let r = read_record(&file, &cli.keys)?;
            println!("{}", serde_json::to_string_pretty(&describe(&r)).unwrap());
        }
        Command::Wrap {
            input,
            output,
            container: boxed,
            wii_u,
        } => {
            let r = read_record(&input, &cli.keys)?.for_sending();
            let wrapped = mii::wrap(&load_keys(&cli.keys)?.record, &r);
            if boxed {
                let length = if wii_u {
                    container::LENGTH_WII_U
                } else {
                    container::LENGTH_3DS
                };
                write(&output, &container::build(&wrapped, length))?;
            } else {
                write(&output, &wrapped)?;
            }
        }
        Command::Unwrap { input, output } => {
            write(&output, read_record(&input, &cli.keys)?.as_bytes())?;
        }
        Command::Decode { pcap } => decode::run(&pcap, &load_keys(&cli.keys)?)?,
        Command::Scan {
            interface,
            rounds,
            force,
            verbose,
        } => {
            let beacon_key = load_keys(&cli.keys).ok().and_then(|k| k.beacon);
            let radio = open_radio(&interface, force)?;
            let mut radio = Counting {
                inner: radio,
                channel: 0,
                frames: 0,
                verbose,
            };
            scan::scan(
                &mut radio,
                &MII_MAKER,
                rounds,
                beacon_key.as_ref(),
                &STOP,
                |f| {
                    println!("{}", found_json(f));
                },
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let _ = ctrlc::set_handler(|| STOP.store(true, Ordering::Relaxed));
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("miidrop: {e}");
            ExitCode::FAILURE
        }
    }
}
