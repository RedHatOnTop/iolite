mod config;
mod control;
mod hangul;
mod ibus;
mod media;
mod mutter;
mod pairing;
mod probe;
mod server;

use std::net::{Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use iolite_proto::identity::Identity;
use iolite_proto::DEFAULT_PORT;
use tracing_subscriber::EnvFilter;

use crate::config::Clients;

#[derive(Parser)]
#[command(
    name = "iolite",
    version,
    about = "Remote desktop host for GNOME on Wayland"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Share this desktop with paired devices.
    Serve(ServeArgs),
    /// Accept a waiting pairing request.
    Approve {
        #[arg(required = true)]
        code: Vec<String>,
    },
    /// Decline a waiting pairing request.
    Decline {
        #[arg(required = true)]
        code: Vec<String>,
    },
    /// List waiting pairing requests.
    Pending,
    /// List paired devices, or forget one.
    Clients {
        /// Forget the devices whose fingerprint starts with this.
        #[arg(long, value_name = "FINGERPRINT_PREFIX")]
        forget: Option<String>,
    },
    /// Print this computer's certificate fingerprint.
    Fingerprint,
    /// Connect to a host as a test client and report what arrives.
    Probe(ProbeArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// UDP address to listen on.
    #[arg(long, default_value_t = SocketAddr::from((Ipv6Addr::UNSPECIFIED, DEFAULT_PORT)))]
    listen: SocketAddr,
    /// Monitor connector to share, such as eDP-1. Defaults to the primary monitor.
    #[arg(long)]
    monitor: Option<String>,
    #[arg(long, default_value_t = 60)]
    fps: u32,
    /// Starting video bitrate in kbit/s.
    #[arg(long, default_value_t = 6000)]
    bitrate: u32,
    /// Ceiling for the adaptive bitrate in kbit/s.
    #[arg(long, default_value_t = 20000)]
    max_bitrate: u32,
    #[arg(long)]
    no_audio: bool,
    /// Stream a test pattern and log input instead of sharing the desktop.
    #[arg(long)]
    test_source: bool,
    /// Pair every device that connects without asking. For automated tests only.
    #[arg(long)]
    insecure_auto_approve: bool,
}

#[derive(Args)]
struct ProbeArgs {
    /// Host name or address, optionally with :port.
    host: String,
    /// How long to stream after the host says welcome.
    #[arg(long, default_value_t = 5)]
    seconds: u64,
    /// Write the received AV1 stream (OBU format) to this file.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Click at this normalized position, e.g. 0.5,0.5.
    #[arg(long, value_parser = parse_point)]
    click: Option<(f32, f32)>,
    /// Type this text.
    #[arg(long = "type")]
    text: Option<String>,
    /// Replay input method steps separated by '|': "~text" composes, "=text" commits, "^" is
    /// a backspace key. For example "~ㅎ|~하|~한|=한|~ㄱ|~그|~글|=글".
    #[arg(long)]
    ime: Option<String>,
    /// Tap with one finger at this normalized position.
    #[arg(long, value_parser = parse_point)]
    tap: Option<(f32, f32)>,
}

fn parse_point(s: &str) -> Result<(f32, f32), String> {
    let (x, y) = s.split_once(',').ok_or("expected X,Y")?;
    let parse = |v: &str| v.trim().parse::<f32>().map_err(|e| e.to_string());
    Ok((parse(x)?, parse(y)?))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match Cli::parse().command {
        Command::Serve(a) => {
            server::serve(server::Options {
                listen: a.listen,
                monitor: a.monitor,
                fps: a.fps,
                bitrate: a.bitrate,
                max_bitrate: a.max_bitrate,
                audio: !a.no_audio,
                test_source: a.test_source,
                auto_approve: a.insecure_auto_approve,
            })
            .await
        }
        Command::Approve { code } => reply(&format!("approve {}", code.concat())).await,
        Command::Decline { code } => reply(&format!("decline {}", code.concat())).await,
        Command::Pending => reply("pending").await,
        Command::Clients { forget } => clients(forget),
        Command::Fingerprint => {
            let id = Identity::load_or_generate(&config::config_dir(), "host", "iolite host")?;
            println!("{}", id.fingerprint());
            Ok(())
        }
        Command::Probe(a) => {
            probe::probe(probe::ProbeOptions {
                host: a.host,
                seconds: a.seconds,
                out: a.out,
                click: a.click,
                text: a.text,
                ime: a.ime,
                tap: a.tap,
            })
            .await
        }
    }
}

async fn reply(line: &str) -> Result<()> {
    println!("{}", control::command(line).await?);
    Ok(())
}

fn clients(forget: Option<String>) -> Result<()> {
    let mut clients = Clients::load()?;
    if let Some(prefix) = forget {
        if prefix.len() < 4 {
            bail!("give at least 4 characters of the fingerprint");
        }
        let gone = clients.remove_prefix(&prefix);
        if gone.is_empty() {
            bail!("no paired device has a fingerprint starting with {prefix}");
        }
        clients.save()?;
        for c in gone {
            println!("forgot {} ({})", c.name, short(&c.fingerprint));
        }
        return Ok(());
    }
    if clients.list.is_empty() {
        println!("no paired devices");
    }
    for c in &clients.list {
        println!("{}  {}", short(&c.fingerprint), c.name);
    }
    Ok(())
}

fn short(fingerprint: &str) -> &str {
    fingerprint.get(..16).unwrap_or(fingerprint)
}
