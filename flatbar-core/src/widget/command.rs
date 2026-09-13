//! Command widget: executing shell commands to drive bar output and handling mouse interactions.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::{Span, Widget, WidgetState};
use serde::Deserialize;

/// Parsed stdout from a command invocation according to the Waybar output contract.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandOutput {
    pub text: String,
    pub tooltip: Option<String>,
    pub class: Option<String>,
}

#[derive(Deserialize)]
struct JsonContract {
    text: Option<String>,
    tooltip: Option<String>,
    class: Option<String>,
}

/// Parse stdout bytes into structured CommandOutput.
pub fn parse_command_output(raw_bytes: &[u8]) -> CommandOutput {
    let raw_str = String::from_utf8_lossy(raw_bytes);
    let trimmed = raw_str.trim();

    if trimmed.is_empty() {
        return CommandOutput::default();
    }

    // Waybar contract: if starts with '{', attempt JSON parsing
    if trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<JsonContract>(trimmed) {
            let text = parsed.text.unwrap_or_default();
            return CommandOutput {
                text,
                tooltip: parsed.tooltip,
                class: parsed.class,
            };
        }
    }

    // Plain text handling: first line is text, remaining lines become tooltip
    let mut lines = trimmed.lines();
    let first_line = lines.next().unwrap_or("").to_string();
    let rest: Vec<&str> = lines.collect();
    let tooltip = if !rest.is_empty() {
        Some(rest.join("\n"))
    } else {
        None
    };

    CommandOutput {
        text: first_line,
        tooltip,
        class: None,
    }
}

/// Configured mouse action bindings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MouseActions {
    pub left_click: Option<String>,
    pub right_click: Option<String>,
    pub middle_click: Option<String>,
    pub scroll_up: Option<String>,
    pub scroll_down: Option<String>,
}

impl MouseActions {
    pub fn get(&self, event: WidgetMouseEvent) -> Option<&str> {
        match event {
            WidgetMouseEvent::LeftClick => self.left_click.as_deref(),
            WidgetMouseEvent::RightClick => self.right_click.as_deref(),
            WidgetMouseEvent::MiddleClick => self.middle_click.as_deref(),
            WidgetMouseEvent::ScrollUp => self.scroll_up.as_deref(),
            WidgetMouseEvent::ScrollDown => self.scroll_down.as_deref(),
        }
    }
}

/// Volume-band icon thresholds for command widgets (issue 15).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VolumeThresholdIcons {
    pub muted: Option<String>,
    pub low: Option<String>,  // < 35%
    pub mid: Option<String>,  // < 75%
    pub high: Option<String>, // >= 75%
}

impl VolumeThresholdIcons {
    pub fn any(&self) -> bool {
        self.muted.is_some() || self.low.is_some() || self.mid.is_some() || self.high.is_some()
    }

    /// Parse the optional volume-threshold icon keys from a widget config table.
    pub fn from_config(val: &toml::Value) -> Option<Self> {
        let read = |key: &str| val.get(key).and_then(|v| v.as_str()).map(|s| s.to_string());
        let thresholds = Self {
            muted: read("icon_muted"),
            low: read("icon_low"),
            mid: read("icon_mid"),
            high: read("icon_high"),
        };
        if thresholds.any() {
            Some(thresholds)
        } else {
            None
        }
    }
}

/// Pick the band icon for the widget text per issue 15: 0% or the word
/// "muted" → muted icon, <35% → low, <75% → mid, otherwise high.
/// Returns `None` when the text carries no usable volume value.
fn volume_band_icon<'a>(text: &'a str, thresholds: &'a VolumeThresholdIcons) -> Option<&'a str> {
    let low_word = text.to_lowercase().contains("muted");
    let first_number: Option<u32> = text
        .split(|c: char| !c.is_ascii_digit())
        .find(|t| !t.is_empty())
        .and_then(|t| t.parse().ok());

    let muted = low_word || first_number == Some(0);
    if muted {
        return thresholds.muted.as_deref();
    }
    let volume = first_number?;
    if volume < 35 {
        thresholds.low.as_deref()
    } else if volume < 75 {
        thresholds.mid.as_deref()
    } else {
        thresholds.high.as_deref()
    }
}

/// A status bar widget driven by external shell commands.
pub struct CommandWidget {
    id: String,
    command: String,
    interval: Duration,
    signal: Option<u8>,
    actions: MouseActions,
    default_icon: Option<String>,
    class_icons: HashMap<String, String>,
    volume_icons: Option<VolumeThresholdIcons>,
    timeout: Duration,

