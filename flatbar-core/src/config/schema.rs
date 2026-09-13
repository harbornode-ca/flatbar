//! JSON schema generator for Flatbar configuration files.

use serde_json::json;

/// Format the JSON schema as a pretty-printed JSON string.
pub fn print_schema_json() -> String {
    serde_json::to_string_pretty(&generate_json_schema()).unwrap_or_default()
}

/// Generate a JSON Schema (Draft 7) for Flatbar `config.toml`.
pub fn generate_json_schema() -> serde_json::Value {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "Flatbar Configuration",
        "description": "Configuration schema for flatbar - a lightweight, two-color Wayland status bar",
        "type": "object",
        "properties": {
            "bar": {
                "type": "object",
                "description": "Bar visual appearance, sizing, and layout configuration",
                "properties": {
                    "name": {
                        "type": "string",
                        "default": "main",
                        "description": "Instance name for layer-shell namespace (org.flatbar.<name>)"
                    },
                    "height": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 30,
                        "description": "Bar height in logical pixels"
                    },
                    "background": {
                        "type": "string",
                        "default": "#1e1e2e",
                        "description": "Background color (HEX #RRGGBB or #RRGGBBAA)"
                    },
                    "foreground": {
                        "type": "string",
                        "default": "#cdd6f4",
                        "description": "Foreground / text color (HEX #RRGGBB or #RRGGBBAA)"
                    },
                    "position": {
                        "type": "string",
                        "enum": ["top", "bottom"],
                        "default": "top",
                        "description": "Screen edge placement"
                    },
                    "output": {
                        "type": "string",
                        "default": "all",
                        "description": "Target output monitor name (e.g. 'DP-1') or 'all'"
                    },
                    "font_family": {
                        "type": "string",
                        "default": "sans-serif",
                        "description": "Primary UI font family"
                    },
                    "font_size": {
                        "type": "number",
                        "minimum": 1.0,
                        "default": 13.0,
                        "description": "Font size in points/pixels"
                    },
                    "font_fallback": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Fallback font families"
                    },
                    "inner_padding": {
                        "type": "integer",
                        "default": 2,
                        "description": "Gap in pixels between spans within one widget (icon-text and adjacent icon/icon gaps); widget-to-widget spacing is unchanged"
                    },
                    "menu_command": {
                        "type": "string",
                        "description": "External menu launcher command (e.g. 'fuzzel -d')"
                    },
                    "menu_backend": {
                        "type": "string",
                        "enum": ["popup", "launcher"],
                        "default": "launcher",
                        "description": "Menu rendering mode (internal XDG popup or external launcher)"
                    },
                    "layout": {
                        "type": "object",
                        "description": "Widget positioning across bar sections",
                        "properties": {
                            "left": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Widgets aligned to the left edge"
                            },
                            "center": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Widgets centered in the bar"
                            },
                            "right": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Widgets aligned to the right edge"
                            }
                        }
                    }
                },
                "required": ["height", "background", "foreground"]
            },
            "widgets": {
                "type": "object",
                "description": "Widget instance configurations keyed by widget ID",
                "additionalProperties": {
                    "type": "object",
                    "properties": {
                            "type": {
                                "type": "string",
                                "enum": ["workspaces", "datetime", "stats", "disk", "battery", "performance", "network", "wifi", "bluetooth", "systray", "tray", "command", "clipboard", "notifications"],
                                "description": "Widget type"
                            },
                        "interval": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Update interval in seconds (0 = event-driven)"
                        },
                        "format": {
                            "type": "string",
                            "description": "Display format string (e.g. '{icon} {device_name}')"
                        },
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute for command widgets"
                        },
                        "left_click": {
                            "type": "string",
                            "description": "Shell command on left click"
                        },
                        "right_click": {
                            "type": "string",
                            "description": "Shell command on right click"
                        },
                        "middle_click": {
                            "type": "string",
                            "description": "Shell command on middle click"
                        },
                        "scroll_up": {
                            "type": "string",
                            "description": "Shell command on scroll up"
                        },
                        "scroll_down": {
                            "type": "string",
                            "description": "Shell command on scroll down"
                        },
                        "assume_ac": {
                            "type": "boolean",
                            "default": true,
                            "description": "Render a synthetic powered (plug) state when no power supplies exist (battery widget)"
                        },
                        "units": {
                            "type": "string",
                            "enum": ["auto", "MB", "GB", "TB", "MiB", "GiB", "TiB"],
                            "default": "auto",
                            "description": "Storage widget size unit system for menu labels and notifications"
                        },
                        "file_manager": {
                            "type": "string",
                            "default": "rc",
                            "description": "Storage widget: file-manager command launched on right-click (e.g. 'rc' for rat-commander); left-click still opens the storage menu"
                        },
                        "icon_powersaver": {
                            "type": "string",
                            "default": "fa-leaf",
                            "description": "Icon shown by the performance widget when the power-saver profile is active"
                        },
                        "icon_balanced": {
                            "type": "string",
                            "default": "fa-scale-balanced",
                            "description": "Icon shown by the performance widget when the balanced profile is active"
                        },
                        "icon_performance": {
                            "type": "string",
                            "default": "fa-rocket",
                            "description": "Icon shown by the performance widget when the performance profile is active"
                        },
                        "icon_off": {
                            "type": "string",
                            "default": "fa-bluetooth",
                            "description": "Icon alias when the bluetooth adapter is powered off (bluetooth widget)"
                        },
                        "icon_on": {
                            "type": "string",
                            "default": "fa-bluetooth",
                            "description": "Icon alias when the bluetooth adapter is powered on (bluetooth widget)"
                        },
                        "icon_connected": {
                            "type": "string",
                            "default": "fa-bluetooth-b",
                            "description": "Icon alias when at least one device is connected (bluetooth widget)"
                        },
                        "emphasis_on_connected": {
                            "type": "boolean",
                            "default": true,
                            "description": "Invert colors while a bluetooth device is connected (bluetooth widget)"
                        },
                        "icon_size": {
                            "type": "integer",
                            "default": 16,
                            "description": "Tray icon size in logical pixels (systray widget)"
                        },
                        "cpu": {
                            "type": "boolean",
                            "default": true,
                            "description": "Show CPU usage (stats widget)"
                        },
                        "ram": {
                            "type": "boolean",
                            "default": true,
                            "description": "Show RAM usage (stats widget)"
                        },
                        "temp": {
                            "type": "boolean",
                            "default": true,
                            "description": "Show core temperatures (stats widget)"
                        },
                        "interface": {
                            "type": "string",
                            "description": "Preferred network interface name (network and wifi widgets), e.g. 'wlp13s0'"
                        },
                        "show_device": {
                            "type": "boolean",
                            "default": false,
                            "description": "Show the device name next to the network/wifi glyph (false = glyph only; the device is always shown in the left-click menu)"
                        },
                        "public_ip": {
                            "type": "boolean",
                            "default": true,
                            "description": "Network widget: the 'External' menu row shows the machine's public IP (fetched via curl, cached 5 minutes). false falls back to the default-route interface's local IP"
                        },
                        "public_ip_url": {
                            "type": "string",
                            "default": "https://icanhazip.com",
                            "description": "Network widget: URL endpoint returning the public IP as plain text (curl -fsS --max-time 3)"
                        },
                        "calendar": {
                            "type": "boolean",
                            "default": true,
                            "description": "Open an interactive calendar popup on left click (datetime widget)"
                        },
                        "first_day": {
                            "type": "string",
                            "enum": ["monday", "sunday"],
                            "default": "monday",
                            "description": "First day of the week in the calendar popup (datetime widget)"
                        },
                        "calendar_launch": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Launch button rendered under the calendar grid (datetime widget), e.g. [\"gsimplecal\"]. Runs with app-id 'flatbar.datetime' so window_rules apply"
                        },
                        "patterns": {
                            "type": "array",
                            "description": "Right-click strftime pattern menu presets (datetime widget). Clicking an entry copies the formatted value",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string", "description": "Menu label" },
                                    "format": { "type": "string", "description": "strftime pattern" }
                                }
                            }
                        },
                        "mounts": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Mount points shown by the disk widget"
                        },
                        "warn_above": {
                            "type": "number",
                            "description": "Used-percentage threshold above which the disk widget is emphasized; also the launch-time 'Low disk space' notification threshold (one notification per mount, at startup only)"
                        },
                        "low_below": {
                            "type": "integer",
                            "description": "Battery percentage below which the widget is emphasized (battery widget)"
                        },
                        "icon_muted": {
                            "type": "string",
                            "description": "Icon alias for 0%/muted volume (command widget volume thresholds)"
                        },
                        "icon_low": {
                            "type": "string",
                            "description": "Icon alias for volume < 35% (command widget)"
                        },
                        "icon_mid": {
                            "type": "string",
                            "description": "Icon alias for volume < 75% (command widget)"
                        },
                        "icon_high": {
                            "type": "string",
                            "description": "Icon alias for volume >= 75% (command widget)"
                        },
                        "mute_command": {
                            "type": "string",
                            "description": "Right-click fallback when right_click is unset (command widget), e.g. 'pamixer -t'"
                        },
                        "icon": {
                            "type": "string",
                            "description": "Default icon alias for command widgets"
                        },
                        "manage_command": {
                            "type": "string",
                            "description": "Bluetooth device-management command launched by the Manage Devices menu entry (default 'bluetui', not shipped — install it yourself or set another device manager; resolved next to the flatbar binary, then via PATH)"
                        },
                        "summary_icon": {
                            "type": "string",
                            "description": "Icon-font alias shown on the bar for the systray widget when no item needs attention (e.g. 'fa-chevron-right')"
                        },
                        "attention_icon": {
                            "type": "string",
                            "description": "Icon-font alias shown on the bar for the systray widget when any item reports NeedsAttention (e.g. 'fa-chevron-down')"
                        },
                        "hidden": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Systray item id/title blacklist"
                        },
                        "only": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Systray item id/title whitelist (empty = show all)"
                        },
                        "tray_poll_fallback_secs": {
                            "type": "integer",
                            "default": 10,
                            "description": "Systray safety-net poll interval in seconds for items that emit no DBus signals (0 = event-driven only)"
                        }
                    }
                }
            },
            "window_rules": {
                "type": "object",
                "description": "Rules for applications launched by Flatbar",
                "properties": {
                    "terminal": {
                        "type": "string",
                        "default": "foot",
                        "description": "Terminal emulator used to host TUI applications (foot, alacritty, kitty, etc.)"
                    }
                },
                "additionalProperties": {
                    "type": "object",
                    "properties": {
                        "mode": {
                            "type": "string",
                            "enum": ["floating", "tiling"]
                        },
                        "size": {
                            "type": "string",
                            "pattern": "^[0-9]+x[0-9]+$",
                            "description": "Window dimensions, e.g. '800x600'"
                        },
                        "position": {
                            "type": "string",
                            "description": "Optional position or coordinate"
                        },
                        "workspace": {
                            "type": "string",
                            "description": "Target workspace assignment"
                        }
                    }
                }
            }
        },
        "required": ["bar"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_schema_validity() {
        let schema = generate_json_schema();
        assert_eq!(
            schema.get("$schema").and_then(|v| v.as_str()),
            Some("http://json-schema.org/draft-07/schema#")
        );
        assert!(schema.get("properties").is_some());
    }
}
