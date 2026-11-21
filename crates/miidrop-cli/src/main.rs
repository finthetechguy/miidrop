use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use miidrop::container;
use miidrop::keys::Keys;
use miidrop::mii::{self, FIELDS, RECORD_SIZE, Record, STORED_SIZE, WRAPPED_SIZE};
use serde_json::{Value, json};

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
}

fn load_keys(path: &Option<PathBuf>) -> Result<Keys, String> {
    let path = path
        .clone()
        .or_else(|| std::env::var_os("MIIDROP_KEYS").map(PathBuf::from))
        .ok_or("no key file: pass --keys or set MIIDROP_KEYS")?;
    Keys::load(&path).map_err(|e| e.to_string())
}

fn hex(bytes: &[u8]) -> String {
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
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("miidrop: {e}");
            ExitCode::FAILURE
        }
    }
}
