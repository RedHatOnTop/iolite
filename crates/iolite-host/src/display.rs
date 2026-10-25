//! The monitor layout, through Mutter's DisplayConfig API, and where the screen Mutter adds for
//! a client that asked for one of its own goes in it.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use futures_util::{Stream, StreamExt};
use iolite_proto::DisplayMode;
use zbus::zvariant::{OwnedValue, Value};

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
type MonitorConfig = (String, String, HashMap<String, OwnedValue>);
type LogicalMonitorConfig = (i32, i32, f64, u32, bool, Vec<MonitorConfig>);

#[zbus::proxy(
    interface = "org.gnome.Mutter.DisplayConfig",
    default_service = "org.gnome.Mutter.DisplayConfig",
    default_path = "/org/gnome/Mutter/DisplayConfig",
    gen_blocking = false
)]
trait DisplayConfig {
    fn get_current_state(&self) -> zbus::Result<CurrentState>;
    fn apply_monitors_config(
        &self,
        serial: u32,
        method: u32,
        logical_monitors: Vec<LogicalMonitorConfig>,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    fn monitors_changed(&self) -> zbus::Result<()>;
}

/// A temporary config lasts until the monitors change and is never saved, so the computer's
/// own layout comes back when the client's screen goes away.
const METHOD_TEMPORARY: u32 = 1;
const LAYOUT_PHYSICAL: u32 = 2;
/// Mutter's product name for the monitors it adds for virtual screen casts.
const VIRTUAL_PRODUCT: &str = "Virtual remote monitor";

pub struct Monitors {
    proxy: DisplayConfigProxy<'static>,
}

impl Monitors {
    pub async fn new(bus: &zbus::Connection) -> Result<Self> {
        Ok(Self {
            proxy: DisplayConfigProxy::new(bus).await?,
        })
    }

    async fn layout(&self) -> Result<Layout> {
        let state = self
            .proxy
            .get_current_state()
            .await
            .context("reading the monitor layout")?;
        Ok(Layout::from_state(state))
    }

    pub async fn primary_connector(&self) -> Result<String> {
        let layout = self.layout().await?;
        layout
            .logical
            .iter()
            .find(|l| l.primary)
            .or_else(|| layout.logical.first())
            .and_then(|l| l.connectors.first().cloned())
            .context("no monitor is on")
    }

    /// One item per change Mutter reports; a single change often comes as several.
    pub async fn changes(&self) -> Result<impl Stream<Item = ()> + Unpin> {
        Ok(self.proxy.receive_monitors_changed().await?.map(|_| ()))
    }
}

/// Where the client's screen goes: alone, or to the right of the computer's own.
pub struct Placement {
    mode: DisplayMode,
    scale: f64,
    /// Connectors there before the client's screen was asked for; the client's is the new one.
    existing: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arranged {
    /// The client's screen is not there.
    Missing,
    /// A new layout went to Mutter, which reports it as another change.
    Applied,
    /// The layout already is the one wanted.
    Settled,
}

impl Placement {
    /// Call before recording the virtual screen.
    pub async fn new(monitors: &Monitors, mode: DisplayMode, scale: f32) -> Result<Self> {
        let layout = monitors.layout().await?;
        Ok(Self {
            mode,
            scale: f64::from(scale),
            existing: layout.connectors.into_iter().map(|(c, _)| c).collect(),
        })
    }

