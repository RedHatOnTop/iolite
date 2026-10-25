//! Wire protocol shared by the iolite host and its clients.
//!
//! A session is one QUIC connection. The client opens a bidirectional control stream and sends
//! [`ClientMsg::Hello`]; the host answers with [`HostMsg`] values on the same stream. Media flows
//! on unidirectional streams the host opens after [`HostMsg::Welcome`]: the first byte names the
//! [`wire::StreamKind`], the rest is a sequence of [`wire::PacketHeader`]-prefixed packets.

pub mod identity;
pub mod quic;
pub mod wire;

use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"iolite/1";
pub const PROTOCOL_VERSION: u16 = 3;
pub const DEFAULT_PORT: u16 = 46100;
/// Name sent in the TLS handshake. Peers are identified by certificate fingerprint, not by name.
pub const SERVER_NAME: &str = "iolite";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello(Hello),
    Input(Input),
    RequestKeyframe,
    Ping {
        client_time_us: u64,
    },
    /// The client's display area changed size, e.g. a phone was rotated or unfolded. Only a
    /// screen made for the client ([`DisplayMode::Fit`], [`DisplayMode::Extend`]) follows it.
    Resize {
        width: u32,
        height: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u16,
    pub device_name: String,
    /// Client display size in physical pixels.
    pub display_width: u32,
    pub display_height: u32,
    pub audio: bool,
    pub display: DisplayMode,
    /// GNOME scale the client would like on a screen made for it; the host picks the nearest
    /// one the screen supports. 0 lets the host choose.
    pub scale: f32,
}

/// What the client shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayMode {
    /// The computer's own primary screen.
    #[default]
    Mirror,
    /// A screen of the client's size that is the only one on while the client is connected, as
    /// a remote desktop session gets its own screen on Windows. The computer's own screens come
    /// back when the session ends.
    Fit,
    /// A screen of the client's size next to the computer's own, like a second monitor.
    Extend,
}

/// The protocol version a client's first message claims, read without decoding the rest, so
/// the host can tell an older or newer client why they cannot talk.
pub fn hello_version(body: &[u8]) -> Option<u16> {
    let (variant, rest) = postcard::take_from_bytes::<u32>(body).ok()?;
    if variant != 0 {
        return None;
    }
    postcard::take_from_bytes::<u16>(rest).ok().map(|(v, _)| v)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMsg {
    /// The client is not paired. Both ends now show the pairing code derived from the two
    /// certificate fingerprints and the host user accepts or declines.
    PairingRequired,
    Rejected {
        reason: String,
    },
    Welcome(Welcome),
    Pong {
        client_time_us: u64,
    },
    Bye {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    pub host_name: String,
    pub width: u32,
    pub height: u32,
    pub codec: VideoCodec,
    pub audio: Option<AudioFormat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    /// Low-overhead OBU stream, one temporal unit per packet.
    Av1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u8,
    /// RFC 7845 identification header ("OpusHead"), passed to the decoder as codec config.
    pub opus_head: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Absolute positions are normalized to the video frame: (0, 0) is its top-left corner and
/// (1, 1) its bottom-right corner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Input {
    Touch {
        phase: TouchPhase,
        slot: u32,
        x: f32,
        y: f32,
    },
    PointerMove {
        x: f32,
        y: f32,
    },
    /// Relative motion in video frame pixels.
    PointerRelative {
        dx: f32,
        dy: f32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    /// Smooth scroll in video frame pixels; `finish` ends the gesture so kinetic scrolling starts.
    Scroll {
        dx: f32,
        dy: f32,
        finish: bool,
    },
    /// X11 keysym.
    Key {
        keysym: u32,
        down: bool,
    },
    /// The input method's composing text, sent each time it changes. The host shows it in the
    /// focused application as it would show its own input method's preedit.
    Compose {
        text: String,
    },
    /// Committed text. It replaces the composing text, if there is any.
    Text {
        text: String,
    },
}

/// X11 keysym for a character, as libxkbcommon's `xkb_utf32_to_keysym` maps it.
pub fn keysym_for_char(c: char) -> u32 {
    let cp = c as u32;
    match cp {
        0x0a | 0x0d => 0xff0d,
        0x08 => 0xff08,
        0x09 => 0xff09,
        0x1b => 0xff1b,
        0x7f => 0xffff,
        0x20..=0x7e | 0xa0..=0xff => cp,
        _ => 0x0100_0000 | cp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_version_survives_layout_changes() {
        let hello = ClientMsg::Hello(Hello {
            version: PROTOCOL_VERSION,
            device_name: "phone".into(),
            display_width: 1080,
            display_height: 2400,
            audio: true,
            display: DisplayMode::Fit,
            scale: 2.0,
        });
        let body = postcard::to_stdvec(&hello).unwrap();
        assert_eq!(hello_version(&body), Some(PROTOCOL_VERSION));
        // A version 2 hello ended after `audio`.
        let old = postcard::to_stdvec(&(0u32, 2u16, "phone", 1080u32, 2400u32, true)).unwrap();
        assert_eq!(hello_version(&old), Some(2));
        assert!(postcard::from_bytes::<ClientMsg>(&old).is_err());
        let input = postcard::to_stdvec(&ClientMsg::RequestKeyframe).unwrap();
        assert_eq!(hello_version(&input), None);
    }

    #[test]
    fn keysyms_follow_xkb() {
        assert_eq!(keysym_for_char('a'), 0x61);
        assert_eq!(keysym_for_char('\n'), 0xff0d);
        assert_eq!(keysym_for_char('é'), 0xe9);
        assert_eq!(keysym_for_char('가'), 0x0100_ac00);
    }
}
