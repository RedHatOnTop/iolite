//! Just enough of IBus to type Korean through ibus-hangul: whether it is the active engine, how
//! it is set up, and how to put it back in a known mode.
//!
//! IBus has no way to ask an engine for its mode, but re-creating the engine leaves it in its
//! initial mode, and from there the host tracks every toggle it sends.

use std::path::PathBuf;

use anyhow::{Context, Result};
use zbus::zvariant::{OwnedValue, Value};

const HANGUL_ENGINE: &str = "hangul";
/// Plain keyboard engine the hangul engine is swapped with to re-create it.
const LATIN_ENGINE: &str = "xkb:us::eng";
const SCHEMA: &str = "org.freedesktop.ibus.engine.hangul";
pub const XK_HANGUL: u32 = 0xff31;

#[zbus::proxy(
    interface = "org.freedesktop.IBus",
    default_service = "org.freedesktop.IBus",
    default_path = "/org/freedesktop/IBus",
    gen_blocking = false
)]
trait IBus {
    fn set_global_engine(&self, engine_name: &str) -> zbus::Result<()>;
    fn get_global_engine(&self) -> zbus::Result<OwnedValue>;
}

/// The key ibus-hangul switches between Hangul and Latin mode on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Keysym(u32),
    ShiftSpace,
}

pub struct Hangul {
    ibus: IBusProxy<'static>,
    pub toggle: Toggle,
    /// Mode the engine starts in after a reset.
    pub starts_in_hangul: bool,
}

impl Hangul {
    /// Connects when ibus-hangul is the active engine with the dubeolsik layout; otherwise
    /// Korean text has to be pasted.
    pub async fn connect() -> Option<Self> {
        match Self::try_connect().await {
            Ok(hangul) => hangul,
            Err(e) => {
                tracing::info!("no IBus, Korean text will be pasted: {e:#}");
                None
            }
        }
    }

    async fn try_connect() -> Result<Option<Self>> {
        let address = address()?;
        let bus = zbus::connection::Builder::address(address.as_str())?
            .build()
            .await
            .context("connecting to IBus")?;
        let ibus = IBusProxy::new(&bus).await?;
        let desc = ibus.get_global_engine().await?;
        let engine = engine_name(&desc);
        if engine.as_deref() != Some(HANGUL_ENGINE) {
            tracing::info!("IBus engine is {engine:?}, not hangul; Korean text will be pasted");
            return Ok(None);
        }
        let (layout, switch_keys, initial) = tokio::task::spawn_blocking(|| {
            (
                gsetting("hangul-keyboard"),
                gsetting("switch-keys"),
                gsetting("initial-input-mode"),
            )
        })
        .await?;
        if layout.as_deref().unwrap_or("2") != "2" {
            tracing::info!(
                "ibus-hangul uses layout {layout:?}, not dubeolsik; Korean text will be pasted"
            );
            return Ok(None);
        }
        let switch_keys = switch_keys.unwrap_or_else(|| "Hangul,Shift+space".into());
        let Some(toggle) = toggle_from(&switch_keys) else {
            tracing::info!("ibus-hangul switches on {switch_keys:?}, which the host cannot press");
            return Ok(None);
        };
        tracing::info!("typing Korean through ibus-hangul");
        Ok(Some(Self {
            ibus,
            toggle,
            starts_in_hangul: initial.as_deref() == Some("hangul"),
        }))
    }

    /// Re-creates the engine, which leaves it in its initial mode.
    pub async fn reset(&self) -> zbus::Result<()> {
        self.ibus.set_global_engine(LATIN_ENGINE).await?;
        self.ibus.set_global_engine(HANGUL_ENGINE).await
    }
}

/// The daemon's address from `$IBUS_ADDRESS` or the newest file IBus wrote for a display.
fn address() -> Result<String> {
    if let Ok(address) = std::env::var("IBUS_ADDRESS") {
        return Ok(address);
    }
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("no home directory")?
        .join("ibus/bus");
    let newest = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| {
            let e = e.ok()?;
            Some((e.metadata().ok()?.modified().ok()?, e.path()))
        })
        .max()
        .context("IBus is not running")?
        .1;
    std::fs::read_to_string(&newest)?
        .lines()
        .find_map(|l| l.strip_prefix("IBUS_ADDRESS="))
        .map(str::to_owned)
        .with_context(|| format!("{} has no address", newest.display()))
}

/// Name field of the IBusEngineDesc that GetGlobalEngine returns.
fn engine_name(desc: &Value<'_>) -> Option<String> {
    match desc {
        Value::Value(inner) => engine_name(inner),
        Value::Structure(s) => match s.fields().get(2)? {
            Value::Str(name) => Some(name.to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn gsetting(key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", SCHEMA, key])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let value = String::from_utf8(out.stdout).ok()?;
    Some(value.trim().trim_matches('\'').to_owned())
}

fn toggle_from(switch_keys: &str) -> Option<Toggle> {
    let keys: Vec<&str> = switch_keys.split(',').map(str::trim).collect();
    if keys.contains(&"Hangul") {
        Some(Toggle::Keysym(XK_HANGUL))
    } else if keys.contains(&"Shift+space") {
        Some(Toggle::ShiftSpace)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{StructureBuilder, Value};

    #[test]
    fn reads_engine_name() {
        let desc = StructureBuilder::new()
            .add_field("IBusEngineDesc")
            .add_field(std::collections::HashMap::<String, Value>::new())
            .add_field("hangul")
            .add_field("Hangul")
            .build()
            .unwrap();
        let desc = Value::from(desc);
        assert_eq!(engine_name(&desc).as_deref(), Some("hangul"));
        assert_eq!(
            engine_name(&Value::Value(Box::new(desc))).as_deref(),
            Some("hangul")
        );
    }

    #[test]
    fn picks_a_toggle_the_host_can_press() {
        assert_eq!(
            toggle_from("Hangul,Shift+space,Alt_R"),
            Some(Toggle::Keysym(XK_HANGUL))
        );
        assert_eq!(toggle_from("Shift+space"), Some(Toggle::ShiftSpace));
        assert_eq!(toggle_from("Alt_R"), None);
    }
}
