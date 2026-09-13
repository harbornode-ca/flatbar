//! Application launcher with compositor window-rule injection and terminal embedding.

use crate::config::window_rules::WindowRulesConfig;
use crate::dbus::notify::send_notification;
use crate::ipc::window_rules::apply_runtime_rule;
use std::process::{Command, Stdio};
use std::thread;

/// Format a terminal command invocation that sets the specified `app_id` / class.
pub fn format_terminal_command(
    terminal: &str,
    app_id: &str,
    command: &[&str],
) -> (String, Vec<String>) {
    let term_lower = terminal.to_ascii_lowercase();

    if term_lower == "foot" || term_lower.ends_with("/foot") {
        let mut args = vec!["-a".to_string(), app_id.to_string()];
        args.extend(command.iter().map(|s| s.to_string()));
        (terminal.to_string(), args)
    } else if term_lower == "alacritty" || term_lower.ends_with("/alacritty") {
        let mut args = vec![
            "--class".to_string(),
            format!("{app_id},{app_id}"),
            "-e".to_string(),
        ];
        args.extend(command.iter().map(|s| s.to_string()));
        (terminal.to_string(), args)
    } else if term_lower == "kitty" || term_lower.ends_with("/kitty") {
        let mut args = vec!["--class".to_string(), app_id.to_string()];
        args.extend(command.iter().map(|s| s.to_string()));
        (terminal.to_string(), args)
    } else if term_lower == "wezterm" || term_lower.ends_with("/wezterm") {
        let mut args = vec![
            "start".to_string(),
            "--class".to_string(),
            app_id.to_string(),
            "--".to_string(),
        ];
        args.extend(command.iter().map(|s| s.to_string()));
        (terminal.to_string(), args)
    } else if term_lower == "ghostty" || term_lower.ends_with("/ghostty") {
        let mut args = vec![format!("--class-name={app_id}"), "-e".to_string()];
        args.extend(command.iter().map(|s| s.to_string()));
        (terminal.to_string(), args)
    } else {
        // Generic fallback terminal
        let mut args = vec!["-e".to_string()];
        args.extend(command.iter().map(|s| s.to_string()));
        (terminal.to_string(), args)
    }
}

/// Replace a bare executable name (no path component) with a same-named
/// binary sitting next to the running flatbar executable, when one exists.
/// This covers locally-built companion tools without relying on PATH.
/// Commands that already include a path separator are used as-is;
/// otherwise `Command::spawn` resolves via PATH.
fn resolve_built_binary(command: &mut [String]) {
    let Some(name) = command.first() else {
        return;
    };
    if name.trim().is_empty() || name.contains('/') {
        return;
    }
    let name = name.clone();
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let candidate = dir.join(&name);
    if is_executable(&candidate) {
        tracing::debug!(
            "Using companion binary {} for '{}'",
            candidate.display(),
            name
        );
        command[0] = candidate.to_string_lossy().into_owned();
    }
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Launch a TUI command in the configured terminal with window rules applied.
pub fn launch_tui(app_id: &str, command: &[&str], window_rules: &WindowRulesConfig) {
    if command.is_empty() {
        return;
    }

    let mut command: Vec<String> = command.iter().map(|s| s.to_string()).collect();
    resolve_built_binary(&mut command);

    // 1. Inject runtime window rules (Sway / Hyprland)
    apply_runtime_rule(app_id, window_rules);

    // 2. Build terminal command
    let (prog, args) = format_terminal_command(
        &window_rules.terminal,
        app_id,
        &command.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );

    // 3. Spawn detached process
    match Command::new(&prog).args(&args).spawn() {
        Ok(_) => {
            tracing::info!(
                "Launched TUI app '{}' in terminal '{}' with command {:?}",
                app_id,
                prog,
                command
            );
        }
        Err(e) => {
            let prog_name = command[0].clone();
            tracing::error!("Failed to launch '{prog_name}' via terminal '{prog}': {e}");
            let _ = send_notification(
                "Flatbar",
                &format!("Could not launch '{prog_name}': {e}\nPlease ensure '{prog}' and '{prog_name}' are installed."),
                "dialog-error",
            );
        }
    }
}

/// Launch a GUI command with window rules applied.
pub fn launch_gui(app_id: &str, command: &[&str], window_rules: &WindowRulesConfig) {
    if command.is_empty() {
        return;
    }

    apply_runtime_rule(app_id, window_rules);

    let prog = command[0];
    let args = &command[1..];

    match Command::new(prog).args(args).spawn() {
        Ok(_) => {
            tracing::info!("Launched GUI app '{app_id}' with command {:?}", command);
        }
        Err(e) => {
            tracing::error!("Failed to launch GUI app '{prog}': {e}");
            let _ = send_notification(
                "Flatbar",
                &format!("Could not launch '{prog}': {e}"),
                "dialog-error",
            );
        }
    }
}

/// Copy text to the system clipboard via `wl-copy` and surface a short
/// confirmation notification. Failures are logged and never crash the bar.
pub fn copy_to_clipboard(text: &str) {
    let child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to spawn wl-copy for clipboard copy: {e}");
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
        // Dropping stdin closes the pipe; wl-copy then takes ownership.
    }

    // Reap wl-copy in a detached thread; the daemon stays alive holding the
    // clipboard beyond our process lifetime is wl-copy's own concern.
    thread::spawn(move || {
        let _ = child.wait();
    });

    let preview: String = text.chars().take(40).collect();
    let preview = if text.chars().count() > 40 {
        format!("{preview}…")
    } else {
        preview
    };
    let _ = send_notification(
        "Flatbar",
        &format!("Copied: {preview}"),
        "dialog-information",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_terminal_command_foot() {
        let (prog, args) = format_terminal_command("foot", "flatbar.bluetooth", &["bluetui"]);
        assert_eq!(prog, "foot");
        assert_eq!(args, vec!["-a", "flatbar.bluetooth", "bluetui"]);
    }

    #[test]
    fn test_format_terminal_command_alacritty() {
        let (prog, args) = format_terminal_command("alacritty", "flatbar.stats", &["btop"]);
        assert_eq!(prog, "alacritty");
        assert_eq!(
            args,
            vec!["--class", "flatbar.stats,flatbar.stats", "-e", "btop"]
        );
    }

    #[test]
    fn test_format_terminal_command_kitty() {
        let (prog, args) = format_terminal_command("kitty", "flatbar.network", &["nmtui"]);
        assert_eq!(prog, "kitty");
        assert_eq!(args, vec!["--class", "flatbar.network", "nmtui"]);
    }

    #[test]
    fn test_resolve_built_binary_leaves_explicit_paths_alone() {
        let mut cmd = vec!["/usr/bin/unknown-tui".to_string(), "--flag".to_string()];
        resolve_built_binary(&mut cmd);
        assert_eq!(cmd[0], "/usr/bin/unknown-tui");
    }

    #[test]
    fn test_resolve_built_binary_ignores_bare_names_without_local_build() {
        // No binary with this name sits next to the test binary; PATH resolution
        // must remain untouched.
        let mut cmd = vec!["definitely-not-a-built-tool-xyz".to_string()];
        resolve_built_binary(&mut cmd);
        assert_eq!(cmd[0], "definitely-not-a-built-tool-xyz");
    }
}
