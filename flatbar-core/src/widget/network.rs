use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::window_rules::WindowRulesConfig;
use crate::dbus::notify::send_notification;
use crate::ipc::launch::{copy_to_clipboard, launch_tui};
use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::command::{execute_shell_action, MouseActions};
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::sysfs::FsPathProvider;
use crate::widget::{Span, Widget, WidgetState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    Online,
    Offline,
    Down,
}

/// Default provider for the "External" (public) IP row.
pub const DEFAULT_PUBLIC_IP_URL: &str = "https://icanhazip.com";
/// How long a fetched public IP is reused before the next background refresh.
pub const PUBLIC_IP_CACHE_TTL: Duration = Duration::from_secs(300);

/// Query the globally-scoped IP address(es) of an interface via `ip`.
/// Returns the first IPv4 address, falling back to IPv6.
pub fn query_interface_ip(iface: &str) -> Option<String> {
    let output = Command::new("ip")
        .args(["-o", "addr", "show", "dev", iface, "scope", "global"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut first_ipv6: Option<String> = None;

    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        for (idx, field) in fields.iter().enumerate() {
            if *field == "inet" || *field == "inet6" {
                let Some(addr) = fields.get(idx + 1) else {
                    continue;
                };
                let ip = addr.split('/').next().unwrap_or(addr);
                if *field == "inet" {
                    return Some(ip.to_string());
                }
                if first_ipv6.is_none() {
                    first_ipv6 = Some(ip.to_string());
                }
            }
        }
    }

    first_ipv6
}

/// Default notifier: fires `notify-send` from a detached thread so a stuck
/// notification daemon never delays the refresh loop.
fn spawn_send_notification(summary: &str, body: &str, app_name: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    let app_name = app_name.to_string();
    std::thread::Builder::new()
        .name("flatbar-network-notify".to_string())
        .spawn(move || {
            let _ = send_notification(&summary, &body, &app_name);
        })
        .ok();
}

/// Interface holding the default route, from /proc/net/route.
pub fn default_route_interface(fs: &FsPathProvider) -> Option<String> {
    let routes = fs.read_string(&fs.proc_path("net/route"))?;
    for line in routes.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(1) == Some(&"00000000") {
            if let Some(iface) = fields.first() {
                return Some(iface.to_string());
            }
        }
    }
    None
}

/// List non-loopback interfaces from /sys/class/net with their operstate.
pub fn list_network_interfaces(fs: &FsPathProvider) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(fs.sys_path("class/net")) {
        for entry in entries.flatten() {
            let iface = entry.file_name().to_string_lossy().to_string();
            if iface == "lo" {
                continue;
            }
            let operstate = fs
                .read_string(&entry.path().join("operstate"))
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            out.push((iface, operstate));
        }
    }
    out.sort();
    out
}

/// Wireless status for an interface: `Some((ssid, signal_percent))` when known.
fn wireless_status_for(fs: &FsPathProvider, iface: &str) -> Option<(Option<String>, u8)> {
    let content = fs.read_string(&fs.proc_path("net/wireless"))?;
    let mut found = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Inter-") || trimmed.starts_with("face") || !trimmed.contains(':') {
            continue;
        }
        if let Some(rest) = trimmed.split(':').nth(1) {
            if !trimmed.split(':').next()?.trim_start().starts_with(iface) {
                continue;
            }
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if let Some(Ok(q)) = fields
                .first()
                .map(|f| f.trim_end_matches('.').parse::<f64>())
            {
                let percent = if q <= 70.0 {
                    ((q / 70.0) * 100.0).round() as u8
                } else {
                    q.min(100.0).round() as u8
                };
                found = Some(percent);
            }
        }
    }
    let percent = found?;
    Some((crate::widget::wifi::query_ssid(fs, iface), percent))
}

/// Detect network state from sysfs and procfs.
pub fn detect_network_state(fs: &FsPathProvider) -> (NetworkState, Option<String>) {
    detect_network_state_preferred(fs, None)
}

/// Returns true when the interface is a wireless device,
/// detected via the `wireless` or `phy80211` sysfs sentinel files.
pub fn is_wireless_interface(fs: &FsPathProvider, iface: &str) -> bool {
    fs.sys_path(format!("class/net/{iface}/wireless")).is_dir()
        || fs.sys_path(format!("class/net/{iface}/phy80211")).is_dir()
}

