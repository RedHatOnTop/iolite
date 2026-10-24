//! `iolite probe`: a headless client for testing a host from a terminal.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iolite_client::{Config, Event};
use iolite_proto::identity::{Fingerprint, Identity};
use iolite_proto::{ClientMsg, Hello, Input, MouseButton, DEFAULT_PORT, PROTOCOL_VERSION};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::config;
use crate::pairing::spaced;

pub struct ProbeOptions {
    pub host: String,
    pub seconds: u64,
    pub out: Option<PathBuf>,
    pub click: Option<(f32, f32)>,
    pub text: Option<String>,
}

pub async fn probe(opts: ProbeOptions) -> Result<()> {
    let addr = resolve(&opts.host).await?;
    let dir = config::config_dir().join("probe");
    let identity = Identity::load_or_generate(&dir, "client", "iolite probe")?;
    let pin_path = dir.join(format!("{}.pin", opts.host.replace(['/', ':'], "_")));
    let pinned_host = std::fs::read_to_string(&pin_path)
        .ok()
        .and_then(|s| Fingerprint::from_hex(&s));
    let hello = Hello {
        version: PROTOCOL_VERSION,
        device_name: format!("iolite probe on {}", config::host_name()),
        display_width: 1920,
        display_height: 1200,
        audio: true,
    };
    let (commands, commands_rx) = mpsc::unbounded_channel();
    let (events_tx, mut events) = mpsc::channel(64);
    let session = tokio::spawn(iolite_client::run(
        Config {
            addr,
            identity,
            pinned_host,
            hello,
        },
        commands_rx,
        events_tx,
    ));
    println!("connecting to {addr}");

    let mut out = match &opts.out {
        Some(path) => Some(tokio::fs::File::create(path).await?),
        None => None,
    };
    let mut commands = Some(commands);
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut stats = Stats::default();
    loop {
        let event = match deadline {
            Some(at) => match tokio::time::timeout_at(at, events.recv()).await {
                Ok(event) => event,
                Err(_) => {
                    deadline = None;
                    commands = None;
                    continue;
                }
            },
            None => events.recv().await,
        };
        let Some(event) = event else { break };
        match event {
            Event::PairingCode(code) => {
                println!(
                    "pairing code {}: accept it on the host (or run `iolite approve {code}` there)",
                    spaced(&code)
                );
            }
            Event::Welcome { host, welcome } => {
                std::fs::write(&pin_path, host.to_hex())?;
                println!(
                    "streaming {}x{} from {} (host {host:?}, audio {})",
                    welcome.width,
                    welcome.height,
                    welcome.host_name,
                    if welcome.audio.is_some() { "on" } else { "off" }
                );
                stats.welcomed = Some(Instant::now());
                deadline = Some(tokio::time::Instant::now() + Duration::from_secs(opts.seconds));
                if let Some(tx) = &commands {
                    send_test_input(tx, &opts);
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        let _ = tx.send(ClientMsg::RequestKeyframe);
                    });
                }
            }
            Event::Video(packet) => {
                stats.video(packet.data.len(), packet.keyframe);
                if let Some(file) = &mut out {
                    file.write_all(&packet.data).await?;
                }
            }
            Event::Audio(_) => stats.audio += 1,
            Event::Closed(reason) => {
                println!("closed: {reason}");
                break;
            }
        }
    }
    drop(commands);
    session.await?;
    stats.report();
    Ok(())
}

fn send_test_input(tx: &mpsc::UnboundedSender<ClientMsg>, opts: &ProbeOptions) {
    if let Some((x, y)) = opts.click {
        for input in [
            Input::PointerMove { x, y },
            Input::Button {
                button: MouseButton::Left,
                down: true,
            },
            Input::Button {
                button: MouseButton::Left,
                down: false,
            },
        ] {
            let _ = tx.send(ClientMsg::Input(input));
        }
    }
    if let Some(text) = &opts.text {
        let _ = tx.send(ClientMsg::Input(Input::Text { text: text.clone() }));
    }
}

async fn resolve(host: &str) -> Result<SocketAddr> {
    if let Ok(addr) = host.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let target = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:{DEFAULT_PORT}")
    };
    let mut addrs = tokio::net::lookup_host(target.as_str())
        .await
        .with_context(|| format!("resolving {target}"))?;
    let addr = addrs
        .next()
        .with_context(|| format!("{target} has no address"))?;
    Ok(addr)
}

#[derive(Default)]
struct Stats {
    welcomed: Option<Instant>,
    first_frame: Option<Duration>,
    frames: u64,
    keyframes: u64,
    bytes: u64,
    audio: u64,
}

impl Stats {
    fn video(&mut self, len: usize, keyframe: bool) {
        if self.first_frame.is_none() {
            self.first_frame = self.welcomed.map(|t| t.elapsed());
        }
        self.frames += 1;
        self.keyframes += u64::from(keyframe);
        self.bytes += len as u64;
    }

    fn report(&self) {
        let secs = self
            .welcomed
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0)
            .max(0.001);
        println!(
            "video: {} frames ({} key), {:.1} fps, {:.0} kbit/s, first frame {} ms after welcome; audio: {} packets",
            self.frames,
            self.keyframes,
            self.frames as f64 / secs,
            self.bytes as f64 * 8.0 / 1000.0 / secs,
            self.first_frame.map(|d| d.as_millis()).unwrap_or(0),
            self.audio
        );
    }
}
