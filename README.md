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

On the phone, tap the handle on the right edge for the keyboard, the input mode and disconnect.
Touch mode passes your fingers through as touches; trackpad mode moves the pointer (tap to
click, two fingers to scroll or right-click, long press to right-click, tap and drag to drag).

## Known limits

- Mutter can only type characters that exist in the computer's keyboard layout. Anything else
  (Hangul, accented letters) is put on the clipboard and pasted with Shift+Insert. That works in
  GTK apps, browsers and Electron apps, and in xterm.js terminals; GNOME Terminal and Ptyxis
  paste the primary selection on Shift+Insert instead. Pasting replaces the clipboard.
- Text typed from the phone goes through the computer's input method. If it is in Hangul mode,
  Latin letters get composed into Hangul; switch it to Latin first.
- One monitor per session: the primary one, or `iolite serve --monitor <connector>`.
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
input goes back on the control stream. One device streams at a time; a new connection takes
over.

`iolite probe <host>` is a headless client for testing: it pairs, streams for a few seconds,
can click and type, and writes the received AV1 to a file.

## Licence

GPL-3.0-or-later.
