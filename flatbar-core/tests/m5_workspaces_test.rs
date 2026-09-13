use flatbar_core::config::Config;
use flatbar_core::ipc::provider::{WorkspaceEvent, WorkspaceInfo, WorkspaceProvider};
use flatbar_core::ipc::sway::{decode_frame, encode_frame, SwayClient, GET_WORKSPACES, SUBSCRIBE};
use flatbar_core::shell::build_widgets;
use flatbar_core::shell::pointer::WidgetMouseEvent;
use flatbar_core::widget::workspaces::WorkspaceWidget;
use flatbar_core::widget::Widget;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

struct MockWorkspaceProvider {
    workspaces: Mutex<Vec<WorkspaceInfo>>,
    focused_log: Mutex<Vec<String>>,
}

impl MockWorkspaceProvider {
    fn new(workspaces: Vec<WorkspaceInfo>) -> Self {
        Self {
            workspaces: Mutex::new(workspaces),
            focused_log: Mutex::new(Vec::new()),
        }
    }
}

impl WorkspaceProvider for MockWorkspaceProvider {
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, String> {
        Ok(self.workspaces.lock().unwrap().clone())
    }

    fn focus(&self, id: &str) -> Result<(), String> {
        self.focused_log.lock().unwrap().push(id.to_string());
        let mut ws = self.workspaces.lock().unwrap();
        for w in ws.iter_mut() {
            w.focused = w.id == id;
        }
        Ok(())
    }

    fn subscribe(&self, _tx: Sender<WorkspaceEvent>) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn test_sway_wire_framing() {
    let frame = encode_frame(GET_WORKSPACES, "");
    let mut cursor = Cursor::new(frame);
    let (msg_type, payload) = decode_frame(&mut cursor).unwrap();
    assert_eq!(msg_type, GET_WORKSPACES);
    assert!(payload.is_empty());

    let frame2 = encode_frame(SUBSCRIBE, r#"["workspace"]"#);
    let mut cursor2 = Cursor::new(frame2);
    let (msg_type2, payload2) = decode_frame(&mut cursor2).unwrap();
    assert_eq!(msg_type2, SUBSCRIBE);
    assert_eq!(String::from_utf8(payload2).unwrap(), r#"["workspace"]"#);
}

#[test]
fn test_workspace_widget_layout_and_emphasis() {
    let list = vec![
        WorkspaceInfo {
            id: "1".to_string(),
            name: "1".to_string(),
            focused: true,
            urgent: false,
            monitor: "DP-1".to_string(),
        },
        WorkspaceInfo {
            id: "2".to_string(),
            name: "2".to_string(),
            focused: false,
            urgent: true,
            monitor: "DP-1".to_string(),
        },
        WorkspaceInfo {
            id: "3".to_string(),
            name: "3".to_string(),
            focused: false,
            urgent: false,
            monitor: "DP-1".to_string(),
        },
    ];

    let mock = Box::new(MockWorkspaceProvider::new(list));
    let widget = WorkspaceWidget::with_provider("workspaces", mock);

    let state = widget.state();
    assert_eq!(state.spans.len(), 3);
    assert!(state.spans[0].is_emphasized()); // focused -> emphasized
    assert_eq!(state.spans[0].as_text(), Some(" 1 "));
    assert!(state.spans[1].is_emphasized()); // urgent -> emphasized
    assert_eq!(state.spans[1].as_text(), Some(" !2 "));
    assert!(!state.spans[2].is_emphasized());
    assert_eq!(state.spans[2].as_text(), Some(" 3 "));
}

#[test]
fn test_workspace_widget_click_interaction() {
    let list = vec![
        WorkspaceInfo {
            id: "ws-1".to_string(),
            name: "1".to_string(),
            focused: true,
            urgent: false,
            monitor: "DP-1".to_string(),
        },
        WorkspaceInfo {
            id: "ws-2".to_string(),
            name: "2".to_string(),
            focused: false,
            urgent: false,
            monitor: "DP-1".to_string(),
        },
    ];

    let mock = Box::new(MockWorkspaceProvider::new(list));
    let widget = WorkspaceWidget::with_provider("workspaces", mock);

    // Click on index 1 (ws-2)
    widget.handle_mouse_event_at(WidgetMouseEvent::LeftClick, 40, 10, Some(1));

    let state = widget.state();
    assert!(!state.spans[0].is_emphasized());
    assert!(state.spans[1].is_emphasized());
}

#[test]
fn test_config_builder_with_workspaces() {
    let toml_str = r#"
        [bar]
        name = "test_ws_bar"
        layout = { left = ["workspaces"], center = ["datetime"], right = [] }

        [widgets.workspaces]
        type = "workspaces"
    "#;

    let config = Config::parse_str(toml_str, PathBuf::from("test.toml")).unwrap();
    let widgets = build_widgets(&config);

    assert_eq!(widgets.len(), 2);
    let ids: Vec<&str> = widgets.iter().map(|w| w.id()).collect();
    assert!(ids.contains(&"workspaces"));
    assert!(ids.contains(&"datetime"));
}

#[test]
fn test_headless_sway_ipc_roundtrip() {
    // If running in an environment with SWAYSOCK (e.g. headless sway test), test live IPC
    if let Ok(sock) = std::env::var("SWAYSOCK") {
        let client = SwayClient::with_socket_path(sock);
        if let Ok(workspaces) = client.workspaces() {
            assert!(
                !workspaces.is_empty(),
                "Live Sway should have at least 1 workspace"
            );
        }
    }
}
