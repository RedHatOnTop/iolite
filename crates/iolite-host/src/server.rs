//! The QUIC server: one connection per client, one streaming session at a time.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use iolite_proto::identity::{self, pairing_code, Fingerprint, Identity};
use iolite_proto::wire::{self, StreamKind, FLAG_KEYFRAME};
use iolite_proto::{
    quic, AudioFormat, ClientMsg, Hello, HostMsg, VideoCodec, Welcome, PROTOCOL_VERSION,
};
use quinn::crypto::rustls::QuicServerConfig;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{self, Clients};
use crate::control;
use crate::media::{self, AudioPipeline, Packet, VideoPipeline, VideoSource, MIN_KBPS};
use crate::mutter::DesktopSession;
use crate::pairing::Pairing;

/// Bytes QUIC may buffer ahead of acknowledgements. Small enough that a slow link blocks the
/// sender within a few frames, which is the signal the bitrate control reacts to.
const SEND_WINDOW: u64 = 192 * 1024;

pub struct Options {
    pub listen: SocketAddr,
    pub monitor: Option<String>,
    pub fps: u32,
    pub bitrate: u32,
    pub max_bitrate: u32,
    pub audio: bool,
    pub test_source: bool,
    pub auto_approve: bool,
}

struct Host {
    opts: Options,
    fingerprint: Fingerprint,
    host_name: String,
    bus: Option<zbus::Connection>,
    pairing: Arc<Pairing>,
    active: Mutex<Option<(u64, quinn::Connection)>>,
    next_id: AtomicU64,
}

