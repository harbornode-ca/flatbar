//! Compositor window-rules generator and runtime injector.

use crate::config::window_rules::{WindowMode, WindowRule, WindowRulesConfig};
use crate::ipc::hyprland::HyprlandClient;
use crate::ipc::provider::{detect_compositor, CompositorType};
use crate::ipc::sway::SwayClient;
use std::collections::HashMap;

/// Generate Sway/i3 IPC command string for a specific window rule.
pub fn generate_sway_rule(app_id: &str, rule: &WindowRule) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(mode) = rule.mode {
        match mode {
            WindowMode::Floating => parts.push("floating enable".to_string()),
            WindowMode::Tiling => parts.push("floating disable".to_string()),
        }
    }

    if let Some((w, h)) = rule.parse_size() {
        parts.push(format!("resize set {w} {h}"));
    }

    if let Some(ws) = &rule.workspace {
        parts.push(format!("move to workspace \"{ws}\""));
    }

    if parts.is_empty() {
        None
    } else {
        Some(format!(
            "for_window [app_id=\"{app_id}\"] {}",
            parts.join(", ")
        ))
    }
}

/// Generate Hyprland dispatch commands for a specific window rule.
pub fn generate_hyprland_rules(app_id: &str, rule: &WindowRule) -> Vec<String> {
    let mut cmds = Vec::new();
    let escaped_id = regex_escape(app_id);

    if let Some(mode) = rule.mode {
        match mode {
            WindowMode::Floating => {
                cmds.push(format!("dispatch windowrule \"float,^({escaped_id})$\""));
            }
            WindowMode::Tiling => {
                cmds.push(format!("dispatch windowrule \"tile,^({escaped_id})$\""));
            }
        }
    }

    if let Some((w, h)) = rule.parse_size() {
        cmds.push(format!(
            "dispatch windowrule \"size {w} {h},^({escaped_id})$\""
        ));
    }

    if let Some(ws) = &rule.workspace {
        cmds.push(format!(
            "dispatch windowrule \"workspace {ws},^({escaped_id})$\""
        ));
    }

    cmds
}

/// Generate Niri KDL window-rules snippet for all configured rules.
pub fn generate_niri_snippet(rules: &HashMap<String, WindowRule>) -> String {
    let mut out = String::new();
    out.push_str("// Flatbar generated window rules for Niri\n");
    out.push_str("// Paste these blocks into your ~/.config/niri/config.kdl\n\n");

    let mut sorted_keys: Vec<&String> = rules.keys().collect();
    sorted_keys.sort();

    for app_id in sorted_keys {
        if let Some(rule) = rules.get(app_id) {
            out.push_str("window-rule {\n");
            out.push_str(&format!(
                "    match app-id=r#\"^{}$\"#\n",
                regex_escape(app_id)
            ));

            if let Some(mode) = rule.mode {
                match mode {
                    WindowMode::Floating => out.push_str("    open-floating true\n"),
                    WindowMode::Tiling => out.push_str("    open-floating false\n"),
                }
            }

            if let Some((w, h)) = rule.parse_size() {
                out.push_str(&format!("    default-floating-size {w} {h}\n"));
            }

            if let Some(ws) = &rule.workspace {
                out.push_str(&format!("    open-on-workspace \"{ws}\"\n"));
            }

            out.push_str("}\n\n");
        }
    }

    out
}

/// Inject runtime window rules into the active compositor before or upon launching an application.
pub fn apply_runtime_rule(app_id: &str, window_rules: &WindowRulesConfig) {
    let rule = match window_rules.rules.get(app_id) {
        Some(r) => r,
        None => return,
    };

    match detect_compositor() {
        Some(CompositorType::Sway) => {
            if let Some(cmd) = generate_sway_rule(app_id, rule) {
                let client = SwayClient::new();
                if let Err(e) = client.run_command(&cmd) {
                    tracing::debug!("Failed to inject Sway window rule for {app_id}: {e}");
                }
            }
        }
        Some(CompositorType::Hyprland) => {
            let client = HyprlandClient::new();
            for cmd in generate_hyprland_rules(app_id, rule) {
                if let Err(e) = client.run_command(&cmd) {
                    tracing::debug!("Failed to inject Hyprland window rule for {app_id}: {e}");
                }
            }
        }
        Some(CompositorType::Niri) => {
            // Niri is declarative only; rules are configured in niri config.kdl
        }
        None => {}
    }
}

fn regex_escape(s: &str) -> String {
    s.replace('.', "\\.")
        .replace('-', "\\-")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_sway_rule() {
        let rule = WindowRule {
            mode: Some(WindowMode::Floating),
            size: Some("800x600".to_string()),
            position: None,
            workspace: Some("2".to_string()),
        };

        let cmd = generate_sway_rule("flatbar.bluetooth", &rule).unwrap();
        assert_eq!(
            cmd,
            "for_window [app_id=\"flatbar.bluetooth\"] floating enable, resize set 800 600, move to workspace \"2\""
        );
    }

    #[test]
    fn test_generate_hyprland_rules() {
        let rule = WindowRule {
            mode: Some(WindowMode::Floating),
            size: Some("800x600".to_string()),
            position: None,
            workspace: None,
        };

        let cmds = generate_hyprland_rules("flatbar.bluetooth", &rule);
        assert_eq!(cmds.len(), 2);
        assert_eq!(
            cmds[0],
            "dispatch windowrule \"float,^(flatbar\\.bluetooth)$\""
        );
        assert_eq!(
            cmds[1],
            "dispatch windowrule \"size 800 600,^(flatbar\\.bluetooth)$\""
        );
    }

    #[test]
    fn test_generate_niri_snippet() {
        let mut rules = HashMap::new();
        rules.insert(
            "flatbar.bluetooth".to_string(),
            WindowRule {
                mode: Some(WindowMode::Floating),
                size: Some("800x600".to_string()),
                position: None,
                workspace: None,
            },
        );

        let snippet = generate_niri_snippet(&rules);
        assert!(snippet.contains("window-rule {"));
        assert!(snippet.contains("match app-id=r#\"^flatbar\\.bluetooth$\"#"));
        assert!(snippet.contains("open-floating true"));
        assert!(snippet.contains("default-floating-size 800 600"));
    }
}
