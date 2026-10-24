//! Mutter's private screen cast and remote desktop D-Bus APIs.
//!
//! These are what gnome-remote-desktop uses: they record the live session without a portal
//! dialog and inject input as virtual devices. Mutter ends a session when the bus connection
//! that created it goes away, so a crashed host cannot leave one behind.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iolite_proto::{keysym_for_char, Input, MouseButton, TouchPhase};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

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
const XK_SPACE: u32 = 0x20;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;

pub struct DesktopSession {
    rd: RemoteDesktopSessionProxy<'static>,
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
        Ok(Self {
            rd,
            stream: stream_path.as_str().to_owned(),
            node_id,
            connector,
        })
    }

    pub async fn stop(&self) {
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
                for c in text.chars() {
                    if c.is_ascii() {
                        self.tap(keysym_for_char(c)).await?;
                    } else {
                        self.type_unicode(c).await?;
                    }
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
    /// non-ASCII text. IBus (and GTK without an input method) accept Ctrl+Shift+U, the code
    /// point in hex and a space instead.
    async fn type_unicode(&self, c: char) -> zbus::Result<()> {
        self.rd.notify_keyboard_keysym(XK_CONTROL_L, true).await?;
        self.rd.notify_keyboard_keysym(XK_SHIFT_L, true).await?;
        self.tap(u32::from('u')).await?;
        self.rd.notify_keyboard_keysym(XK_SHIFT_L, false).await?;
        self.rd.notify_keyboard_keysym(XK_CONTROL_L, false).await?;
        for digit in format!("{:x}", u32::from(c)).chars() {
            self.tap(u32::from(digit)).await?;
        }
        self.tap(XK_SPACE).await
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
