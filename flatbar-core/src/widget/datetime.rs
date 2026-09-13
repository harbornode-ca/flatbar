use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::calendar::FirstDay;
use crate::widget::command::{execute_shell_action, MouseActions};
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::{PopupRequest, Span, Widget, WidgetState};
use std::ffi::CString;
use std::time::Duration;

/// Default datetime pattern presets (issue 7) when none are configured.
fn default_patterns() -> Vec<(String, String)> {
    vec![
        ("Long date".into(), "%A, %B %d, %Y".into()),
        ("Short date".into(), "%a %b %d, %Y".into()),
        ("Year first".into(), "%Y%m%d".into()),
        ("Year first with separators".into(), "%Y/%m/%d".into()),
        ("Standard international".into(), "%d/%m/%Y".into()),
        ("American".into(), "%m/%d/%Y".into()),
        ("UTC 24hr".into(), "%H:%M UTC".into()),
        ("UTC 12hr".into(), "%I:%M %p UTC".into()),
        ("Local 24hr".into(), "%H:%M".into()),
        ("Local 12hr".into(), "%I:%M %p".into()),
        ("Local TZ 24hr".into(), "%H:%M %Z".into()),
        ("Local TZ 12hr".into(), "%I:%M %p %Z".into()),
    ]
}

/// Datetime widget displaying formattable timestamps.
pub struct DatetimeWidget {
    id: String,
    format: String,
    interval: Duration,
    calendar: bool,
    first_day: FirstDay,
    calendar_launch_command: Option<Vec<String>>,
    window_rules: crate::config::window_rules::WindowRulesConfig,
    patterns: Vec<(String, String)>,
    actions: MouseActions,
}

impl DatetimeWidget {
    pub fn new(id: impl Into<String>, format: impl Into<String>, interval_secs: u64) -> Self {
        Self {
            id: id.into(),
            format: format.into(),
            interval: Duration::from_secs(interval_secs.max(1)),
            calendar: true,
            first_day: FirstDay::Monday,
            calendar_launch_command: None,
            window_rules: Default::default(),
            patterns: default_patterns(),
            actions: MouseActions::default(),
        }
    }

    pub fn with_actions(mut self, actions: MouseActions) -> Self {
        self.actions = actions;
        self
    }

    pub fn with_calendar(mut self, calendar: bool) -> Self {
        self.calendar = calendar;
        self
    }

    pub fn with_first_day(mut self, first_day: FirstDay) -> Self {
        self.first_day = first_day;
        self
    }

    pub fn with_calendar_launch(mut self, command: Option<Vec<String>>) -> Self {
        self.calendar_launch_command = command;
        self
    }

    pub fn with_window_rules(
        mut self,
        rules: crate::config::window_rules::WindowRulesConfig,
    ) -> Self {
        self.window_rules = rules;
        self
    }

    pub fn with_patterns(mut self, patterns: Vec<(String, String)>) -> Self {
        self.patterns = patterns;
        self
    }

    /// Format current local time using the widget's format string.
    pub fn format_now(&self) -> String {
        self.format_time_at(&self.format)
    }

    /// Render an arbitrary strftime pattern.
    pub fn format_time_at(&self, fmt: &str) -> String {
        format_time(fmt)
    }
}

pub fn format_time(fmt: &str) -> String {
    unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);

        let c_fmt = match CString::new(fmt) {
            Ok(c) => c,
            Err(_) => return "invalid-format".to_string(),
        };

        let mut buf = vec![0u8; 256];
        let len = libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c_fmt.as_ptr(),
            &tm,
        );

        if len > 0 {
            String::from_utf8_lossy(&buf[..len]).to_string()
        } else {
            // In case buffer was too small
            let mut large_buf = vec![0u8; 1024];
            let len2 = libc::strftime(
                large_buf.as_mut_ptr() as *mut libc::c_char,
                large_buf.len(),
                c_fmt.as_ptr(),
                &tm,
            );
            if len2 > 0 {
                String::from_utf8_lossy(&large_buf[..len2]).to_string()
            } else {
                "".to_string()
            }
        }
    }
}

impl Widget for DatetimeWidget {
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

    fn popup_request(&self) -> Option<PopupRequest> {
        if self.calendar {
            // Left-click always opens the calendar popup (not configurable).
            Some(PopupRequest::Calendar {
                first_day: self.first_day,
                launch_command: self.calendar_launch_command.clone(),
            })
        } else {
            None
        }
    }

    fn get_menu(&self) -> Option<MenuModel> {
        if self.patterns.is_empty() {
            return None;
        }
        let mut items = Vec::new();
        for (label, pattern) in &self.patterns {
            let formatted = format_time(pattern);
            items.push(MenuItem::item(
                format!("pattern:{pattern}"),
                format!("{label}: {formatted}"),
            ));
        }
        Some(MenuModel::new(items).with_title("Date & Time"))
    }

    fn handle_menu_action(&self, action_id: &str) {
        if let Some(pattern) = action_id.strip_prefix("pattern:") {
            let value = format_time(pattern);
            crate::ipc::launch::copy_to_clipboard(&value);
            return;
        }
        if action_id == "calendar-launch" {
            if let Some(cmd) = &self.calendar_launch_command {
                let cmd: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
                crate::ipc::launch::launch_gui("flatbar.datetime", &cmd, &self.window_rules);
            }
        }
    }

    fn state(&self) -> WidgetState {
        let formatted = self.format_now();
        WidgetState {
            spans: vec![Span::text(formatted)],
            tooltip: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_datetime_formatting() {
        let dt = DatetimeWidget::new("datetime", "%Y", 1);
        let out = dt.format_now();
        assert_eq!(out.len(), 4);
        assert!(out.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_datetime_state() {
        let dt = DatetimeWidget::new("datetime", "%H:%M", 1);
        let state = dt.state();
        assert_eq!(state.spans.len(), 1);
        assert!(state.spans[0].kind == crate::widget::SpanKind::Text(dt.format_now()));
    }
}

// Additional tests for issue 7 (pattern menu) — appended after the unit block.
#[cfg(test)]
mod pattern_tests {
    use super::*;

    #[test]
    fn test_default_patterns_present() {
        let dt = DatetimeWidget::new("dt", "%H:%M", 1);
        let menu = dt.get_menu().unwrap();
        assert_eq!(menu.title.as_deref(), Some("Date & Time"));
        // Issue 7: 6 date presets + 6 time presets.
        assert_eq!(menu.items.len(), 12);
        let first = &menu.items[0];
        assert!(first.id().unwrap().starts_with("pattern:"));
        assert!(first.label().unwrap().starts_with("Long date:"));
    }

    #[test]
    fn test_configured_patterns_override_defaults() {
        let dt = DatetimeWidget::new("dt", "%H:%M", 1)
            .with_patterns(vec![("Custom".into(), "%Y".into())]);
        let menu = dt.get_menu().unwrap();
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].id(), Some("pattern:%Y"));
        assert!(menu.items[0].label().unwrap().starts_with("Custom: 20"));
    }

    #[test]
    fn test_pattern_action_does_not_crash() {
        let dt = DatetimeWidget::new("dt", "%H:%M", 1);
        dt.handle_menu_action("pattern:%Y");
        dt.handle_menu_action("calendar-launch");
        dt.handle_menu_action("unknown");
    }
}
