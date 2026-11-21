use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use miidrop::container;
use miidrop::host::HostLink;
use miidrop::keys::Keys;
use miidrop::link::Link;
use miidrop::mii::{self, FIELDS, RECORD_SIZE, Record, STORED_SIZE, WRAPPED_SIZE};
use miidrop::radio::Radio;
use miidrop::scan;
use miidrop::session::{Session, Transport};
use miidrop::uds::{MII_MAKER, NetworkInfo, NodeInfo};
use serde_json::{Value, json};

mod decode;
mod serve;

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
    /// Send a Mii to a console in Mii Maker.
    Send {
        #[command(flatten)]
        join: JoinArgs,
        /// Record, wrapped record or container to send.
        file: PathBuf,
    },
    /// Run as the desktop app's radio helper, speaking JSON lines on stdin and stdout.
    Serve {
        /// Wireless interface to take over.
        #[arg(short, long)]
        interface: String,
        /// Take over the interface even if it is connected to a network.
        #[arg(long)]
        force: bool,
    },
    /// Save a Mii a console in Mii Maker sends.
    Receive {
        #[command(flatten)]
        join: JoinArgs,
        /// Where to save the record; defaults to its Mii ID with a .cfsd extension.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
struct JoinArgs {
    /// Wireless interface to take over.
    #[arg(short, long)]
    interface: String,
    /// Only join the console with this MAC address.
    #[arg(long)]
    host: Option<String>,
    /// Name the console shows for this computer, up to 10 characters.
    #[arg(long, default_value = "miidrop")]
    name: String,
    /// Seconds to wait for each step that needs someone at the console.
    #[arg(long, default_value_t = 300)]
    timeout: u64,
    /// Take over the interface even if it is connected to a network.
    #[arg(long)]
    force: bool,
    /// Host a network and wait for a searching console instead of joining one.
    #[arg(long, conflicts_with = "host")]
    wait: bool,
    /// Channel to host on.
    #[arg(long, default_value_t = 6, requires = "wait", value_parser = clap::value_parser!(u8).range(1..=13))]
    channel: u8,
}

static STOP: LazyLock<Arc<AtomicBool>> = LazyLock::new(Default::default);

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

fn parse_mac(text: &str) -> Result<miidrop::wifi::frame::Mac, String> {
    let bytes: Vec<u8> = text
        .split(':')
        .map(|b| u8::from_str_radix(b, 16))
        .collect::<Result<_, _>>()
        .map_err(|_| format!("bad MAC address {text}"))?;
    bytes
        .try_into()
        .map_err(|_| format!("bad MAC address {text}"))
}

type LinuxRadio = miidrop::radio::linux::LinuxRadio;
type Joined = Session<Box<dyn Transport>>;

/// Joins a waiting console, or with --wait hosts and waits for a searching one.
fn connect(args: &JoinArgs, keys: &Keys) -> Result<Joined, String> {
    if args.name.encode_utf16().count() > 10 {
        return Err("--name must be at most 10 characters".into());
    }
    let want = args.host.as_deref().map(parse_mac).transpose()?;
    let radio = open_radio(&args.interface, args.force)?;
    if !radio.capabilities.active_monitor {
        return Err(format!(
            "{} cannot acknowledge frames in monitor mode, so it cannot connect",
            args.interface
        ));
    }
    let mac = radio.mac();
    let console_id = u64::from_be_bytes([0, 0, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]]);
    let me = NodeInfo::new(console_id, &args.name);
    let link: Box<dyn Transport> = if args.wait {
        Box::new(host(radio, args, keys, me)?)
    } else {
        Box::new(join(radio, args, keys, me, want)?)
    };
    let mut s = Session::new(link, keys.record);
    s.set_cancel(STOP.clone());
    Ok(s)
}