pub async fn serve(opts: Options) -> Result<()> {
    gstreamer::init()?;
    let identity = Identity::load_or_generate(&config::config_dir(), "host", "iolite host")?;
    let fingerprint = identity.fingerprint();
    let crypto = QuicServerConfig::try_from(identity::server_config(&identity)?)?;
    let mut server_cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let mut transport = quic::transport();
    transport.send_window(SEND_WINDOW);
    server_cfg.transport_config(Arc::new(transport));
    let endpoint = quinn::Endpoint::server(server_cfg, opts.listen)
        .with_context(|| format!("listening on {}", opts.listen))?;

    let bus = if opts.test_source {
        None
    } else {
        Some(
            zbus::Connection::session()
                .await
                .context("connecting to the session bus")?,
        )
    };
    let pairing = Pairing::new(bus.as_ref()).await;
    tokio::spawn(control::listen(pairing.clone()));
    if opts.auto_approve {
        warn!("--insecure-auto-approve: any device that connects is paired without asking");
    }
    let host = Arc::new(Host {
        opts,
        fingerprint,
        host_name: config::host_name(),
        bus,
        pairing,
        active: Mutex::new(None),
        next_id: AtomicU64::new(1),
    });
    info!(
        "listening on {} (UDP); host fingerprint {fingerprint}",
        endpoint.local_addr()?
    );

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let host = host.clone();
                tokio::spawn(async move {
                    if let Err(e) = host.handle(incoming).await {
                        warn!("connection failed: {e:#}");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
        }
    }
    info!("shutting down");
    if let Some((_, conn)) = host.active.lock().unwrap().take() {
        close(&conn, "the computer stopped sharing");
    }
    endpoint.close(0u32.into(), b"shutdown");
    let _ = tokio::time::timeout(Duration::from_secs(2), endpoint.wait_idle()).await;
    Ok(())
}

fn close(conn: &quinn::Connection, reason: &str) {
    let mut end = reason.len().min(200);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    conn.close(0u32.into(), &reason.as_bytes()[..end]);
}

/// Sends a final message and gives the client a moment to read it before closing.
async fn say_goodbye(conn: &quinn::Connection, send: &mut quinn::SendStream, msg: HostMsg) {
    let reason = match &msg {
        HostMsg::Rejected { reason } | HostMsg::Bye { reason } => reason.clone(),
        _ => String::new(),
    };
    if wire::write_message(send, &msg).await.is_ok() {
        let _ = send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
    }
    close(conn, &reason);
}

impl Host {
    async fn handle(self: Arc<Self>, incoming: quinn::Incoming) -> Result<()> {
        let conn = incoming.await?;
        let peer = conn.remote_address();
        let client =
            Fingerprint::of_peer(conn.peer_identity()).context("client sent no certificate")?;
        let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi())
            .await
            .context("client opened no control stream")??;
        let hello = tokio::time::timeout(
            Duration::from_secs(10),
            wire::read_message::<ClientMsg, _>(&mut recv),
        )
        .await
        .context("client sent no hello")??;
        let Some(ClientMsg::Hello(hello)) = hello else {
            bail!("{peer} did not start with a hello");
        };
        info!("{peer}: {} ({client:?})", hello.device_name);

        if hello.version != PROTOCOL_VERSION {
            let reason = format!(
                "this app speaks protocol {} but the computer speaks {PROTOCOL_VERSION}; update both",
                hello.version
            );
            say_goodbye(&conn, &mut send, HostMsg::Rejected { reason }).await;
            return Ok(());
        }

        if !Clients::load()?.contains(&client) {
            wire::write_message(&mut send, &HostMsg::PairingRequired).await?;
            let code = pairing_code(&self.fingerprint, &client);
            let approved = self.opts.auto_approve
                || self
                    .pairing
                    .request(&code, &hello.device_name, conn.closed())
                    .await;
            if !approved {
                let reason = "pairing was declined on the computer".to_owned();
                say_goodbye(&conn, &mut send, HostMsg::Rejected { reason }).await;
                return Ok(());
            }
            let mut clients = Clients::load()?;
            clients.add(&hello.device_name, &client);
            clients.save()?;
            info!("paired with {} ({client:?})", hello.device_name);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let previous = self.active.lock().unwrap().replace((id, conn.clone()));
        if let Some((_, previous)) = previous {
            close(&previous, "another device took over the session");
        }

        let result = self.run_session(&conn, &mut send, &mut recv, &hello).await;
        {
            let mut active = self.active.lock().unwrap();
            if active.as_ref().is_some_and(|(i, _)| *i == id) {
                *active = None;
            }
        }
        let reason = match result {
            Ok(reason) => reason,
            Err(e) => format!("{e:#}"),
        };
        info!("{peer}: session ended: {reason}");
        say_goodbye(&conn, &mut send, HostMsg::Bye { reason }).await;
        Ok(())
    }

    async fn run_session(
        &self,
        conn: &quinn::Connection,
        send: &mut quinn::SendStream,
        recv: &mut quinn::RecvStream,
        hello: &Hello,
    ) -> Result<String> {
        let desktop = match &self.bus {
            Some(bus) => Some(DesktopSession::start(bus, self.opts.monitor.as_deref()).await?),
            None => None,
        };
        if let Some(d) = &desktop {
            info!("recording {} (PipeWire node {})", d.connector, d.node_id);
        }
        let result = self.stream(conn, send, recv, hello, desktop.as_ref()).await;
        if let Some(d) = &desktop {
            d.stop().await;
        }
        result
    }

    async fn stream(
        &self,
        conn: &quinn::Connection,
        send: &mut quinn::SendStream,
        recv: &mut quinn::RecvStream,
        hello: &Hello,
        desktop: Option<&DesktopSession>,
    ) -> Result<String> {
        let source = match desktop {
            Some(d) => VideoSource::PipeWire { node_id: d.node_id },
            None => VideoSource::Test,
        };
        let video = Arc::new(VideoPipeline::new(
            &source,
            self.opts.fps,
            self.opts.bitrate,
        )?);
        video.start()?;
        let first = {
            let video = video.clone();
            tokio::task::spawn_blocking(move || video.pull(Duration::from_secs(5))).await??
        }
        .context("no video frame within 5 s")?;
        let (width, height) = first.size.context("encoder caps carry no frame size")?;

        let audio = if hello.audio && self.opts.audio {
            match AudioPipeline::new(desktop.is_none()).and_then(|a| a.start().map(|()| a)) {
                Ok(a) => Some(Arc::new(a)),
                Err(e) => {
                    warn!("audio disabled: {e:#}");
                    None
                }
            }
        } else {
            None
        };

        let welcome = Welcome {
            host_name: self.host_name.clone(),
            width,
            height,
            codec: VideoCodec::Av1,
            audio: audio.as_ref().map(|_| AudioFormat {
                sample_rate: 48_000,
                channels: 2,
                opus_head: media::opus_head(media::OPUS_PRE_SKIP),
            }),
        };
        wire::write_message(send, &HostMsg::Welcome(welcome)).await?;
        info!("streaming {width}x{height} to {}", hello.device_name);

        let stop = Arc::new(AtomicBool::new(false));
        let (frames_tx, frames_rx) = mpsc::channel(2);
        frames_tx.send(Ok(first)).await.ok();
        let video_puller = {
            let video = video.clone();
            spawn_puller("iolite-video", stop.clone(), frames_tx, move |t| {
                video.pull(t)
            })?
        };
        let audio_puller = match &audio {
            Some(audio) => {
                let (tx, rx) = mpsc::channel(16);
                let audio = audio.clone();
                let handle =
                    spawn_puller("iolite-audio", stop.clone(), tx, move |t| audio.pull(t))?;
                Some((handle, rx))
            }
            None => None,
        };
        let (audio_handle, audio_rx) = match audio_puller {
            Some((h, rx)) => (Some(h), Some(rx)),
            None => (None, None),
        };

        let reason = tokio::select! {
            r = send_video(conn.clone(), frames_rx, video.clone(), (width, height), self.opts.bitrate, self.opts.max_bitrate) => r,
            r = send_audio(conn.clone(), audio_rx) => r,
            r = control(recv, send, desktop, &video, (width, height)) => r,
            _ = closed_by_desktop(desktop) => Ok("screen sharing was stopped on the computer".into()),
        };

        stop.store(true, Ordering::Relaxed);
        video.stop();
        if let Some(audio) = &audio {
            audio.stop();
        }
        let _ = tokio::task::spawn_blocking(move || {
            let _ = video_puller.join();
            if let Some(h) = audio_handle {
                let _ = h.join();
            }
        })
        .await;
        reason
    }
}

async fn closed_by_desktop(desktop: Option<&DesktopSession>) {
    match desktop {
        Some(d) => d.closed().await,
        None => std::future::pending().await,
    }
}

/// Moves packets from a blocking GStreamer sink into an async channel. A full channel blocks
/// the thread, which backs up the pipeline until its leaky queue starts dropping raw frames.
fn spawn_puller<F>(
    name: &str,
    stop: Arc<AtomicBool>,
    tx: mpsc::Sender<Result<Packet>>,
    mut pull: F,
) -> Result<std::thread::JoinHandle<()>>
where
    F: FnMut(Duration) -> Result<Option<Packet>> + Send + 'static,
{
    Ok(std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match pull(Duration::from_millis(100)) {
                    Ok(Some(packet)) => {
                        if tx.blocking_send(Ok(packet)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        break;
                    }
                }
            }
        })?)
}

