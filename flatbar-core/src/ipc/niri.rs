//! Niri IPC client implementation for workspace queries and event stream subscription.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ipc::provider::{WorkspaceEvent, WorkspaceInfo, WorkspaceProvider};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NiriWorkspaceRaw {
    pub id: u64,
    pub idx: Option<usize>,
    pub name: Option<String>,
    pub output: Option<String>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_focused: bool,
    #[serde(default)]
    pub is_urgent: bool,
}

/// Niri IPC Client.
#[derive(Debug, Clone)]
pub struct NiriClient {
    socket_path: Option<PathBuf>,
}

impl Default for NiriClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NiriClient {
    pub fn new() -> Self {
        let socket_path = env::var_os("NIRI_SOCKET").map(PathBuf::from);
        Self { socket_path }
    }

    pub fn with_socket_path(path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: Some(path.as_ref().to_path_buf()),
        }
    }

    pub fn get_socket_path(&self) -> Result<&Path, String> {
        self.socket_path
            .as_deref()
            .ok_or_else(|| "NIRI_SOCKET is not set".to_string())
    }

    fn send_request(&self, req: &str) -> Result<String, String> {
        let path = self.get_socket_path()?;
        let mut stream =
            UnixStream::connect(path).map_err(|e| format!("Failed to connect to {path:?}: {e}"))?;

        let mut req_str = req.to_string();
        if !req_str.ends_with('\n') {
            req_str.push('\n');
        }

        stream
            .write_all(req_str.as_bytes())
            .map_err(|e| format!("Failed to write to Niri IPC socket: {e}"))?;

        let mut reader = BufReader::new(stream);
        let mut reply = String::new();
        reader
            .read_line(&mut reply)
            .map_err(|e| format!("Failed to read reply from Niri IPC socket: {e}"))?;

        Ok(reply)
    }
    pub fn parse_workspaces_json(reply: &str) -> Result<Vec<WorkspaceInfo>, String> {
        let val: serde_json::Value = serde_json::from_str(reply)
            .map_err(|e| format!("Failed to parse Niri JSON response: {e}"))?;

        let ws_array = if let Some(arr) = val.get("Ok").and_then(|ok| ok.get("Workspaces")) {
            arr.as_array()
        } else if let Some(arr) = val.get("Workspaces") {
            arr.as_array()
        } else {
            val.as_array()
        };

        let mut raw_workspaces: Vec<NiriWorkspaceRaw> = match ws_array {
            Some(arr) => serde_json::from_value(serde_json::Value::Array(arr.clone()))
                .map_err(|e| format!("Failed to parse Niri workspaces array: {e}"))?,
            None => Vec::new(),
        };

        raw_workspaces.sort_by_key(|w| (w.idx.unwrap_or(usize::MAX), w.id));

        let workspaces: Vec<WorkspaceInfo> = raw_workspaces
            .into_iter()
            .map(|w| {
                let name = w.name.unwrap_or_else(|| {
                    w.idx
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| w.id.to_string())
                });
                WorkspaceInfo {
                    id: w.id.to_string(),
                    name,
                    focused: w.is_focused || w.is_active,
                    urgent: w.is_urgent,
                    monitor: w.output.unwrap_or_default(),
                }
            })
            .collect();

        Ok(workspaces)
    }
}

impl WorkspaceProvider for NiriClient {
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, String> {
        let reply = self.send_request("\"Workspaces\"")?;
        Self::parse_workspaces_json(&reply)
    }

