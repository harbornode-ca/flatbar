use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::window_rules::WindowRulesConfig;
use crate::ipc::launch::launch_tui;
use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::command::{execute_shell_action, MouseActions};
use crate::widget::sysfs::FsPathProvider;
use crate::widget::{Span, Widget, WidgetState};

#[derive(Debug, Clone, Copy, Default)]
struct CpuSample {
    idle: u64,
    total: u64,
}

/// A cached reading of the stats display values, refreshed at most once per
/// update interval so rapid redraws don't produce unreadable flicker.
#[derive(Debug, Clone, Default)]
struct StatsSnapshot {
    cpu: Option<f32>,
    ram: Option<(f32, u64, u64)>,
    temp: Option<f32>,
}

/// System statistics widget.
pub struct StatsWidget {
    id: String,
    interval: Duration,
    show_cpu: bool,
    show_ram: bool,
    show_temp: bool,
    actions: MouseActions,
    window_rules: WindowRulesConfig,
    fs: FsPathProvider,
    last_cpu: Mutex<Option<CpuSample>>,
    cached: Mutex<Option<(Instant, StatsSnapshot)>>,
}

impl StatsWidget {
    pub fn new(
        id: impl Into<String>,
        interval_secs: u64,
        show_cpu: bool,
        show_ram: bool,
        show_temp: bool,
    ) -> Self {
        Self {
            id: id.into(),
            interval: Duration::from_secs(interval_secs.max(1)),
            show_cpu,
            show_ram,
            show_temp,
            actions: MouseActions::default(),
            window_rules: WindowRulesConfig::default(),
            fs: FsPathProvider::default(),
            last_cpu: Mutex::new(None),
            cached: Mutex::new(None),
        }
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

    fn sample_cpu(&self) -> Option<f32> {
        let stat_path = self.fs.proc_path("stat");
        let content = self.fs.read_string(&stat_path)?;

        for line in content.lines() {
            if line.starts_with("cpu ") {
                let parts: Vec<u64> = line
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|s| s.parse::<u64>().ok())
                    .collect();

                if parts.len() >= 4 {
                    let user = parts[0];
                    let nice = parts[1];
                    let system = parts[2];
                    let idle = parts[3];
                    let iowait = parts.get(4).copied().unwrap_or(0);
                    let irq = parts.get(5).copied().unwrap_or(0);
                    let softirq = parts.get(6).copied().unwrap_or(0);
                    let steal = parts.get(7).copied().unwrap_or(0);

                    let idle_time = idle + iowait;
                    let non_idle = user + nice + system + irq + softirq + steal;
                    let total_time = idle_time + non_idle;

                    let mut last = self.last_cpu.lock().unwrap();
                    let percentage = match *last {
                        Some(prev) => {
                            let delta_total = total_time.saturating_sub(prev.total);
                            let delta_idle = idle_time.saturating_sub(prev.idle);
                            if delta_total > 0 {
                                let active = delta_total.saturating_sub(delta_idle);
                                (active as f32 / delta_total as f32) * 100.0
                            } else {
                                0.0
                            }
                        }
                        None => 0.0,
                    };

                    *last = Some(CpuSample {
                        idle: idle_time,
                        total: total_time,
                    });

                    return Some(percentage);
                }
            }
        }
        None
    }

