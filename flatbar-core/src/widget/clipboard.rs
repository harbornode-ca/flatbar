//! Clipboard history widget: polls `wl-paste -n` and keeps the last 10 unique
//! clipboards (`wl-clipboard` binaries are the interface; no in-process
//! Wayland clipboard protocol).

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ipc::launch::copy_to_clipboard;
use crate::widget::command::MouseActions;
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::{Span, Widget, WidgetState};

const MAX_HISTORY: usize = 10;
/// Characters kept per line in menu rows before ellipsis truncation.
const MENU_ROW_WIDTH: usize = 64;

pub fn cache_dir() -> Option<PathBuf> {
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join(".cache")),
    }?;
    Some(base.join("flatbar"))
}

fn cache_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("clipboard.json"))
}

/// Pure history mutation: prepend `value`, dedupe against existing entries,
/// cap at `MAX_HISTORY`. Returns the new history.
pub fn push_history(history: Vec<String>, value: &str) -> Vec<String> {
    let existing = history.iter().position(|h| h == value);
    let remaining: Vec<String> = history
        .into_iter()
        .enumerate()
        .filter(|(idx, _)| Some(*idx) != existing)
        .map(|(_, h)| h)
        .collect();
    let mut out = vec![value.to_string()];
    out.extend(remaining);
    out.truncate(MAX_HISTORY);
    out
}

/// Truncate a clipboard value into a two-line ellipsized menu row.
pub fn truncate_menu_label(value: &str) -> String {
    let mut lines = value.lines();

    let first = lines.next().unwrap_or("");
    let (l1, l1_cut) = split_truncated(first, MENU_ROW_WIDTH);

    let mut out = vec![l1];
    if l1_cut {
        out.push("…".to_string());
    } else if let Some(second) = lines.next() {
        let (l2, l2_cut) = split_truncated(second, MENU_ROW_WIDTH);
        out.push(format!("{l2}{}", if l2_cut { "…" } else { "" }));
    }

    out.join("\n").trim_end().to_string()
}

/// Split a line into (kept-string, was-truncated).
fn split_truncated(line: &str, max: usize) -> (String, bool) {
    let count = line.chars().count();
    if count <= max {
        return (line.to_string(), false);
    }
    let head: String = line.chars().take(max).collect();
    (head, true)
}

#[derive(Serialize, Deserialize)]
struct StoredHistory {
    history: Vec<String>,
}

fn load_history() -> Vec<String> {
    let Some(path) = cache_file() else {
        return Vec::new();
    };
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<StoredHistory>(&content)
            .map(|s| s.history.into_iter().take(MAX_HISTORY).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save_history(history: &[String]) {
    let Some(path) = cache_file() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let json = match serde_json::to_string(&StoredHistory {
        history: history.to_vec(),
    }) {
        Ok(j) => j,
        Err(_) => return,
    };
    let _ = fs::write(&path, json);
}

/// Default bar icon shown for the clipboard widget.
pub const DEFAULT_CLIPBOARD_ICON: &str = "fa-clipboard";

/// Clipboard-history widget backed by `wl-paste -n` polling.
pub struct ClipboardWidget {
    id: String,
    actions: MouseActions,
    history: Mutex<Vec<String>>,
    notify: Option<calloop::channel::Sender<()>>,
    icon: String,
}

impl ClipboardWidget {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            actions: MouseActions::default(),
            history: Mutex::new(load_history()),
            notify: None,
            icon: DEFAULT_CLIPBOARD_ICON.to_string(),
        }
    }

    pub fn with_actions(mut self, actions: MouseActions) -> Self {
        self.actions = actions;
        self
    }

    /// Set the icon alias/codepoint shown on the bar.
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Install a channel used to wake the event loop when fresh state is available.
    pub fn set_notify(&mut self, tx: calloop::channel::Sender<()>) {
        self.notify = Some(tx);
    }

    fn poll_once(&self) {
        let Ok(output) = std::process::Command::new("wl-paste").arg("-n").output() else {
            return;
        };
        if !output.status.success() {
            return;
        }
        let value = String::from_utf8_lossy(&output.stdout);
        let value = value.trim_end_matches('\n');
        if value.trim().is_empty() {
            return;
        }

        let mut history = self.history.lock().unwrap();
        if history.first().map(|h| h.as_str()) == Some(value) {
            return;
        }
        *history = push_history(history.clone(), value);
        save_history(&history);
        if let Some(tx) = &self.notify {
            let _ = tx.send(());
        }
    }
}

