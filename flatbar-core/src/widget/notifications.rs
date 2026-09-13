//! Notifications widget: a list/menu viewer for mako-owned notifications.
//! The 48h trash collection is issue 12's "clear old entries" behavior.
//!
//! mako ≥1.11's `history -j` carries no timestamps, so the widget maintains a
//! persisted *first-seen* map (`id → first fetch time`) to give entries real
//! ages: an id mako still holds is pinned to when flatbar first saw it and
//! expires from the list after 48 hours without resurrecting.

use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::Duration;

#[cfg(test)]
use crate::dbus::mako::parse_mako_history;
use crate::dbus::mako::{MakoBackend, NotificationBackend, NotificationEntry};
use crate::widget::command::MouseActions;
use crate::widget::menu::{MenuItem, MenuModel};
use crate::widget::{Span, Widget, WidgetState};

const TRASH_AFTER_SECS: u64 = 48 * 60 * 60;
const MENU_ROW_WIDTH: usize = 64;

fn cache_file() -> Option<std::path::PathBuf> {
    Some(crate::widget::clipboard::cache_dir()?.join("notifications.json"))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredEntries {
    entries: Vec<NotificationEntry>,
}

fn load_entries() -> Vec<NotificationEntry> {
    let Some(path) = cache_file() else {
        return Vec::new();
    };
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<StoredEntries>(&content)
            .map(|s| s.entries)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save_entries(entries: &[NotificationEntry]) {
    let Some(path) = cache_file() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let json = match serde_json::to_string(&StoredEntries {
        entries: entries.to_vec(),
    }) {
        Ok(j) => j,
        Err(_) => return,
    };
    let _ = fs::write(&path, json);
}

/// Drop entries older than `cut_ms` (48h trash).
pub fn trash_old_entries(entries: Vec<NotificationEntry>, now_ms: u64) -> Vec<NotificationEntry> {
    entries
        .into_iter()
        .filter(|e| {
            e.timestamp_ms == 0 || now_ms.saturating_sub(e.timestamp_ms) / 1000 < TRASH_AFTER_SECS
        })
        .collect()
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct StoredSeen {
    seen: HashMap<String, u64>,
}

fn load_seen() -> HashMap<String, u64> {
    let Some(path) =
        crate::widget::clipboard::cache_dir().map(|d| d.join("notifications_seen.json"))
    else {
        return HashMap::new();
    };
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<StoredSeen>(&content)
            .map(|s| s.seen)
            .unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

fn save_seen(seen: &HashMap<String, u64>) {
    let Some(path) =
        crate::widget::clipboard::cache_dir().map(|d| d.join("notifications_seen.json"))
    else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(&StoredSeen { seen: seen.clone() }) {
        let _ = fs::write(&path, json);
    }
}

/// Assign real ages to timestamp-less fetched entries and apply the 48h trash.
///
/// * ids already in `seen` keep their original first-seen time — an entry
///   mako still holds stays expired (hidden) instead of resurrecting.
/// * entries fetched with a real backend timestamp (legacy mako shape) keep
///   it and the timestamp is recorded.
/// * unknown ids are stamped `now_ms`.
/// * the seen map is pruned to ids still present in the current history —
///   including expired ones, so their age never resets while mako holds them
///   — bounding its growth and re-arming the clock if mako drops an id.
///
/// Returns the entries younger than 48h, in fetch order.
pub fn apply_first_seen(
    seen: &mut HashMap<String, u64>,
    fetched_all: Vec<NotificationEntry>,
    now_ms: u64,
) -> Vec<NotificationEntry> {
    let mut merged = Vec::with_capacity(fetched_all.len());
    for entry in &fetched_all {
        let mut entry = entry.clone();
        let prior = if entry.timestamp_ms != 0 {
            entry.timestamp_ms
        } else {
            *seen.get(&entry.id).filter(|t| **t != 0).unwrap_or(&0)
        };
        let effective = if prior != 0 { prior } else { now_ms };
        entry.timestamp_ms = effective;
        seen.insert(entry.id.clone(), effective);

        if now_ms.saturating_sub(effective) / 1000 < TRASH_AFTER_SECS {
            merged.push(entry);
        }
    }

    seen.retain(|id, _| {
        merged.iter().any(|e| e.id == *id) || fetched_all.iter().any(|e| e.id == *id)
    });
    merged
}

/// Current unix time in ms.
fn now_ms() -> u64 {
    (unsafe { libc::time(std::ptr::null_mut()) }).max(0) as u64 * 1000
}

/// Captive notification text for a menu row.
fn truncate_entry_label(entry: &NotificationEntry) -> String {
    let line = format!("{}: {}", entry.app_name, entry.summary);
    if line.chars().count() > MENU_ROW_WIDTH {
        let head: String = line.chars().take(MENU_ROW_WIDTH).collect();
        format!("{head}…")
    } else if !entry.body.is_empty() {
        format!("{line}: {}", entry.body)
    } else {
        line
    }
}

/// Notifications widget polling `makoctl history`.
pub struct NotificationsWidget {
    id: String,
    actions: MouseActions,
    backend: MakoBackend,
    entries: Mutex<Vec<NotificationEntry>>,
    /// Persisted `id → first-seen (unix ms)` for timestamp-less backends.
    first_seen: Mutex<HashMap<String, u64>>,
    notify: Option<calloop::channel::Sender<()>>,
}

impl NotificationsWidget {
    pub fn new(id: impl Into<String>) -> Self {
        let loaded = trash_old_entries(load_entries(), now_ms());
        Self {
            id: id.into(),
            actions: MouseActions::default(),
            backend: MakoBackend,
            entries: Mutex::new(loaded),
            first_seen: Mutex::new(load_seen()),
            notify: None,
        }
    }

    pub fn with_actions(mut self, actions: MouseActions) -> Self {
        self.actions = actions;
        self
    }

    /// Install a channel used to wake the event loop when fresh state is available.
    pub fn set_notify(&mut self, tx: calloop::channel::Sender<()>) {
        self.notify = Some(tx);
    }

    fn fetch(&self) -> Option<Vec<NotificationEntry>> {
        self.backend.history().ok()
    }

    fn clear_entries(&self) {
        *self.entries.lock().unwrap() = Vec::new();
        self.first_seen.lock().unwrap().clear();
        if let Some(d) = crate::widget::clipboard::cache_dir() {
            let _ = fs::remove_file(d.join("notifications.json"));
            let _ = fs::remove_file(d.join("notifications_seen.json"));
        }
    }
}

impl Widget for NotificationsWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn update_interval(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn as_notifications_mut(&mut self) -> Option<&mut NotificationsWidget> {
        Some(self)
    }

    fn refresh(&self) {
        // Trash collection keeps running even when makoctl is unreachable.
        let now = now_ms();
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|e| {
                e.timestamp_ms == 0 || now.saturating_sub(e.timestamp_ms) / 1000 < TRASH_AFTER_SECS
            });
        }
        if let Some(fetched) = self.fetch() {
            let (filtered, seen_update) = {
                let mut seen = self.first_seen.lock().unwrap();
                let filtered = apply_first_seen(&mut seen, fetched, now);
                (filtered, true)
            };
            save_entries(&filtered);
            // Persist the seen map from a detached thread: small JSON write,
            // never on the render path.
            let snapshot: HashMap<String, u64> = self.first_seen.lock().unwrap().clone();
            std::thread::Builder::new()
                .name("flatbar-notifications-seen-save".to_string())
                .spawn(move || save_seen(&snapshot))
                .ok();
            if let Ok(mut entries) = self.entries.lock() {
                *entries = filtered;
            }
            if seen_update {
                if let Some(tx) = &self.notify {
                    let _ = tx.send(());
                }
            }
        }
    }

    fn get_menu(&self) -> Option<MenuModel> {
        let entries = self.entries.lock().unwrap().clone();
        let mut items = Vec::new();
        items.push(MenuItem::item("clear-all", "Clear all"));
        if !entries.is_empty() {
            items.push(MenuItem::Separator);
        }
        for entry in entries {
            items.push(MenuItem::item(
                format!("notif:{}", entry.id),
                truncate_entry_label(&entry),
            ));
        }
        Some(
            MenuModel::new(items)
                .with_max_width(512)
                .with_title("Notifications"),
        )
    }

    fn handle_menu_action(&self, action_id: &str) {
        if action_id == "clear-all" {
            let _ = self.backend.dismiss(None);
            self.clear_entries();
            if let Some(tx) = &self.notify {
                let _ = tx.send(());
            }
            return;
        }

        let Some(id) = action_id.strip_prefix("notif:") else {
            return;
        };
        let _ = self.backend.dismiss(Some(id));
        {
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|e| e.id != id);
            save_entries(&entries);
        }
        if let Some(tx) = &self.notify {
            let _ = tx.send(());
        }
    }

    fn state(&self) -> WidgetState {
        let has = !self.entries.lock().unwrap().is_empty();
        let icon = if has { "fa-circle" } else { "fa-circle-o" };
        let tooltip = if has {
            "Notifications".to_string()
        } else {
            "No notifications".to_string()
        };
        WidgetState {
            spans: vec![Span::icon(icon)],
            tooltip: Some(tooltip),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_entry(id: &str, ts_ms: u64) -> NotificationEntry {
        NotificationEntry {
            id: id.to_string(),
            app_name: "app".into(),
            summary: "sum".into(),
            body: "body".into(),
            timestamp_ms: ts_ms,
        }
    }

    #[test]
    fn test_trash_after_48h() {
        let now = 1_760_000_000_000u64;
        let entries = vec![
            fake_entry("1", now - 1000),             // 1s old → keep
            fake_entry("2", now - 47 * 3600 * 1000), // 47h old → keep
            fake_entry("3", now - 49 * 3600 * 1000), // 49h old → drop
            fake_entry("4", 0),                      // unknown ts → keep
        ];
        let kept = trash_old_entries(entries, now);
        let ids: Vec<&str> = kept.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "4"]);
    }

    #[test]
    fn test_parse_entries_via_mako_module() {
        let json = serde_json::json!({
            "body": [
                {"id": {"value": "7"}, "app-name": {"value": "app"},
                 "summary": {"value": "s"}, "content": {"value": "b"},
                 "timestamp": {"value": "1760000000000000"}}
            ]
        });
        let entries = parse_mako_history(serde_json::to_vec(&json).unwrap().as_slice()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "7");
        assert_eq!(entries[0].app_name, "app");
    }

    fn entry(id: &str, ts: u64) -> NotificationEntry {
        NotificationEntry {
            id: id.to_string(),
            app_name: "app".to_string(),
            summary: "s".to_string(),
            body: "b".to_string(),
            timestamp_ms: ts,
        }
    }

    fn entry_map(id: &str, ts: u64) -> NotificationEntry {
        entry(id, ts)
    }

    #[test]
    fn test_apply_first_seen_stamps_unknown_ids_with_now() {
        let now: u64 = 1_770_000_000_000;
        let mut seen = HashMap::new();
        let merged = apply_first_seen(&mut seen, vec![entry("10", 0)], now);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].timestamp_ms, now);
        assert_eq!(seen.get("10"), Some(&now));
    }

    #[test]
    fn test_apply_first_seen_pins_known_ids() {
        let now: u64 = 1_770_000_000_000;
        let first_seen: u64 = now - 60_000;
        let mut seen = HashMap::new();
        seen.insert("10".to_string(), first_seen);

        let merged = apply_first_seen(&mut seen, vec![entry("10", 0)], now);
        assert_eq!(merged[0].timestamp_ms, first_seen);
        assert_eq!(seen.get("10"), Some(&first_seen));
    }

    #[test]
    fn test_apply_first_seen_old_id_stays_expired_without_resurrecting() {
        let now: u64 = 1_770_000_000_000;
        let stale: u64 = now - (48 * 60 * 60 + 120) * 1000;
        let mut seen = HashMap::new();
        seen.insert("10".to_string(), stale);

        let merged = apply_first_seen(&mut seen, vec![entry("10", 0)], now);
        assert!(
            merged.is_empty(),
            "expired entry must not reappear with a fresh stamp"
        );
        // The age is pinned: mako still holds it, so it stays hidden forever.
        assert_eq!(seen.get("10"), Some(&stale));
    }

    #[test]
    fn test_apply_first_seen_prunes_absent_ids() {
        let now: u64 = 1_770_000_000_000;
        let mut seen = HashMap::new();
        seen.insert("gone".to_string(), now - 1_000);

        let _ = apply_first_seen(&mut seen, vec![entry("10", 0)], now);
        assert!(!seen.contains_key("gone"), "ids mako dropped are pruned");
        assert!(seen.contains_key("10"));
    }

    #[test]
    fn test_apply_first_seen_legacy_timestamp_wins() {
        let now: u64 = 1_770_000_000_000;
        let mut seen = HashMap::new();
        let legacy_wins = now - 3_600_000;

        let merged = apply_first_seen(&mut seen, vec![entry_map("9", legacy_wins)], now);
        assert_eq!(merged[0].timestamp_ms, legacy_wins);
        assert_eq!(seen.get("9"), Some(&legacy_wins));
    }
}
