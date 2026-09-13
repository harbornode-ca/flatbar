use flatbar_core::config::Config;
use flatbar_core::shell::build_widgets;
use flatbar_core::widget::battery::{read_battery_info, BatteryStatus, BatteryWidget};
use flatbar_core::widget::disk::{format_bytes, DiskWidget};
use flatbar_core::widget::network::{detect_network_state, NetworkState};
use flatbar_core::widget::stats::StatsWidget;
use flatbar_core::widget::sysfs::FsPathProvider;
use flatbar_core::widget::wifi::{parse_proc_net_wireless, WifiTier};
use flatbar_core::widget::Widget;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_stats_widget_fixture_pipeline() {
    let dir = tempdir().unwrap();
    let proc_dir = dir.path().join("proc");
    let sys_dir = dir.path().join("sys");
    fs::create_dir_all(&proc_dir).unwrap();
    fs::create_dir_all(&sys_dir).unwrap();

    // Setup initial /proc/stat
    fs::write(proc_dir.join("stat"), "cpu  1000 200 300 5000 100 0 0 0\n").unwrap();

    // Setup /proc/meminfo: 16 GB total, 8 GB available -> 50%
    fs::write(
        proc_dir.join("meminfo"),
        "MemTotal:        16384000 kB\nMemAvailable:     8192000 kB\n",
    )
    .unwrap();

    // Setup hwmon temp sensor
    let hwmon0 = sys_dir.join("class/hwmon/hwmon0");
    fs::create_dir_all(&hwmon0).unwrap();
    fs::write(hwmon0.join("temp1_input"), "48000\n").unwrap(); // 48.0°C

    let fs_prov = FsPathProvider::new(&proc_dir, &sys_dir);
    let widget = StatsWidget::new("stats", 1, true, true, true).with_fs(fs_prov);

    // Initial state sample
    let _s1 = widget.state();

    // Second sample for CPU delta (200 active delta, 300 total delta -> 66.7%)
    fs::write(proc_dir.join("stat"), "cpu  1200 200 300 5100 100 0 0 0\n").unwrap();

    let s2 = widget.state();
    assert_eq!(s2.spans.len(), 8); // 3 items (icon + text each) separated by spaces
    assert!(s2.spans[1].as_text().unwrap().contains('%'));
    assert!(s2.spans[4].as_text().unwrap().contains("50%"));
    assert!(s2.spans[7].as_text().unwrap().contains("48°C"));
}

#[test]
fn test_disk_widget_formatting_and_thresholds() {
    assert_eq!(format_bytes(500 * 1024 * 1024), "0G");
    assert_eq!(format_bytes(50 * 1024 * 1024 * 1024), "50G");
    assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 * 1024), "3.0T");

    let widget = DiskWidget::new("disk", vec!["/".to_string()], 10, Some(80.0));
    let state = widget.state();
    assert!(!state.spans.is_empty());
}

#[test]
fn test_battery_aggregation_and_charging() {
    let dir = tempdir().unwrap();
    let bat0 = dir.path().join("class/power_supply/BAT0");
    let bat1 = dir.path().join("class/power_supply/BAT1");
    fs::create_dir_all(&bat0).unwrap();
    fs::create_dir_all(&bat1).unwrap();

    fs::write(bat0.join("type"), "Battery\n").unwrap();
    fs::write(bat0.join("status"), "Discharging\n").unwrap();
    fs::write(bat0.join("energy_now"), "10000000\n").unwrap();
    fs::write(bat0.join("energy_full"), "50000000\n").unwrap();
    fs::write(bat0.join("power_now"), "10000000\n").unwrap();

    fs::write(bat1.join("type"), "Battery\n").unwrap();
    fs::write(bat1.join("status"), "Charging\n").unwrap();
    fs::write(bat1.join("energy_now"), "40000000\n").unwrap();
    fs::write(bat1.join("energy_full"), "50000000\n").unwrap();

    let fs_prov = FsPathProvider::new(dir.path(), dir.path());
    let info = read_battery_info(&fs_prov).unwrap();

    // 50M / 100M = 50%
    assert_eq!(info.percentage, 50);
    // Charging since bat1 is charging
    assert_eq!(info.status, BatteryStatus::Charging);

    let widget = BatteryWidget::new("bat", 1, Some(20)).with_fs(fs_prov);
    let state = widget.state();
    assert!(!state.spans.is_empty());
    // Charging should trigger emphasis
    assert!(state.spans[0].is_emphasized());
}

