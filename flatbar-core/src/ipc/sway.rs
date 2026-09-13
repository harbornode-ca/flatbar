//! Sway and i3 IPC client implementation with binary framing and event subscription.

use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ipc::provider::{WorkspaceEvent, WorkspaceInfo, WorkspaceProvider};

pub const MAGIC: &[u8; 6] = b"i3-ipc";
pub const RUN_COMMAND: u32 = 0;
pub const GET_WORKSPACES: u32 = 1;
pub const SUBSCRIBE: u32 = 2;
pub const EVENT_WORKSPACE: u32 = 0x8000_0000;

/// Serialize an i3/Sway IPC message into binary frame.
pub fn encode_frame(msg_type: u32, payload: &str) -> Vec<u8> {
    let payload_bytes = payload.as_bytes();
    let length = payload_bytes.len() as u32;

    let mut buf = Vec::with_capacity(6 + 4 + 4 + payload_bytes.len());
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&length.to_le_bytes());
    buf.extend_from_slice(&msg_type.to_le_bytes());
    buf.extend_from_slice(payload_bytes);
    buf
}

/// Decode a single i3/Sway IPC message frame from a reader.
pub fn decode_frame(reader: &mut impl Read) -> Result<(u32, Vec<u8>), String> {
    let mut magic_buf = [0u8; 6];
    reader
        .read_exact(&mut magic_buf)
        .map_err(|e| format!("Failed to read IPC magic: {e}"))?;

    if &magic_buf != MAGIC {
        return Err(format!(
            "Invalid IPC magic header: {:?}",
            String::from_utf8_lossy(&magic_buf)
        ));
    }

    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .map_err(|e| format!("Failed to read payload length: {e}"))?;
    let length = u32::from_le_bytes(len_buf) as usize;

    let mut type_buf = [0u8; 4];
    reader
        .read_exact(&mut type_buf)
        .map_err(|e| format!("Failed to read message type: {e}"))?;
    let msg_type = u32::from_le_bytes(type_buf);

    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|e| format!("Failed to read payload of length {length}: {e}"))?;

    Ok((msg_type, payload))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SwayWorkspaceRaw {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub num: Option<i32>,
    pub name: String,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub urgent: bool,
    #[serde(default)]
    pub output: Option<String>,
}

/// Sway / i3 IPC Client.
#[derive(Debug, Clone)]
pub struct SwayClient {
    socket_path: Option<PathBuf>,
}

impl Default for SwayClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SwayClient {
    pub fn new() -> Self {
        let socket_path = env::var_os("SWAYSOCK")
            .or_else(|| env::var_os("I3SOCK"))
            .map(PathBuf::from);

        Self { socket_path }
    }

    pub fn with_socket_path(path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: Some(path.as_ref().to_path_buf()),
        }
    }

    fn get_socket_path(&self) -> Result<&Path, String> {
        self.socket_path
            .as_deref()
            .ok_or_else(|| "Neither SWAYSOCK nor I3SOCK is set".to_string())
    }

    fn connect(&self) -> Result<UnixStream, String> {
        let path = self.get_socket_path()?;
        UnixStream::connect(path).map_err(|e| format!("Failed to connect to {path:?}: {e}"))
    }

    fn send_recv(&self, msg_type: u32, payload: &str) -> Result<Vec<u8>, String> {
        let mut stream = self.connect()?;
        let frame = encode_frame(msg_type, payload);
        stream
            .write_all(&frame)
            .map_err(|e| format!("Failed to write IPC frame: {e}"))?;

        let (_reply_type, payload) = decode_frame(&mut stream)?;
        Ok(payload)
    }

    /// Execute an i3/Sway command over IPC.
    pub fn run_command(&self, cmd: &str) -> Result<String, String> {
        let payload = self.send_recv(RUN_COMMAND, cmd)?;
        String::from_utf8(payload).map_err(|e| format!("Invalid UTF-8 in command reply: {e}"))
    }
}