fn host(
    radio: LinuxRadio,
    args: &JoinArgs,
    keys: &Keys,
    me: NodeInfo,
) -> Result<HostLink<LinuxRadio>, String> {
    if keys.beacon.is_none() {
        eprintln!(
            "miidrop: the key file has no udsBeaconKey, so a searching console may not list this computer"
        );
    }
    let mut link = HostLink::start(
        radio,
        &MII_MAKER,
        &keys.session,
        keys.beacon,
        me,
        args.channel,
    )
    .map_err(|e| e.to_string())?;
    eprintln!(
        "Hosting on channel {}. Choose Search for Players on the console...",
        args.channel
    );
    let until = Instant::now() + Duration::from_secs(args.timeout);
    let node = loop {
        if STOP.load(Ordering::Relaxed) {
            link.close();
            return Err("cancelled".into());
        }
        let left = until.saturating_duration_since(Instant::now());
        match link.wait_client(left.min(Duration::from_millis(500))) {
            Ok(n) => break n,
            Err(miidrop::Error::Timeout(_)) if !left.is_zero() => {}
            Err(e) => {
                link.close();
                return Err(e.to_string());
            }
        }
    };
    eprintln!("{} joined. Press OK on the console.", node.name());
    Ok(link)
}

fn join(
    mut radio: LinuxRadio,
    args: &JoinArgs,
    keys: &Keys,
    me: NodeInfo,
    want: Option<miidrop::wifi::frame::Mac>,
) -> Result<Link<LinuxRadio>, String> {
    eprintln!("Searching for a console waiting in Mii Maker...");
    let until = Instant::now() + Duration::from_secs(args.timeout);
    let open = |n: &NetworkInfo| {
        want.is_none_or(|m| n.host == m)
            && n.appdata.starts_with(b"C:1")
            && n.total_nodes < n.max_nodes
    };
    let network = loop {
        if STOP.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if Instant::now() >= until {
            return Err("no console is waiting".into());
        }
        let found = scan::scan(
            &mut radio,
            &MII_MAKER,
            1,
            keys.beacon.as_ref(),
            &STOP,
            |_| {},
        )
        .map_err(|e| e.to_string())?;
        if let Some(f) = found.into_iter().find(|f| open(&f.network)) {
            break f.network;
        }
    };
    eprintln!(
        "Found {} on channel {}, joining...",
        mac_text(&network.host),
        network.channel
    );
    let link =
        Link::join(radio, &network, &keys.session, &MII_MAKER, me).map_err(|e| e.to_string())?;
    eprintln!(
        "Connected to {}. Press OK on the console.",
        link.host_name().unwrap_or_else(|| "the console".into())
    );
    Ok(link)
}

/// Leaves the network whatever happened, after a short linger on success.
fn finish<T>(mut s: Joined, result: Result<T, miidrop::Error>) -> Result<T, String> {
    if result.is_ok() {
        let _ = s.linger(Duration::from_secs(1));
    }
    s.into_link().close();
    result.map_err(|e| e.to_string())
}

fn send(args: &JoinArgs, keys: &Keys, record: &Record) -> Result<(), String> {
    let mut s = connect(args, keys)?;
    let t = Duration::from_secs(args.timeout);
    let mut run = || {
        s.wait_ready(t)?;
        // Gives the console a moment to reach its selection screen.
        s.linger(Duration::from_secs(1))?;
        eprintln!("Sending {}...", record.name());
        s.send_mii(record, t)
    };
    let result = run();
    if finish(s, result)? {
        eprintln!("The console accepted {}.", record.name());
        Ok(())
    } else {
        Err(format!("the console rejected {}", record.name()))
    }
}

fn receive(args: &JoinArgs, keys: &Keys, output: Option<PathBuf>) -> Result<(), String> {
    let mut s = connect(args, keys)?;
    let t = Duration::from_secs(args.timeout);
    let mut run = || {
        s.wait_ready(t)?;
        eprintln!("Waiting for the console to send a Mii...");
        s.receive_mii(t)
    };
    let result = run();
    let got = finish(s, result)?;
    let record = got
        .record
        .map_err(|e| format!("the Mii could not be read: {e}"))?;
    let path = output.unwrap_or_else(|| PathBuf::from(format!("{:08x}.cfsd", record.mii_id())));
    write(&path, record.as_bytes())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&describe(&record)).unwrap()
    );
    eprintln!("Received {}, saved to {}.", record.name(), path.display());
    if !got.accepted {
        eprintln!("The record failed validation, so the console was told it was rejected.");
    }
    Ok(())
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
        Command::Send { join, file } => {
            let record = read_record(&file, &cli.keys)?;
            send(&join, &load_keys(&cli.keys)?, &record)?;
        }
        Command::Serve { interface, force } => serve::run(&interface, force)?,
        Command::Receive { join, output } => receive(&join, &load_keys(&cli.keys)?, output)?,
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