impl Widget for ClipboardWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn update_interval(&self) -> Duration {
        Duration::from_secs(1)
    }

    fn as_clipboard_mut(&mut self) -> Option<&mut ClipboardWidget> {
        Some(self)
    }

    fn refresh(&self) {
        self.poll_once();
    }

    fn get_menu(&self) -> Option<MenuModel> {
        let history = self.history.lock().unwrap().clone();
        if history.is_empty() {
            return Some(
                MenuModel::new(vec![
                    MenuItem::item("current-empty", "Current: (empty)").disabled(true)
                ])
                .with_title("Clipboard"),
            );
        }

        let mut items = Vec::new();
        let current = &history[0];
        items.push(
            MenuItem::item(
                "current",
                format!("Current: {}", truncate_menu_label(current)),
            )
            .disabled(true),
        );
        items.push(MenuItem::Separator);
        items.push(MenuItem::item("history-header", "History:").disabled(true));

        for (idx, value) in history.iter().enumerate() {
            items.push(MenuItem::item(
                format!("item:{idx}"),
                truncate_menu_label(value),
            ));
        }

        Some(
            MenuModel::new(items)
                .with_max_width(512)
                .with_title("Clipboard"),
        )
    }

    fn handle_menu_action(&self, action_id: &str) {
        let Some(idx) = action_id.strip_prefix("item:") else {
            return;
        };
        let Ok(idx) = idx.parse::<usize>() else {
            return;
        };
        if let Some(value) = self.history.lock().unwrap().get(idx) {
            copy_to_clipboard(value);
        }
    }

    fn state(&self) -> WidgetState {
        WidgetState {
            spans: vec![Span::icon(self.icon.clone())],
            tooltip: Some("Clipboard history (last 10 entries)".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_history_dedupe_and_cap() {
        let hist: Vec<String> = (0..10).map(|i| format!("entry{i}")).collect();

        // Existing head re-push: unchanged
        let h = push_history(hist.clone(), "entry0");
        assert_eq!(h, hist);

        // Move an old entry to the front
        let h = push_history(hist.clone(), "entry5");
        assert_eq!(h.first().unwrap(), "entry5");
        assert_eq!(h.len(), 10);
        assert_eq!(h.iter().filter(|e| **e == "entry5").count(), 1);

        // Push a new entry into a full history; cap is preserved
        let h = push_history(hist, "entry11");
        assert_eq!(h.len(), 10);
        assert_eq!(h.first().unwrap(), "entry11");
    }

    #[test]
    fn test_truncate_menu_label_two_lines() {
        let label = truncate_menu_label("line one\nline two\nline three");
        assert_eq!(label, "line one\nline two");

        let long = "x".repeat(MENU_ROW_WIDTH + 10);
        let label = truncate_menu_label(&long);
        assert!(label.ends_with('…'));
        // First truncated line + newline + ellipsis continuation line.
        assert!(label.chars().count() <= MENU_ROW_WIDTH + 3);
    }
}

// Issue 13/14 menu structure test (no real wl-copy invocation paths).
#[cfg(test)]
mod menu_tests {
    use super::*;

    #[test]
    fn test_menu_structure_current_and_history() {
        let widget = ClipboardWidget::new("clip");
        {
            // The widget seeds its history from the persisted cache file, so
            // the test overwrites it with a known fixture rather than
            // asserting on machine state.
            let mut h = widget.history.lock().unwrap();
            *h = vec!["current value".to_string(), "older".to_string()];
        }

        let menu = widget.get_menu().unwrap();
        assert_eq!(menu.title.as_deref(), Some("Clipboard"));
        let labels: Vec<&str> = menu.items.iter().filter_map(|i| i.label()).collect();
        assert!(labels
            .iter()
            .any(|l| l.starts_with("Current: current value")));
        assert!(labels.contains(&"History:"));
        assert!(labels.iter().any(|l| l.contains("older")));

        // item:0 maps to the current (head) entry.
        assert_eq!(
            menu.items
                .iter()
                .find(|i| i.id() == Some("item:0"))
                .unwrap()
                .label(),
            Some("current value")
        );
    }
}