    pub async fn arrange(&self, monitors: &Monitors) -> Result<Arranged> {
        let layout = monitors.layout().await?;
        let Some(ours) = layout.find_new(&self.existing) else {
            return Ok(Arranged::Missing);
        };
        let wanted = layout
            .wanted(&ours, self.mode, self.scale)
            .context("the client's screen has no mode")?;
        if same(&layout.logical, &wanted) {
            return Ok(Arranged::Settled);
        }
        tracing::info!("arranging the screens: {wanted:?}");
        monitors
            .proxy
            .apply_monitors_config(
                layout.serial,
                METHOD_TEMPORARY,
                layout.config(&wanted),
                HashMap::new(),
            )
            .await
            .context("applying the monitor layout")?;
        Ok(Arranged::Applied)
    }
}

/// A monitor that is on, as far as a layout needs it.
#[derive(Debug, Clone)]
struct Output {
    connector: String,
    mode: String,
    width: i32,
    height: i32,
    preferred_scale: f64,
    scales: Vec<f64>,
}

#[derive(Debug, Clone)]
struct Logical {
    x: i32,
    y: i32,
    scale: f64,
    transform: u32,
    primary: bool,
    /// Sorted; more than one when monitors mirror each other.
    connectors: Vec<String>,
}

struct Layout {
    serial: u32,
    /// Monitor sizes are not divided by their scale in the layout.
    physical: bool,
    /// Every connected monitor with its product name.
    connectors: Vec<(String, String)>,
    outputs: Vec<Output>,
    logical: Vec<Logical>,
}

impl Layout {
    fn from_state((serial, monitors, logical, props): CurrentState) -> Self {
        let connectors = monitors
            .iter()
            .map(|((connector, _, product, _), _, _)| (connector.clone(), product.clone()))
            .collect();
        let outputs = monitors
            .into_iter()
            .filter_map(|((connector, ..), modes, _)| {
                let (mode, width, height, _, preferred_scale, scales, _) =
                    modes.into_iter().find(|m| flag(&m.6, "is-current"))?;
                Some(Output {
                    connector,
                    mode,
                    width,
                    height,
                    preferred_scale,
                    scales,
                })
            })
            .collect();
        let logical = logical
            .into_iter()
            .map(|(x, y, scale, transform, primary, specs, _)| {
                let mut connectors: Vec<String> = specs.into_iter().map(|s| s.0).collect();
                connectors.sort();
                Logical {
                    x,
                    y,
                    scale,
                    transform,
                    primary,
                    connectors,
                }
            })
            .collect();
        let physical =
            props.get("layout-mode").and_then(|v| u32::try_from(v).ok()) == Some(LAYOUT_PHYSICAL);
        Self {
            serial,
            physical,
            connectors,
            outputs,
            logical,
        }
    }

    fn output(&self, connector: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.connector == connector)
    }

    fn find_new(&self, existing: &HashSet<String>) -> Option<String> {
        let mut new = self
            .connectors
            .iter()
            .filter(|(c, _)| !existing.contains(c));
        let first = new.clone().next();
        new.find(|(_, product)| product == VIRTUAL_PRODUCT)
            .or(first)
            .map(|(c, _)| c.clone())
    }

    /// The area a logical monitor covers in layout coordinates.
    fn extent(&self, logical: &Logical) -> (i32, i32) {
        let Some(out) = logical.connectors.first().and_then(|c| self.output(c)) else {
            return (0, 0);
        };
        let (w, h) = if logical.transform % 2 == 1 {
            (out.height, out.width)
        } else {
            (out.width, out.height)
        };
        if self.physical {
            (w, h)
        } else {
            let scaled = |v: i32| (f64::from(v) / logical.scale).round() as i32;
            (scaled(w), scaled(h))
        }
    }

    fn wanted(&self, ours: &str, mode: DisplayMode, scale: f64) -> Option<Vec<Logical>> {
        let out = self.output(ours)?;
        let target = if scale > 0.0 {
            scale
        } else {
            out.preferred_scale
        };
        let scale = out
            .scales
            .iter()
            .copied()
            .min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()))
            .unwrap_or(1.0);
        let alone = Logical {
            x: 0,
            y: 0,
            scale,
            transform: 0,
            primary: true,
            connectors: vec![ours.to_owned()],
        };
        let mut others: Vec<Logical> = self
            .logical
            .iter()
            .filter(|l| !l.connectors.iter().any(|c| c == ours))
            .cloned()
            .collect();
        if mode != DisplayMode::Extend || others.is_empty() {
            return Some(vec![alone]);
        }
        // Next to whichever monitor reaches furthest right, level with its top edge.
        let (x, y) = others
            .iter()
            .map(|l| (l.x + self.extent(l).0, l.y))
            .max_by_key(|&(right, top)| (right, -top))?;
        if !others.iter().any(|l| l.primary) {
            others[0].primary = true;
        }
        others.push(Logical {
            x,
            y,
            primary: false,
            ..alone
        });
        Some(others)
    }

    fn config(&self, logical: &[Logical]) -> Vec<LogicalMonitorConfig> {
        logical
            .iter()
            .map(|l| {
                let monitors = l
                    .connectors
                    .iter()
                    .filter_map(|c| self.output(c))
                    .map(|o| (o.connector.clone(), o.mode.clone(), HashMap::new()))
                    .collect();
                (l.x, l.y, l.scale, l.transform, l.primary, monitors)
            })
            .collect()
    }
}

fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    props
        .get(key)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

