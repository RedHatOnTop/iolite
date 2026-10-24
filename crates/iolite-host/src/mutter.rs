//! Mutter's private screen cast and remote desktop D-Bus APIs.
//!
//! These are what gnome-remote-desktop uses: they record the live session without a portal
//! dialog and inject input as virtual devices. Mutter ends a session when the bus connection
//! that created it goes away, so a crashed host cannot leave one behind.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iolite_proto::{keysym_for_char, Input, MouseButton, TouchPhase};
use tokio::sync::Notify;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

use crate::hangul;
use crate::ibus::{Hangul, Toggle, XK_HANGUL};

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
const XK_BACKSPACE: u32 = 0xff08;
const XK_ESCAPE: u32 = 0xff1b;
const XK_SPACE: u32 = 0x20;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;
const XK_ALT_R: u32 = 0xffea;
const XK_INSERT: u32 = 0xff63;
/// IBus applies a new engine after SetGlobalEngine returns.
const ENGINE_SETTLE: Duration = Duration::from_millis(200);
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
    /// Transfers answered so far.
    transfers: AtomicU64,
    served: Notify,
}

impl Clipboard {
    /// Waits until more than `seen` transfers have been answered.
    async fn transfer_after(&self, seen: u64, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let served = self.served.notified();
            tokio::pin!(served);
            served.as_mut().enable();
            if self.transfers.load(Ordering::Acquire) > seen {
                return true;
            }
            if tokio::time::timeout_at(deadline, served).await.is_err() {
                return false;
            }
        }
    }
}

/// The phone's input method text as the computer's ibus-hangul holds it.
#[derive(Default)]
struct Composer {
    /// Dubeolsik keys typed for the phone's composing text; ibus-hangul holds them as preedit.
    preedit: String,
    /// ibus-hangul's mode as the host last set it, or None before the first reset and after a
    /// key that may have switched it.
    hangul_mode: Option<bool>,
    shift: bool,
}

impl Composer {
    fn track_key(&mut self, keysym: u32, down: bool) {
        match keysym {
            XK_SHIFT_L | XK_SHIFT_R => self.shift = down,
            XK_HANGUL | XK_ALT_R | XK_ESCAPE if down => self.hangul_mode = None,
            XK_SPACE if down && self.shift => self.hangul_mode = None,
            _ => {}
        }
    }
}

pub struct DesktopSession {
    rd: RemoteDesktopSessionProxy<'static>,
    clipboard: Arc<Clipboard>,
    transfers: tokio::task::JoinHandle<()>,
    hangul: Option<Hangul>,
    composer: tokio::sync::Mutex<Composer>,
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
        if let Err(e) = warm_up_keyboard(&rd).await {
            tracing::debug!("warming up the keyboard: {e}");
        }
        let clipboard = Arc::new(Clipboard::default());
        let transfers = tokio::spawn(serve_clipboard(rd.clone(), clipboard.clone()));
        if let Err(e) = rd.enable_clipboard(HashMap::new()).await {
            tracing::warn!("clipboard unavailable, non-ASCII text will not type: {e}");
        }
        let hangul = Hangul::connect().await;
        Ok(Self {
            rd,
            clipboard,
            transfers,
            hangul,
            composer: Default::default(),
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
        if let Input::Touch {
            phase: TouchPhase::Down,
            ..
        }
        | Input::Button { down: true, .. }
        | Input::Key { .. } = input
        {
            let mut composer = self.composer.lock().await;
            self.finish_preedit(&mut composer).await?;
            if let Input::Key { keysym, down } = input {
                composer.track_key(*keysym, *down);
            }
        }
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
            Input::Compose { text } => self.compose(text).await,
            Input::Text { text } => self.commit(text).await,
        }
    }
}

