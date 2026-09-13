use std::ffi::CString;
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::WindowRulesConfig;
use crate::ipc::launch_gui;
use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::command::{execute_shell_action, MouseActions};
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::{Span, Widget, WidgetState};

/// Disk space info for a mount point.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskMountInfo {
    pub mount_point: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: f64,
}

/// Unit system for human-readable byte formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskUnits {
    /// Largest binary unit that fits each value (default).
    Auto,
    MB,
    GB,
    TB,
    MiB,
    GiB,
    TiB,
}

impl DiskUnits {
    /// Parse a `units` config string. Unknown values fall back to `Auto`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "mb" => Self::MB,
            "gb" => Self::GB,
            "tb" => Self::TB,
            "mib" => Self::MiB,
            "gib" => Self::GiB,
            "tib" => Self::TiB,
            _ => Self::Auto,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::MB => "MB",
            Self::GB => "GB",
            Self::TB => "TB",
            Self::MiB => "MiB",
            Self::GiB => "GiB",
            Self::TiB => "TiB",
        }
    }
}

/// Query filesystem stats using libc::statvfs.
pub fn query_statvfs(path: &str) -> Option<DiskMountInfo> {
    unsafe {
        let c_path = CString::new(path).ok()?;
        let mut stat: libc::statvfs = mem::zeroed();
        let ret = libc::statvfs(c_path.as_ptr(), &mut stat);

        if ret != 0 {
            return None;
        }

        let frsize = stat.f_frsize as u64;
        let total_bytes = stat.f_blocks as u64 * frsize;
        let free_bytes = stat.f_bavail as u64 * frsize;
        let used_bytes = total_bytes.saturating_sub(free_bytes);

        let used_percent = if total_bytes > 0 {
            (used_bytes as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };

        Some(DiskMountInfo {
            mount_point: path.to_string(),
            total_bytes,
            free_bytes,
            used_bytes,
            used_percent,
        })
    }
}

/// Format bytes into human-readable binary units (GiB / TiB), picking the
/// largest unit that fits each value.
pub fn format_bytes(bytes: u64) -> String {
    format_bytes_with_units(bytes, DiskUnits::Auto)
}

/// Format bytes in the requested unit system. `Auto` picks the largest unit
/// that fits; explicit units force one scale for every value.
pub fn format_bytes_with_units(bytes: u64, units: DiskUnits) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1000.0 * 1000.0;
    const GB: f64 = 1000.0 * 1000.0 * 1000.0;
    const TB: f64 = 1000.0 * 1000.0 * 1000.0 * 1000.0;

    match units {
        DiskUnits::Auto => {
            if bytes as f64 >= TIB {
                format!("{:.1}T", bytes as f64 / TIB)
            } else {
                format!("{:.0}G", bytes as f64 / GIB)
            }
        }
        DiskUnits::MB => format!("{:.0}MB", bytes as f64 / MB),
        DiskUnits::GB => format!("{:.1}GB", bytes as f64 / GB),
        DiskUnits::TB => format!("{:.2}TB", bytes as f64 / TB),
        DiskUnits::MiB => format!("{:.0}MiB", bytes as f64 / MIB),
        DiskUnits::GiB => format!("{:.1}GiB", bytes as f64 / GIB),
        DiskUnits::TiB => format!("{:.2}TiB", bytes as f64 / TIB),
    }
}

/// Human-friendly display name for a mount point: `/` → `root`, the user
/// home directory (incl. `/home/*` paths) → `home`.
pub fn display_mount_name(mount: &str) -> String {
    if mount == "/" {
        return "root".to_string();
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty()
            && (mount == home
                || mount
                    .strip_prefix(&home)
                    .is_some_and(|s| s.starts_with('/')))
        {
            return "home".to_string();
        }
    }
    if mount.starts_with("/home/") {
        return "home".to_string();
    }
    mount.to_string()
}

