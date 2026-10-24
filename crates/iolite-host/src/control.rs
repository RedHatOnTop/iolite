//! Line protocol on a Unix socket so `iolite approve` can reach the running host.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::warn;

use crate::config;
use crate::pairing::{spaced, Pairing};

pub async fn listen(pairing: Arc<Pairing>) {
    let path = config::control_socket();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            warn!("control socket {}: {e}", path.display());
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let pairing = pairing.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(stream, &pairing).await {
                warn!("control client: {e}");
            }
        });
    }
}

async fn serve(stream: UnixStream, pairing: &Pairing) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let words: Vec<&str> = line.split_whitespace().collect();
        let reply = match words.as_slice() {
            [verb @ ("approve" | "decline"), code @ ..] => {
                let code: String = code.concat();
                if pairing.decide(&code, *verb == "approve") {
                    format!("{verb}d {}", spaced(&code))
                } else {
                    format!("no pairing request with code {}", spaced(&code))
                }
            }
            ["pending"] => {
                let pending = pairing.list();
                if pending.is_empty() {
                    "no pairing requests".to_owned()
                } else {
                    pending
                        .iter()
                        .map(|(code, device)| format!("{}  {device}", spaced(code)))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
            _ => "unknown command".to_owned(),
        };
        write.write_all(format!("{reply}\n\n").as_bytes()).await?;
    }
    Ok(())
}

/// Sends one command to the running host and returns its reply.
pub async fn command(line: &str) -> Result<String> {
    let path = config::control_socket();
    let stream = UnixStream::connect(&path).await.with_context(|| {
        format!(
            "no running host at {} (start it with `iolite serve`)",
            path.display()
        )
    })?;
    let (read, mut write) = stream.into_split();
    write.write_all(format!("{line}\n").as_bytes()).await?;
    let mut lines = BufReader::new(read).lines();
    let mut reply = Vec::new();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            break;
        }
        reply.push(line);
    }
    Ok(reply.join("\n"))
}
