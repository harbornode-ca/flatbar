//! Battery widget: monitoring power supply capacity, charging states, and time estimates with multi-battery aggregation.

use std::fs;
use std::str::FromStr;
use std::time::Duration;

use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::command::{execute_shell_action, MouseActions};
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::sysfs::FsPathProvider;
use crate::widget::{Span, Widget, WidgetState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

impl FromStr for BatteryStatus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let status = match s.trim().to_lowercase().as_str() {
            "charging" => Self::Charging,
            "discharging" => Self::Discharging,
            "full" => Self::Full,
            "not charging" => Self::NotCharging,
            _ => Self::Unknown,
        };
        Ok(status)
    }
}

/// Aggregated battery information across all system batteries and AC power supplies.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregatedBattery {
    pub percentage: u8,
    pub status: BatteryStatus,
    pub time_remaining: Option<Duration>, // Time to empty (if discharging) or time to full (if charging)
    pub on_ac: bool,
    /// False when no physical battery exists (AC-only desktop/VM).
    pub battery_present: bool,
    /// Battery health (full ÷ design capacity × 100) when design capacity is readable.
    pub health: Option<u8>,
}

/// Read and aggregate all battery power supplies and AC adapter state.
pub fn read_battery_info(fs: &FsPathProvider) -> Option<AggregatedBattery> {
    let power_supply_dir = fs.sys_path("class/power_supply");
    let entries = fs::read_dir(power_supply_dir).ok()?;

    let mut total_energy_now = 0u64;
    let mut total_energy_full = 0u64;
    let mut total_energy_design = 0u64;
    let mut total_power_now = 0u64;

    let mut capacities = Vec::new();
    let mut statuses = Vec::new();

    let mut has_mains = false;
    let mut on_ac = false;

    for entry in entries.flatten() {
        let path = entry.path();
        let ptype = fs.read_string(&path.join("type")).unwrap_or_default();

        if ptype.eq_ignore_ascii_case("Mains") {
            has_mains = true;
            if let Some(online) = fs.read_int::<u8>(&path.join("online")) {
                if online == 1 {
                    on_ac = true;
                }
            }
            continue;
        }

        if !ptype.eq_ignore_ascii_case("Battery") {
            continue;
        }

        let status_str = fs.read_string(&path.join("status")).unwrap_or_default();
        statuses.push(status_str.parse().unwrap_or(BatteryStatus::Unknown));

        if let Some(cap) = fs.read_int::<u8>(&path.join("capacity")) {
            capacities.push(cap);
        }

        let energy_now = fs
            .read_int::<u64>(&path.join("energy_now"))
            .or_else(|| fs.read_int::<u64>(&path.join("charge_now")));
        let energy_full = fs
            .read_int::<u64>(&path.join("energy_full"))
            .or_else(|| fs.read_int::<u64>(&path.join("charge_full")));
        let energy_design = fs
            .read_int::<u64>(&path.join("energy_full_design"))
            .or_else(|| fs.read_int::<u64>(&path.join("charge_full_design")));
        let power_now = fs
            .read_int::<u64>(&path.join("power_now"))
            .or_else(|| fs.read_int::<u64>(&path.join("current_now")));

        if let (Some(now), Some(full)) = (energy_now, energy_full) {
            total_energy_now += now;
            total_energy_full += full;
        }
        if let Some(design) = energy_design {
            total_energy_design += design;
        }
        if let Some(p) = power_now {
            total_power_now += p;
        }
    }

    if statuses.is_empty() && capacities.is_empty() {
        if has_mains && on_ac {
            return Some(AggregatedBattery {
                percentage: 100,
                status: BatteryStatus::Full,
                time_remaining: None,
                on_ac: true,
                battery_present: false,
                health: None,
            });
        }
        return None; // No power supplies present (desktop)
    }

    // Determine overall status
    let status = if statuses.contains(&BatteryStatus::Charging) {
        BatteryStatus::Charging
    } else if statuses.iter().all(|s| *s == BatteryStatus::Full) {
        BatteryStatus::Full
    } else if statuses.contains(&BatteryStatus::Discharging) {
        BatteryStatus::Discharging
    } else {
        statuses.first().copied().unwrap_or(BatteryStatus::Unknown)
    };

    // Calculate overall percentage
    let percentage = if total_energy_full > 0 {
        ((total_energy_now as f64 / total_energy_full as f64) * 100.0).round() as u8
    } else if !capacities.is_empty() {
        let sum: u32 = capacities.iter().map(|&c| c as u32).sum();
        (sum / capacities.len() as u32) as u8
    } else {
        0
    };

    // Calculate battery health from design capacity
    let health = if total_energy_design > 0 && total_energy_full > 0 {
        let h = (total_energy_full as f64 / total_energy_design as f64) * 100.0;
        Some(h.round().clamp(0.0, 100.0) as u8)
    } else {
        None
    };

    // Calculate time remaining estimate
    let time_remaining = if total_power_now > 0 {
        if status == BatteryStatus::Discharging && total_energy_now > 0 {
            let hours = total_energy_now as f64 / total_power_now as f64;
            Some(Duration::from_secs_f64(hours * 3600.0))
        } else if status == BatteryStatus::Charging && total_energy_full > total_energy_now {
            let needed = total_energy_full - total_energy_now;
            let hours = needed as f64 / total_power_now as f64;
            Some(Duration::from_secs_f64(hours * 3600.0))
        } else {
            None
        }
    } else {
        None
    };

    Some(AggregatedBattery {
        percentage: percentage.min(100),
        status,
        time_remaining,
        on_ac,
        battery_present: true,
        health,
    })
}