/// Build the storage menu: one row per mount with used/total/free sizes.
/// Public + data-driven so tests can feed synthetic statvfs results.
pub fn build_storage_menu(infos: &[(String, DiskMountInfo)], units: DiskUnits) -> MenuModel {
    let mut items = Vec::new();
    for (mount, info) in infos {
        let name = display_mount_name(mount);
        let label = format!(
            "{}: {}/{} · free: {}",
            name,
            format_bytes_with_units(info.used_bytes, units),
            format_bytes_with_units(info.total_bytes, units),
            format_bytes_with_units(info.free_bytes, units),
        );
        // No-op action id: clicking a row just closes the menu.
        items.push(MenuItem::item(format!("mount:{mount}"), label));
    }
    MenuModel::new(items).with_title("Storage")
}

/// Pure helper: per-mount low-space notification bodies for mounts whose used
/// percentage is at or above `warn_above`. Data-driven so tests can feed
/// synthetic statvfs results. One notification is produced per low mount.
pub fn low_space_bodies(
    infos: &[(String, DiskMountInfo)],
    warn_above: f64,
    units: DiskUnits,
) -> Vec<String> {
    infos
        .iter()
        .filter(|(_, info)| info.used_percent >= warn_above)
        .map(|(mount, info)| {
            format!(
                "{}: {} free ({}% used, {} total)",
                display_mount_name(mount),
                format_bytes_with_units(info.free_bytes, units),
                info.used_percent.round(),
                format_bytes_with_units(info.total_bytes, units),
            )
        })
        .collect()
}

/// Default notifier: fires `notify-send` from a detached thread so a stuck
/// notification daemon never delays the render path.
fn spawn_notify(summary: &str, body: &str, app_name: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    let app_name = app_name.to_string();
    std::thread::Builder::new()
        .name("flatbar-storage-notify".to_string())
        .spawn(move || {
            let _ = crate::dbus::notify::send_notification(&summary, &body, &app_name);
        })
        .ok();
}

/// Disk freespace monitoring widget: a drive icon on the bar; left-click opens
/// a menu listing configured mount points with used/total/free sizes;
/// right-click opens a file manager.
pub struct DiskWidget {
    id: String,
    mounts: Vec<String>,
    interval: Duration,
    warn_above: f64,
    units: DiskUnits,
    actions: MouseActions,
    window_rules: WindowRulesConfig,
    file_manager: String,
    low_space_notified: AtomicBool,
    notification_count: Arc<AtomicUsize>,
    notifier: crate::widget::NotifyFn,
}

impl DiskWidget {
    pub fn new(
        id: impl Into<String>,
        mounts: Vec<String>,
        interval_secs: u64,
        warn_above: Option<f64>,
    ) -> Self {
        let mounts = if mounts.is_empty() {
            vec!["/".to_string()]
        } else {
            mounts
        };

        Self {
            id: id.into(),
            mounts,
            interval: Duration::from_secs(interval_secs.max(1)),
            warn_above: warn_above.unwrap_or(90.0),
            units: DiskUnits::Auto,
            actions: MouseActions::default(),
            window_rules: WindowRulesConfig::default(),
            file_manager: "rc".to_string(),
            low_space_notified: AtomicBool::new(false),
            notification_count: Arc::new(AtomicUsize::new(0)),
            notifier: Arc::new(spawn_notify),
        }
    }

    pub fn with_actions(mut self, actions: MouseActions) -> Self {
        self.actions = actions;
        self
    }

    pub fn with_units(mut self, units: DiskUnits) -> Self {
        self.units = units;
        self
    }

    pub fn with_window_rules(mut self, rules: WindowRulesConfig) -> Self {
        self.window_rules = rules;
        self
    }

    pub fn with_file_manager(mut self, command: impl Into<String>) -> Self {
        self.file_manager = command.into();
        self
    }

    /// Test seam: replace the notification transport.
    #[cfg(test)]
    fn with_notifier(mut self, f: crate::widget::NotifyFn) -> Self {
        self.notifier = f;
        self
    }
}

