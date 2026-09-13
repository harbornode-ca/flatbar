//! Bluetooth widget displaying adapter state, connected devices, and quick connect/disconnect menu.

use std::time::Duration;

use crate::config::window_rules::WindowRulesConfig;
use crate::config::MenuBackend;
use crate::dbus::bluetooth::{BluetoothClient, BluetoothState};
use crate::ipc::launch::launch_tui;
use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::state::WidgetState;
use crate::widget::{Span, Widget};

/// Configuration for Bluetooth widget.
#[derive(Debug, Clone)]
pub struct BluetoothConfig {
    pub interval_secs: u64,
    pub format: String,
    pub icon_off: String,
    pub icon_on: String,
    pub icon_connected: String,
    pub emphasis_on_connected: bool,
    pub menu_backend: MenuBackend,
    /// Command (tokens) used by the menu's Manage Devices entry.
    /// Default `bluetui` — not shipped; the user installs it or overrides
    /// this. Resolved next to the flatbar binary, then via PATH.
    pub manage_command: String,
    pub window_rules: WindowRulesConfig,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        Self {
            interval_secs: 5,
            format: "{icon}".to_string(),
            icon_off: "fa-bluetooth".to_string(),
            icon_on: "fa-bluetooth".to_string(),
            icon_connected: "fa-bluetooth-b".to_string(),
            emphasis_on_connected: true,
            menu_backend: MenuBackend::Launcher,
            manage_command: "bluetui".to_string(),
            window_rules: WindowRulesConfig::default(),
        }
    }
}

/// Tokenize the configurable `manage_command` string into argv tokens.
pub fn manage_command_argv(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(str::to_string).collect()
}

/// Bluetooth status bar widget.
pub struct BluetoothWidget {
    id: String,
    config: BluetoothConfig,
    client: BluetoothClient,
}

impl BluetoothWidget {
    pub fn new(id: impl Into<String>, config: BluetoothConfig) -> Self {
        Self {
            id: id.into(),
            config,
            client: BluetoothClient::new(),
        }
    }

    pub fn with_client(
        id: impl Into<String>,
        config: BluetoothConfig,
        client: BluetoothClient,
    ) -> Self {
        Self {
            id: id.into(),
            config,
            client,
        }
    }

    /// Build the interactive popup `MenuModel` for Bluetooth devices.
    pub fn build_menu_model(&self) -> MenuModel {
        let state = self.client.state();
        let mut items = Vec::new();

        // 1. Manage Devices entry at the top of the menu
        items.push(MenuItem::item("manage_devices", "Manage Devices"));
        items.push(MenuItem::Separator);

        // 2. Adapter Power Toggle Header
        let power_label = if state.adapter_powered {
            "Bluetooth: Powered On"
        } else {
            "Bluetooth: Powered Off"
        };
        items.push(MenuItem::checkbox(
            "toggle_power",
            power_label,
            state.adapter_powered,
        ));

        items.push(MenuItem::Separator);

        // 3. Paired Devices List
        let paired = state.paired_devices();
        if paired.is_empty() {
            items.push(MenuItem::Item {
                id: "no_devices".to_string(),
                label: "No paired devices".to_string(),
                icon: None,
                enabled: false,
            });
        } else {
            for dev in paired {
                let dev_name = &dev.name;
                let is_connected = dev.connected;
                let glyph = if is_connected { "●" } else { "○" };

                let label = if is_connected {
                    if let Some(bat) = dev.battery {
                        format!("{glyph} {} ({}%)", dev_name, bat)
                    } else {
                        format!("{glyph} {}", dev_name)
                    }
                } else {
                    format!("{glyph} {}", dev_name)
                };

                let item_id = format!("device:{}", dev.path);
                items.push(MenuItem::checkbox(item_id, label, is_connected));
            }
        }

        MenuModel::new(items).with_title("Bluetooth")
    }

    fn generate_tooltip(&self, state: &BluetoothState) -> String {
        if !state.adapter_present {
            return "Bluetooth: No adapter found".to_string();
        }
        if !state.adapter_powered {
            return "Bluetooth: Powered off".to_string();
        }

        let connected = state.connected_devices();
        if connected.is_empty() {
            "Bluetooth: Powered on (no devices connected)".to_string()
        } else {
            let mut s = format!("Bluetooth ({} connected):\n", connected.len());
            for (i, dev) in connected.iter().enumerate() {
                if let Some(bat) = dev.battery {
                    s.push_str(&format!("  {}. {} ({}%)\n", i + 1, dev.name, bat));
                } else {
                    s.push_str(&format!("  {}. {}\n", i + 1, dev.name));
                }
            }
            s.trim_end().to_string()
        }
    }
}

