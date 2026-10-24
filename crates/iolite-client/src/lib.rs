//! Client side of the iolite protocol.
//!
//! [`run`] drives one session: it connects, pairs if needed, and turns everything the host sends
//! into [`Event`]s. Commands for the host go in through an unbounded channel; dropping its sender
//! ends the session. The last event is always [`Event::Closed`].

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use iolite_proto::identity::{self, Fingerprint, Identity};
use iolite_proto::wire::{self, StreamKind, FLAG_KEYFRAME};
use iolite_proto::{quic, ClientMsg, Hello, HostMsg, Welcome, SERVER_NAME};
use quinn::crypto::rustls::QuicClientConfig;
use tokio::sync::mpsc;

pub use iolite_proto;

pub struct Config {
    pub addr: SocketAddr,
    pub identity: Identity,
    /// The host fingerprint saved after the last successful session, if any.
    pub pinned_host: Option<Fingerprint>,
    pub hello: Hello,
}

pub struct Packet {
    pub data: Vec<u8>,
    pub pts_us: u64,
    pub keyframe: bool,
}

impl fmt::Debug for Packet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packet")
            .field("len", &self.data.len())
            .field("pts_us", &self.pts_us)
            .field("keyframe", &self.keyframe)
            .finish()
    }
}

#[derive(Debug)]
pub enum Event {
    /// The host does not know this client yet. Show the code; the host user compares it with
    /// the one on their screen and accepts or declines.
    PairingCode(String),
    /// Streaming starts. Save `host` and pass it as `pinned_host` next time.
    Welcome {
        host: Fingerprint,
        welcome: Welcome,
    },
    Video(Packet),
    Audio(Packet),
    Closed(String),
}

pub async fn run(
    cfg: Config,
    mut commands: mpsc::UnboundedReceiver<ClientMsg>,
    events: mpsc::Sender<Event>,
) {
    let reason = match session(cfg, &mut commands, &events).await {
        Ok(reason) => reason,
        Err(e) => format!("{e:#}"),
    };
    log::info!("session closed: {reason}");
    let _ = events.send(Event::Closed(reason)).await;
}

async fn session(
    cfg: Config,
    commands: &mut mpsc::UnboundedReceiver<ClientMsg>,
    events: &mpsc::Sender<Event>,
) -> Result<String> {
    let crypto = identity::client_config(&cfg.identity, cfg.pinned_host)?;
    let mut client_cfg = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto)?));
    client_cfg.transport_config(Arc::new(quic::transport()));
    let bind: SocketAddr = if cfg.addr.is_ipv6() {
        "[::]:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(client_cfg);

    let conn = tokio::time::timeout(
        Duration::from_secs(10),
        endpoint.connect(cfg.addr, SERVER_NAME)?,
    )
    .await
    .map_err(|_| anyhow!("no answer from {}", cfg.addr))?
    .map_err(|e| match e {
        quinn::ConnectionError::TransportError(ref t)
            if cfg.pinned_host.is_some() && t.to_string().contains("certificate") =>
        {
            anyhow!("the host identity changed since this device paired with it")
        }
        e => anyhow!(e).context(format!("connecting to {}", cfg.addr)),
    })?;
    let host = Fingerprint::of_peer(conn.peer_identity()).context("host sent no certificate")?;
    let client = cfg.identity.fingerprint();
    log::info!("connected to {} as {client:?}, host {host:?}", cfg.addr);

    let (mut send, mut recv) = conn.open_bi().await?;
    wire::write_message(&mut send, &ClientMsg::Hello(cfg.hello)).await?;

    let welcome = loop {
        match wire::read_message::<HostMsg, _>(&mut recv).await? {
            Some(HostMsg::PairingRequired) => {
                events
                    .send(Event::PairingCode(identity::pairing_code(&host, &client)))
                    .await?;
            }
            Some(HostMsg::Welcome(w)) => break w,
            Some(HostMsg::Rejected { reason }) | Some(HostMsg::Bye { reason }) => {
                return Ok(reason)
            }
            Some(HostMsg::Pong { .. }) => {}
            None => return Ok("host closed the connection".into()),
        }
    };
    events.send(Event::Welcome { host, welcome }).await?;

    let reason = tokio::select! {
        r = control_in(&mut recv) => r?,
        r = control_out(&mut send, commands) => r?,
        r = accept_media(conn.clone(), events.clone()) => r?,
    };
    conn.close(0u32.into(), reason.as_bytes());
    let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    Ok(reason)
}

async fn control_in(recv: &mut quinn::RecvStream) -> Result<String> {
    loop {
        match wire::read_message::<HostMsg, _>(recv).await? {
            Some(HostMsg::Bye { reason }) | Some(HostMsg::Rejected { reason }) => {
                return Ok(reason)
            }
            Some(HostMsg::Pong { .. }) => {}
            Some(other) => log::warn!("unexpected message mid-session: {other:?}"),
            None => return Ok("host closed the connection".into()),
        }
    }
}

async fn control_out(
    send: &mut quinn::SendStream,
    commands: &mut mpsc::UnboundedReceiver<ClientMsg>,
) -> Result<String> {
    while let Some(msg) = commands.recv().await {
        wire::write_message(send, &msg).await?;
    }
    let _ = send.finish();
    Ok("disconnected".into())
}

async fn accept_media(conn: quinn::Connection, events: mpsc::Sender<Event>) -> Result<String> {
    loop {
        let mut recv = match conn.accept_uni().await {
            Ok(recv) => recv,
            Err(quinn::ConnectionError::ApplicationClosed(close)) => {
                return Ok(String::from_utf8_lossy(&close.reason).into_owned())
            }
            Err(e) => return Err(e.into()),
        };
        let events = events.clone();
        tokio::spawn(async move {
            if let Err(e) = read_media(&mut recv, &events).await {
                log::debug!("media stream ended: {e:#}");
            }
        });
    }
}

async fn read_media(recv: &mut quinn::RecvStream, events: &mpsc::Sender<Event>) -> Result<()> {
    let kind = wire::read_stream_kind(recv).await?;
    while let Some((header, data)) = wire::read_packet(recv).await? {
        let packet = Packet {
            data,
            pts_us: header.pts_us,
            keyframe: header.flags & FLAG_KEYFRAME != 0,
        };
        let event = match kind {
            StreamKind::Video => Event::Video(packet),
            StreamKind::Audio => Event::Audio(packet),
        };
        if events.send(event).await.is_err() {
            bail!("event receiver dropped");
        }
    }
    Ok(())
}
