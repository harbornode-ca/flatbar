//! Client implementation for `com.canonical.dbusmenu`.
//!
//! Converts DBusMenu trees into Flatbar's internal [`MenuModel`] and [`MenuItem`] hierarchy.

use std::collections::HashMap;
use zbus::zvariant::{OwnedValue, Value};

use crate::widget::menu::{MenuItem, MenuModel};

/// A node in a DBusMenu layout tree.
#[derive(Debug, Clone, PartialEq)]
pub struct DBusMenuItem {
    pub id: i32,
    pub properties: HashMap<String, OwnedValue>,
    pub children: Vec<DBusMenuItem>,
}

impl DBusMenuItem {
    /// Parse a DBusMenu layout node from a zvariant `Value`.
    ///
    /// The layout tuple format is `(id: i32, properties: Dict<String, Value>, children: Array<Value>)`.
    pub fn parse(value: &Value<'_>) -> Option<Self> {
        let structure = match value {
            Value::Structure(s) => s,
            _ => return None,
        };

        let fields = structure.fields();
        if fields.len() < 3 {
            return None;
        }

        let id = match &fields[0] {
            Value::I32(id) => *id,
            Value::U32(id) => *id as i32,
            _ => return None,
        };

        let mut properties = HashMap::new();
        if let Value::Dict(dict) = &fields[1] {
            for (k, v) in dict.iter() {
                if let Value::Str(key_str) = k {
                    if let Ok(owned_v) = OwnedValue::try_from(v.clone()) {
                        properties.insert(key_str.to_string(), owned_v);
                    }
                }
            }
        }

        let mut children = Vec::new();
        if let Value::Array(arr) = &fields[2] {
            for child_val in arr.iter() {
                if let Some(child_item) = DBusMenuItem::parse(child_val) {
                    children.push(child_item);
                }
            }
        }

        Some(Self {
            id,
            properties,
            children,
        })
    }

    /// Convert this DBusMenu item into a Flatbar [`MenuItem`].
    pub fn to_flatbar_item(&self) -> Option<MenuItem> {
        let visible = self
            .properties
            .get("visible")
            .and_then(|v| match &**v {
                Value::Bool(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(true);

        if !visible {
            return None;
        }

        let item_type = self
            .properties
            .get("type")
            .and_then(|v| match &**v {
                Value::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        if item_type == "separator" {
            return Some(MenuItem::Separator);
        }

        let raw_label = self
            .properties
            .get("label")
            .and_then(|v| match &**v {
                Value::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        let label = sanitize_label(raw_label);

        let enabled = self
            .properties
            .get("enabled")
            .and_then(|v| match &**v {
                Value::Bool(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(true);

        let icon = self.properties.get("icon-name").and_then(|v| match &**v {
            Value::Str(s) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        });

        // Check if item has children or children-display
        let children_display = self
            .properties
            .get("children-display")
            .and_then(|v| match &**v {
                Value::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        if !self.children.is_empty() || children_display == "submenu" {
            let mut sub_items = Vec::new();
            for child in &self.children {
                if let Some(sub) = child.to_flatbar_item() {
                    sub_items.push(sub);
                }
            }
            return Some(MenuItem::SubMenu {
                id: self.id.to_string(),
                label,
                icon,
                items: sub_items,
            });
        }

        // Check if toggle-type (checkbox or radio)
        let toggle_type = self
            .properties
            .get("toggle-type")
            .and_then(|v| match &**v {
                Value::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        if toggle_type == "checkmark" || toggle_type == "radio" {
            let toggle_state = self
                .properties
                .get("toggle-state")
                .and_then(|v| match &**v {
                    Value::I32(s) => Some(*s),
                    Value::U32(s) => Some(*s as i32),
                    _ => None,
                })
                .unwrap_or(0);

            return Some(MenuItem::Checkbox {
                id: self.id.to_string(),
                label,
                checked: toggle_state == 1,
                enabled,
            });
        }

        Some(MenuItem::Item {
            id: self.id.to_string(),
            label,
            icon,
            enabled,
        })
    }
}

/// Convert a root `DBusMenuItem` into a `MenuModel`.
pub fn dbusmenu_to_menu_model(root: &DBusMenuItem) -> MenuModel {
    let mut items = Vec::new();
    for child in &root.children {
        if let Some(item) = child.to_flatbar_item() {
            items.push(item);
        }
    }
    MenuModel::new(items)
}

/// Strip GTK/Qt mnemonic underscores (e.g. `_File` -> `File`, `__` -> `_`).
pub fn sanitize_label(label: &str) -> String {
    let mut result = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '_' {
            if let Some(&next) = chars.peek() {
                if next == '_' {
                    // Escaped underscore
                    result.push('_');
                    chars.next();
                }
                // Single underscore mnemonic -> skip
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_label() {
        assert_eq!(sanitize_label("_Play Media"), "Play Media");
        assert_eq!(sanitize_label("Mute _Audio"), "Mute Audio");
        assert_eq!(sanitize_label("Save __ As"), "Save _ As");
        assert_eq!(sanitize_label("Simple"), "Simple");
    }

    #[test]
    fn test_dbusmenu_item_conversion() {
        let mut root_props = HashMap::new();
        root_props.insert(
            "children-display".to_string(),
            OwnedValue::try_from(Value::from("submenu")).unwrap(),
        );

        let mut item1_props = HashMap::new();
        item1_props.insert(
            "label".to_string(),
            OwnedValue::try_from(Value::from("_First Item")).unwrap(),
        );
        item1_props.insert(
            "enabled".to_string(),
            OwnedValue::try_from(Value::from(true)).unwrap(),
        );

        let mut item2_props = HashMap::new();
        item2_props.insert(
            "type".to_string(),
            OwnedValue::try_from(Value::from("separator")).unwrap(),
        );

        let mut item3_props = HashMap::new();
        item3_props.insert(
            "label".to_string(),
            OwnedValue::try_from(Value::from("Toggle Feature")).unwrap(),
        );
        item3_props.insert(
            "toggle-type".to_string(),
            OwnedValue::try_from(Value::from("checkmark")).unwrap(),
        );
        item3_props.insert(
            "toggle-state".to_string(),
            OwnedValue::try_from(Value::from(1i32)).unwrap(),
        );

        let root = DBusMenuItem {
            id: 0,
            properties: root_props,
            children: vec![
                DBusMenuItem {
                    id: 1,
                    properties: item1_props,
                    children: Vec::new(),
                },
                DBusMenuItem {
                    id: 2,
                    properties: item2_props,
                    children: Vec::new(),
                },
                DBusMenuItem {
                    id: 3,
                    properties: item3_props,
                    children: Vec::new(),
                },
            ],
        };

        let model = dbusmenu_to_menu_model(&root);
        assert_eq!(model.items.len(), 3);
        assert_eq!(model.items[0].label(), Some("First Item"));
        assert!(model.items[1].is_separator());
        match &model.items[2] {
            MenuItem::Checkbox { label, checked, .. } => {
                assert_eq!(label, "Toggle Feature");
                assert!(*checked);
            }
            _ => panic!("Expected checkbox"),
        }
    }
}