impl WorkspaceProvider for SwayClient {
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, String> {
        let payload = self.send_recv(GET_WORKSPACES, "")?;
        let raw: Vec<SwayWorkspaceRaw> = serde_json::from_slice(&payload)
            .map_err(|e| format!("Failed to parse Sway workspaces JSON: {e}"))?;

        let workspaces = raw
            .into_iter()
            .map(|w| WorkspaceInfo {
                id: w
                    .id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| w.name.clone()),
                name: w.name,
                focused: w.focused,
                urgent: w.urgent,
                monitor: w.output.unwrap_or_default(),
            })
            .collect();

        Ok(workspaces)
    }

    fn focus(&self, id_or_name: &str) -> Result<(), String> {
        // Send focus command: workspace <name>
        let cmd = format!("workspace \"{id_or_name}\"");
        self.send_recv(RUN_COMMAND, &cmd)?;
        Ok(())
    }

    fn subscribe(&self, tx: Sender<WorkspaceEvent>) -> Result<(), String> {
        let client = self.clone();

        thread::Builder::new()
            .name("flatbar-sway-ipc".to_string())
            .spawn(move || {
                let mut backoff = Duration::from_millis(250);
                const MAX_BACKOFF: Duration = Duration::from_secs(5);

                loop {
                    match client.connect() {
                        Ok(mut stream) => {
                            backoff = Duration::from_millis(250);

                            // Send SUBSCRIBE ["workspace"]
                            let sub_payload = r#"["workspace"]"#;
                            let frame = encode_frame(SUBSCRIBE, sub_payload);
                            if let Err(e) = stream.write_all(&frame) {
                                tracing::warn!("Failed to send subscribe frame to Sway: {e}");
                                thread::sleep(backoff);
                                continue;
                            }

                            // Read subscription confirmation
                            if let Err(e) = decode_frame(&mut stream) {
                                tracing::warn!("Failed to read subscribe response from Sway: {e}");
                                thread::sleep(backoff);
                                continue;
                            }

                            // Send initial workspace state
                            if let Ok(ws) = client.workspaces() {
                                let _ = tx.send(WorkspaceEvent::Full(ws));
                            }

                            // Event loop
                            loop {
                                match decode_frame(&mut stream) {
                                    Ok((msg_type, _payload)) => {
                                        // If workspace event
                                        if msg_type == EVENT_WORKSPACE || (msg_type & 0x8000_0000 != 0) {
                                            if let Ok(ws) = client.workspaces() {
                                                if tx.send(WorkspaceEvent::Full(ws)).is_err() {
                                                    tracing::debug!("Workspace event receiver dropped; terminating thread");
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        tracing::warn!("Sway IPC read error / disconnection: {err}");
                                        break;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            tracing::debug!("Failed to connect to Sway IPC: {err}");
                        }
                    }

                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            })
            .map_err(|e| format!("Failed to spawn sway IPC thread: {e}"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_encode_and_decode_frame() {
        let payload = r#"["workspace"]"#;
        let encoded = encode_frame(SUBSCRIBE, payload);

        let mut cursor = Cursor::new(encoded);
        let (msg_type, decoded_payload) = decode_frame(&mut cursor).unwrap();

        assert_eq!(msg_type, SUBSCRIBE);
        assert_eq!(String::from_utf8(decoded_payload).unwrap(), payload);
    }

    #[test]
    fn test_decode_invalid_magic() {
        let mut bad_data = Vec::new();
        bad_data.extend_from_slice(b"badmag");
        bad_data.extend_from_slice(&0u32.to_le_bytes());
        bad_data.extend_from_slice(&0u32.to_le_bytes());

        let mut cursor = Cursor::new(bad_data);
        assert!(decode_frame(&mut cursor).is_err());
    }

    #[test]
    fn test_parse_canned_workspaces() {
        let json = r#"[
            {
                "id": 10,
                "num": 1,
                "name": "1:web",
                "visible": true,
                "focused": true,
                "urgent": false,
                "output": "DP-1"
            },
            {
                "id": 20,
                "num": 2,
                "name": "2:code",
                "visible": false,
                "focused": false,
                "urgent": true,
                "output": "DP-1"
            }
        ]"#;

        let raw: Vec<SwayWorkspaceRaw> = serde_json::from_str(json).unwrap();
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0].name, "1:web");
        assert!(raw[0].focused);
        assert!(!raw[0].urgent);
        assert_eq!(raw[1].name, "2:code");
        assert!(!raw[1].focused);
        assert!(raw[1].urgent);
    }
}