/// Detect network state, preferring a configured interface when one is set.
pub fn detect_network_state_preferred(
    fs: &FsPathProvider,
    preferred: Option<&str>,
) -> (NetworkState, Option<String>) {
    if let Some(wanted) = preferred {
        if fs
            .sys_path(format!("class/net/{wanted}/operstate"))
            .exists()
        {
            let state = interface_network_state(fs, wanted);
            return (state, Some(wanted.to_string()));
        }
        tracing::warn!(
            "Network widget: configured interface '{wanted}' not found in /sys/class/net; \
             falling back to auto-detection"
        );
    }

    let net_dir = fs.sys_path("class/net");
    let entries = match fs::read_dir(net_dir) {
        Ok(e) => e,
        Err(_) => return (NetworkState::Down, None),
    };

    // Priority when multiple interfaces are up: the interface carrying the
    // default route, then ethernet (non-wireless), then the first wireless.
    let mut wired_up = None;
    let mut wireless_up = None;

    for entry in entries.flatten() {
        let iface = entry.file_name().to_string_lossy().to_string();
        if iface == "lo" {
            continue;
        }

        let operstate = fs
            .read_string(&entry.path().join("operstate"))
            .unwrap_or_default();
        if !operstate.eq_ignore_ascii_case("up") {
            continue;
        }

        if is_wireless_interface(fs, &iface) {
            wireless_up = wireless_up.or_else(|| Some(iface.clone()));
        } else {
            wired_up = wired_up.or_else(|| Some(iface.clone()));
        }
    }

    let route_iface = default_route_interface(fs).filter(|route| {
        fs.read_string(&fs.sys_path(format!("class/net/{route}/operstate")))
            .map(|s| s.eq_ignore_ascii_case("up"))
            .unwrap_or(false)
    });

    let iface = route_iface.or(wired_up).or(wireless_up);
    let Some(iface) = iface else {
        return (NetworkState::Offline, None);
    };

    let state = interface_network_state(fs, &iface);
    (state, Some(iface))
}

/// Determine online/offline for a single interface via the default route check.
fn interface_network_state(fs: &FsPathProvider, iface: &str) -> NetworkState {
    let _ = iface;
    // Check if default route exists in /proc/net/route
    let route_path = fs.proc_path("net/route");
    let has_default_route = if let Some(routes) = fs.read_string(&route_path) {
        routes.lines().skip(1).any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // Destination is field 1, "00000000" represents default route (0.0.0.0)
            fields.get(1) == Some(&"00000000")
        })
    } else {
        true // If /proc/net/route is unreadable, assume online since interface is UP
    };

    if has_default_route {
        NetworkState::Online
    } else {
        NetworkState::Offline
    }
}

/// Status bar network connection widget.
pub struct NetworkWidget {
    id: String,
    interval: Duration,
    actions: MouseActions,
    window_rules: WindowRulesConfig,
    fs: FsPathProvider,
    preferred_interface: Option<String>,
    show_device: bool,
    /// Whether the External row shows the public IP (default) or the
    /// default-route interface's local IP.
    public_ip: bool,
    public_ip_url: String,
    /// Cached public IP: (fetched_at, ip). Refreshed in the background.
    public_ip_cache: Arc<Mutex<Option<(Instant, String)>>>,
    /// Injectable fetch seam (tests); production shells out to curl.
    public_ip_fetcher: crate::widget::PublicIpFn,
    /// Last connectivity state, for edge-triggered connect/disconnect
    /// notifications. 0 = uninitialized, 1 = Online, 2 = Offline, 3 = Down.
    last_connectivity: AtomicU8,
    /// Notification transport seam (tests); production uses notify-send.
    notifier: crate::widget::NotifyFn,
}