impl Widget for DiskWidget {
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
        // Left-click opens the storage menu via popup_request().
    }

    fn get_menu(&self) -> Option<MenuModel> {
        let mut infos = Vec::new();
        for mount in &self.mounts {
            if let Some(info) = query_statvfs(mount) {
                infos.push((mount.clone(), info));
            }
        }
        if infos.is_empty() {
            return None;
        }
        Some(build_storage_menu(&infos, self.units))
    }

    fn right_click_launch(&self) -> bool {
        let argv: Vec<String> = self
            .file_manager
            .split_whitespace()
            .map(String::from)
            .collect();
        if argv.is_empty() {
            return false;
        }
        let argv: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        launch_gui("flatbar.disk", &argv, &self.window_rules);
        true
    }

    fn state(&self) -> WidgetState {
        let mut spans = Vec::new();
        let mut any_emphasized = false;

        let mut low: Vec<(String, DiskMountInfo)> = Vec::new();
        for mount in &self.mounts {
            if let Some(info) = query_statvfs(mount) {
                if info.used_percent >= self.warn_above {
                    any_emphasized = true;
                    low.push((mount.clone(), info.clone()));
                }
            }
        }

        // Launch-only alert: on the first evaluation, one notification per
        // low mount. The latch never resets, so later threshold crossings
        // are shown by icon emphasis only.
        if !low.is_empty() && !self.low_space_notified.swap(true, Ordering::SeqCst) {
            let bodies = low_space_bodies(&low, self.warn_above, self.units);
            let notifier = Arc::clone(&self.notifier);
            let notification_count = Arc::clone(&self.notification_count);
            std::thread::Builder::new()
                .name("flatbar-storage-low-notify".to_string())
                .spawn(move || {
                    for body in bodies {
                        notification_count.fetch_add(1, Ordering::SeqCst);
                        notifier("Low disk space", &body, "flatbar.storage");
                    }
                })
                .ok();
        }

        if self.mounts.is_empty() {
            return WidgetState {
                spans,
                tooltip: None,
            };
        }

        if any_emphasized {
            spans.push(Span::emphasized_icon("fa-hard-drive"));
        } else {
            spans.push(Span::icon("fa-hard-drive"));
        }

        WidgetState {
            spans,
            tooltip: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_statvfs_root() {
        let info = query_statvfs("/").expect("statvfs on / should succeed");
        assert!(info.total_bytes > 0);
        assert!(info.used_percent >= 0.0 && info.used_percent <= 100.0);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(10 * 1024 * 1024 * 1024), "10G");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024 * 1024), "2.0T");
    }

    #[test]
    fn test_format_bytes_units() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(format_bytes_with_units(10 * gib, DiskUnits::GiB), "10.0GiB");
        assert_eq!(format_bytes_with_units(10 * gib, DiskUnits::GB), "10.7GB");
        assert_eq!(format_bytes_with_units(10 * gib, DiskUnits::Auto), "10G");
        assert_eq!(
            format_bytes_with_units(2 * 1024 * gib, DiskUnits::TiB),
            "2.00TiB"
        );
        assert_eq!(format_bytes_with_units(2 * gib, DiskUnits::TiB), "0.00TiB");
        assert_eq!(format_bytes_with_units(0, DiskUnits::MiB), "0MiB");
    }

    #[test]
    fn test_units_parse() {
        assert_eq!(DiskUnits::parse("GiB"), DiskUnits::GiB);
        assert_eq!(DiskUnits::parse("gib"), DiskUnits::GiB);
        assert_eq!(DiskUnits::parse(" TB "), DiskUnits::TB);
        assert_eq!(DiskUnits::parse("nonsense"), DiskUnits::Auto);
        assert_eq!(DiskUnits::parse("auto"), DiskUnits::Auto);
    }

    #[test]
    fn test_state_is_icon_only() {
        let widget = DiskWidget::new("disk", vec!["/".to_string()], 30, None);
        let state = widget.state();
        assert_eq!(state.spans.len(), 1);
        assert_eq!(state.spans[0].as_icon(), Some("fa-hard-drive"));
        assert!(state.spans.iter().all(|s| s.as_text().is_none()));
    }

    #[test]
    fn test_menu_lists_mounts_with_sizes() {
        let gib = 1024u64 * 1024 * 1024;
        let infos = vec![(
            "/".to_string(),
            DiskMountInfo {
                mount_point: "/".to_string(),
                total_bytes: 100 * gib,
                free_bytes: 75 * gib,
                used_bytes: 25 * gib,
                used_percent: 25.0,
            },
        )];

        let menu = build_storage_menu(&infos, DiskUnits::GiB);
        assert_eq!(menu.title.as_deref(), Some("Storage"));
        assert_eq!(menu.items.len(), 1);
        let label = menu.items[0].label().unwrap();
        assert!(label.starts_with("root: "));
        assert!(label.contains("25.0GiB/100.0GiB"));
        assert!(label.contains("free: 75.0GiB"));
    }

    #[test]
    fn test_menu_empty_when_no_mounts_stat() {
        let widget = DiskWidget::new(
            "disk",
            vec!["/definitely/not/a/mount".to_string()],
            30,
            None,
        );
        // statvfs fails on nonexistent paths; menu should be absent.
        if query_statvfs("/definitely/not/a/mount").is_none() {
            assert!(widget.get_menu().is_none());
        }
    }

    fn mount_info(mount: &str, used_percent: f64) -> (String, DiskMountInfo) {
        (
            mount.to_string(),
            DiskMountInfo {
                mount_point: mount.to_string(),
                total_bytes: 100 * 1024 * 1024 * 1024,
                free_bytes: 1024 * 1024 * 1024,
                used_bytes: 99 * 1024 * 1024 * 1024,
                used_percent,
            },
        )
    }

    #[test]
    fn test_low_space_bodies_one_per_low_mount() {
        let low_root = mount_info("/", 95.0);
        let low_home = mount_info("/home/kevin", 91.0);
        let healthy = mount_info("/data", 30.0);
        let infos = vec![low_root, low_home, healthy];

        let bodies = low_space_bodies(&infos, 90.0, DiskUnits::GiB);
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].starts_with("root: "));
        assert!(bodies[0].contains("(95% used, 100.0GiB total)"));
        assert!(bodies[1].starts_with("home: "));

        let no_low = low_space_bodies(&[mount_info("/data", 89.9)], 90.0, DiskUnits::GiB);
        assert!(no_low.is_empty());
    }

    #[test]
    fn test_low_space_body_threshold_equality_triggers() {
        let infos = vec![mount_info("/", 90.0)];
        assert!(!low_space_bodies(&infos, 90.0, DiskUnits::GiB).is_empty());
    }

    #[test]
    fn test_low_space_notify_is_launch_only_latch() {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();
        let notifier = Arc::new(move |_: &str, body: &str, _: &str| {
            let _ = tx.send(body.to_string());
        });

        // Real statvfs on the real root filesystem: high threshold (0%) makes
        // every readable mount "low", guaranteeing a notification.
        let widget =
            DiskWidget::new("disk", vec!["/".to_string()], 30, Some(0.0)).with_notifier(notifier);

        widget.state();
        widget.state();
        widget.state();
        // The notify fan-out runs on its own thread; give it a beat.
        std::thread::sleep(Duration::from_millis(150));

        // First state() issued exactly one "/ ..." body; later calls re-issued
        // nothing (latch fired once, launch-only).
        let bodies: Vec<String> = rx.try_iter().collect();
        assert_eq!(
            bodies.len(),
            1,
            "expected exactly one launch-time notification, got {bodies:?}"
        );
        assert!(bodies[0].starts_with("root: "));
    }

    #[test]
    fn test_no_notify_when_devices_healthy_halfway_threshold(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::mpsc;
        let (tx_rx, rx) = mpsc::channel();
        let notifier = Arc::new(move |_: &str, _: &str, _: &str| {
            let _ = tx_rx.send(());
        });
        // unrealistically high threshold for a healthy system's usage
        let widget =
            DiskWidget::new("disk", vec!["/".to_string()], 30, Some(200.0)).with_notifier(notifier);
        let _ = widget.state();
        let _ = widget.state();
        assert!(rx.try_iter().next().is_none());
        Ok(())
    }
}
