use std::process::Command;
use std::time::Duration;

use crate::config::window_rules::WindowRulesConfig;
use crate::ipc::launch::{copy_to_clipboard, launch_tui};
use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::command::{execute_shell_action, MouseActions};
use crate::widget::menu::MenuModel;
use crate::widget::network::build_network_menu;
use crate::widget::sysfs::FsPathProvider;
use crate::widget::{Span, Widget, WidgetState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiTier {
    Excellent,
    Good,
    Weak,
    Disconnected,
}

impl WifiTier {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Excellent | Self::Good => "fa-wifi",
            Self::Weak => "fa-wifi-weak",
            Self::Disconnected => "fa-wifi-slash",
        }
    }
}

/// Wireless interface connection information.
#[derive(Debug, Clone, PartialEq)]
pub struct WifiInfo {
    pub interface: String,
    pub ssid: Option<String>,
    pub signal_percent: u8,
    pub tier: WifiTier,
}

/// Parse `/proc/net/wireless` contents into (interface, signal_percent).
pub fn parse_proc_net_wireless(content: &str) -> Vec<(String, u8)> {
    let mut results = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Inter-") || trimmed.starts_with("face") || !trimmed.contains(':') {
            continue;
        }

        let mut parts = trimmed.split(':');
        let iface = match parts.next() {
            Some(i) => i.trim().to_string(),
            None => continue,
        };

        let stats = match parts.next() {
            Some(s) => s.trim(),
            None => continue,
        };

        let fields: Vec<&str> = stats.split_whitespace().collect();
        if fields.len() >= 2 {
            // Quality link is field 1 (e.g. "54." or "70")
            let quality_str = fields[1].trim_end_matches('.');
            if let Ok(quality_val) = quality_str.parse::<f64>() {
                // Typically quality is 0-70 in wireless extensions or 0-100%
                let percent = if quality_val <= 70.0 {
                    ((quality_val / 70.0) * 100.0).round() as u8
                } else {
                    quality_val.min(100.0).round() as u8
                };
                results.push((iface, percent));
            }
        }
    }

    results
}