impl NetworkWidget {
    pub fn new(id: impl Into<String>, interval_secs: u64) -> Self {
        Self {
            id: id.into(),
            interval: Duration::from_secs(interval_secs.max(1)),
            actions: MouseActions::default(),
            window_rules: WindowRulesConfig::default(),
            fs: FsPathProvider::default(),
            preferred_interface: None,
            show_device: false,
            public_ip: true,
            public_ip_url: DEFAULT_PUBLIC_IP_URL.to_string(),
            public_ip_cache: Arc::new(Mutex::new(None)),
            public_ip_fetcher: Arc::new(fetch_public_ip_url),
            last_connectivity: AtomicU8::new(0),
            notifier: Arc::new(spawn_send_notification),
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

    pub fn with_public_ip(mut self, enabled: bool, url: Option<String>) -> Self {
        self.public_ip = enabled;
        if let Some(u) = url {
            self.public_ip_url = u;
        }
        self
    }

    /// Test seam: replace the public-IP fetch function.
    #[cfg(test)]
    fn with_public_ip_fetcher(mut self, f: crate::widget::PublicIpFn) -> Self {
        self.public_ip_fetcher = f;
        self
    }

    /// Test seam: replace the notification transport.
    #[cfg(test)]
    fn with_notifier(mut self, f: crate::widget::NotifyFn) -> Self {
        self.notifier = f;
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

/// Plausibility check for a public-IP fetch result: non-empty, short, and
/// either dotted-quad-ish or contains a colon (IPv6).
pub fn plausible_ip(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && t.len() <= 45
        && (t.contains('.') || t.contains(':'))
        && t.chars()
            .all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':')
}

/// Fetch the machine's public IP with `curl` (blocking; run off the main
/// thread). Returns None on any failure or implausible output.
pub fn fetch_public_ip_url(url: &str) -> Option<String> {
    let output = Command::new("curl")
        .args(["-fsS", "--max-time", "3", url])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if plausible_ip(&text) {
        Some(text.trim().to_string())
    } else {
        None
    }
}

/// Shared network menu: one copy-row per interface with IP/SSID, an External
/// (public) IP row, and a Network Manager launcher. `public_ip` supplies the
/// cached "External" value (None ⇒ unavailable row).
pub fn build_network_menu(fs: &FsPathProvider, public_ip: Option<&str>) -> Option<MenuModel> {
    let mut items = Vec::new();

    for (iface, operstate) in list_network_interfaces(fs) {
        if operstate == "up" {
            if let Some((ssid, pct)) = wireless_status_for(fs, &iface) {
                let name = ssid.unwrap_or_else(|| iface.clone());
                items.push(MenuItem::item(
                    format!("iface:{iface}"),
                    format!("{iface}: {name} ({pct}%)"),
                ));
                if let Some(ip) = query_interface_ip(&iface) {
                    items.push(MenuItem::item(
                        format!("copy:{ip}"),
                        format!("  {iface} IP: {ip}"),
                    ));
                }
                continue;
            }

            match query_interface_ip(&iface) {
                Some(ip) => items.push(MenuItem::item(
                    format!("copy:{ip}"),
                    format!("{iface}: {ip}"),
                )),
                None => {
                    items.push(
                        MenuItem::item(format!("iface:{iface}"), format!("{iface}: (no address)"))
                            .disabled(true),
                    );
                }
            }
        } else {
            items.push(
                MenuItem::item(format!("iface:{iface}"), format!("{iface}: Down")).disabled(true),
            );
        }
    }

    match public_ip {
        Some(ip) => items.push(MenuItem::item(
            format!("copy:{ip}"),
            format!("External: {ip}"),
        )),
        None => items.push(
            MenuItem::item("copy-unavailable-external", "External: unavailable").disabled(true),
        ),
    }

    items.push(MenuItem::Separator);
    items.push(MenuItem::item_with_icon(
        "open-network-manager",
        "Network Manager",
        "fa-gear",
    ));

    if items.len() == 2 {
        return None; // no interfaces and no NM entry beyond separator
    }
    Some(MenuModel::new(items).with_title("Network"))
}

impl NetworkWidget {
    /// Return the cached public IP if fresh; otherwise kick off a background
    /// refresh and return the stale value (if any) so the menu still shows
    /// something while offline-ish.
    fn external_ip(&self) -> Option<String> {
        if !self.public_ip {
            let route = default_route_interface(&self.fs);
            return route.and_then(|ref i| query_interface_ip(i));
        }

        {
            let cache = self.public_ip_cache.lock().unwrap();
            if let Some((at, ip)) = cache.as_ref() {
                if at.elapsed() < PUBLIC_IP_CACHE_TTL {
                    return Some(ip.clone());
                }
            }
        }

        let cache = Arc::clone(&self.public_ip_cache);
        let fetcher = Arc::clone(&self.public_ip_fetcher);
        let url = self.public_ip_url.clone();
        std::thread::Builder::new()
            .name("flatbar-public-ip-fetch".to_string())
            .spawn(move || {
                if let Some(ip) = fetcher(url.trim()) {
                    let mut lock = cache.lock().unwrap();
                    *lock = Some((Instant::now(), ip));
                }
            })
            .ok();

        self.public_ip_cache
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, ip)| ip.clone())
    }

    /// Edge-triggered connectivity notifications: Online → anything else
    /// fires "disconnected"; anything → Online fires "reconnected". The
    /// first evaluation only records the baseline.
    fn notify_connectivity_change(&self, state: NetworkState, iface: Option<&str>) {
        const ONLINE: u8 = 1;
        const OFFLINE: u8 = 2;
        const DOWN: u8 = 3;
        let code = match state {
            NetworkState::Online => ONLINE,
            NetworkState::Offline => OFFLINE,
            NetworkState::Down => DOWN,
        };

        let prev_code = self.last_connectivity.swap(code, Ordering::SeqCst);
        if prev_code == 0 || state == NetworkState::Down && prev_code == OFFLINE {
            return;
        }
        if state == NetworkState::Online && prev_code == ONLINE {
            return;
        }

        let device = iface.unwrap_or("the network interface");
        let (summary, body) = if state == NetworkState::Online {
            ("Internet reconnected", format!("{device} is back online"))
        } else {
            (
                "Internet disconnected",
                format!("{device} is no longer online"),
            )
        };
        let notifier = Arc::clone(&self.notifier);
        std::thread::Builder::new()
            .name("flatbar-network-connectivity-notify".to_string())
            .spawn(move || notifier(summary, &body, "flatbar.network"))
            .ok();
    }
}

impl Widget for NetworkWidget {
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
        let external = self.external_ip();
        build_network_menu(&self.fs, external.as_deref())
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
        let preferred = self.preferred_interface.as_deref();
        let (state, iface) = detect_network_state_preferred(&self.fs, preferred);
        self.notify_connectivity_change(state, iface.as_deref());

