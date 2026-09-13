//! External dmenu/fuzzel launcher integration for popup menus.

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;

use crate::widget::menu::{MenuItem, MenuModel};

/// A flattened menu item for dmenu input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenedMenuItem {
    pub id: String,
    pub display: String,
}

/// Flatten a `MenuModel` into a 1-level list of display items and IDs suitable for dmenu/fuzzel.
pub fn flatten_menu(menu: &MenuModel) -> Vec<FlattenedMenuItem> {
    let mut result = Vec::new();
    flatten_items(&menu.items, None, &mut result);
    result
}

fn flatten_items(
    items: &[MenuItem],
    parent_prefix: Option<&str>,
    out: &mut Vec<FlattenedMenuItem>,
) {
    for item in items {
        match item {
            MenuItem::Item { id, label, .. } => {
                let display = match parent_prefix {
                    Some(prefix) => format!("{prefix} — {label}"),
                    None => label.clone(),
                };
                out.push(FlattenedMenuItem {
                    id: id.clone(),
                    display,
                });
            }
            MenuItem::Checkbox {
                id, label, checked, ..
            } => {
                let check_str = if *checked { "[x]" } else { "[ ]" };
                let display = match parent_prefix {
                    Some(prefix) => format!("{prefix} — {check_str} {label}"),
                    None => format!("{check_str} {label}"),
                };
                out.push(FlattenedMenuItem {
                    id: id.clone(),
                    display,
                });
            }
            MenuItem::SubMenu {
                label,
                items: sub_items,
                ..
            } => {
                let prefix = match parent_prefix {
                    Some(p) => format!("{p} — {label}"),
                    None => label.clone(),
                };
                flatten_items(sub_items, Some(&prefix), out);
            }
            MenuItem::Separator => {
                // Separators are omitted in dmenu view
            }
        }
    }
}

/// Match a dmenu/fuzzel selection string back to a MenuItem ID.
pub fn match_selection(selection: &str, items: &[FlattenedMenuItem]) -> Option<String> {
    let trimmed = selection.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. Exact match against display string
    for item in items {
        if item.display == trimmed {
            return Some(item.id.clone());
        }
    }

    // 2. Fallback: match against trimmed display string
    for item in items {
        if item.display.trim() == trimmed {
            return Some(item.id.clone());
        }
    }

    None
}

/// Spawn the external launcher command on a background worker thread.
/// When the user makes a selection, `tx.send((widget_id, action_id))` delivers it back to the event loop.
pub fn spawn_menu_launcher(
    menu_cmd: &str,
    widget_id: &str,
    menu: MenuModel,
    tx: calloop::channel::Sender<(String, String)>,
) {
    let items = flatten_menu(&menu);
    if items.is_empty() {
        return;
    }

    let cmd_str = if menu_cmd.trim().is_empty() {
        "fuzzel --dmenu -p flatbar"
    } else {
        menu_cmd
    }
    .to_string();

    let wid = widget_id.to_string();

    thread::Builder::new()
        .name("flatbar-launcher".to_string())
        .spawn(move || {
            let mut input_data = String::new();
            for item in &items {
                input_data.push_str(&item.display);
                input_data.push('\n');
            }

            let parts: Vec<&str> = cmd_str.split_whitespace().collect();
            if parts.is_empty() {
                return;
            }

            let mut cmd = Command::new(parts[0]);
            for arg in &parts[1..] {
                cmd.arg(arg);
            }

            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(err) => {
                    tracing::error!("Failed to spawn menu command '{cmd_str}': {err}");
                    return;
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(input_data.as_bytes());
            }

            let output = match child.wait_with_output() {
                Ok(out) => out,
                Err(err) => {
                    tracing::error!("Menu command '{cmd_str}' failed: {err}");
                    return;
                }
            };

            if output.status.success() {
                let stdout_str = String::from_utf8_lossy(&output.stdout);
                if let Some(selected_id) = match_selection(&stdout_str, &items) {
                    let _ = tx.send((wid, selected_id));
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flatten_menu_hierarchy_and_checkboxes() {
        let menu = MenuModel::new(vec![
            MenuItem::item("item1", "Item One"),
            MenuItem::checkbox("cb1", "Enable Feature", true),
            MenuItem::checkbox("cb2", "Disable Feature", false),
            MenuItem::Separator,
            MenuItem::submenu(
                "sub1",
                "Advanced",
                vec![
                    MenuItem::item("sub_item1", "Sub Action"),
                    MenuItem::checkbox("sub_cb", "Sub Toggle", true),
                ],
            ),
        ]);

        let flattened = flatten_menu(&menu);
        assert_eq!(flattened.len(), 5);
        assert_eq!(flattened[0].id, "item1");
        assert_eq!(flattened[0].display, "Item One");
        assert_eq!(flattened[1].id, "cb1");
        assert_eq!(flattened[1].display, "[x] Enable Feature");
        assert_eq!(flattened[2].id, "cb2");
        assert_eq!(flattened[2].display, "[ ] Disable Feature");
        assert_eq!(flattened[3].id, "sub_item1");
        assert_eq!(flattened[3].display, "Advanced — Sub Action");
        assert_eq!(flattened[4].id, "sub_cb");
        assert_eq!(flattened[4].display, "Advanced — [x] Sub Toggle");
    }

    #[test]
    fn test_match_selection() {
        let items = vec![
            FlattenedMenuItem {
                id: "first".to_string(),
                display: "First Action".to_string(),
            },
            FlattenedMenuItem {
                id: "second".to_string(),
                display: "[x] Second Option".to_string(),
            },
        ];

        // Exact match with newline from dmenu
        assert_eq!(
            match_selection("First Action\n", &items),
            Some("first".to_string())
        );
        assert_eq!(
            match_selection("[x] Second Option", &items),
            Some("second".to_string())
        );

        // Unknown / empty / escape
        assert_eq!(match_selection("", &items), None);
        assert_eq!(match_selection("   \n", &items), None);
        assert_eq!(match_selection("Nonexistent", &items), None);
    }
}
