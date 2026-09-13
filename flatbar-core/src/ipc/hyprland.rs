//! Hyprland IPC client implementation for workspace queries, event streaming, and dispatch actions.

use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ipc::provider::{WorkspaceEvent, WorkspaceInfo, WorkspaceProvider};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HyprWorkspaceRaw {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub monitor: String,
    #[serde(default)]
    pub windows: i32,
    #[serde(default)]
    pub urgent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HyprActiveWorkspaceRaw {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub monitor: String,
}

/// Hyprland IPC Client.
#[derive(Debug, Clone)]
pub struct HyprlandClient {
    base_dir: Option<PathBuf>,
}

impl Default for HyprlandClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HyprlandClient {
    pub fn new() -> Self {
        let base_dir = env::var_os("HYPRLAND_INSTANCE_SIGNATURE").map(|sig| {
            let xdg = env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            xdg.join("hypr").join(sig)
        });

        Self { base_dir }
    }

    pub fn with_base_dir(dir: impl AsRef<Path>) -> Self {
        Self {
            base_dir: Some(dir.as_ref().to_path_buf()),
        }
    }

    pub fn command_socket_path(&self) -> Result<PathBuf, String> {
        let dir = self
            .base_dir
            .as_ref()
            .ok_or_else(|| "HYPRLAND_INSTANCE_SIGNATURE is not set".to_string())?;
        Ok(dir.join(".socket.sock"))
    }

    pub fn event_socket_path(&self) -> Result<PathBuf, String> {
        let dir = self
            .base_dir
            .as_ref()
            .ok_or_else(|| "HYPRLAND_INSTANCE_SIGNATURE is not set".to_string())?;
        Ok(dir.join(".socket2.sock"))
    }

    fn send_command(&self, cmd: &str) -> Result<String, String> {
        let path = self.command_socket_path()?;
        let mut stream = UnixStream::connect(&path)
            .map_err(|e| format!("Failed to connect to {path:?}: {e}"))?;

        stream
            .write_all(cmd.as_bytes())
            .map_err(|e| format!("Failed to write to Hyprland command socket: {e}"))?;

        let mut reply = String::new();
        stream
            .read_to_string(&mut reply)
            .map_err(|e| format!("Failed to read reply from Hyprland command socket: {e}"))?;

        Ok(reply)
    }

    /// Execute a command or dispatch on Hyprland IPC socket.
    pub fn run_command(&self, cmd: &str) -> Result<String, String> {
        self.send_command(cmd)
    }
}

impl WorkspaceProvider for HyprlandClient {
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, String> {
        let ws_reply = self.send_command("j/workspaces")?;
        let active_reply = self.send_command("j/activeworkspace").unwrap_or_default();

        let mut raw_workspaces: Vec<HyprWorkspaceRaw> = serde_json::from_str(&ws_reply)
            .map_err(|e| format!("Failed to parse Hyprland workspaces JSON: {e}"))?;

        let active_id: Option<i64> = serde_json::from_str::<HyprActiveWorkspaceRaw>(&active_reply)
            .ok()
            .map(|a| a.id);

        raw_workspaces.sort_by_key(|w| w.id);

        let workspaces = raw_workspaces
            .into_iter()
            .map(|w| {
                let focused = active_id == Some(w.id);
                WorkspaceInfo {
                    id: w.id.to_string(),
                    name: w.name,
                    focused,
                    urgent: w.urgent,
                    monitor: w.monitor,
                }
            })
            .collect();

        Ok(workspaces)
    }

    fn focus(&self, id_or_name: &str) -> Result<(), String> {
        let cmd = format!("dispatch workspace {id_or_name}");
        let _ = self.send_command(&cmd)?;
        Ok(())
    }

    fn subscribe(&self, tx: Sender<WorkspaceEvent>) -> Result<(), String> {
        let client = self.clone();

        thread::Builder::new()
            .name("flatbar-hyprland-ipc".to_string())
            .spawn(move || {
                let mut backoff = Duration::from_millis(250);
                const MAX_BACKOFF: Duration = Duration::from_secs(5);

                loop {
                    let socket_path = match client.event_socket_path() {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::debug!("Hyprland socket path error: {e}");
                            thread::sleep(backoff);
                            continue;
                        }
                    };

                    match UnixStream::connect(&socket_path) {
                        Ok(stream) => {
                            backoff = Duration::from_millis(250);

                            // Initial state
                            if let Ok(ws) = client.workspaces() {
                                let _ = tx.send(WorkspaceEvent::Full(ws));
                            }

                            let reader = BufReader::new(stream);
                            for line in reader.lines() {
                                match line {
                                    Ok(l) => {
                                        if l.starts_with("workspace>>")
                                            || l.starts_with("focusedmon>>")
                                            || l.starts_with("createworkspace>>")
                                            || l.starts_with("destroyworkspace>>")
                                            || l.starts_with("urgent>>")
                                        {
                                            if let Ok(ws) = client.workspaces() {
                                                if tx.send(WorkspaceEvent::Full(ws)).is_err() {
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        tracing::warn!("Hyprland event socket read error: {err}");
                                        break;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            tracing::debug!("Failed to connect to Hyprland event socket: {err}");
                        }
                    }

                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            })
            .map_err(|e| format!("Failed to spawn Hyprland IPC thread: {e}"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hyprland_workspaces_and_active() {
        let ws_json = r#"[
            {
                "id": 1,
                "name": "1",
                "monitor": "DP-1",
                "monitorID": 0,
                "windows": 2,
                "hasfullscreen": false,
                "lastwindow": "0x55c",
                "lastwindowtitle": "Terminal"
            },
            {
                "id": 2,
                "name": "2",
                "monitor": "DP-1",
                "monitorID": 0,
                "windows": 1,
                "hasfullscreen": false,
                "lastwindow": "0x55d",
                "lastwindowtitle": "Browser"
            }
        ]"#;

        let active_json = r#"{
            "id": 2,
            "name": "2",
            "monitor": "DP-1"
        }"#;

        let raw_workspaces: Vec<HyprWorkspaceRaw> = serde_json::from_str(ws_json).unwrap();
        let active: HyprActiveWorkspaceRaw = serde_json::from_str(active_json).unwrap();

        let mut list: Vec<WorkspaceInfo> = raw_workspaces
            .into_iter()
            .map(|w| WorkspaceInfo {
                focused: w.id == active.id,
                id: w.id.to_string(),
                name: w.name,
                urgent: w.urgent,
                monitor: w.monitor,
            })
            .collect();
        list.sort_by_key(|w| w.id.clone());

        assert_eq!(list.len(), 2);
        assert!(!list[0].focused);
        assert!(list[1].focused);
    }
}