        let wireless = iface
            .as_deref()
            .map(|name| is_wireless_interface(&self.fs, name))
            .unwrap_or(false);
        // Issue 10: X glyph when no device is connected.
        let icon = match state {
            NetworkState::Online => {
                if wireless {
                    "fa-wifi"
                } else {
                    "fa-network-wired"
                }
            }
            _ => "fa-xmark",
        };

        let text = match state {
            NetworkState::Online => {
                if self.show_device {
                    format!(" {}", iface.unwrap_or_else(|| "eth".to_string()))
                } else {
                    String::new()
                }
            }
            NetworkState::Offline => " offline".to_string(),
            NetworkState::Down => " down".to_string(),
        };

        let mut spans = vec![Span::icon(icon)];
        if !text.is_empty() {
            spans.push(Span::text(text));
        }

        WidgetState {
            spans,
            tooltip: Some(format!("State: {:?}", state)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_network_state_online_fixture() {
        let dir = tempdir().unwrap();
        let net_dir = dir.path().join("class/net/eth0");
        let proc_net = dir.path().join("proc/net");
        fs::create_dir_all(&net_dir).unwrap();
        fs::create_dir_all(&proc_net).unwrap();

        fs::write(net_dir.join("operstate"), "up\n").unwrap();
        fs::write(
            proc_net.join("route"),
            "Iface\tDestination\tGateway\tFlags\neth0\t00000000\t0101A8C0\t0003\n",
        )
        .unwrap();

        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path());
        let (state, iface) = detect_network_state(&fs_prov);
        assert_eq!(state, NetworkState::Online);
        assert_eq!(iface, Some("eth0".to_string()));
    }

    #[test]
    fn test_network_preferred_interface_used() {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        fs::create_dir_all(&proc_net).unwrap();
        fs::write(
            proc_net.join("route"),
            "Iface\tDestination\tGateway\tFlags\nwlp1s0\t00000000\t0101A8C0\t0003\n",
        )
        .unwrap();

        let wired = dir.path().join("class/net/eth0");
        let wireless = dir.path().join("class/net/wlp1s0");
        fs::create_dir_all(&wired).unwrap();
        fs::create_dir_all(&wireless).unwrap();
        fs::write(wired.join("operstate"), "up\n").unwrap();
        fs::write(wireless.join("operstate"), "up\n").unwrap();
        fs::create_dir_all(wireless.join("wireless")).unwrap();

        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path());
        let (state, iface) = detect_network_state_preferred(&fs_prov, Some("wlp1s0"));
        assert_eq!(state, NetworkState::Online);
        assert_eq!(iface, Some("wlp1s0".to_string()));
        assert!(is_wireless_interface(&fs_prov, "wlp1s0"));
        assert!(!is_wireless_interface(&fs_prov, "eth0"));

        let widget = NetworkWidget::new("net", 3)
            .with_fs(fs_prov.clone())
            .with_interface(Some("wlp1s0".to_string()))
            .with_show_device(true);
        let state = widget.state();
        // Icon must be the wireless one (spans: icon first)
        assert_eq!(state.spans[0].as_icon(), Some("fa-wifi"));
        assert_eq!(state.spans[1].as_text(), Some(" wlp1s0"));

        // Default (show_device off): glyph only — the device name lives in
        // the left-click menu.
        let compact = NetworkWidget::new("net", 3)
            .with_fs(fs_prov.clone())
            .with_interface(Some("wlp1s0".to_string()));
        let compact_state = compact.state();
        assert_eq!(compact_state.spans.len(), 1);
        assert_eq!(compact_state.spans[0].as_icon(), Some("fa-wifi"));
    }

