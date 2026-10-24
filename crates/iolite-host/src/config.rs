use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use iolite_proto::identity::Fingerprint;
use serde::{Deserialize, Serialize};

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"));
    base.join("iolite")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn control_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("iolite.sock")
}

pub fn host_name() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_owned())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "computer".into())
}

/// Whether to talk to the person at the computer in Korean.
pub fn korean() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .is_some_and(|v| v.starts_with("ko"))
}

/// Paired devices, kept in `clients.toml` and re-read on every connection so edits apply at once.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Clients {
    #[serde(default, rename = "client")]
    pub list: Vec<ClientEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientEntry {
    pub name: String,
    pub fingerprint: String,
    pub paired_at: u64,
}

impl Clients {
    pub fn path() -> PathBuf {
        config_dir().join("clients.toml")
    }

    pub fn load() -> Result<Self> {
        match fs::read_to_string(Self::path()) {
            Ok(s) => toml::from_str(&s).with_context(|| format!("{}", Self::path().display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        fs::create_dir_all(config_dir())?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, toml::to_string_pretty(self)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn contains(&self, fp: &Fingerprint) -> bool {
        let hex = fp.to_hex();
        self.list.iter().any(|c| c.fingerprint == hex)
    }

    pub fn add(&mut self, name: &str, fp: &Fingerprint) {
        if self.contains(fp) {
            return;
        }
        let paired_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.list.push(ClientEntry {
            name: name.to_owned(),
            fingerprint: fp.to_hex(),
            paired_at,
        });
    }

    pub fn remove_prefix(&mut self, prefix: &str) -> Vec<ClientEntry> {
        let prefix = prefix.to_ascii_lowercase();
        let (gone, kept) = std::mem::take(&mut self.list)
            .into_iter()
            .partition(|c| c.fingerprint.starts_with(&prefix));
        self.list = kept;
        gone
    }
}