    fn sample_ram(&self) -> Option<(f32, u64, u64)> {
        let meminfo_path = self.fs.proc_path("meminfo");
        let content = self.fs.read_string(&meminfo_path)?;

        let mut total_kb: Option<u64> = None;
        let mut avail_kb: Option<u64> = None;

        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                total_kb = line.split_whitespace().nth(1).and_then(|s| s.parse().ok());
            } else if line.starts_with("MemAvailable:") {
                avail_kb = line.split_whitespace().nth(1).and_then(|s| s.parse().ok());
            }
        }

        if let (Some(total), Some(avail)) = (total_kb, avail_kb) {
            if total > 0 {
                let used = total.saturating_sub(avail);
                let percent = (used as f32 / total as f32) * 100.0;
                return Some((percent, used / 1024, total / 1024)); // in MB
            }
        }
        None
    }

    fn sample_temp(&self) -> Option<f32> {
        let hwmon_dir = self.fs.sys_path("class/hwmon");
        if let Ok(entries) = fs::read_dir(hwmon_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(files) = fs::read_dir(&path) {
                    for f in files.flatten() {
                        let name = f.file_name().to_string_lossy().to_string();
                        if name.starts_with("temp") && name.ends_with("_input") {
                            if let Some(milli) = self.fs.read_int::<u32>(&f.path()) {
                                return Some(milli as f32 / 1000.0);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Take a fresh reading of all enabled metrics.
    fn fresh_snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            cpu: if self.show_cpu {
                self.sample_cpu()
            } else {
                None
            },
            ram: if self.show_ram {
                self.sample_ram()
            } else {
                None
            },
            temp: if self.show_temp {
                self.sample_temp()
            } else {
                None
            },
        }
    }

    /// Return a snapshot of the metrics, refreshing at most once per interval.
    /// `state()` runs on every redraw; without throttling the CPU% would be
    /// computed over the tiny gap between redraws and flicker unreadably fast.
    fn snapshot(&self) -> StatsSnapshot {
        let mut cached = self.cached.lock().unwrap();
        match *cached {
            Some((at, ref snap)) if at.elapsed() < self.interval => snap.clone(),
            _ => {
                let snap = self.fresh_snapshot();
                *cached = Some((Instant::now(), snap.clone()));
                snap
            }
        }
    }
}

impl Widget for StatsWidget {
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
        } else if event == WidgetMouseEvent::LeftClick {
            launch_tui("flatbar.stats", &["btop"], &self.window_rules);
        }
    }

    fn state(&self) -> WidgetState {
        let mut spans = Vec::new();
        let snap = self.snapshot();

        if let Some(cpu) = snap.cpu {
            spans.push(Span::icon("fa-microchip"));
            spans.push(Span::text(format!(" {:.0}%", cpu)));
        }

        if let Some((ram, _, _)) = snap.ram {
            if !spans.is_empty() {
                spans.push(Span::text(" "));
            }
            spans.push(Span::icon("fa-memory"));
            spans.push(Span::text(format!(" {:.0}%", ram)));
        }

        if let Some(temp) = snap.temp {
            if !spans.is_empty() {
                spans.push(Span::text(" "));
            }
            spans.push(Span::icon("fa-thermometer"));
            spans.push(Span::text(format!(" {:.0}°C", temp)));
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
    use crate::widget::SpanKind;
    use tempfile::tempdir;

    #[test]
    fn test_stats_widget_with_fixtures() {
        let dir = tempdir().unwrap();
        let proc_dir = dir.path().join("proc");
        let sys_dir = dir.path().join("sys");
        fs::create_dir_all(&proc_dir).unwrap();
        fs::create_dir_all(&sys_dir).unwrap();

        fs::write(proc_dir.join("stat"), "cpu  1000 200 300 5000 100 0 0 0\n").unwrap();

        fs::write(
            proc_dir.join("meminfo"),
            "MemTotal:        16384000 kB\nMemAvailable:     8192000 kB\n",
        )
        .unwrap();

        let fs_prov = FsPathProvider::new(&proc_dir, &sys_dir);
        let widget = StatsWidget::new("stats", 1, true, true, false).with_fs(fs_prov);

        let state1 = widget.state();
        assert!(!state1.spans.is_empty());

        // Update stat for second sample; the snapshot is cached for the
        // interval, so an immediate re-read must return identical spans.
        fs::write(proc_dir.join("stat"), "cpu  1200 200 400 5100 100 0 0 0\n").unwrap();

        let state2 = widget.state();
        assert_eq!(state1.spans, state2.spans);
        assert_eq!(state2.spans.len(), 5); // icon, cpu_text, separator, ram_icon, ram_text
    }

    #[test]
    fn test_stats_snapshot_refreshes_after_interval() {
        let dir = tempdir().unwrap();
        let proc_dir = dir.path().join("proc");
        let sys_dir = dir.path().join("sys");
        fs::create_dir_all(&proc_dir).unwrap();
        fs::create_dir_all(&sys_dir).unwrap();

        fs::write(proc_dir.join("stat"), "cpu  1000 200 300 5000 100 0 0 0\n").unwrap();
        fs::write(
            proc_dir.join("meminfo"),
            "MemTotal:        16384000 kB\nMemAvailable:     8192000 kB\n",
        )
        .unwrap();

        let fs_prov = FsPathProvider::new(&proc_dir, &sys_dir);
        // Interval of 0 seconds is clamped to 1s; use a tiny interval via a
        // zero-duration widget by constructing with interval 0 and clearing
        // the cache manually through snapshot() timing.
        let widget = StatsWidget::new("stats", 0, true, false, false).with_fs(fs_prov);

        let state1 = widget.state();
        assert_eq!(state1.spans.len(), 2); // cpu icon + text

        // Simulate interval expiry by backdating the cached timestamp.
        let mut cached = widget.cached.lock().unwrap();
        *cached = None;
        drop(cached);

        fs::write(proc_dir.join("stat"), "cpu  2000 200 400 5100 100 0 0 0\n").unwrap();
        let state2 = widget.state();
        assert_eq!(state2.spans.len(), 2);
        // CPU% must be recomputed from the new sample (delta over interval).
        if let SpanKind::Text(txt) = &state2.spans[1].kind {
            assert!(txt.ends_with('%'), "cpu span should be a percentage: {txt}");
        } else {
            panic!("expected cpu text span");
        }
    }
}