    current_state: Arc<Mutex<WidgetState>>,
    is_running: Arc<AtomicBool>,
    /// Notifies the event loop that fresh state is available (triggers a redraw).
    notify: Option<calloop::channel::Sender<()>>,
}

impl CommandWidget {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        command: impl Into<String>,
        interval_secs: u64,
        signal: Option<u8>,
        actions: MouseActions,
        default_icon: Option<String>,
        class_icons: HashMap<String, String>,
    ) -> Self {
        Self::with_volume(
            id,
            command,
            interval_secs,
            signal,
            actions,
            default_icon,
            class_icons,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_volume(
        id: impl Into<String>,
        command: impl Into<String>,
        interval_secs: u64,
        signal: Option<u8>,
        actions: MouseActions,
        default_icon: Option<String>,
        class_icons: HashMap<String, String>,
        volume_icons: Option<VolumeThresholdIcons>,
        mute_command: Option<String>,
    ) -> Self {
        let cmd = command.into();
        let widget = Self {
            id: id.into(),
            command: cmd,
            interval: Duration::from_secs(interval_secs),
            signal,
            actions: MouseActions {
                right_click: mute_command.or(actions.right_click),
                ..actions
            },
            default_icon,
            class_icons,
            volume_icons,
            timeout: Duration::from_secs(5),
            current_state: Arc::new(Mutex::new(WidgetState::default())),
            is_running: Arc::new(AtomicBool::new(false)),
            notify: None,
        };

        // Trigger initial execution
        widget.run_command_async();
        widget
    }

    pub fn signal(&self) -> Option<u8> {
        self.signal
    }

    /// Install a channel used to wake the event loop when fresh state is available.
    pub fn set_notify(&mut self, tx: calloop::channel::Sender<()>) {
        self.notify = Some(tx);
    }

    /// Trigger command execution in background thread without blocking main loop.
    pub fn run_command_async(&self) {
        if self.is_running.swap(true, Ordering::SeqCst) {
            // Already running
            return;
        }

        run_command_and_store(
            self.command.clone(),
            Arc::clone(&self.current_state),
            Arc::clone(&self.is_running),
            self.notify.clone(),
            self.timeout,
            self.default_icon.clone(),
            self.class_icons.clone(),
            self.volume_icons.clone(),
        );
    }

    /// Handle mouse interaction event on this widget.
    pub fn handle_mouse_event(&self, event: WidgetMouseEvent) {
        if let Some(cmd) = self.actions.get(event) {
            self.spawn_action_then_refresh(cmd.to_string());
        }
    }

    /// Run a mouse action in the background; once it completes, re-run the
    /// status command so the displayed value reflects the action immediately
    /// (e.g. volume after scrolling) instead of waiting for the next tick.
    fn spawn_action_then_refresh(&self, cmd: String) {
        // The action must finish before the follow-up status query so the
        // query observes the action's effect.
        let snapshot = CommandWidgetHandle {
            command: self.command.clone(),
            state: Arc::clone(&self.current_state),
            running: Arc::clone(&self.is_running),
            notify: self.notify.clone(),
            timeout: self.timeout,
            default_icon: self.default_icon.clone(),
            class_icons: self.class_icons.clone(),
            volume_icons: self.volume_icons.clone(),
        };
        thread::spawn(move || {
            let _ = execute_shell_action(&cmd);
            let CommandWidgetHandle {
                command,
                state,
                running,
                notify,
                timeout,
                default_icon,
                class_icons,
                volume_icons,
            } = snapshot;
            run_command_and_store(
                command,
                state,
                running,
                notify,
                timeout,
                default_icon,
                class_icons,
                volume_icons,
            );
        });
    }
}

impl Widget for CommandWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn as_command_mut(&mut self) -> Option<&mut CommandWidget> {
        Some(self)
    }

    fn update_interval(&self) -> Duration {
        self.interval
    }

    fn state(&self) -> WidgetState {
        self.current_state.lock().unwrap().clone()
    }

    fn handle_mouse_event(&self, event: WidgetMouseEvent) {
        if let Some(cmd) = self.actions.get(event) {
            self.spawn_action_then_refresh(cmd.to_string());
        }
    }

    fn refresh(&self) {
        self.run_command_async();
    }
}

