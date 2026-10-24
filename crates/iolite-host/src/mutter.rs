//! Mutter's private screen cast and remote desktop D-Bus APIs.
//!
//! These are what gnome-remote-desktop uses: they record the live session without a portal
//! dialog and inject input as virtual devices. Mutter ends a session when the bus connection
//! that created it goes away, so a crashed host cannot leave one behind.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iolite_proto::{keysym_for_char, Input, MouseButton, TouchPhase};
use tokio::sync::Notify;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    default_path = "/org/gnome/Mutter/RemoteDesktop",
    gen_blocking = false
)]
trait RemoteDesktop {
    fn create_session(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop.Session",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    gen_blocking = false
)]
trait RemoteDesktopSession {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn notify_keyboard_keysym(&self, keysym: u32, state: bool) -> zbus::Result<()>;
    fn notify_pointer_button(&self, button: i32, state: bool) -> zbus::Result<()>;
    fn notify_pointer_axis(&self, dx: f64, dy: f64, flags: u32) -> zbus::Result<()>;
    fn notify_pointer_motion_relative(&self, dx: f64, dy: f64) -> zbus::Result<()>;
    fn notify_pointer_motion_absolute(&self, stream: &str, x: f64, y: f64) -> zbus::Result<()>;
    fn notify_touch_down(&self, stream: &str, slot: u32, x: f64, y: f64) -> zbus::Result<()>;
    fn notify_touch_motion(&self, stream: &str, slot: u32, x: f64, y: f64) -> zbus::Result<()>;
    fn notify_touch_up(&self, slot: u32) -> zbus::Result<()>;
    fn enable_clipboard(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn set_selection(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn selection_write(&self, serial: u32) -> zbus::Result<OwnedFd>;
    fn selection_write_done(&self, serial: u32, success: bool) -> zbus::Result<()>;
    #[zbus(signal)]
    fn selection_transfer(&self, mime_type: &str, serial: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn closed(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn session_id(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast",
    default_service = "org.gnome.Mutter.ScreenCast",
    default_path = "/org/gnome/Mutter/ScreenCast",
    gen_blocking = false
)]
trait ScreenCast {
    fn create_session(&self, properties: HashMap<&str, Value<'_>>)
        -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast.Session",
    default_service = "org.gnome.Mutter.ScreenCast",
    gen_blocking = false
)]
trait ScreenCastSession {
    fn record_monitor(
        &self,
        connector: &str,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast.Stream",
    default_service = "org.gnome.Mutter.ScreenCast",
    gen_blocking = false
)]
trait ScreenCastStream {
    #[zbus(signal, name = "PipeWireStreamAdded")]
    fn pipewire_stream_added(&self, node_id: u32) -> zbus::Result<()>;
}

type MonitorSpec = (String, String, String, String);
type Mode = (
    String,
    i32,
    i32,
    f64,
    f64,
    Vec<f64>,
    HashMap<String, OwnedValue>,
);
type Monitor = (MonitorSpec, Vec<Mode>, HashMap<String, OwnedValue>);
type LogicalMonitor = (
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<MonitorSpec>,
    HashMap<String, OwnedValue>,
);
type CurrentState = (
    u32,
    Vec<Monitor>,
    Vec<LogicalMonitor>,
    HashMap<String, OwnedValue>,
);

#[zbus::proxy(
    interface = "org.gnome.Mutter.DisplayConfig",
    default_service = "org.gnome.Mutter.DisplayConfig",
    default_path = "/org/gnome/Mutter/DisplayConfig",
    gen_blocking = false
)]
trait DisplayConfig {
    fn get_current_state(&self) -> zbus::Result<CurrentState>;
}

const CURSOR_EMBEDDED: u32 = 1;
const BTN_LEFT: i32 = 0x110;
const BTN_RIGHT: i32 = 0x111;
const BTN_MIDDLE: i32 = 0x112;
const AXIS_FINISH: u32 = 1 << 0;
const AXIS_SOURCE_FINGER: u32 = 1 << 2;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_INSERT: u32 = 0xff63;
const TEXT_MIME_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

/// Text offered on the clipboard for the next paste.
#[derive(Default)]
struct Clipboard {
    text: Mutex<String>,
    served: Notify,
}

pub struct DesktopSession {
    rd: RemoteDesktopSessionProxy<'static>,
    clipboard: Arc<Clipboard>,
    transfers: tokio::task::JoinHandle<()>,
    /// Object path of the screen cast stream; absolute input positions are relative to it.
    stream: String,
    pub node_id: u32,
    pub connector: String,
}

impl DesktopSession {
    pub async fn start(bus: &zbus::Connection, monitor: Option<&str>) -> Result<Self> {
        let connector = match monitor {
            Some(m) => m.to_owned(),
            None => primary_connector(bus).await?,
        };
        let rd_path = RemoteDesktopProxy::new(bus)
            .await?
            .create_session()
            .await
            .context("creating a Mutter remote desktop session")?;
        let rd = RemoteDesktopSessionProxy::builder(bus)
            .path(rd_path)?
            .build()
            .await?;
        let session_id = rd.session_id().await?;
        let sc_path = ScreenCastProxy::new(bus)
            .await?
            .create_session(HashMap::from([(
                "remote-desktop-session-id",
                Value::from(session_id.as_str()),
            )]))
            .await
            .context("creating a Mutter screen cast session")?;
        let sc = ScreenCastSessionProxy::builder(bus)
            .path(sc_path)?
            .build()
            .await?;
        let stream_path = sc
            .record_monitor(
                &connector,
                HashMap::from([("cursor-mode", Value::from(CURSOR_EMBEDDED))]),
            )
            .await
            .with_context(|| format!("recording monitor {connector}"))?;
        let stream = ScreenCastStreamProxy::builder(bus)
            .path(stream_path.clone())?
            .build()
            .await?;
        let mut added = stream.receive_pipewire_stream_added().await?;
        rd.start()
            .await
            .context("starting the remote desktop session")?;
        let signal = tokio::time::timeout(Duration::from_secs(5), added.next())
            .await
            .context("Mutter did not open a PipeWire stream")?
            .context("PipeWire stream signal ended")?;
        let node_id = *signal.args()?.node_id();
        let clipboard = Arc::new(Clipboard::default());
        let transfers = tokio::spawn(serve_clipboard(rd.clone(), clipboard.clone()));
        if let Err(e) = rd.enable_clipboard(HashMap::new()).await {
            tracing::warn!("clipboard unavailable, non-ASCII text will not type: {e}");
        }
        Ok(Self {
            rd,
            clipboard,
            transfers,
            stream: stream_path.as_str().to_owned(),
            node_id,
            connector,
        })
    }

    pub async fn stop(&self) {
        self.transfers.abort();
        if let Err(e) = self.rd.stop().await {
            tracing::debug!("stopping the Mutter session: {e}");
        }
    }

    /// Resolves when Mutter ends the session, e.g. when the user stops sharing from the panel.
    pub async fn closed(&self) {
        if let Ok(mut closed) = self.rd.receive_closed().await {
            closed.next().await;
        } else {
            std::future::pending::<()>().await;
        }
    }

    /// `frame` is the video frame size; Mutter maps stream pixels to the monitor.
    pub async fn inject(&self, input: &Input, frame: (u32, u32)) -> zbus::Result<()> {
        let (w, h) = (f64::from(frame.0), f64::from(frame.1));
        let pos = |x: f32, y: f32| {
            (
                f64::from(x).clamp(0.0, 1.0) * w,
                f64::from(y).clamp(0.0, 1.0) * h,
            )
        };
        match input {
            Input::Touch { phase, slot, x, y } => {
                let (x, y) = pos(*x, *y);
                match phase {
                    TouchPhase::Down => self.rd.notify_touch_down(&self.stream, *slot, x, y).await,
                    TouchPhase::Move => {
                        self.rd.notify_touch_motion(&self.stream, *slot, x, y).await
                    }
                    TouchPhase::Up | TouchPhase::Cancel => self.rd.notify_touch_up(*slot).await,
                }
            }
            Input::PointerMove { x, y } => {
                let (x, y) = pos(*x, *y);
                self.rd
                    .notify_pointer_motion_absolute(&self.stream, x, y)
                    .await
            }
            Input::PointerRelative { dx, dy } => {
                self.rd
                    .notify_pointer_motion_relative(f64::from(*dx), f64::from(*dy))
                    .await
            }
            Input::Button { button, down } => {
                let code = match button {
                    MouseButton::Left => BTN_LEFT,
                    MouseButton::Middle => BTN_MIDDLE,
                    MouseButton::Right => BTN_RIGHT,
                };
                self.rd.notify_pointer_button(code, *down).await
            }
            Input::Scroll { dx, dy, finish } => {
                let flags = AXIS_SOURCE_FINGER | if *finish { AXIS_FINISH } else { 0 };
                self.rd
                    .notify_pointer_axis(f64::from(*dx), f64::from(*dy), flags)
                    .await
            }
            Input::Key { keysym, down } => self.rd.notify_keyboard_keysym(*keysym, *down).await,
            Input::Text { text } => {
                let mut run = String::new();
                for c in text.chars() {
                    if c.is_ascii() {
                        if !run.is_empty() {
                            self.paste(&std::mem::take(&mut run)).await?;
                        }
                        self.tap(keysym_for_char(c)).await?;
                    } else {
                        run.push(c);
                    }
                }
                if !run.is_empty() {
                    self.paste(&run).await?;
                }
                Ok(())
            }
        }
    }
}

impl DesktopSession {
    async fn tap(&self, keysym: u32) -> zbus::Result<()> {
        self.rd.notify_keyboard_keysym(keysym, true).await?;
        self.rd.notify_keyboard_keysym(keysym, false).await
    }

    /// Mutter can only type keysyms the current keymap has, which leaves out Hangul and most
    /// other non-ASCII text, so such text goes through the clipboard. Shift+Insert pastes in
    /// GTK, Chromium and Electron apps and in xterm.js terminals; VTE terminals paste the
    /// primary selection on it instead.
    async fn paste(&self, text: &str) -> zbus::Result<()> {
        *self.clipboard.text.lock().unwrap() = text.to_owned();
        let served = self.clipboard.served.notified();
        tokio::pin!(served);
        served.as_mut().enable();
        self.rd
            .set_selection(HashMap::from([(
                "mime-types",
                Value::from(TEXT_MIME_TYPES.to_vec()),
            )]))
            .await?;
        self.rd.notify_keyboard_keysym(XK_SHIFT_L, true).await?;
        self.tap(XK_INSERT).await?;
        self.rd.notify_keyboard_keysym(XK_SHIFT_L, false).await?;
        if tokio::time::timeout(Duration::from_millis(500), served)
            .await
            .is_err()
        {
            tracing::debug!("no application asked for the pasted text");
        }
        Ok(())
    }
}

/// Answers paste requests with the text last put on the clipboard.
async fn serve_clipboard(rd: RemoteDesktopSessionProxy<'static>, clipboard: Arc<Clipboard>) {
    let mut transfers = match rd.receive_selection_transfer().await {
        Ok(transfers) => transfers,
        Err(e) => {
            tracing::warn!("cannot follow clipboard requests: {e}");
            return;
        }
    };
    while let Some(signal) = transfers.next().await {
        let Ok(args) = signal.args() else { continue };
        let serial = *args.serial();
        let text = clipboard.text.lock().unwrap().clone();
        let written = match rd.selection_write(serial).await {
            Ok(fd) => std::fs::File::from(std::os::fd::OwnedFd::from(fd))
                .write_all(text.as_bytes())
                .is_ok(),
            Err(e) => {
                tracing::debug!("clipboard write {serial}: {e}");
                false
            }
        };
        let _ = rd.selection_write_done(serial, written).await;
        clipboard.served.notify_waiters();
    }
}

async fn primary_connector(bus: &zbus::Connection) -> Result<String> {
    let (_, _, logical, _) = DisplayConfigProxy::new(bus)
        .await?
        .get_current_state()
        .await
        .context("reading the monitor layout")?;
    logical
        .iter()
        .find(|l| l.4)
        .or_else(|| logical.first())
        .and_then(|l| l.5.first())
        .map(|spec| spec.0.clone())
        .context("no monitor is on")
}
