//! mako notification-daemon access via `makoctl`.
//!
//! flatbar is only a viewer: it never claims `org.freedesktop.Notifications`
//! ownership — mako stays the daemon. All interaction goes through the
//! `makoctl` CLI.

use serde::Serialize;
use std::process::Command;
use std::thread;

/// A single notification entry shown in the menu.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct NotificationEntry {
    pub id: String,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    /// Unix timestamp in milliseconds (0 when unknown).
    pub timestamp_ms: u64,
}

/// Notification backend contract, so alternative daemons can plug in later.
pub trait NotificationBackend: Send + Sync {
    /// List current (non-dismissed) notifications.
    fn history(&self) -> Result<Vec<NotificationEntry>, String>;
    /// Dismiss one entry or all (id `None`).
    fn dismiss(&self, id: Option<&str>) -> Result<(), String>;
}

/// makoctl-backed implementation of [`NotificationBackend`].
pub struct MakoBackend;

impl MakoBackend {
    /// Run `makoctl <args>` and return stdout.
    ///
    /// `history` needs the eager `-j` flag: mako 1.11 (and friends) print a
    /// plain-text listing by default and only emit JSON with `-j`.
    fn makoctl(args: &[&str]) -> Result<Vec<u8>, String> {
        let output = Command::new("makoctl")
            .args(args)
            .output()
            .map_err(|e| format!("Failed to run makoctl: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "makoctl {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output.stdout)
    }

    fn makoctl_json(subcommand: &str) -> Result<Vec<u8>, String> {
        Self::makoctl(&[subcommand, "-j"])
    }
}

impl NotificationBackend for MakoBackend {
    fn history(&self) -> Result<Vec<NotificationEntry>, String> {
        let stdout = Self::makoctl_json("history")?;
        parse_mako_history(&stdout)
    }

    fn dismiss(&self, id: Option<&str>) -> Result<(), String> {
        let mut args = vec!["dismiss"];
        match id {
            Some(id) => {
                args.push("--id");
                args.push(id);
            }
            None => args.push("--all"),
        }
        Self::makoctl(&args).map(|_| ())
    }
}

/// Parse `makoctl history -j` output across mako versions:
///
/// * mako ≥1.x (e.g. 1.11): a bare JSON array of entries with `id`,
///   `app_name`, `summary`, `body` (plain values, no timestamps).
/// * older backends/tests: an object with a `body` array whose fields may be
///   values (`{"value": ...}`) or plain strings.
pub fn parse_mako_history(bytes: &[u8]) -> Result<Vec<NotificationEntry>, String> {
    let json: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("Failed to parse makoctl history: {e}"))?;

    // Shape 1: bare array (mako 1.11+). Return early — an array root can
    // never be the legacy object shape.
    if let Some(items) = json.as_array() {
        let entries = items
            .iter()
            .filter_map(|item| {
                let id = extract_id_value(item.get("id")?)?;
                if id.is_empty() {
                    return None;
                }
                Some(NotificationEntry {
                    id,
                    app_name: extract_string(item, "app_name"),
                    summary: extract_string(item, "summary"),
                    body: extract_string(item, "body"),
                    // mako 1.11 history carries no timestamps.
                    timestamp_ms: 0,
                })
            })
            .collect();
        return Ok(entries);
    }

    // Shape 2: {"body": [...]}, fields as values or plain strings.
    legacy_parse_mako_history(&json)
}

/// Numeric-or-string id extraction for the bare-array mako 1.11 shape.
fn extract_id_value(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => map.get("value").and_then(extract_id_value),
        _ => None,
    }
}
fn legacy_parse_mako_history(json: &serde_json::Value) -> Result<Vec<NotificationEntry>, String> {
    let body = json
        .get("body")
        .and_then(|b| b.as_array())
        .ok_or_else(|| "makoctl history: missing 'body' array".to_string())?;

    let mut entries = Vec::new();
    for item in body {
        let id = extract_string(item, "id");
        if id.is_empty() {
            continue;
        }
        let timestamp_ms = item
            .get("timestamp")
            .map(|t| extract_string(t, "timestamp").parse::<u64>().unwrap_or(0))
            .unwrap_or(0);
        entries.push(NotificationEntry {
            id,
            app_name: extract_string(item, "app-name"),
            summary: extract_string(item, "summary"),
            body: extract_string(item, "content"),
            timestamp_ms: timestamp_ms / 1000,
        });
    }
    Ok(entries)
}

fn extract_string(item: &serde_json::Value, key: &str) -> String {
    let v = item.get(key);
    match v {
        Some(serde_json::Value::Object(map)) => map
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Send a makoctl command in the background so menu clicks never block the
/// event loop.
pub fn dismiss_background(backend: impl FnOnce() + Send + 'static) {
    thread::Builder::new()
        .name("flatbar-mako-dismiss".to_string())
        .spawn(backend)
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mako_history_nested_values() {
        let json = r#"{
            "body": [
                {
                    "id": {"value": "42"},
                    "app-name": {"value": "App One"},
                    "summary": {"value": "Summ"},
                    "content": {"value": "Body text"},
                    "timestamp": {"value": "1757680000000000"}
                }
            ]
        }"#;
        let entries = parse_mako_history(json.as_bytes()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "42");
        assert_eq!(entries[0].app_name, "App One");
        assert_eq!(entries[0].summary, "Summ");
        assert_eq!(entries[0].body, "Body text");
    }

    #[test]
    fn test_parse_mako_111_bare_array_shape() {
        // Verbatim shape of `makoctl history -j` on mako 1.11: bare array,
        // snake_case fields, numeric ids, no timestamps.
        let json = r#"[
            {
                "id": 94,
                "app_name": "flatbar",
                "app_icon": null,
                "category": null,
                "desktop_entry": null,
                "summary": "Test",
                "body": "Body",
                "urgency": "normal",
                "actions": {}
            },
            {
                "id": 95,
                "app_name": "dialog-information",
                "summary": "Flatbar",
                "body": "Copied: 2026"
            }
        ]"#;
        let entries = parse_mako_history(json.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "94");
        assert_eq!(entries[0].app_name, "flatbar");
        assert_eq!(entries[0].summary, "Test");
        assert_eq!(entries[0].body, "Body");
        assert_eq!(entries[0].timestamp_ms, 0);
        assert_eq!(entries[1].id, "95");
        assert_eq!(entries[1].body, "Copied: 2026");
    }

    #[test]
    fn test_parse_mako_history_plain_text_is_an_error() {
        // Missing `-j` (or invoking an old format): plain-text output must
        // surface as an error string, never a silent empty list.
        let plaintext = b"Notification 94: Test\n  App name: flatbar\n";
        assert!(parse_mako_history(plaintext).is_err());
    }

    /// Live check: when mako is running, `history()` must actually parse the
    /// daemon's output (regression: the missing `-j` produced plain text and
    /// an empty widget). Skips when makoctl/mako are absent.
    #[test]
    fn test_live_makoctl_history_json() {
        if Command::new("makoctl")
            .args(["list"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            == false
        {
            eprintln!("makoctl unavailable; skipping live check");
            return;
        }
        match MakoBackend.history() {
            Ok(entries) => {
                assert!(
                    !entries.is_empty(),
                    "mako is running with history; empty parse means a shape regression"
                );
                for entry in entries.iter().take(3) {
                    assert!(!entry.id.is_empty());
                    assert!(!entry.summary.is_empty());
                }
            }
            Err(e) => eprintln!("mako not running or history empty: {e}"),
        }
    }
}