    #[test]
    fn test_network_preferred_interface_missing_falls_back() {
        let dir = tempdir().unwrap();
        let net_dir = dir.path().join("class/net/eth0");
        let proc_net = dir.path().join("proc/net");
        fs::create_dir_all(&net_dir).unwrap();
        fs::create_dir_all(&proc_net).unwrap();

        fs::write(net_dir.join("operstate"), "up\n").unwrap();
        fs::write(
            proc_net.join("route"),
            "Iface\tDestination\tGateway\tFlags\neth0\t00000000\t0101A8C0\t0003\n",
        )
        .unwrap();

        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path());
        let (state, iface) = detect_network_state_preferred(&fs_prov, Some("wlan9"));
        assert_eq!(state, NetworkState::Online);
        assert_eq!(iface, Some("eth0".to_string()));
    }

    fn fixture_with(
        ifaces: &[(&str, &str, bool)], // (name, operstate, wireless)
        route_line: Option<&str>,
    ) -> (FsPathProvider, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        fs::create_dir_all(&proc_net).unwrap();
        let mut route = String::from("Iface\tDestination\tGateway\tFlags\n");
        if let Some(line) = route_line {
            route.push_str(line);
        }
        fs::write(proc_net.join("route"), route).unwrap();

        for (name, operstate, wireless) in ifaces {
            let net = dir.path().join(format!("class/net/{name}"));
            fs::create_dir_all(&net).unwrap();
            fs::write(net.join("operstate"), format!("{operstate}\n")).unwrap();
            if *wireless {
                fs::create_dir_all(net.join("wireless")).unwrap();
            }
        }
        (
            FsPathProvider::new(dir.path().join("proc"), dir.path()),
            dir,
        )
    }

    #[test]
    fn test_default_route_interface_wins_over_wired_and_wireless() {
        let (fs_prov, _dir) = fixture_with(
            &[
                ("eth0", "up", false),
                ("wlan0", "up", true),
                ("ens3", "up", false),
            ],
            Some("ens3\t00000000\t0101A8C0\t0003\n"),
        );
        let (_state, iface) = detect_network_state_preferred(&fs_prov, None);
        assert_eq!(iface, Some("ens3".to_string()));
    }

    #[test]
    fn test_ethernet_preferred_when_wired_and_wireless_both_up() {
        let (fs_prov, _dir) = fixture_with(
            &[("wlan0", "up", true), ("eth0", "up", false)],
            Some("eth0\t00000000\t0101A8C0\t0003\n"),
        );
        // The default-route interface (eth0) wins; even without a default
        // route the wired interface beats wireless.
        let (_state, iface) = detect_network_state_preferred(&fs_prov, None);
        assert_eq!(iface, Some("eth0".to_string()));

        let (empty_route, _dir) =
            fixture_with(&[("wlan0", "up", true), ("eth0", "up", false)], Some(""));
        let (_state, iface) = detect_network_state_preferred(&empty_route, None);
        assert_eq!(iface, Some("eth0".to_string()));
    }

