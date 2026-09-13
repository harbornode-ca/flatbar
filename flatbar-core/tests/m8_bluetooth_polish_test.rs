//! M8 Integration tests covering Bluetooth widget, BlueZ parsing, Window Rules, JSON Schema, and SIGHUP reload resilience.

use flatbar_core::config::schema::{generate_json_schema, print_schema_json};
use flatbar_core::config::window_rules::{WindowMode, WindowRule, WindowRulesConfig};
use flatbar_core::config::Config;
use flatbar_core::dbus::bluetooth::parse_bluez_managed_objects;
use flatbar_core::ipc::launch::format_terminal_command;
use flatbar_core::ipc::window_rules::{
    generate_hyprland_rules, generate_niri_snippet, generate_sway_rule,
};
use flatbar_core::shell::build_widgets;
use flatbar_core::widget::bluetooth::{BluetoothConfig, BluetoothWidget};
use flatbar_core::widget::menu::MenuItem;
use flatbar_core::widget::Widget;
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::NamedTempFile;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

#[test]
fn test_bluetooth_bluez_managed_objects_parsing() {
    let mut objects: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> =
        HashMap::new();

    // Adapter 0
    let mut adapter_props = HashMap::new();
    adapter_props.insert(
        "Powered".to_string(),
        OwnedValue::try_from(Value::Bool(true)).unwrap(),
    );
    adapter_props.insert(
        "Alias".to_string(),
        OwnedValue::try_from(Value::Str("Controller0".into())).unwrap(),
    );
    let mut adapter_ifaces = HashMap::new();
    adapter_ifaces.insert("org.bluez.Adapter1".to_string(), adapter_props);
    objects.insert(
        OwnedObjectPath::try_from("/org/bluez/hci0").unwrap(),
        adapter_ifaces,
    );

    // Device: Bose QC45 (Connected with 90% Battery)
    let mut dev_props = HashMap::new();
    dev_props.insert(
        "Name".to_string(),
        OwnedValue::try_from(Value::Str("Bose QuietComfort 45".into())).unwrap(),
    );
    dev_props.insert(
        "Address".to_string(),
        OwnedValue::try_from(Value::Str("04:52:C7:AA:BB:CC".into())).unwrap(),
    );
    dev_props.insert(
        "Connected".to_string(),
        OwnedValue::try_from(Value::Bool(true)).unwrap(),
    );
    dev_props.insert(
        "Paired".to_string(),
        OwnedValue::try_from(Value::Bool(true)).unwrap(),
    );

    let mut bat_props = HashMap::new();
    bat_props.insert(
        "Percentage".to_string(),
        OwnedValue::try_from(Value::U8(90)).unwrap(),
    );

    let mut dev_ifaces = HashMap::new();
    dev_ifaces.insert("org.bluez.Device1".to_string(), dev_props);
    dev_ifaces.insert("org.bluez.Battery1".to_string(), bat_props);
    objects.insert(
        OwnedObjectPath::try_from("/org/bluez/hci0/dev_04_52_C7_AA_BB_CC").unwrap(),
        dev_ifaces,
    );

    let state = parse_bluez_managed_objects(&objects).expect("Valid BlueZ state");
    assert!(state.adapter_present);
    assert!(state.adapter_powered);
    assert_eq!(state.adapter_name.as_deref(), Some("Controller0"));
    assert_eq!(state.devices.len(), 1);

    let dev = &state.devices[0];
    assert_eq!(dev.name, "Bose QuietComfort 45");
    assert!(dev.connected);
    assert_eq!(dev.battery, Some(90));
}

#[test]
fn test_bluetooth_widget_rendering_and_tooltip() {
    let config = BluetoothConfig {
        format: "{icon} {device_name}".to_string(),
        emphasis_on_connected: true,
        ..Default::default()
    };

    let widget = BluetoothWidget::new("bluetooth", config);
    let state = widget.state();

    assert!(!state.spans.is_empty());
    assert!(state.tooltip.is_some());
}

#[test]
fn test_bluetooth_menu_model_structure() {
    let config = BluetoothConfig::default();
    let widget = BluetoothWidget::new("bluetooth", config);
    let menu = widget.build_menu_model();

    assert_eq!(menu.title.as_deref(), Some("Bluetooth"));
    assert!(menu.items.len() >= 3);

    // First item is Manage Devices
    match &menu.items[0] {
        MenuItem::Item { id, label, .. } => {
            assert_eq!(id, "manage_devices");
            assert!(label.contains("Manage Devices"));
        }
        _ => panic!("Expected item for Manage Devices"),
    }

    // Third item (after separator) is power toggle
    match &menu.items[2] {
        MenuItem::Checkbox { id, label, .. } => {
            assert_eq!(id, "toggle_power");
            assert!(label.contains("Bluetooth: Powered"));
        }
        _ => panic!("Expected checkbox for power toggle"),
    }
}

#[test]
fn test_window_rules_configuration_parsing() {
    let toml_str = r##"
        terminal = "alacritty"
        "flatbar.bluetooth" = { size = "800x600", mode = "floating", workspace = "3" }
        "flatbar.network" = { size = "900x700", mode = "floating" }
        "flatbar.stats" = { mode = "tiling" }
    "##;

    let parsed: WindowRulesConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(parsed.terminal, "alacritty");

    let bt_rule = parsed.rules.get("flatbar.bluetooth").unwrap();
    assert_eq!(bt_rule.mode, Some(WindowMode::Floating));
    assert_eq!(bt_rule.parse_size(), Some((800, 600)));
    assert_eq!(bt_rule.workspace.as_deref(), Some("3"));

    let stats_rule = parsed.rules.get("flatbar.stats").unwrap();
    assert_eq!(stats_rule.mode, Some(WindowMode::Tiling));
    assert_eq!(stats_rule.parse_size(), None);
}

