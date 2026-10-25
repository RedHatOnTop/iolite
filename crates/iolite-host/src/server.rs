//! The QUIC server: one connection per client, one streaming session at a time.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures_util::{Stream, StreamExt};
use iolite_proto::identity::{self, pairing_code, Fingerprint, Identity};
use iolite_proto::wire::{self, StreamKind, FLAG_KEYFRAME};
use iolite_proto::{
    hello_version, quic, AudioFormat, ClientMsg, DisplayMode, Hello, HostMsg, VideoCodec, Welcome,
    PROTOCOL_VERSION,
};
use quinn::crypto::rustls::QuicServerConfig;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{self, Clients};
use crate::control;
use crate::display::{Arranged, Monitors, Placement};
use crate::media::{self, AudioPipeline, Packet, VideoPipeline, VideoSource, MIN_KBPS};
use crate::mutter::{Capture, DesktopSession};
use crate::pairing::Pairing;

/// Bytes QUIC may buffer ahead of acknowledgements. Small enough that a slow link blocks the
/// sender within a few frames, which is the signal the bitrate control reacts to.
const SEND_WINDOW: u64 = 192 * 1024;
/// Limits for each side of a screen made for a client.
const MIN_SIDE: u32 = 320;
const MAX_SIDE: u32 = 4096;
/// Mutter reports one layout change as a burst of signals.
const SETTLE: Duration = Duration::from_millis(150);
/// Layouts tried in a row before the host leaves Mutter's own.
const MAX_ARRANGE_ATTEMPTS: u32 = 3;
/// A new pipeline that sends nothing for this long is built again.
const STALL: Duration = Duration::from_secs(3);
const MAX_RESTARTS: u32 = 3;

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
    /// Held from recording to putting the monitors back, so a session that takes over starts
    /// from the computer's own layout rather than one still holding the last client's screen.
    desktop: tokio::sync::Mutex<()>,
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
        desktop: tokio::sync::Mutex::new(()),
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
        let body =
            tokio::time::timeout(Duration::from_secs(10), wire::read_message_body(&mut recv))
                .await
                .context("client sent no hello")??
                .with_context(|| format!("{peer} closed the control stream"))?;
        // A hello from another protocol version may not decode, but its version always does.
        let version = hello_version(&body);
        if let Some(version) = version.filter(|&v| v != PROTOCOL_VERSION) {
            info!("{peer}: speaks protocol {version}");
            let reason = format!(
                "this app speaks protocol {version} but the computer speaks {PROTOCOL_VERSION}; update both"
            );
            say_goodbye(&conn, &mut send, HostMsg::Rejected { reason }).await;
            return Ok(());
        }
        let Ok(ClientMsg::Hello(hello)) = wire::decode_message::<ClientMsg>(&body) else {
            bail!("{peer} did not start with a hello");
        };
        info!(
            "{peer}: {} ({client:?}), {:?} at {}x{}",
            hello.device_name, hello.display, hello.display_width, hello.display_height
        );

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
        let _turn = self.desktop.lock().await;
        let desktop = match &self.bus {
            Some(bus) => Some(self.open_desktop(bus, hello).await?),
            None => None,
        };
        let result = self.stream(conn, send, recv, hello, desktop.as_ref()).await;
        if let Some(d) = &desktop {
            d.session.stop().await;
            if let Some(placement) = &d.placement {
                if let Err(e) = placement.restore(&d.monitors).await {
                    warn!("putting the monitors back: {e:#}");
                }
            }
        }
        result
    }

    async fn open_desktop(&self, bus: &zbus::Connection, hello: &Hello) -> Result<Desktop> {
        let monitors = Monitors::new(bus).await?;
        let (capture, placement) = match hello.display {
            DisplayMode::Mirror => {
                let connector = match &self.opts.monitor {
                    Some(m) => m.clone(),
                    None => monitors.primary_connector().await?,
                };
                (Capture::Monitor(connector), None)
            }
            mode => {
                let placement = Placement::new(&monitors, mode, hello.scale).await?;
                (Capture::Virtual, Some(placement))
            }
        };
        let session = DesktopSession::start(bus, &capture).await?;
        let node = session.node_id;
        match &capture {
            Capture::Monitor(connector) => info!("recording {connector} (PipeWire node {node})"),
            Capture::Virtual => {
                info!("recording a screen made for the device (PipeWire node {node})")
            }
        }
        Ok(Desktop {
            session,
            monitors,
            placement,
        })
    }

    async fn stream(
        &self,
        conn: &quinn::Connection,
        send: &mut quinn::SendStream,
        recv: &mut quinn::RecvStream,
        hello: &Hello,
        desktop: Option<&Desktop>,
    ) -> Result<String> {
        let source = match desktop {
            Some(d) => VideoSource::PipeWire {
                node_id: d.session.node_id,
            },
            None => VideoSource::Test,
        };
        // The test source follows the client's size too, so the emulator test covers resizing.
        let size = (hello.display != DisplayMode::Mirror)
            .then(|| fit_size(hello.display_width, hello.display_height));
        let feed = Arc::new(Feed::new(source, self.opts.fps, self.opts.bitrate));
        let first = {
            let feed = feed.clone();
            tokio::task::spawn_blocking(move || {
                feed.start(size)?;
                feed.pull(Duration::from_secs(5))
            })
            .await??
        }
        .context("no video frame within 5 s")?;
        let (width, height) = first.size.context("encoder caps carry no frame size")?;
        feed.set_size((width, height));

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
            let feed = feed.clone();
            spawn_puller("iolite-video", stop.clone(), frames_tx, move |t| {
                feed.pull(t)
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

        let (resizes_tx, resizes_rx) = mpsc::unbounded_channel();
        let session = desktop.map(|d| &d.session);
        let reason = tokio::select! {
            r = send_video(conn.clone(), frames_rx, &feed, (width, height), self.opts.max_bitrate) => r,
            r = send_audio(conn.clone(), audio_rx) => r,
            r = control(recv, send, session, &feed, resizes_tx) => r,
            r = follow_screens(&feed, desktop, size, resizes_rx) => r,
            _ = closed_by_desktop(session) => Ok("screen sharing was stopped on the computer".into()),
        };

        stop.store(true, Ordering::Relaxed);
        feed.stop();
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

/// What a session records, and for a screen made for the client, where it goes.
struct Desktop {
    session: DesktopSession,
    monitors: Monitors,
    placement: Option<Placement>,
}

async fn closed_by_desktop(desktop: Option<&DesktopSession>) {
    match desktop {
        Some(d) => d.closed().await,
        None => std::future::pending().await,
    }
}

/// A size for a screen made for the client: whole multiples of 8 within limits, which every
/// encoder takes.
fn fit_size(width: u32, height: u32) -> (u32, u32) {
    let side = |v: u32| v.clamp(MIN_SIDE, MAX_SIDE) / 8 * 8;
    (side(width), side(height))
}

/// The video pipeline, replaced whenever the screen it records changes. The video thread pulls
/// from whichever one is current.
struct Feed {
    source: VideoSource,
    fps: u32,
    kbps: AtomicU32,
    video: Mutex<Option<Arc<VideoPipeline>>>,
    /// Size of the frames the client is getting; input positions are relative to it.
    size: Mutex<(u32, u32)>,
    frames: AtomicU64,
}

impl Feed {
    fn new(source: VideoSource, fps: u32, kbps: u32) -> Self {
        Self {
            source,
            fps,
            kbps: AtomicU32::new(kbps),
            video: Mutex::new(None),
            size: Mutex::new((0, 0)),
            frames: AtomicU64::new(0),
        }
    }

    /// Replaces the pipeline with a new one that asks for `size`.
    fn start(&self, size: Option<(u32, u32)>) -> Result<()> {
        self.stop();
        let kbps = self.kbps.load(Ordering::Relaxed);
        let video = VideoPipeline::new(&self.source, size, self.fps, kbps)?;
        video.start()?;
        *self.video.lock().unwrap() = Some(Arc::new(video));
        Ok(())
    }

    fn stop(&self) {
        let old = self.video.lock().unwrap().take();
        if let Some(old) = old {
            old.stop();
        }
    }

    fn pull(&self, timeout: Duration) -> Result<Option<Packet>> {
        let current = self.video.lock().unwrap().clone();
        let Some(video) = current else {
            std::thread::sleep(Duration::from_millis(10));
            return Ok(None);
        };
        match video.pull(timeout) {
            Ok(packet) => {
                if packet.is_some() {
                    self.frames.fetch_add(1, Ordering::Relaxed);
                }
                Ok(packet)
            }
            // The pipeline was replaced while this waited on it.
            Err(_) if !self.is_current(&video) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn is_current(&self, video: &Arc<VideoPipeline>) -> bool {
        self.video
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|v| Arc::ptr_eq(v, video))
    }

    fn with_current(&self, f: impl FnOnce(&VideoPipeline)) {
        if let Some(video) = self.video.lock().unwrap().as_deref() {
            f(video);
        }
    }

    fn force_keyframe(&self) {
        self.with_current(VideoPipeline::force_keyframe);
    }

    fn kbps(&self) -> u32 {
        self.kbps.load(Ordering::Relaxed)
    }

    fn set_bitrate(&self, kbps: u32) {
        self.kbps.store(kbps, Ordering::Relaxed);
        self.with_current(|v| v.set_bitrate(kbps));
    }

    fn size(&self) -> (u32, u32) {
        *self.size.lock().unwrap()
    }

    fn set_size(&self, size: (u32, u32)) {
        *self.size.lock().unwrap() = size;
    }

    fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }
}

/// Keeps the video going through monitor changes. Mutter stops filling a screen cast stream
/// after any change of monitor layout until a consumer connects to it again, so each change
/// ends with a new pipeline. A screen made for the client also follows the client's size and is
/// kept where its display mode puts it; Mutter lays the monitors out afresh whenever that
/// screen changes size.
async fn follow_screens(
    feed: &Arc<Feed>,
    desktop: Option<&Desktop>,
    mut size: Option<(u32, u32)>,
    mut resizes: mpsc::UnboundedReceiver<(u32, u32)>,
) -> Result<String> {
    let mut changes = match desktop {
        Some(d) => Some(d.monitors.changes().await?),
        None => None,
    };
    let placement = desktop.and_then(|d| Some((&d.monitors, d.placement.as_ref()?)));
    // Mutter has just added the client's screen wherever it puts new monitors.
    let mut settle = placement.map(|_| tokio::time::Instant::now());
    let mut attempts = 0;
    let mut applied = false;
    let mut watch: Option<(tokio::time::Instant, u64)> = None;
    let mut restarts = 0;
    loop {
        tokio::select! {
            () = next_change(&mut changes) => {
                settle = Some(tokio::time::Instant::now() + SETTLE);
            }
            Some((width, height)) = resizes.recv() => {
                let wanted = fit_size(width, height);
                if size.is_some_and(|s| s != wanted) {
                    info!("resizing the screen to {}x{}", wanted.0, wanted.1);
                    size = Some(wanted);
                    attempts = 0;
                    watch = Some(restart(feed, size).await?);
                }
            }
            () = wait_until(settle) => {
                settle = None;
                let Some((monitors, place)) = placement.filter(|_| attempts < MAX_ARRANGE_ATTEMPTS)
                else {
                    watch = Some(restart(feed, size).await?);
                    continue;
                };
                match place.arrange(monitors).await {
                    Ok(Arranged::Applied) => {
                        attempts += 1;
                        applied = true;
                    }
                    Ok(Arranged::Settled) => {
                        attempts = 0;
                        if std::mem::take(&mut applied) {
                            watch = Some(restart(feed, size).await?);
                        }
                    }
                    Ok(Arranged::Missing) => {}
                    Err(e) => {
                        attempts += 1;
                        warn!("{e:#}");
                        settle = Some(tokio::time::Instant::now() + Duration::from_secs(1));
                    }
                }
            }
            () = wait_until(watch.map(|(at, _)| at)) => {
                let Some((_, frames)) = watch.take() else { continue };
                if feed.frames() != frames {
                    restarts = 0;
                    continue;
                }
                restarts += 1;
                if restarts > MAX_RESTARTS {
                    bail!("the screen stopped sending video");
                }
                warn!("no video since the pipeline was rebuilt; rebuilding it again");
                watch = Some(restart(feed, size).await?);
            }
        }
    }
}

/// Builds a new pipeline; returns when to check that it sends video, and the frame count to
/// check against.
async fn restart(
    feed: &Arc<Feed>,
    size: Option<(u32, u32)>,
) -> Result<(tokio::time::Instant, u64)> {
    let frames = feed.frames();
    let new = feed.clone();
    tokio::task::spawn_blocking(move || new.start(size)).await??;
    debug!("video pipeline rebuilt");
    Ok((tokio::time::Instant::now() + STALL, frames))
}

async fn wait_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn next_change<S: Stream<Item = ()> + Unpin>(changes: &mut Option<S>) {
    if let Some(changes) = changes {
        if changes.next().await.is_some() {
            return;
        }
    }
    std::future::pending().await
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
    feed: &Feed,
    mut size: (u32, u32),
    max_kbps: u32,
) -> Result<String> {
    let mut stream = conn.open_uni().await?;
    stream.set_priority(0)?;
    tokio::io::AsyncWriteExt::write_all(&mut stream, &[StreamKind::Video as u8]).await?;
    let mut rate = RateControl::new(feed.kbps(), max_kbps, Instant::now());
    let mut awaiting_keyframe = false;
    while let Some(frame) = frames.recv().await {
        let frame = frame?;
        let mut announce = None;
        if let Some(new) = frame.size.filter(|&s| s != size) {
            // The client starts a new decoder for a new size, and a decoder starts on a keyframe.
            if !frame.keyframe {
                if !std::mem::replace(&mut awaiting_keyframe, true) {
                    feed.force_keyframe();
                }
                continue;
            }
            info!("video is {}x{} now", new.0, new.1);
            awaiting_keyframe = false;
            size = new;
            feed.set_size(new);
            announce = Some(new);
        }
        let started = Instant::now();
        let flags = if frame.keyframe { FLAG_KEYFRAME } else { 0 };
        wire::write_video_packet(&mut stream, flags, frame.pts_us, announce, &frame.data).await?;
        if let Some(kbps) = rate.observe(Instant::now(), started.elapsed(), frames.len()) {
            debug!("video bitrate {kbps} kbit/s");
            feed.set_bitrate(kbps);
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
    feed: &Feed,
    resizes: mpsc::UnboundedSender<(u32, u32)>,
) -> Result<String> {
    loop {
        let Some(msg) = wire::read_message::<ClientMsg, _>(recv).await? else {
            return Ok("the device disconnected".into());
        };
        match msg {
            ClientMsg::Input(input) => match desktop {
                Some(d) => {
                    if let Err(e) = d.inject(&input, feed.size()).await {
                        debug!("input {input:?} rejected: {e}");
                    }
                }
                None => info!("input: {input:?}"),
            },
            ClientMsg::RequestKeyframe => feed.force_keyframe(),
            ClientMsg::Ping { client_time_us } => {
                wire::write_message(send, &HostMsg::Pong { client_time_us }).await?
            }
            ClientMsg::Resize { width, height } => {
                let _ = resizes.send((width, height));
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
    fn client_screens_take_whole_multiples_of_eight() {
        assert_eq!(fit_size(2184, 1968), (2184, 1968));
        assert_eq!(fit_size(1080, 2343), (1080, 2336));
        assert_eq!(fit_size(10, 99_999), (MIN_SIDE, MAX_SIDE));
    }

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