    #[test]
    fn test_stale_default_route_is_ignored() {
        // /proc/net/route still points at wlan0, but wlan0 is down: the
        // up ethernet interface must be shown.
        let (fs_prov, _dir) = fixture_with(
            &[("wlan0", "down", true), ("eth0", "up", false)],
            Some("wlan0\t00000000\t0101A8C0\t0003\n"),
        );
        let (_state, iface) = detect_network_state_preferred(&fs_prov, None);
        assert_eq!(iface, Some("eth0".to_string()));
    }
}

// Issue 6 tests: interface menu rows, X icon, show_device spans.
#[cfg(test)]
mod menu_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_network_menu_down_rows() {
        let dir = tempdir().unwrap();
        let net0 = dir.path().join("class/net/eth0");
        let net1 = dir.path().join("class/net/wlan0");
        fs::create_dir_all(&net0).unwrap();
        fs::create_dir_all(&net1).unwrap();
        fs::write(net0.join("operstate"), "down\n").unwrap();
        fs::write(net1.join("operstate"), "down\n").unwrap();

        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path());
        let menu = build_network_menu(&fs_prov, None).unwrap();
        let rows: Vec<(&str, &str)> = menu
            .items
            .iter()
            .filter(|i| i.id().is_some())
            .map(|i| (i.id().unwrap(), i.label().unwrap()))
            .collect();
        assert!(rows.contains(&("iface:eth0", "eth0: Down")));
        assert!(rows.contains(&("iface:wlan0", "wlan0: Down")));
        // Down rows are disabled (no IP to copy).
        let down = menu
            .items
            .iter()
            .find(|i| i.id() == Some("iface:eth0"))
            .unwrap();
        assert!(!down.is_enabled());
        // Network Manager entry is present.
        assert!(rows.iter().any(|(id, _)| *id == "open-network-manager"));
    }

    #[test]
    fn test_x_icon_when_no_connected_device() {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        fs::create_dir_all(dir.path().join("class/net")).unwrap();
        fs::create_dir_all(&proc_net).unwrap();

        let fs_prov = FsPathProvider::new(&proc_net, dir.path());
        let widget = NetworkWidget::new("net", 3).with_fs(fs_prov);
        let state = widget.state();
        assert_eq!(state.spans[0].as_icon(), Some("fa-xmark"));
    }

    #[test]
    fn test_show_device_false_renders_glyph_only() {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        fs::create_dir_all(dir.path().join("class/net/eth0")).unwrap();
        fs::create_dir_all(&proc_net).unwrap();
        fs::write(dir.path().join("class/net/eth0/operstate"), "up\n").unwrap();

        let fs_prov = FsPathProvider::new(&proc_net, dir.path());
        let widget = NetworkWidget::new("net", 3)
            .with_fs(fs_prov)
            .with_show_device(false);
        let state = widget.state();
        assert_eq!(state.spans.len(), 1); // glyph only
    }
}

/// Connectivity-edge notification + public-IP cache tests. The notifier and
/// fetcher are replaced with test doubles; state transitions are driven by
/// calling the internal helpers directly (same-module privacy).
#[cfg(test)]
mod connectivity_tests {
    use super::*;
    use std::sync::mpsc;

    fn edge_notifier() -> (crate::widget::NotifyFn, mpsc::Receiver<(String, String)>) {
        let (tx, rx) = mpsc::channel();
        (
            Arc::new(move |summary: &str, body: &str, _: &str| {
                let _ = tx.send((summary.to_string(), body.to_string()));
            }),
            rx,
        )
    }

    fn collect<T>(rx: &mut mpsc::Receiver<T>) -> Vec<T> {
        let mut out = Vec::new();
        while let Ok(item) = rx.try_recv() {
            out.push(item);
        }
        out
    }

    #[test]
    fn test_first_call_records_baseline_silently() {
        let (notifier, mut rx) = edge_notifier();
        let widget = NetworkWidget::new("net", 3).with_notifier(notifier);
        widget.notify_connectivity_change(NetworkState::Online, Some("eth0"));
        assert_eq!(widget.last_connectivity.load(Ordering::SeqCst), 1);
        assert!(collect(&mut rx).is_empty());
    }