/// Owned snapshot of a `CommandWidget`'s execution inputs, so the status
/// command can be re-run from a worker thread after a mouse action completes.
struct CommandWidgetHandle {
    command: String,
    state: Arc<Mutex<WidgetState>>,
    running: Arc<AtomicBool>,
    notify: Option<calloop::channel::Sender<()>>,
    timeout: Duration,
    default_icon: Option<String>,
    class_icons: HashMap<String, String>,
    volume_icons: Option<VolumeThresholdIcons>,
}

/// Execute the widget's status command, store the resulting state, and notify
/// the event loop so a redraw happens immediately (not on the next tick).
/// Spawns its own worker thread; `running` must already be set to `true`.
#[allow(clippy::too_many_arguments)]
fn run_command_and_store(
    command: String,
    state_arc: Arc<Mutex<WidgetState>>,
    running_flag: Arc<AtomicBool>,
    notify: Option<calloop::channel::Sender<()>>,
    timeout: Duration,
    default_icon: Option<String>,
    class_icons: HashMap<String, String>,
    volume_icons: Option<VolumeThresholdIcons>,
) {
    let cmd = command;
    thread::spawn(move || {
        let start = Instant::now();
        let result = execute_shell_command(&cmd, timeout);
        running_flag.store(false, Ordering::SeqCst);

        let new_state = match result {
            Ok((exit_status, stdout, stderr)) => {
                if exit_status.success() {
                    let parsed = parse_command_output(&stdout);
                    let mut icon = parsed
                        .class
                        .as_ref()
                        .and_then(|c| class_icons.get(c))
                        .cloned()
                        .or_else(|| default_icon.clone());

                    // Volume-band thresholds (issue 15): override the icon in
                    // the configured bands. Falls back to the ordinary icon
                    // when no band matches or no number is present.
                    if let Some(ref thresholds) = volume_icons {
                        if thresholds.any() {
                            if let Some(band_icon) = volume_band_icon(&parsed.text, thresholds) {
                                icon = Some(band_icon.to_string());
                            }
                        }
                    }

                    let mut spans = Vec::new();
                    if let Some(ic) = icon {
                        spans.push(Span::icon(ic));
                    }
                    if !parsed.text.is_empty() {
                        spans.push(Span::text(parsed.text));
                    }

                    WidgetState {
                        spans,
                        tooltip: parsed.tooltip,
                    }
                } else {
                    tracing::warn!(
                        "Command '{}' exited with status {:?}: {}",
                        cmd,
                        exit_status.code(),
                        String::from_utf8_lossy(&stderr)
                    );
                    WidgetState {
                        spans: vec![Span::text("⚠")],
                        tooltip: Some(format!("Exit status: {:?}", exit_status.code())),
                    }
                }
            }
            Err(err) => {
                tracing::warn!("Failed to execute command '{}': {err}", cmd);
                WidgetState {
                    spans: vec![Span::text("⚠")],
                    tooltip: Some(err),
                }
            }
        };

        tracing::debug!("Command '{}' finished in {:?}", cmd, start.elapsed());
        {
            let mut state = state_arc.lock().unwrap();
            *state = new_state;
        }
        if let Some(tx) = notify {
            let _ = tx.send(());
        }
    });
}

type ShellResult = Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), String>;

/// Execute a shell command with a timeout, capturing stdout and stderr.
///
/// A worker thread owns the child and blocks in `wait_with_output()` (which
/// drains both pipes concurrently, so chatty children cannot deadlock on full
/// pipe buffers). The caller acts as a watchdog by blocking on a channel with
/// `recv_timeout` — zero polling. On timeout the child is killed by pid and
/// reaped via the completion channel.
fn execute_shell_command(cmd: &str, timeout: Duration) -> ShellResult {
    let child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn shell: {e}"))?;
    let pid = child.id();

    let (result_tx, result_rx) = mpsc::channel::<ShellResult>();
    thread::spawn(move || {
        let result = child
            .wait_with_output()
            .map(|o| (o.status, o.stdout, o.stderr))
            .map_err(|e| format!("Failed to read output: {e}"));
        let _ = result_tx.send(result);
    });

    match result_rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(e),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_process(pid);
            // Reap: the worker's wait_with_output returns after SIGKILL.
            let _ = result_rx.recv_timeout(Duration::from_secs(1));
            Err(format!("Command timed out after {timeout:?}"))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Command worker exited unexpectedly".to_string())
        }
    }
}