/// Synthetic aggregated state for systems with no power supplies at all.
fn synthetic_ac_info() -> AggregatedBattery {
    AggregatedBattery {
        percentage: 100,
        status: BatteryStatus::Full,
        time_remaining: None,
        on_ac: true,
        battery_present: false,
        health: None,
    }
}

/// Format a duration as `Xh YYm` (or `YYm` under an hour).
fn format_duration(time: Duration) -> String {
    let mins = time.as_secs() / 60;
    let h = mins / 60;
    let m = mins % 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// Status bar battery widget.
pub struct BatteryWidget {
    id: String,
    interval: Duration,
    low_below: u8,
    /// When no power supplies exist at all (mains-less desktops/VMs),
    /// render a synthetic always-on-AC state instead of hiding.
    assume_ac: bool,
    actions: MouseActions,
    fs: FsPathProvider,
}

impl BatteryWidget {
    pub fn new(id: impl Into<String>, interval_secs: u64, low_below: Option<u8>) -> Self {
        Self {
            id: id.into(),
            interval: Duration::from_secs(interval_secs.max(1)),
            low_below: low_below.unwrap_or(15),
            assume_ac: true,
            actions: MouseActions::default(),
            fs: FsPathProvider::default(),
        }
    }

    pub fn with_actions(mut self, actions: MouseActions) -> Self {
        self.actions = actions;
        self
    }

    pub fn with_assume_ac(mut self, assume_ac: bool) -> Self {
        self.assume_ac = assume_ac;
        self
    }

    pub fn with_fs(mut self, fs: FsPathProvider) -> Self {
        self.fs = fs;
        self
    }
}

impl Widget for BatteryWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn update_interval(&self) -> Duration {
        self.interval
    }

    fn handle_mouse_event(&self, event: WidgetMouseEvent) {
        if let Some(cmd) = self.actions.get(event) {
            let _ = execute_shell_action(cmd);
        }
    }

    fn get_menu(&self) -> Option<MenuModel> {
        let info_opt = read_battery_info(&self.fs).or_else(|| {
            if self.assume_ac {
                Some(synthetic_ac_info())
            } else {
                None
            }
        });
        let info = info_opt?;
        let mut items = Vec::new();

        if !info.battery_present {
            // AC-only desktop: single status line.
            items.push(MenuItem::item("ac-power", "AC-Power"));
            return Some(MenuModel::new(items).with_title("Power"));
        }

        // 1. Current status line.
        let status_label = match info.status {
            BatteryStatus::Charging => "Charging",
            BatteryStatus::Discharging => "Discharging",
            BatteryStatus::Full | BatteryStatus::NotCharging if info.on_ac => "AC-Power",
            BatteryStatus::Full => "Full",
            BatteryStatus::NotCharging => "Not Charging",
            BatteryStatus::Unknown => "Unknown",
        };
        items.push(MenuItem::item("battery-status", status_label));

        // 2. Battery level.
        items.push(MenuItem::item(
            "battery-level",
            format!("Battery: {}%", info.percentage),
        ));

        // 3. Time left (discharging) / time to full (charging).
        if let Some(time) = info.time_remaining {
            let time_str = match info.status {
                BatteryStatus::Discharging => format!("Time left: {}", format_duration(time)),
                BatteryStatus::Charging => format!("Time to full: {}", format_duration(time)),
                _ => format!("Time: {}", format_duration(time)),
            };
            items.push(MenuItem::item("battery-time", time_str));
        }

        // 4. Battery health.
        if let Some(health) = info.health {
            items.push(MenuItem::item(
                "battery-health",
                format!("Health: {health}%"),
            ));
        }

        Some(MenuModel::new(items).with_title("Power"))
    }

    fn state(&self) -> WidgetState {
        let info = match read_battery_info(&self.fs) {
            Some(info) => info,
            None => {
                if self.assume_ac {
                    // Mains-less desktop/VM: no supplies at all; show powered-by-AC state.
                    synthetic_ac_info()
                } else {
                    tracing::debug!(
                        "Battery widget: no power supplies in /sys/class/power_supply; hidden"
                    );
                    return WidgetState::default();
                }
            }
        };

        // 3-state icon logic:
        // 1. Charging -> fa-bolt
        // 2. On AC (or Full) -> fa-plug
        // 3. Discharging -> level icons
        let icon = if info.status == BatteryStatus::Charging {
            "fa-bolt"
        } else if info.on_ac || info.status == BatteryStatus::Full {
            "fa-plug"
        } else {
            match info.percentage {
                90..=100 => "fa-battery-full",
                65..=89 => "fa-battery-three-quarters",
                40..=64 => "fa-battery-half",
                15..=39 => "fa-battery-quarter",
                _ => "fa-battery-empty",
            }
        };

        let is_low = info.status == BatteryStatus::Discharging && info.percentage < self.low_below;
        let is_charging = info.status == BatteryStatus::Charging;
        let emphasis = is_low || is_charging;

        let time_str = if let Some(time) = info.time_remaining {
            format!(" ({})", format_duration(time))
        } else {
            "".to_string()
        };

        let text = format!(" {}%{}", info.percentage, time_str);

        let spans = if !info.battery_present {
            // AC-only machine: glyph only, never a percentage.
            if emphasis {
                vec![Span::emphasized_icon(icon)]
            } else {
                vec![Span::icon(icon)]
            }
        } else if emphasis {
            vec![Span::emphasized_icon(icon), Span::emphasized_text(text)]
        } else {
            vec![Span::icon(icon), Span::text(text)]
        };

        WidgetState {
            spans,
            tooltip: Some(if info.battery_present {
                format!("Status: {:?}", info.status)
            } else {
                "AC Power".to_string()
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_battery_single_fixture() {
        let dir = tempdir().unwrap();
        let bat_dir = dir.path().join("class/power_supply/BAT0");
        fs::create_dir_all(&bat_dir).unwrap();

        fs::write(bat_dir.join("type"), "Battery\n").unwrap();
        fs::write(bat_dir.join("status"), "Discharging\n").unwrap();
        fs::write(bat_dir.join("capacity"), "80\n").unwrap();
        fs::write(bat_dir.join("energy_now"), "40000000\n").unwrap();
        fs::write(bat_dir.join("energy_full"), "50000000\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let info = read_battery_info(&fs_prov).unwrap();
        assert_eq!(info.percentage, 80);
        assert_eq!(info.status, BatteryStatus::Discharging);
        assert!(!info.on_ac);
    }

    #[test]
    fn test_battery_health_from_design_capacity() {
        let dir = tempdir().unwrap();
        let bat_dir = dir.path().join("class/power_supply/BAT0");
        fs::create_dir_all(&bat_dir).unwrap();

        fs::write(bat_dir.join("type"), "Battery\n").unwrap();
        fs::write(bat_dir.join("status"), "Discharging\n").unwrap();
        fs::write(bat_dir.join("energy_now"), "40000000\n").unwrap();
        fs::write(bat_dir.join("energy_full"), "45000000\n").unwrap();
        fs::write(bat_dir.join("energy_full_design"), "50000000\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let info = read_battery_info(&fs_prov).unwrap();
        assert_eq!(info.health, Some(90));

        // Menu shows the health line.
        let widget = BatteryWidget::new("bat", 1, None).with_fs(fs_prov);
        let menu = widget.get_menu().unwrap();
        assert!(menu.items.iter().any(|i| i.label() == Some("Health: 90%")));
    }

    #[test]
    fn test_battery_no_health_without_design_capacity() {
        let dir = tempdir().unwrap();
        let bat_dir = dir.path().join("class/power_supply/BAT0");
        fs::create_dir_all(&bat_dir).unwrap();

        fs::write(bat_dir.join("type"), "Battery\n").unwrap();
        fs::write(bat_dir.join("status"), "Discharging\n").unwrap();
        fs::write(bat_dir.join("energy_now"), "40000000\n").unwrap();
        fs::write(bat_dir.join("energy_full"), "50000000\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let info = read_battery_info(&fs_prov).unwrap();
        assert_eq!(info.health, None);
    }

    #[test]
    fn test_battery_ac_mains_detection() {
        let dir = tempdir().unwrap();
        let ac_dir = dir.path().join("class/power_supply/AC");
        fs::create_dir_all(&ac_dir).unwrap();

        fs::write(ac_dir.join("type"), "Mains\n").unwrap();
        fs::write(ac_dir.join("online"), "1\n").unwrap();

        let bat_dir = dir.path().join("class/power_supply/BAT0");
        fs::create_dir_all(&bat_dir).unwrap();
        fs::write(bat_dir.join("type"), "Battery\n").unwrap();
        fs::write(bat_dir.join("status"), "Full\n").unwrap();
        fs::write(bat_dir.join("capacity"), "100\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let info = read_battery_info(&fs_prov).unwrap();
        assert_eq!(info.percentage, 100);
        assert_eq!(info.status, BatteryStatus::Full);
        assert!(info.on_ac);

        let widget = BatteryWidget::new("bat", 1, None).with_fs(fs_prov);
        let state = widget.state();
        assert_eq!(state.spans[0].as_icon(), Some("fa-plug"));
    }

    #[test]
    fn test_battery_dual_aggregation() {
        let dir = tempdir().unwrap();
        let bat0 = dir.path().join("class/power_supply/BAT0");
        let bat1 = dir.path().join("class/power_supply/BAT1");
        fs::create_dir_all(&bat0).unwrap();
        fs::create_dir_all(&bat1).unwrap();

        fs::write(bat0.join("type"), "Battery\n").unwrap();
        fs::write(bat0.join("status"), "Discharging\n").unwrap();
        fs::write(bat0.join("energy_now"), "20000000\n").unwrap();
        fs::write(bat0.join("energy_full"), "40000000\n").unwrap();

        fs::write(bat1.join("type"), "Battery\n").unwrap();
        fs::write(bat1.join("status"), "Charging\n").unwrap();
        fs::write(bat1.join("energy_now"), "30000000\n").unwrap();
        fs::write(bat1.join("energy_full"), "60000000\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let info = read_battery_info(&fs_prov).unwrap();
        assert_eq!(info.percentage, 50);
        assert_eq!(info.status, BatteryStatus::Charging);
    }

    #[test]
    fn test_battery_desktop_renders_empty_when_assume_ac_off() {
        let dir = tempdir().unwrap();
        let ps_dir = dir.path().join("class/power_supply");
        fs::create_dir_all(&ps_dir).unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let widget = BatteryWidget::new("bat", 1, None)
            .with_fs(fs_prov)
            .with_assume_ac(false);
        let state = widget.state();
        assert!(state.spans.is_empty());
    }

    #[test]
    fn test_battery_mains_less_desktop_renders_plug_by_default() {
        let dir = tempdir().unwrap();
        let ps_dir = dir.path().join("class/power_supply");
        fs::create_dir_all(&ps_dir).unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let widget = BatteryWidget::new("bat", 1, None).with_fs(fs_prov.clone());
        let state = widget.state();
        assert_eq!(state.spans[0].as_icon(), Some("fa-plug"));
        // AC-only: icon only, never a percentage.
        assert_eq!(state.spans.len(), 1);
        assert!(state.spans.iter().all(|s| s.as_text().is_none()));

        // Menu reads a single AC-Power line.
        let menu = widget.get_menu().unwrap();
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].id(), Some("ac-power"));
        assert_eq!(menu.items[0].label(), Some("AC-Power"));
    }

    #[test]
    fn test_battery_menu_model() {
        let dir = tempdir().unwrap();
        let bat_dir = dir.path().join("class/power_supply/BAT0");
        fs::create_dir_all(&bat_dir).unwrap();

        fs::write(bat_dir.join("type"), "Battery\n").unwrap();
        fs::write(bat_dir.join("status"), "Discharging\n").unwrap();
        fs::write(bat_dir.join("capacity"), "75\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let widget = BatteryWidget::new("bat", 1, None).with_fs(fs_prov);
        let menu = widget.get_menu().unwrap();
        assert_eq!(menu.title.as_deref(), Some("Power"));
        assert!(!menu.items.is_empty());
        // Status line first, then level.
        assert_eq!(menu.items[0].label(), Some("Discharging"));
        assert_eq!(menu.items[1].label(), Some("Battery: 75%"));
        // No health line without design capacity.
        assert!(menu
            .items
            .iter()
            .all(|i| !i.label().is_some_and(|l| l.starts_with("Health"))));
    }

    #[test]
    fn test_battery_menu_time_labels() {
        let dir = tempdir().unwrap();
        let bat_dir = dir.path().join("class/power_supply/BAT0");
        fs::create_dir_all(&bat_dir).unwrap();

        fs::write(bat_dir.join("type"), "Battery\n").unwrap();
        fs::write(bat_dir.join("status"), "Discharging\n").unwrap();
        fs::write(bat_dir.join("energy_now"), "20000000\n").unwrap();
        fs::write(bat_dir.join("energy_full"), "50000000\n").unwrap();
        fs::write(bat_dir.join("power_now"), "10000000\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path(), dir.path());
        let widget = BatteryWidget::new("bat", 1, None).with_fs(fs_prov);
        let menu = widget.get_menu().unwrap();
        assert!(menu
            .items
            .iter()
            .any(|i| i.label().is_some_and(|l| l.starts_with("Time left: "))));

        // Charging case: time to full.
        fs::write(bat_dir.join("status"), "Charging\n").unwrap();
        let menu = widget.get_menu().unwrap();
        assert!(menu
            .items
            .iter()
            .any(|i| i.label().is_some_and(|l| l.starts_with("Time to full: "))));
    }
}