fn same(a: &[Logical], b: &[Logical]) -> bool {
    let sorted = |v: &[Logical]| {
        let mut v = v.to_vec();
        v.sort_by(|p, q| p.connectors.cmp(&q.connectors));
        v
    };
    let (a, b) = (sorted(a), sorted(b));
    a.len() == b.len()
        && a.iter().zip(&b).all(|(p, q)| {
            p.x == q.x
                && p.y == q.y
                && (p.scale - q.scale).abs() < 1e-3
                && p.transform == q.transform
                && p.primary == q.primary
                && p.connectors == q.connectors
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(connector: &str, width: i32, height: i32, scales: &[f64]) -> Output {
        Output {
            connector: connector.into(),
            mode: format!("{width}x{height}@60"),
            width,
            height,
            preferred_scale: 1.0,
            scales: scales.to_vec(),
        }
    }

    fn logical(x: i32, y: i32, scale: f64, primary: bool, connector: &str) -> Logical {
        Logical {
            x,
            y,
            scale,
            transform: 0,
            primary,
            connectors: vec![connector.into()],
        }
    }

    /// The Zenbook Duo with both panels on at 125 %, stacked, and a phone screen Mutter put
    /// to the right at 100 %.
    fn duo() -> Layout {
        Layout {
            serial: 7,
            physical: false,
            connectors: vec![
                ("eDP-1".into(), "0x4170".into()),
                ("eDP-2".into(), "0x4170".into()),
                ("Meta-1".into(), VIRTUAL_PRODUCT.into()),
            ],
            outputs: vec![
                output("eDP-1", 1920, 1200, &[1.0, 1.25, 1.5]),
                output("eDP-2", 1920, 1200, &[1.0, 1.25, 1.5]),
                output("Meta-1", 2184, 1968, &[1.0, 1.33, 1.5, 2.0, 2.67, 3.0]),
            ],
            logical: vec![
                logical(0, 0, 1.25, true, "eDP-1"),
                logical(0, 960, 1.25, false, "eDP-2"),
                logical(1536, 0, 1.0, false, "Meta-1"),
            ],
        }
    }

    #[test]
    fn finds_the_screen_added_for_the_client() {
        let layout = duo();
        let existing = HashSet::from(["eDP-1".to_owned(), "eDP-2".to_owned()]);
        assert_eq!(layout.find_new(&existing).as_deref(), Some("Meta-1"));
        let all = HashSet::from(["eDP-1".into(), "eDP-2".into(), "Meta-1".into()]);
        assert_eq!(layout.find_new(&all), None);
    }

    #[test]
    fn fit_leaves_only_the_client_screen_at_the_nearest_scale() {
        let layout = duo();
        let wanted = layout.wanted("Meta-1", DisplayMode::Fit, 1.97).unwrap();
        assert!(same(&wanted, &[logical(0, 0, 2.0, true, "Meta-1")]));
        let config = layout.config(&wanted);
        assert_eq!(config.len(), 1);
        assert_eq!(config[0].5[0].0, "Meta-1");
        assert_eq!(config[0].5[0].1, "2184x1968@60");
        assert!(!same(&layout.logical, &wanted));
    }

    #[test]
    fn extend_goes_right_of_the_widest_reach() {
        let layout = duo();
        let wanted = layout.wanted("Meta-1", DisplayMode::Extend, 2.0).unwrap();
        assert!(same(
            &wanted,
            &[
                logical(0, 0, 1.25, true, "eDP-1"),
                logical(0, 960, 1.25, false, "eDP-2"),
                logical(1536, 0, 2.0, false, "Meta-1"),
            ]
        ));
        let mut wide = duo();
        wide.outputs[1] = output("eDP-2", 2880, 1800, &[1.0, 1.25]);
        let wanted = wide.wanted("Meta-1", DisplayMode::Extend, 0.0).unwrap();
        let ours = wanted.iter().find(|l| l.connectors == ["Meta-1"]).unwrap();
        assert_eq!((ours.x, ours.y, ours.scale), (2304, 960, 1.0));
    }

    #[test]
    fn a_settled_layout_is_left_alone() {
        let mut layout = duo();
        layout.logical = vec![logical(0, 0, 2.0, true, "Meta-1")];
        let wanted = layout.wanted("Meta-1", DisplayMode::Fit, 2.0).unwrap();
        assert!(same(&layout.logical, &wanted));
    }

    #[test]
    fn rotated_and_physical_extents() {
        let mut layout = duo();
        layout.logical[0].transform = 1;
        assert_eq!(layout.extent(&layout.logical[0]), (960, 1536));
        layout.physical = true;
        assert_eq!(layout.extent(&layout.logical[1]), (1920, 1200));
    }
}