#[test]
fn test_compositor_window_rule_generation_sway_hyprland_niri() {
    let rule = WindowRule {
        mode: Some(WindowMode::Floating),
        size: Some("800x600".to_string()),
        position: None,
        workspace: Some("scratch".to_string()),
    };

    // Sway
    let sway_cmd = generate_sway_rule("flatbar.bluetooth", &rule).unwrap();
    assert_eq!(
        sway_cmd,
        "for_window [app_id=\"flatbar.bluetooth\"] floating enable, resize set 800 600, move to workspace \"scratch\""
    );

    // Hyprland
    let hypr_cmds = generate_hyprland_rules("flatbar.bluetooth", &rule);
    assert_eq!(hypr_cmds.len(), 3);
    assert_eq!(
        hypr_cmds[0],
        "dispatch windowrule \"float,^(flatbar\\.bluetooth)$\""
    );
    assert_eq!(
        hypr_cmds[1],
        "dispatch windowrule \"size 800 600,^(flatbar\\.bluetooth)$\""
    );
    assert_eq!(
        hypr_cmds[2],
        "dispatch windowrule \"workspace scratch,^(flatbar\\.bluetooth)$\""
    );

    // Niri
    let mut rules = HashMap::new();
    rules.insert("flatbar.bluetooth".to_string(), rule);
    let niri_snippet = generate_niri_snippet(&rules);
    assert!(niri_snippet.contains("window-rule {"));
    assert!(niri_snippet.contains("match app-id=r#\"^flatbar\\.bluetooth$\"#"));
    assert!(niri_snippet.contains("open-floating true"));
    assert!(niri_snippet.contains("default-floating-size 800 600"));
    assert!(niri_snippet.contains("open-on-workspace \"scratch\""));
}

#[test]
fn test_terminal_launcher_command_formatting() {
    let (prog, args) = format_terminal_command("foot", "flatbar.bluetooth", &["bluetui"]);
    assert_eq!(prog, "foot");
    assert_eq!(args, vec!["-a", "flatbar.bluetooth", "bluetui"]);

    let (prog, args) = format_terminal_command("alacritty", "flatbar.stats", &["btop", "-p", "1"]);
    assert_eq!(prog, "alacritty");
    assert_eq!(
        args,
        vec![
            "--class",
            "flatbar.stats,flatbar.stats",
            "-e",
            "btop",
            "-p",
            "1"
        ]
    );

    let (prog, args) = format_terminal_command("kitty", "flatbar.network", &["nmtui"]);
    assert_eq!(prog, "kitty");
    assert_eq!(args, vec!["--class", "flatbar.network", "nmtui"]);

    let (prog, args) = format_terminal_command("wezterm", "flatbar.stats", &["btop"]);
    assert_eq!(prog, "wezterm");
    assert_eq!(
        args,
        vec!["start", "--class", "flatbar.stats", "--", "btop"]
    );

    let (prog, args) = format_terminal_command("ghostty", "flatbar.bluetooth", &["bluetui"]);
    assert_eq!(prog, "ghostty");
    assert_eq!(
        args,
        vec!["--class-name=flatbar.bluetooth", "-e", "bluetui"]
    );
}

#[test]
fn test_config_json_schema_output() {
    let schema = generate_json_schema();
    assert_eq!(
        schema.get("$schema").and_then(|v| v.as_str()),
        Some("http://json-schema.org/draft-07/schema#")
    );

    let schema_str = print_schema_json();
    assert!(schema_str.contains("\"Flatbar Configuration\""));
    assert!(schema_str.contains("\"window_rules\""));
    assert!(schema_str.contains("\"menu_backend\""));
}

#[test]
fn test_config_builder_with_bluetooth_widget() {
    let toml_str = r##"
        [bar]
        name = "test"
        height = 30
        background = "#1e1e2e"
        foreground = "#cdd6f4"
        layout = { right = ["bluetooth", "battery"] }

        [widgets.bluetooth]
        interval = 10
        format = "{icon} {device_name}"
        emphasis_on_connected = true

        [window_rules]
        terminal = "foot"
        "flatbar.bluetooth" = { size = "800x600", mode = "floating" }
    "##;

    let config = Config::parse_str(toml_str, PathBuf::from("config.toml")).unwrap();
    let widgets = build_widgets(&config);

    assert_eq!(widgets.len(), 2);
    let bt_widget = widgets.iter().find(|w| w.id() == "bluetooth").unwrap();
    assert_eq!(
        bt_widget.update_interval(),
        std::time::Duration::from_secs(10)
    );
}

#[test]
fn test_config_reload_resilience_on_invalid_file() {
    let mut file = NamedTempFile::new().unwrap();
    use std::io::Write;
    writeln!(
        file,
        r##"
        [bar]
        name = "test"
        height = 30
        background = "#1e1e2e"
        foreground = "#cdd6f4"
    "##
    )
    .unwrap();

    let initial_config = Config::load_from_file(file.path()).unwrap();
    assert_eq!(initial_config.bar.height, 30);

    // Overwrite with invalid TOML
    writeln!(file, "invalid syntax [[[}}").unwrap();

    // Reloading bad file returns error without panicking
    let reload_result = Config::load_from_file(file.path());
    assert!(reload_result.is_err());
}