/// Query SSID for a given wireless interface.
pub fn query_ssid(fs: &FsPathProvider, iface: &str) -> Option<String> {
    // 1. Check for fixture or simulated sysfs path: /sys/class/net/<iface>/ssid
    let fixture_ssid_path = fs.sys_path(format!("class/net/{iface}/ssid"));
    if let Some(ssid) = fs.read_string(&fixture_ssid_path) {
        if !ssid.is_empty() {
            return Some(ssid);
        }
    }

    // 2. Try `iw dev <iface> link` if available
    if let Ok(output) = Command::new("iw").args(["dev", iface, "link"]).output() {
        if output.status.success() {
            let out_str = String::from_utf8_lossy(&output.stdout);
            for line in out_str.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("SSID: ") {
                    let ssid = trimmed.strip_prefix("SSID: ").unwrap_or("").trim();
                    if !ssid.is_empty() {
                        return Some(ssid.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Read all Wi-Fi details. Returns None if no wireless interface is present (desktop machine).
pub fn read_wifi_info(fs: &FsPathProvider) -> Option<WifiInfo> {
    read_wifi_info_preferred(fs, None)
}

/// Read all Wi-Fi details, preferring a configured interface when one is set.
pub fn read_wifi_info_preferred(fs: &FsPathProvider, preferred: Option<&str>) -> Option<WifiInfo> {
    let wireless_path = fs.proc_path("net/wireless");
    let content = match fs.read_string(&wireless_path) {
        Some(c) => c,
        None => {
            tracing::debug!("Wifi widget: /proc/net/wireless unavailable; no wireless hardware?");
            return None;
        }
    };
    let parsed = parse_proc_net_wireless(&content);

    let selected = match preferred {
        Some(wanted) => match parsed.iter().find(|(iface, _)| iface == wanted) {
            Some(entry) => Some(entry.clone()),
            None => {
                tracing::warn!(
                    "Wifi widget: configured interface '{wanted}' not found in /proc/net/wireless; \
                     falling back to the first detected interface"
                );
                parsed.into_iter().next()
            }
        },
        None => parsed.into_iter().next(),
    };

    if let Some((iface, signal_percent)) = selected {
        let tier = match signal_percent {
            70..=100 => WifiTier::Excellent,
            35..=69 => WifiTier::Good,
            1..=34 => WifiTier::Weak,
            _ => WifiTier::Disconnected,
        };

        let ssid = query_ssid(fs, &iface);

        Some(WifiInfo {
            interface: iface,
            ssid,
            signal_percent,
            tier,
        })
    } else {
        None
    }
}

/// Wi-Fi monitoring widget.
pub struct WifiWidget {
    id: String,
    interval: Duration,
    actions: MouseActions,
    window_rules: WindowRulesConfig,
    fs: FsPathProvider,
    preferred_interface: Option<String>,
    show_device: bool,
}

impl WifiWidget {
    pub fn new(id: impl Into<String>, interval_secs: u64) -> Self {
        Self {
            id: id.into(),
            interval: Duration::from_secs(interval_secs.max(1)),
            actions: MouseActions::default(),
            window_rules: WindowRulesConfig::default(),
            fs: FsPathProvider::default(),
            preferred_interface: None,
            show_device: true,
        }
    }

    pub fn with_interface(mut self, interface: Option<String>) -> Self {
        self.preferred_interface = interface;
        self
    }

    pub fn with_show_device(mut self, show: bool) -> Self {
        self.show_device = show;
        self
    }

    pub fn with_actions(mut self, actions: MouseActions) -> Self {
        self.actions = actions;
        self
    }

    pub fn with_window_rules(mut self, rules: WindowRulesConfig) -> Self {
        self.window_rules = rules;
        self
    }

    pub fn with_fs(mut self, fs: FsPathProvider) -> Self {
        self.fs = fs;
        self
    }
}

impl Widget for WifiWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn update_interval(&self) -> Duration {
        self.interval
    }

    fn handle_mouse_event_at(
        &self,
        event: WidgetMouseEvent,
        _rel_x: i32,
        _rel_y: i32,
        _span_idx: Option<usize>,
    ) {
        if let Some(cmd) = self.actions.get(event) {
            let _ = execute_shell_action(cmd);
        }
    }

    fn get_menu(&self) -> Option<MenuModel> {
        build_network_menu(&self.fs, None)
    }

    fn handle_menu_action(&self, action_id: &str) {
        if let Some(ip) = action_id.strip_prefix("copy:") {
            copy_to_clipboard(ip);
            return;
        }
        if action_id == "open-network-manager" {
            launch_tui("flatbar.network", &["nmtui"], &self.window_rules);
        }
    }

    fn state(&self) -> WidgetState {
        let info = match read_wifi_info_preferred(&self.fs, self.preferred_interface.as_deref()) {
            Some(i) => i,
            None => {
                // No wireless link at all: X glyph instead of hiding.
                return WidgetState {
                    spans: vec![Span::icon("fa-xmark"), Span::text(" down")],
                    tooltip: Some("No wireless connection".to_string()),
                };
            }
        };

        let icon = if info.tier == WifiTier::Disconnected {
            "fa-xmark"
        } else {
            info.tier.icon()
        };
        let mut spans = vec![Span::icon(icon)];
        if info.tier != WifiTier::Disconnected {
            if self.show_device {
                let display_name = info.ssid.as_deref().unwrap_or(info.interface.as_str());
                spans.push(Span::text(format!(
                    " {} ({}%)",
                    display_name, info.signal_percent
                )));
            }
            return WidgetState {
                spans,
                tooltip: Some(format!(
                    "Interface: {}, Tier: {:?}",
                    info.interface, info.tier
                )),
            };
        }

        WidgetState {
            spans,
            tooltip: Some("No wireless connection".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_parse_proc_net_wireless() {
        let sample = r#"Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
 wlan0: 0000   56.  -54.  -256        0      0      0      0      0        0
"#;
        let parsed = parse_proc_net_wireless(sample);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "wlan0");
        assert_eq!(parsed[0].1, 80); // 56/70 = 80%
    }

    #[test]
    fn test_wifi_fixture_with_ssid() {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        let sys_wlan = dir.path().join("sys/class/net/wlan0");
        fs::create_dir_all(&proc_net).unwrap();
        fs::create_dir_all(&sys_wlan).unwrap();

        let sample = r#"Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
 wlan0: 0000   63.  -45.  -256        0      0      0      0      0        0
"#;
        fs::write(proc_net.join("wireless"), sample).unwrap();
        fs::write(sys_wlan.join("ssid"), "OfficeNet\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path().join("sys"));
        let widget = WifiWidget::new("wifi", 5).with_fs(fs_prov);
        let state = widget.state();

        assert_eq!(state.spans.len(), 2);
        assert_eq!(state.spans[1].as_text(), Some(" OfficeNet (90%)"));
    }

    #[test]
    fn test_wifi_missing_hardware_renders_x_glyph() {
        let dir = tempdir().unwrap();
        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path().join("sys"));
        let widget = WifiWidget::new("wifi", 5).with_fs(fs_prov);
        let state = widget.state();
        // Issue 10: X glyph instead of hiding when nothing is connected.
        assert_eq!(state.spans[0].as_icon(), Some("fa-xmark"));
        assert!(state.spans[1].as_text().unwrap().contains("down"));
    }

    #[test]
    fn test_wifi_preferred_interface_filter() {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        let sys_wlan = dir.path().join("sys/class/net/wlp1s0");
        fs::create_dir_all(&proc_net).unwrap();
        fs::create_dir_all(&sys_wlan).unwrap();

        let sample = r#"Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
 wlan0: 0000   21.  -70.  -256        0      0      0      0      0        0
 wlp1s0: 0000   63.  -45.  -256        0      0      0      0      0        0
"#;
        fs::write(proc_net.join("wireless"), sample).unwrap();
        fs::write(sys_wlan.join("ssid"), "HomeNet\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path().join("sys"));
        let widget = WifiWidget::new("wifi", 5)
            .with_fs(fs_prov)
            .with_interface(Some("wlp1s0".to_string()));
        let state = widget.state();

        assert_eq!(state.spans.len(), 2);
        assert_eq!(state.spans[1].as_text(), Some(" HomeNet (90%)"));
    }

    #[test]
    fn test_wifi_preferred_interface_missing_falls_back() {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        let sys_wlan = dir.path().join("sys/class/net/wlan0");
        fs::create_dir_all(&proc_net).unwrap();
        fs::create_dir_all(&sys_wlan).unwrap();

        let sample = r#"Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
 wlan0: 0000   56.  -54.  -256        0      0      0      0      0        0
"#;
        fs::write(proc_net.join("wireless"), sample).unwrap();
        fs::write(sys_wlan.join("ssid"), "OfficeNet\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path().join("sys"));
        let info = read_wifi_info_preferred(&fs_prov, Some("wlp9s0")).unwrap();
        assert_eq!(info.interface, "wlan0");
    }
}
