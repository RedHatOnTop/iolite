# iolite

Remote desktop for a GNOME Wayland computer, driven from an Android phone.

**Alpha.** The host is tested on Fedora 44 with GNOME 50 on an Intel Core Ultra 7 155H; the
Android app is tested end to end on the emulator in CI. Expect rough edges and protocol changes
between versions.

[한국어 설명](README.ko.md)

## Why

GNOME's built-in remote desktop speaks RDP, and the Microsoft client on Android does not offer
H.264 to it at all, so every frame goes through a CPU codec. Fedora's Intel media driver ships
without H.264 encoding anyway. Audio gets switched off ("client cannot handle graphics and
audio simultaneously"), and over a phone connection the TCP transport stalls and drops.

iolite keeps the parts of GNOME that work and replaces the rest:

| | |
|---|---|
| Capture and input | Mutter's own screen cast and remote desktop D-Bus APIs (the ones gnome-remote-desktop uses), so it shares the session you are logged into, with no portal dialog |
| Video | PipeWire DMA-BUF → GL colour conversion → VA-API **AV1** hardware encoder, about 2 ms per 1920×1200 frame on Meteor Lake |
| Audio | whatever the default output plays, as Opus |
| Transport | QUIC over UDP: no head-of-line stalls on loss, keep-alives through NAT, adaptive bitrate |
| Client | Android app, hardware AV1 decoding through MediaCodec in low-latency mode |
| Trust | self-signed certificates on both ends, paired once by comparing a six-digit code |

## Host

Requirements: GNOME on Wayland (tested with Mutter 50), an Intel GPU whose VA-API driver
encodes AV1 (Arc, Meteor Lake or newer), and GStreamer 1.24+ with the `va`, `opengl`,
`pipewire`, `opus` and `videoparsers` plugins. On Fedora:

```sh
sudo dnf install gstreamer1-plugins-base gstreamer1-plugins-bad-free pipewire-gstreamer \
  libva-intel-media-driver
```

Build (or take `iolite` from the artifacts of the latest CI run):

```sh
sudo dnf install cargo gstreamer1-devel gstreamer1-plugins-base-devel
cargo build --release -p iolite-host
install -Dm755 target/release/iolite ~/.local/bin/iolite
```

Run it in your graphical session, by hand or as a user service:

```sh
iolite serve
# or
install -Dm644 dist/iolite.service ~/.config/systemd/user/iolite.service
systemctl --user enable --now iolite.service
```

It listens on UDP port 46100. Put both devices on the same [Tailscale](https://tailscale.com)
network (or LAN) rather than opening the port to the internet.

## Phone

Install `iolite.apk` from the latest CI run, enter the computer's Tailscale name or address and
connect. The first time, the phone shows a six-digit code and the computer shows a notification
with the same code: accept it there only if the codes match. If notifications are not visible,
`iolite pending` lists requests and `iolite approve 123456` accepts one.

`iolite clients` lists paired devices and `iolite clients --forget <fingerprint prefix>` removes
one.

Computers you connect to are saved on the start screen with the last time you used them; the
`⋯` button on each one renames it, changes its address, input mode or sound, forgets its pairing
or deletes it.

While streaming, the bar along the bottom holds the keys a phone keyboard lacks: Esc, Tab, Home,
End, Page Up/Down, the arrows, Backspace, Delete and Enter, with F1–F12, Print and Insert on a
second page. Ctrl, Alt, Shift and Super apply to the next key, click or shortcut when tapped and
stay held when long-pressed, so Ctrl+click and Super+drag work too; holding an arrow repeats it.
The shortcuts page has GNOME's and the usual application shortcuts (Activities, Alt+Tab,
Super+arrows, copy and paste, terminal copy and paste, browser tabs) and your own, which you add
with `+` and edit by long-pressing. The strip above the keys opens the keyboard, shows the
Activities overview, switches between touch and trackpad input, disconnects and hides the bar;
a three-finger tap also opens or closes the keyboard. The video shrinks to stay above the bar and
the keyboard.

Each computer also has a screen setting. **Own screen**, the default, asks the computer for a
desktop the size of the phone's screen, the way a Windows remote desktop session gets its own:
the computer's monitors stay off while the phone is connected and come back when it disconnects,
and the screen follows the phone when it is turned or unfolded or the key bar is hidden.
**Extend** adds the phone as a second monitor beside the computer's own. **Mirror** shows the
computer's primary monitor as it is. The `⋯` menu while streaming switches between them, which
reconnects.

In touch mode the finger is the pointer: tap to click where you tap, drag to drag, hold to
right-click. In trackpad mode the phone is a touchpad: slide to move the pointer, tap to click,
tap and drag to drag. In both, two fingers scroll, and a two-finger tap right-clicks.

## Korean input

When the computer's input method is ibus-hangul with the dubeolsik layout, Korean typed on the
phone is typed again as dubeolsik keys, so ibus-hangul composes it in the focused application
while you type, terminals included. iolite switches ibus-hangul between Hangul and Latin mode by
itself; it does so by re-creating the engine, so the mode indicator in the top bar may flicker.

## Known limits

- Mutter can only type characters that exist in the computer's keyboard layout. Anything else
  (accented letters, emoji, Korean without ibus-hangul) is put on the clipboard and pasted with
  Shift+Insert, which replaces the clipboard. That works in GTK, browser and Electron apps;
  GNOME Terminal and Ptyxis paste the primary selection on Shift+Insert instead.
- Mirror shows one monitor: the primary one, or `iolite serve --monitor <connector>`.
- When a screen made for the phone changes size, Mutter lays the monitors out afresh, so with
  Own screen the computer's monitors light up for a moment before iolite turns them off again.
- Encoding needs a VA-API AV1 encoder; there is no software fallback for the desktop yet.

## How it fits together

```
crates/iolite-proto    messages, framing, certificates, pairing codes
crates/iolite-client   client session logic (used by the app and by `iolite probe`)
crates/iolite-host     the `iolite` binary: Mutter sessions, GStreamer, QUIC server
crates/iolite-android  JNI bridge for the app
android/               the Android app
```

A session is one QUIC connection. The phone opens a control stream and says hello; the host
either asks for pairing or starts recording and answers with the frame size. Video (one AV1
temporal unit per packet) and audio (one Opus packet per packet) arrive on separate streams;
input goes back on the control stream. When the frame size changes, the first video packet of
the new size, a keyframe, carries it. One device streams at a time; a new connection takes
over.

`iolite probe <host>` is a headless client for testing: it pairs, streams for a few seconds,
can click and type, asks for any screen mode and size (`--display fit --size 1080x2400
--resize 2184x1968@3`), and writes the received AV1 to a file.

## Licence

GPL-3.0-or-later.