    fn focus(&self, id_or_name: &str) -> Result<(), String> {
        let req = if let Ok(id_num) = id_or_name.parse::<u64>() {
            format!(r#"{{"Action":{{"FocusWorkspace":{{"reference":{{"Id":{id_num}}}}}}}}}"#)
        } else {
            format!(
                r#"{{"Action":{{"FocusWorkspace":{{"reference":{{"Name":"{id_or_name}"}}}}}}}}"#
            )
        };
        let _ = self.send_request(&req)?;
        Ok(())
    }

    fn subscribe(&self, tx: Sender<WorkspaceEvent>) -> Result<(), String> {
        let client = self.clone();

        thread::Builder::new()
            .name("flatbar-niri-ipc".to_string())
            .spawn(move || {
                let mut backoff = Duration::from_millis(250);
                const MAX_BACKOFF: Duration = Duration::from_secs(5);

                loop {
                    let path = match client.get_socket_path() {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::debug!("Niri socket error: {e}");
                            thread::sleep(backoff);
                            continue;
                        }
                    };

                    match UnixStream::connect(path) {
                        Ok(mut stream) => {
                            backoff = Duration::from_millis(250);

                            // Send EventStream subscription
                            let sub_req = "{\"Request\":{\"EventStream\":{}}}\n";
                            if stream.write_all(sub_req.as_bytes()).is_err() {
                                thread::sleep(backoff);
                                continue;
                            }

                            // Initial state
                            if let Ok(ws) = client.workspaces() {
                                let _ = tx.send(WorkspaceEvent::Full(ws));
                            }

                            let reader = BufReader::new(stream);
                            for line in reader.lines() {
                                match line {
                                    Ok(_l) => {
                                        if let Ok(ws) = client.workspaces() {
                                            if tx.send(WorkspaceEvent::Full(ws)).is_err() {
                                                return;
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        tracing::warn!("Niri event socket read error: {err}");
                                        break;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            tracing::debug!("Failed to connect to Niri IPC socket: {err}");
                        }
                    }

                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            })
            .map_err(|e| format!("Failed to spawn Niri IPC thread: {e}"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_niri_workspaces_reply() {
        let reply = r#"{"Ok":{"Workspaces":[
            {"id":10,"idx":1,"name":null,"output":"DP-1","is_active":true,"is_focused":true,"is_urgent":false},
            {"id":20,"idx":2,"name":null,"output":"DP-1","is_active":false,"is_focused":false,"is_urgent":true}
        ]}}"#;

        let ws = NiriClient::parse_workspaces_json(reply).unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].id, "10");
        assert_eq!(ws[0].name, "1");
        assert!(ws[0].focused);
        assert!(!ws[0].urgent);
        assert_eq!(ws[1].id, "20");
        assert_eq!(ws[1].name, "2");
        assert!(!ws[1].focused);
        assert!(ws[1].urgent);
    }

    #[test]
    fn test_parse_niri_workspaces_unnamed_idx_based() {
        let reply = r#"{"Ok":{"Workspaces":[
            {"id":101,"idx":1,"name":null,"output":"DP-1","is_active":true,"is_focused":true,"is_urgent":false},
            {"id":102,"idx":2,"name":null,"output":"DP-1","is_active":false,"is_focused":false,"is_urgent":false},
            {"id":103,"idx":3,"name":null,"output":"DP-1","is_active":false,"is_focused":false,"is_urgent":false}
        ]}}"#;
        let ws = NiriClient::parse_workspaces_json(reply).unwrap();
        assert_eq!(ws.len(), 3);
        assert_eq!(ws[0].name, "1");
        assert_eq!(ws[1].name, "2");
        assert_eq!(ws[2].name, "3");
    }

    #[test]
    fn test_parse_niri_workspaces_named_keep_names() {
        let reply = r#"{"Ok":{"Workspaces":[
            {"id":1,"idx":1,"name":"web","output":"DP-1","is_active":true,"is_focused":true,"is_urgent":false},
            {"id":2,"idx":2,"name":"code","output":"DP-1","is_active":false,"is_focused":false,"is_urgent":false}
        ]}}"#;
        let ws = NiriClient::parse_workspaces_json(reply).unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].name, "web");
        assert_eq!(ws[1].name, "code");
    }

    #[test]
    fn test_parse_niri_workspaces_sorted_by_idx() {
        let reply = r#"{"Ok":{"Workspaces":[
            {"id":99,"idx":3,"name":null,"output":"DP-1","is_active":false,"is_focused":false,"is_urgent":false},
            {"id":55,"idx":1,"name":null,"output":"DP-1","is_active":true,"is_focused":true,"is_urgent":false},
            {"id":77,"idx":2,"name":null,"output":"DP-1","is_active":false,"is_focused":false,"is_urgent":false}
        ]}}"#;
        let ws = NiriClient::parse_workspaces_json(reply).unwrap();
        assert_eq!(ws.len(), 3);
        assert_eq!(ws[0].id, "55");
        assert_eq!(ws[0].name, "1");
        assert_eq!(ws[1].id, "77");
        assert_eq!(ws[1].name, "2");
        assert_eq!(ws[2].id, "99");
        assert_eq!(ws[2].name, "3");
    }

    #[test]
    fn test_parse_niri_workspaces_missing_idx_falls_back_to_id() {
        let reply = r#"{"Ok":{"Workspaces":[
            {"id":42,"idx":null,"name":null,"output":"DP-1","is_active":false,"is_focused":false,"is_urgent":false}
        ]}}"#;
        let ws = NiriClient::parse_workspaces_json(reply).unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].id, "42");
        assert_eq!(ws[0].name, "42");
    }
}