async fn send_video(
    conn: quinn::Connection,
    mut frames: mpsc::Receiver<Result<Packet>>,
    video: Arc<VideoPipeline>,
    size: (u32, u32),
    start_kbps: u32,
    max_kbps: u32,
) -> Result<String> {
    let mut stream = conn.open_uni().await?;
    stream.set_priority(0)?;
    tokio::io::AsyncWriteExt::write_all(&mut stream, &[StreamKind::Video as u8]).await?;
    let mut rate = RateControl::new(start_kbps, max_kbps, Instant::now());
    while let Some(frame) = frames.recv().await {
        let frame = frame?;
        if frame.size.is_some_and(|s| s != size) {
            return Ok("the screen size changed".into());
        }
        let started = Instant::now();
        let flags = if frame.keyframe { FLAG_KEYFRAME } else { 0 };
        wire::write_packet(&mut stream, flags, frame.pts_us, &frame.data).await?;
        if let Some(kbps) = rate.observe(Instant::now(), started.elapsed(), frames.len()) {
            debug!("video bitrate {kbps} kbit/s");
            video.set_bitrate(kbps);
        }
    }
    Ok("video stopped".into())
}

async fn send_audio(
    conn: quinn::Connection,
    packets: Option<mpsc::Receiver<Result<Packet>>>,
) -> Result<String> {
    let Some(mut packets) = packets else {
        return std::future::pending().await;
    };
    let result: Result<()> = async {
        let mut stream = conn.open_uni().await?;
        stream.set_priority(1)?;
        tokio::io::AsyncWriteExt::write_all(&mut stream, &[StreamKind::Audio as u8]).await?;
        while let Some(packet) = packets.recv().await {
            let packet = packet?;
            wire::write_packet(&mut stream, 0, packet.pts_us, &packet.data).await?;
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        warn!("audio stopped: {e:#}");
    }
    std::future::pending().await
}

async fn control(
    recv: &mut quinn::RecvStream,
    send: &mut quinn::SendStream,
    desktop: Option<&DesktopSession>,
    video: &VideoPipeline,
    frame: (u32, u32),
) -> Result<String> {
    loop {
        let Some(msg) = wire::read_message::<ClientMsg, _>(recv).await? else {
            return Ok("the device disconnected".into());
        };
        match msg {
            ClientMsg::Input(input) => match desktop {
                Some(d) => {
                    if let Err(e) = d.inject(&input, frame).await {
                        debug!("input {input:?} rejected: {e}");
                    }
                }
                None => info!("input: {input:?}"),
            },
            ClientMsg::RequestKeyframe => video.force_keyframe(),
            ClientMsg::Ping { client_time_us } => {
                wire::write_message(send, &HostMsg::Pong { client_time_us }).await?
            }
            ClientMsg::Hello(_) => warn!("ignoring a second hello"),
        }
    }
}

/// Adapts the encoder bitrate to the link. vaav1enc starts a new keyframe whenever its
/// bitrate changes, so changes are rare: down by 30% after two congested seconds in a row, up by
/// 20% after ten clean seconds, never twice within three seconds.
struct RateControl {
    kbps: u32,
    max: u32,
    window: Instant,
    congested: bool,
    congested_windows: u32,
    clean_since: Instant,
    last_change: Instant,
}

impl RateControl {
    fn new(kbps: u32, max: u32, now: Instant) -> Self {
        Self {
            kbps,
            max: max.max(kbps),
            window: now,
            congested: false,
            congested_windows: 0,
            clean_since: now,
            last_change: now,
        }
    }

    fn observe(&mut self, now: Instant, write_time: Duration, backlog: usize) -> Option<u32> {
        if write_time > Duration::from_millis(30) || backlog > 0 {
            self.congested = true;
        }
        if now.duration_since(self.window) < Duration::from_secs(1) {
            return None;
        }
        self.window = now;
        if std::mem::take(&mut self.congested) {
            self.congested_windows += 1;
            self.clean_since = now;
        } else {
            self.congested_windows = 0;
        }
        if now.duration_since(self.last_change) < Duration::from_secs(3) {
            return None;
        }
        let next = if self.congested_windows >= 2 {
            (self.kbps * 7 / 10).max(MIN_KBPS)
        } else if now.duration_since(self.clean_since) >= Duration::from_secs(10) {
            (self.kbps * 6 / 5).min(self.max)
        } else {
            self.kbps
        };
        if next == self.kbps {
            return None;
        }
        self.kbps = next;
        self.last_change = now;
        self.clean_since = now;
        self.congested_windows = 0;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_control_changes_rarely() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let slow = Duration::from_millis(80);
        let fast = Duration::from_millis(1);
        let mut rate = RateControl::new(10_000, 20_000, t0);

        assert_eq!(rate.observe(at(1), slow, 0), None, "too soon after start");
        assert_eq!(rate.observe(at(2), slow, 0), None, "too soon after start");
        assert_eq!(rate.observe(at(3), slow, 0), Some(7000));
        assert_eq!(
            rate.observe(at(4), slow, 0),
            None,
            "one change per three seconds"
        );
        assert_eq!(rate.observe(at(5), fast, 0), None);
        for s in 6..14 {
            assert_eq!(
                rate.observe(at(s), fast, 0),
                None,
                "not clean for ten seconds at {s}"
            );
        }
        assert_eq!(
            rate.observe(at(14), fast, 0),
            Some(8400),
            "ten clean seconds since t=4"
        );
        assert_eq!(
            rate.observe(at(15), fast, 3),
            None,
            "a backlog is congestion"
        );
        assert_eq!(rate.observe(at(16), fast, 3), None);
        assert_eq!(rate.observe(at(17), fast, 3), Some(5880));
    }
}