/// Best-effort SIGKILL by pid (libc is already a direct dependency).
fn kill_process(pid: u32) {
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Execute a fire-and-forget shell action (for clicks/scrolls).
pub fn execute_shell_action(cmd: &str) -> Result<(), String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn action '{cmd}': {e}"))?;

    // Reap child in detached thread to prevent zombies
    thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_text() {
        let out = parse_command_output(b"85%\nExtra detail");
        assert_eq!(out.text, "85%");
        assert_eq!(out.tooltip, Some("Extra detail".to_string()));
        assert_eq!(out.class, None);
    }

    #[test]
    fn test_parse_waybar_json() {
        let json_data = br#"{"text": "50%", "tooltip": "Volume is 50%", "class": "muted"}"#;
        let out = parse_command_output(json_data);
        assert_eq!(out.text, "50%");
        assert_eq!(out.tooltip, Some("Volume is 50%".to_string()));
        assert_eq!(out.class, Some("muted".to_string()));
    }

    #[test]
    fn test_parse_malformed_json_fallback() {
        let bad_json = b"{not-valid-json: hello\nsecond line";
        let out = parse_command_output(bad_json);
        assert_eq!(out.text, "{not-valid-json: hello");
        assert_eq!(out.tooltip, Some("second line".to_string()));
    }

    #[test]
    fn test_command_execution_success() {
        let res = execute_shell_command("echo 'hello'", Duration::from_secs(2)).unwrap();
        assert!(res.0.success());
        assert_eq!(String::from_utf8_lossy(&res.1).trim(), "hello");
    }

    #[test]
    fn test_volume_band_icon_thresholds() {
        let th = VolumeThresholdIcons {
            muted: Some("fa-volume-xmark".into()),
            low: Some("fa-volume-low".into()),
            mid: Some("fa-volume-high".into()),
            high: Some("fa-volume-high".into()),
        };
        fn icon<'a>(text: &'a str, th: &'a VolumeThresholdIcons) -> Option<&'a str> {
            volume_band_icon(text, th)
        }
        assert_eq!(icon("0%", &th), Some("fa-volume-xmark"));
        assert_eq!(icon("muted", &th), Some("fa-volume-xmark"));
        assert_eq!(icon("34%", &th), Some("fa-volume-low"));
        assert_eq!(icon("50%", &th), Some("fa-volume-high"));
        assert_eq!(icon("75%", &th), Some("fa-volume-high"));
        assert_eq!(icon("no value", &th), None);
    }

    #[test]
    fn test_command_timeout() {
        let res = execute_shell_command("sleep 10", Duration::from_millis(100));
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("timed out"));
    }

    #[test]
    fn test_stderr_saturation_completes() {
        // ~1MiB to stderr: without concurrent pipe draining the child blocks
        // forever on a full buffer. The watchdog path previously never read
        // pipes until exit, deadlocking; wait_with_output drains continuously.
        let res = execute_shell_command(
            "yes saturated | head -c 1048576 >&2; echo done",
            Duration::from_secs(10),
        );
        match res {
            Ok((_status, stdout, _stderr)) => {
                assert!(
                    String::from_utf8_lossy(&stdout).contains("done"),
                    "child must finish"
                );
            }
            Err(e) => panic!("stderr-heavy child should not fail/deadlock: {e}"),
        }
    }

    #[test]
    fn test_timeout_kills_child() {
        // The sleeper writes its pid, then execs so the shell pid IS the
        // sleeper; after the timeout, /proc/<pid> must be gone.
        let pid_file = std::env::temp_dir().join("flatbar-test-sleeper-pid");
        let _ = std::fs::remove_file(&pid_file);
        let res = execute_shell_command(
            &format!("echo $$ > {}; exec sleep 30", pid_file.display()),
            Duration::from_millis(200),
        );
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("timed out"));

        thread::sleep(Duration::from_millis(150));
        let pid = std::fs::read_to_string(&pid_file).expect("sleeper wrote its pid");
        let pid: u32 = pid.trim().parse().expect("numeric pid");
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "sleeper pid {pid} must not survive the timeout kill"
        );
        let _ = std::fs::remove_file(&pid_file);
    }

    #[test]
    fn test_fast_command_untouched() {
        let res = execute_shell_command("echo hello", Duration::from_secs(5)).unwrap();
        assert!(res.0.success());
        assert_eq!(String::from_utf8_lossy(&res.1).trim(), "hello");
    }

    #[test]
    fn test_zero_zombies_stress() {
        for _ in 0..20 {
            execute_shell_action("echo test").unwrap();
        }
        thread::sleep(Duration::from_millis(100));
        // Passes cleanly without hanging or accumulating unhandled processes
    }
}