impl Widget for BluetoothWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn update_interval(&self) -> Duration {
        Duration::from_secs(self.config.interval_secs)
    }

    fn refresh(&self) {
        self.client.refresh();
    }

    fn state(&self) -> WidgetState {
        let state = self.client.state();
        let mut spans = Vec::new();

        let icon_name;
        let is_connected;

        if !state.adapter_present || !state.adapter_powered {
            icon_name = &self.config.icon_off;
            is_connected = false;
        } else {
            let connected = state.connected_devices();
            if connected.is_empty() {
                icon_name = &self.config.icon_on;
                is_connected = false;
            } else {
                icon_name = &self.config.icon_connected;
                is_connected = true;
            }
        }

        let should_emphasize = is_connected && self.config.emphasis_on_connected;
        if should_emphasize {
            spans.push(Span::emphasized_icon(icon_name));
        } else {
            spans.push(Span::icon(icon_name));
        }

        // Format extra text if requested
        let connected_devices = state.connected_devices();
        let connected_count = connected_devices.len();
        let first_device_name = connected_devices
            .first()
            .map(|d| d.name.as_str())
            .unwrap_or("");

        let text_part = self
            .config
            .format
            .replace("{icon}", "")
            .replace("{connected_count}", &connected_count.to_string())
            .replace("{device_name}", first_device_name)
            .trim()
            .to_string();

        if !text_part.is_empty() {
            spans.push(Span::text(" "));
            if should_emphasize {
                spans.push(Span::emphasized_text(text_part));
            } else {
                spans.push(Span::text(text_part));
            }
        }

        let tooltip = Some(self.generate_tooltip(&state));
        WidgetState::new(spans).with_tooltip(tooltip)
    }

    fn handle_mouse_event_at(
        &self,
        _event: WidgetMouseEvent,
        _rel_x: i32,
        _rel_y: i32,
        _span_idx: Option<usize>,
    ) {
    }

    fn get_menu(&self) -> Option<MenuModel> {
        Some(self.build_menu_model())
    }

    fn handle_menu_action(&self, action_id: &str) {
        if action_id == "toggle_power" {
            let state = self.client.state();
            let _ = self.client.set_adapter_power(!state.adapter_powered);
        } else if let Some(dev_path) = action_id.strip_prefix("device:") {
            let state = self.client.state();
            if let Some(dev) = state.devices.iter().find(|d| d.path == dev_path) {
                if dev.connected {
                    let _ = self.client.disconnect_device(dev_path);
                } else {
                    let _ = self.client.connect_device(dev_path);
                }
            }
        } else if action_id == "manage_devices" || action_id == "open_bluetuith" {
            let argv = manage_command_argv(&self.config.manage_command);
            if argv.is_empty() {
                tracing::warn!("Bluetooth manage_command is empty; skipping launch");
                return;
            }
            launch_tui(
                "flatbar.bluetooth",
                &argv.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                &self.config.window_rules,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bluetooth_widget_state_off() {
        let widget = BluetoothWidget::new("bluetooth", BluetoothConfig::default());
        let state = widget.state();
        assert!(!state.spans.is_empty());
        assert!(!state.spans[0].is_emphasized());
    }

    #[test]
    fn test_bluetooth_widget_menu_model() {
        let widget = BluetoothWidget::new("bluetooth", BluetoothConfig::default());
        let menu = widget.build_menu_model();
        assert_eq!(menu.title.as_deref(), Some("Bluetooth"));
        assert!(menu.items.len() >= 3);
    }

    #[test]
    fn test_default_manage_command_is_bluetui() {
        let config = BluetoothConfig::default();
        assert_eq!(config.manage_command, "bluetui");
        let argv = manage_command_argv(&config.manage_command);
        assert_eq!(argv, vec!["bluetui"]);
    }

    #[test]
    fn test_manage_command_tokenize_with_args() {
        assert_eq!(
            manage_command_argv("bluetui --config /tmp/bluetui.toml"),
            vec!["bluetui", "--config", "/tmp/bluetui.toml"]
        );
        // Custom alternate tool still honored
        assert_eq!(
            manage_command_argv("/usr/local/bin/bluetuith"),
            vec!["/usr/local/bin/bluetuith"]
        );
    }

    #[test]
    fn test_manage_command_empty() {
        assert!(manage_command_argv("   ").is_empty());
        assert!(manage_command_argv("").is_empty());
    }
}
