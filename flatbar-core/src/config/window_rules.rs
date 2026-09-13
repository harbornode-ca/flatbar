//! Window rules configuration for launched applications.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Window mode for an application window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    Floating,
    Tiling,
}

/// Window rule definition for an app-id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WindowRule {
    #[serde(default)]
    pub mode: Option<WindowMode>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub position: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
}

impl WindowRule {
    /// Parse size string like "800x600" into (width, height).
    pub fn parse_size(&self) -> Option<(u32, u32)> {
        let s = self.size.as_deref()?;
        let parts: Vec<&str> = s.split('x').collect();
        if parts.len() == 2 {
            let w = parts[0].trim().parse::<u32>().ok()?;
            let h = parts[1].trim().parse::<u32>().ok()?;
            Some((w, h))
        } else {
            None
        }
    }

    /// Parse position string like "100,200" into (x, y).
    pub fn parse_position(&self) -> Option<(i32, i32)> {
        let s = self.position.as_deref()?;
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() == 2 {
            let x = parts[0].trim().parse::<i32>().ok()?;
            let y = parts[1].trim().parse::<i32>().ok()?;
            Some((x, y))
        } else {
            None
        }
    }
}

/// Top-level `[window_rules]` section in config.toml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRulesConfig {
    #[serde(default = "default_terminal")]
    pub terminal: String,
    #[serde(flatten)]
    pub rules: HashMap<String, WindowRule>,
}

impl Default for WindowRulesConfig {
    fn default() -> Self {
        Self {
            terminal: default_terminal(),
            rules: HashMap::new(),
        }
    }
}

fn default_terminal() -> String {
    "foot".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_window_rule_size_and_position() {
        let rule = WindowRule {
            mode: Some(WindowMode::Floating),
            size: Some("800x600".to_string()),
            position: Some("100,200".to_string()),
            workspace: Some("1".to_string()),
        };

        assert_eq!(rule.mode, Some(WindowMode::Floating));
        assert_eq!(rule.parse_size(), Some((800, 600)));
        assert_eq!(rule.parse_position(), Some((100, 200)));
        assert_eq!(rule.workspace.as_deref(), Some("1"));
    }

    #[test]
    fn test_parse_window_rules_section() {
        let toml_str = r#"
            terminal = "alacritty"
            "flatbar.bluetooth" = { size = "800x600", mode = "floating" }
            "flatbar.stats" = { mode = "tiling" }
        "#;

        let config: WindowRulesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.terminal, "alacritty");
        let bt = config.rules.get("flatbar.bluetooth").unwrap();
        assert_eq!(bt.mode, Some(WindowMode::Floating));
        assert_eq!(bt.parse_size(), Some((800, 600)));

        let stats = config.rules.get("flatbar.stats").unwrap();
        assert_eq!(stats.mode, Some(WindowMode::Tiling));
    }
}
