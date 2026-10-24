//! Framing for control messages and media packets.

use std::io;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_MESSAGE: usize = 1 << 20;
pub const MAX_PACKET: usize = 16 << 20;

/// First byte of every unidirectional stream the host opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamKind {
    Video = 1,
    Audio = 2,
}

impl StreamKind {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::Video),
            2 => Some(Self::Audio),
            _ => None,
        }
    }
}

pub const FLAG_KEYFRAME: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub flags: u8,
    pub pts_us: u64,
    pub len: u32,
}

pub const PACKET_HEADER_LEN: usize = 13;

impl PacketHeader {
    pub fn encode(&self) -> [u8; PACKET_HEADER_LEN] {
        let mut b = [0u8; PACKET_HEADER_LEN];
        b[0] = self.flags;
        b[1..9].copy_from_slice(&self.pts_us.to_le_bytes());
        b[9..13].copy_from_slice(&self.len.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8; PACKET_HEADER_LEN]) -> Self {
        Self {
            flags: b[0],
            pts_us: u64::from_le_bytes(b[1..9].try_into().unwrap()),
            len: u32::from_le_bytes(b[9..13].try_into().unwrap()),
        }
    }
}

fn invalid(msg: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Fills `buf` completely. Returns `false` if the stream ended cleanly before the first byte.
async fn read_exact_or_eof<R: AsyncRead + Unpin>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = r.read(&mut buf[filled..]).await?;
        if n == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        filled += n;
    }
    Ok(true)
}

pub fn encode_message<T: Serialize>(msg: &T) -> io::Result<Vec<u8>> {
    let body = postcard::to_stdvec(msg).map_err(invalid)?;
    if body.len() > MAX_MESSAGE {
        return Err(invalid("message too large"));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub async fn write_message<T: Serialize, W: AsyncWrite + Unpin>(
    w: &mut W,
    msg: &T,
) -> io::Result<()> {
    w.write_all(&encode_message(msg)?).await
}

/// Reads one message, or `None` if the stream ended between messages.
pub async fn read_message<T: DeserializeOwned, R: AsyncRead + Unpin>(
    r: &mut R,
) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    if !read_exact_or_eof(r, &mut len).await? {
        return Ok(None);
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(invalid("message too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    postcard::from_bytes(&body).map(Some).map_err(invalid)
}

pub async fn write_packet<W: AsyncWrite + Unpin>(
    w: &mut W,
    flags: u8,
    pts_us: u64,
    data: &[u8],
) -> io::Result<()> {
    let header = PacketHeader {
        flags,
        pts_us,
        len: data.len() as u32,
    };
    w.write_all(&header.encode()).await?;
    w.write_all(data).await
}

/// Reads one packet, or `None` if the stream ended between packets.
pub async fn read_packet<R: AsyncRead + Unpin>(
    r: &mut R,
) -> io::Result<Option<(PacketHeader, Vec<u8>)>> {
    let mut raw = [0u8; PACKET_HEADER_LEN];
    if !read_exact_or_eof(r, &mut raw).await? {
        return Ok(None);
    }
    let header = PacketHeader::decode(&raw);
    if header.len as usize > MAX_PACKET {
        return Err(invalid("packet too large"));
    }
    let mut data = vec![0u8; header.len as usize];
    r.read_exact(&mut data).await?;
    Ok(Some((header, data)))
}

pub async fn read_stream_kind<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<StreamKind> {
    let b = r.read_u8().await?;
    StreamKind::from_byte(b).ok_or_else(|| invalid(format!("unknown stream kind {b}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientMsg, Input, TouchPhase};

    #[tokio::test]
    async fn messages_and_packets_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        let msg = ClientMsg::Input(Input::Touch {
            phase: TouchPhase::Down,
            slot: 3,
            x: 0.25,
            y: 0.75,
        });
        write_message(&mut a, &msg).await.unwrap();
        write_packet(&mut a, FLAG_KEYFRAME, 42, b"obu")
            .await
            .unwrap();
        drop(a);

        match read_message::<ClientMsg, _>(&mut b).await.unwrap() {
            Some(ClientMsg::Input(Input::Touch { slot: 3, x, .. })) => assert_eq!(x, 0.25),
            other => panic!("unexpected {other:?}"),
        }
        let (header, data) = read_packet(&mut b).await.unwrap().unwrap();
        assert_eq!(header.flags, FLAG_KEYFRAME);
        assert_eq!(header.pts_us, 42);
        assert_eq!(data, b"obu");
        assert!(read_packet(&mut b).await.unwrap().is_none());
    }
}