    #[test]
    fn test_online_to_offline_notifies_once() {
        let (notifier, mut rx) = edge_notifier();
        let widget = NetworkWidget::new("net", 3).with_notifier(notifier);

        widget.notify_connectivity_change(NetworkState::Online, Some("eth0"));
        assert!(collect(&mut rx).is_empty());

        widget.notify_connectivity_change(NetworkState::Online, Some("eth0"));
        assert!(collect(&mut rx).is_empty());

        widget.notify_connectivity_change(NetworkState::Offline, Some("wlan0"));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            collect(&mut rx),
            vec![(
                "Internet disconnected".to_string(),
                "wlan0 is no longer online".to_string()
            )]
        );
        assert_eq!(widget.last_connectivity.load(Ordering::SeqCst), 2);

        widget.notify_connectivity_change(NetworkState::Down, Some("wlan0"));
        std::thread::sleep(Duration::from_millis(100));
        // Offline ↔ Down shuffles do not resend: only Online-boundary
        // transitions notify.
        assert!(collect(&mut rx).is_empty());
        assert_eq!(widget.last_connectivity.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_recovery_notifies_reconnect() {
        let (notifier, mut rx) = edge_notifier();
        let widget = NetworkWidget::new("net", 3).with_notifier(notifier);

        widget.notify_connectivity_change(NetworkState::Offline, None);
        widget.notify_connectivity_change(NetworkState::Online, Some("eth0"));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            collect(&mut rx),
            vec![(
                "Internet reconnected".to_string(),
                "eth0 is back online".to_string()
            )]
        );

        widget.notify_connectivity_change(NetworkState::Online, Some("eth0"));
        std::thread::sleep(Duration::from_millis(100));
        assert!(collect(&mut rx).is_empty());
    }

    #[test]
    fn test_plausible_ip_filter() {
        assert!(plausible_ip("93.184.216.34"));
        assert!(plausible_ip("2606:2800:220:1:248:1893:25c8:1946\n"));
        assert!(!plausible_ip(""));
        assert!(!plausible_ip("<html>gateway login</html>"));
        assert!(!plausible_ip(
            "93.184.216.34.93.184.216.34.93.184.216.34.93.184.216.34x"
        ));
    }
}

/// Public-IP cache behavior: fresh hit, stale miss triggers refresh, and the
/// cached value survives into the next call.
#[cfg(test)]
mod public_ip_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tempfile::tempdir;

    #[test]
    fn test_public_ip_cached_and_refreshed_in_background() {
        let dir = tempdir().unwrap();
        let fs_prov = FsPathProvider::new(dir.path().join("proc"), dir.path());

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_tx = Arc::clone(&calls);
        let fetcher = Arc::new(move |_url: &str| {
            let _ = calls_tx.fetch_add(1, Ordering::SeqCst);
            Some("203.0.113.7".to_string())
        });

        // First call fetches (fresh cache is None); the fetcher writes the
        // result into the cache from its background thread.
        let widget = NetworkWidget::new("net", 3)
            .with_fs(fs_prov)
            .with_public_ip_fetcher(fetcher);

        // First call: no cache → background fetch; returns None-ish stale
        // (None) but stores the result a moment later.
        assert_eq!(widget.external_ip(), None);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(widget.external_ip(), Some("203.0.113.7".to_string()));
        assert_eq!(widget.external_ip(), Some("203.0.113.7".to_string())); // cache hit
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_public_ip_disabled_uses_default_route() {
        let dir = tempdir().unwrap();
        let proc_net = dir.path().join("proc/net");
        fs::create_dir_all(&proc_net).unwrap();
        fs::write(
            proc_net.join("route"),
            "Iface\tDestination\tGateway\tFlags\neth1\t00000000\t0101A8C0\t0003\n",
        )
        .unwrap();

        let fetcher_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&fetcher_calls);
        let fetcher = Arc::new(move |_: &str| {
            let _ = calls.fetch_add(1, Ordering::SeqCst);
            Some("999.9.9.9".to_string())
        });

        let widget = NetworkWidget::new("net", 3)
            .with_fs(FsPathProvider::new(dir.path().join("proc"), dir.path()))
            .with_public_ip(false, None)
            .with_public_ip_fetcher(fetcher);

        // With public_ip disabled: never fetch; the route lookup yields None
        // here (no 'ip' output possible in fixtures) rather than the fake.
        assert_eq!(widget.external_ip(), None);
        assert_eq!(fetcher_calls.load(Ordering::SeqCst), 0);
    }
}