#[test]
fn test_network_state_detection() {
    let dir = tempdir().unwrap();
    let proc_net = dir.path().join("proc/net");
    let net_eth = dir.path().join("sys/class/net/eth0");
    let net_lo = dir.path().join("sys/class/net/lo");
    fs::create_dir_all(&proc_net).unwrap();
    fs::create_dir_all(&net_eth).unwrap();
    fs::create_dir_all(&net_lo).unwrap();

    fs::write(net_lo.join("operstate"), "up\n").unwrap();
    fs::write(net_eth.join("operstate"), "down\n").unwrap();

    let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path().join("sys"));
    let (state, _) = detect_network_state(&fs_prov);
    assert_eq!(state, NetworkState::Offline);

    // Turn eth0 up with default route
    fs::write(net_eth.join("operstate"), "up\n").unwrap();
    fs::write(
        proc_net.join("route"),
        "Iface\tDestination\tGateway\tFlags\neth0\t00000000\t0101A8C0\t0003\n",
    )
    .unwrap();

    let (state2, iface) = detect_network_state(&fs_prov);
    assert_eq!(state2, NetworkState::Online);
    assert_eq!(iface, Some("eth0".to_string()));
}

#[test]
fn test_wifi_tiers_and_parsing() {
    let sample = r#"Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
 wlan0: 0000   20.  -80.  -256        0      0      0      0      0        0
"#;
    let parsed = parse_proc_net_wireless(sample);
    assert_eq!(parsed.len(), 1);
    let (iface, sig) = &parsed[0];
    assert_eq!(iface, "wlan0");
    assert_eq!(*sig, 29); // 20/70 = 28.57% -> 29%

    assert_eq!(WifiTier::Weak.icon(), "fa-wifi-weak");
    assert_eq!(WifiTier::Good.icon(), "fa-wifi");
    assert_eq!(WifiTier::Disconnected.icon(), "fa-wifi-slash");
}

#[test]
fn test_build_widgets_from_config() {
    let toml_str = r#"
        [bar]
        name = "test_bar"
        layout = { left = ["stats", "disk"], center = ["datetime"], right = ["battery", "network", "wifi"] }

        [widgets.stats]
        type = "stats"
        interval = 3
        cpu = true
        ram = true
        temp = false

        [widgets.disk]
        type = "disk"
        mounts = ["/", "/home"]
        warn_above = 85.0

        [widgets.battery]
        type = "battery"
        low_below = 20

        [widgets.network]
        type = "network"

        [widgets.wifi]
        type = "wifi"
    "#;

    let config = Config::parse_str(toml_str, PathBuf::from("test.toml")).unwrap();
    let widgets = build_widgets(&config);

    assert_eq!(widgets.len(), 6);
    let ids: Vec<&str> = widgets.iter().map(|w| w.id()).collect();
    assert!(ids.contains(&"stats"));
    assert!(ids.contains(&"disk"));
    assert!(ids.contains(&"datetime"));
    assert!(ids.contains(&"battery"));
    assert!(ids.contains(&"network"));
    assert!(ids.contains(&"wifi"));
}

#[test]
fn test_builtin_widgets_mouse_action_parsing() {
    let toml_str = r#"
        [bar]
        name = "test_actions"
        layout = { left = ["stats", "network"] }

        [widgets.stats]
        type = "stats"
        left_click = "foot -a flatbar.stats btop"
        right_click = "foot -a flatbar.stats htop"

        [widgets.network]
        type = "network"
        left_click = "foot -a flatbar.network nmtui"
    "#;

    let config = Config::parse_str(toml_str, PathBuf::from("actions.toml")).unwrap();
    let widgets = build_widgets(&config);
    assert_eq!(widgets.len(), 2);
}