/// Korean text is typed as dubeolsik keys into ibus-hangul, so it composes in the focused
/// application, terminals included, just as from a keyboard on the computer. The phone's
/// composing text is mirrored key by key: ibus-hangul's backspace removes one key from its
/// preedit, and toggling the mode twice commits the preedit and leaves the mode as it was.
impl DesktopSession {
    async fn compose(&self, text: &str) -> zbus::Result<()> {
        let Some(ime) = &self.hangul else {
            return Ok(());
        };
        let mut composer = self.composer.lock().await;
        let mut chars = text.chars();
        // Composing text longer than a syllable is typed when it is committed.
        let target = match (chars.next(), chars.next()) {
            (Some(c), None) => hangul::keys(c).unwrap_or_default(),
            _ => String::new(),
        };
        if !target.is_empty() {
            self.set_mode(&mut composer, ime, true).await?;
        }
        let kept = composer
            .preedit
            .bytes()
            .zip(target.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        for _ in kept..composer.preedit.len() {
            self.tap(XK_BACKSPACE).await?;
        }
        for k in target[kept..].chars() {
            self.tap(keysym_for_char(k)).await?;
        }
        composer.preedit = target;
        Ok(())
    }

    async fn commit(&self, text: &str) -> zbus::Result<()> {
        let mut composer = self.composer.lock().await;
        let mut rest = text;
        if !composer.preedit.is_empty() {
            let first = text.chars().next();
            if first.and_then(hangul::keys).as_deref() == Some(composer.preedit.as_str()) {
                self.finish_preedit(&mut composer).await?;
                rest = &text[first.map_or(0, char::len_utf8)..];
            } else {
                for _ in 0..composer.preedit.len() {
                    self.tap(XK_BACKSPACE).await?;
                }
                composer.preedit.clear();
            }
        }
        self.type_text(&mut composer, rest).await
    }

    async fn type_text(&self, composer: &mut Composer, text: &str) -> zbus::Result<()> {
        let mut unmapped = String::new();
        for c in text.chars() {
            let keys = self.hangul.as_ref().and(hangul::keys(c));
            if !c.is_ascii() && keys.is_none() {
                unmapped.push(c);
                continue;
            }
            if !unmapped.is_empty() {
                self.paste(&unmapped).await?;
                unmapped.clear();
            }
            match (&self.hangul, keys) {
                (Some(ime), Some(keys)) => {
                    self.set_mode(composer, ime, true).await?;
                    for k in keys.chars() {
                        self.tap(keysym_for_char(k)).await?;
                    }
                    self.toggle(ime.toggle).await?;
                    self.toggle(ime.toggle).await?;
                }
                (ime, _) => {
                    // Hangul mode passes digits, punctuation and space through unchanged.
                    if let (Some(ime), true) = (ime, c.is_ascii_alphabetic()) {
                        self.set_mode(composer, ime, false).await?;
                    }
                    self.tap(keysym_for_char(c)).await?;
                }
            }
        }
        if !unmapped.is_empty() {
            self.paste(&unmapped).await?;
        }
        Ok(())
    }

    async fn finish_preedit(&self, composer: &mut Composer) -> zbus::Result<()> {
        if let (Some(ime), false) = (&self.hangul, composer.preedit.is_empty()) {
            self.toggle(ime.toggle).await?;
            self.toggle(ime.toggle).await?;
        }
        composer.preedit.clear();
        Ok(())
    }

    /// Every switch starts from a reset rather than from the tracked mode: one toggle the
    /// input method missed would otherwise garble everything typed after it.
    async fn set_mode(&self, composer: &mut Composer, ime: &Hangul, on: bool) -> zbus::Result<()> {
        if composer.hangul_mode == Some(on) {
            return Ok(());
        }
        ime.reset().await?;
        tokio::time::sleep(ENGINE_SETTLE).await;
        if ime.starts_in_hangul != on {
            self.toggle(ime.toggle).await?;
        }
        composer.hangul_mode = Some(on);
        Ok(())
    }

    async fn toggle(&self, toggle: Toggle) -> zbus::Result<()> {
        match toggle {
            Toggle::Keysym(keysym) => self.tap(keysym).await,
            Toggle::ShiftSpace => {
                self.rd.notify_keyboard_keysym(XK_SHIFT_L, true).await?;
                self.tap(XK_SPACE).await?;
                self.rd.notify_keyboard_keysym(XK_SHIFT_L, false).await
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
    ///
    /// Mutter's clipboard manager copies every new selection as soon as it is set, so the first
    /// transfer after SetSelection is that copy; the application's paste is the one after the key.
    async fn paste(&self, text: &str) -> zbus::Result<()> {
        let clipboard = &self.clipboard;
        *clipboard.text.lock().unwrap() = text.to_owned();
        let seen = clipboard.transfers.load(Ordering::Acquire);
        self.rd
            .set_selection(HashMap::from([(
                "mime-types",
                Value::from(TEXT_MIME_TYPES.to_vec()),
            )]))
            .await?;
        clipboard
            .transfer_after(seen, Duration::from_millis(150))
            .await;
        let seen = clipboard.transfers.load(Ordering::Acquire);
        self.rd.notify_keyboard_keysym(XK_SHIFT_L, true).await?;
        self.tap(XK_INSERT).await?;
        self.rd.notify_keyboard_keysym(XK_SHIFT_L, false).await?;
        if clipboard
            .transfer_after(seen, Duration::from_millis(500))
            .await
        {
            // The application inserts the text only after reading it asynchronously; a key
            // sent right away would land in front of it.
            tokio::time::sleep(Duration::from_millis(60)).await;
        } else {
            tracing::debug!("no application asked for the pasted text");
        }
        Ok(())
    }
}

/// Mutter loses the first key events of a new session; these Shift taps absorb them.
async fn warm_up_keyboard(rd: &RemoteDesktopSessionProxy<'_>) -> zbus::Result<()> {
    for pause in [100, 50] {
        rd.notify_keyboard_keysym(XK_SHIFT_L, true).await?;
        rd.notify_keyboard_keysym(XK_SHIFT_L, false).await?;
        tokio::time::sleep(Duration::from_millis(pause)).await;
    }
    Ok(())
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
        clipboard.transfers.fetch_add(1, Ordering::AcqRel);
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
