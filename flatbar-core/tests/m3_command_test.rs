use flatbar_core::shell::pointer::WidgetMouseEvent;
use flatbar_core::widget::command::{CommandWidget, MouseActions};
use flatbar_core::widget::{SpanKind, Widget};
use std::collections::HashMap;
use std::fs;
use std::thread;
use std::time::Duration;
use tempfile::NamedTempFile;

#[test]
fn test_command_widget_json_output_contract() {
    let mut class_icons = HashMap::new();
    class_icons.insert("muted".to_string(), "fa-volume-xmark".to_string());
    class_icons.insert("normal".to_string(), "fa-volume-high".to_string());

    let cmd = r#"echo '{"text": "75%", "tooltip": "Volume 75%", "class": "normal"}'"#;
    let widget = CommandWidget::new(
        "vol",
        cmd,
        0,
        None,
        MouseActions::default(),
        Some("fa-volume-low".to_string()),
        class_icons,
    );

    // Wait for background execution
    thread::sleep(Duration::from_millis(150));
    let state = widget.state();

    assert_eq!(state.spans.len(), 2);
    assert_eq!(
        state.spans[0].kind,
        SpanKind::Icon("fa-volume-high".to_string())
    );
    assert_eq!(state.spans[1].kind, SpanKind::Text("75%".to_string()));
    assert_eq!(state.tooltip, Some("Volume 75%".to_string()));
}

#[test]
fn test_command_widget_error_graceful_degradation() {
    let widget = CommandWidget::new(
        "bad_cmd",
        "exit 1",
        0,
        None,
        MouseActions::default(),
        None,
        HashMap::new(),
    );

    thread::sleep(Duration::from_millis(150));
    let state = widget.state();

    assert_eq!(state.spans.len(), 1);
    assert_eq!(state.spans[0].kind, SpanKind::Text("⚠".to_string()));
}

#[test]
fn test_command_widget_mouse_click_action() {
    let temp_file = NamedTempFile::new().unwrap();
    let temp_path = temp_file.path().to_string_lossy().to_string();

    let click_cmd = format!("echo 'CLICKED' >> {temp_path}");
    let actions = MouseActions {
        left_click: Some(click_cmd),
        ..Default::default()
    };

    let widget = CommandWidget::new("counter", "echo 0", 0, None, actions, None, HashMap::new());

    widget.handle_mouse_event(WidgetMouseEvent::LeftClick);
    thread::sleep(Duration::from_millis(200));

    let content = fs::read_to_string(&temp_path).unwrap();
    assert!(content.contains("CLICKED"));
}
