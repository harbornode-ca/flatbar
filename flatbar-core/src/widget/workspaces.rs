//! Workspace widget displaying active, urgent, and inactive workspaces with click-to-focus.

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::ipc::create_workspace_provider;
use crate::ipc::provider::{WorkspaceEvent, WorkspaceInfo, WorkspaceProvider};
use crate::shell::pointer::WidgetMouseEvent;
use crate::widget::{Span, Widget, WidgetState};

/// Live workspace indicator and interactive switcher.
pub struct WorkspaceWidget {
    id: String,
    provider: Option<Arc<dyn WorkspaceProvider>>,
    workspaces: Arc<Mutex<Vec<WorkspaceInfo>>>,
}

impl WorkspaceWidget {
    /// Create a new WorkspaceWidget by auto-detecting the running compositor.
    pub fn new(id: impl Into<String>) -> Self {
        let provider = create_workspace_provider();
        Self::from_optional_provider(id, provider)
    }

    /// Create a WorkspaceWidget with a specific provider (useful for testing).
    pub fn with_provider(id: impl Into<String>, provider: Box<dyn WorkspaceProvider>) -> Self {
        Self::from_optional_provider(id, Some(provider))
    }

    fn from_optional_provider(
        id: impl Into<String>,
        provider: Option<Box<dyn WorkspaceProvider>>,
    ) -> Self {
        let id = id.into();
        let workspaces = Arc::new(Mutex::new(Vec::new()));

        let provider_arc: Option<Arc<dyn WorkspaceProvider>> = provider.map(Arc::from);

        if let Some(ref p) = provider_arc {
            // Initial query
            if let Ok(ws) = p.workspaces() {
                *workspaces.lock().unwrap() = ws;
            }

            // Setup subscription channel
            let (tx, rx) = channel();
            if let Err(e) = p.subscribe(tx) {
                tracing::warn!("Failed to subscribe to workspace events: {e}");
            } else {
                let ws_store = Arc::clone(&workspaces);
                let wid = id.clone();
                thread::Builder::new()
                    .name(format!("flatbar-ws-recv-{wid}"))
                    .spawn(move || {
                        while let Ok(event) = rx.recv() {
                            let mut store = ws_store.lock().unwrap();
                            match event {
                                WorkspaceEvent::Full(ws) => {
                                    *store = ws;
                                }
                                WorkspaceEvent::Focus(focused_id) => {
                                    for w in store.iter_mut() {
                                        w.focused = w.id == focused_id || w.name == focused_id;
                                    }
                                }
                                WorkspaceEvent::Urgent(urgent_id) => {
                                    for w in store.iter_mut() {
                                        if w.id == urgent_id || w.name == urgent_id {
                                            w.urgent = true;
                                        }
                                    }
                                }
                                WorkspaceEvent::Remove(removed_id) => {
                                    store.retain(|w| w.id != removed_id && w.name != removed_id);
                                }
                            }
                        }
                    })
                    .ok();
            }
        }

        Self {
            id,
            provider: provider_arc,
            workspaces,
        }
    }
}

impl Widget for WorkspaceWidget {
    fn id(&self) -> &str {
        &self.id
    }

    fn update_interval(&self) -> Duration {
        Duration::from_millis(500) // Fallback refresh interval
    }

    fn state(&self) -> WidgetState {
        let ws_list = match self.workspaces.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => return WidgetState::default(),
        };

        if ws_list.is_empty() {
            return WidgetState::default();
        }

        let mut spans = Vec::with_capacity(ws_list.len());

        for ws in &ws_list {
            let label = if ws.urgent {
                format!(" !{} ", ws.name)
            } else {
                format!(" {} ", ws.name)
            };

            if ws.focused || ws.urgent {
                spans.push(Span::emphasized_text(label));
            } else {
                spans.push(Span::text(label));
            }
        }

        WidgetState {
            spans,
            tooltip: None,
        }
    }

    fn handle_mouse_event_at(
        &self,
        event: WidgetMouseEvent,
        _rel_x: i32,
        _rel_y: i32,
        span_idx: Option<usize>,
    ) {
        if event == WidgetMouseEvent::LeftClick {
            if let Some(idx) = span_idx {
                let target_id = {
                    let mut store = self.workspaces.lock().unwrap();
                    if idx < store.len() {
                        let target = store[idx].id.clone();
                        // Optimistic focus update for instant response
                        for (i, w) in store.iter_mut().enumerate() {
                            w.focused = i == idx;
                            if i == idx {
                                w.urgent = false;
                            }
                        }
                        Some(target)
                    } else {
                        None
                    }
                };

                if let (Some(id), Some(ref provider)) = (target_id, &self.provider) {
                    if let Err(e) = provider.focus(&id) {
                        tracing::warn!("Failed to focus workspace '{id}': {e}");
                    }
                }
            }
        }
    }

    fn refresh(&self) {
        if let Some(ref p) = self.provider {
            if let Ok(ws) = p.workspaces() {
                if let Ok(mut store) = self.workspaces.lock() {
                    *store = ws;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;

    struct MockProvider {
        workspaces: Mutex<Vec<WorkspaceInfo>>,
    }

    impl MockProvider {
        fn new(list: Vec<WorkspaceInfo>) -> Self {
            Self {
                workspaces: Mutex::new(list),
            }
        }
    }

    impl WorkspaceProvider for MockProvider {
        fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, String> {
            Ok(self.workspaces.lock().unwrap().clone())
        }

        fn focus(&self, id: &str) -> Result<(), String> {
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
    fn test_workspace_widget_rendering() {
        let initial = vec![
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

        let mock = Box::new(MockProvider::new(initial));
        let widget = WorkspaceWidget::with_provider("workspaces", mock);

        let state = widget.state();
        assert_eq!(state.spans.len(), 3);
        assert!(state.spans[0].is_emphasized());
        assert_eq!(state.spans[0].as_text(), Some(" 1 "));
        assert!(state.spans[1].is_emphasized()); // urgent
        assert_eq!(state.spans[1].as_text(), Some(" !2 "));
        assert!(!state.spans[2].is_emphasized());
        assert_eq!(state.spans[2].as_text(), Some(" 3 "));
    }

    #[test]
    fn test_workspace_widget_click_focus() {
        let initial = vec![
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
                urgent: false,
                monitor: "DP-1".to_string(),
            },
        ];

        let mock = Box::new(MockProvider::new(initial));
        let widget = WorkspaceWidget::with_provider("workspaces", mock);

        // Click on span 1 (workspace "2")
        widget.handle_mouse_event_at(WidgetMouseEvent::LeftClick, 50, 10, Some(1));

        let state = widget.state();
        assert!(!state.spans[0].is_emphasized());
        assert!(state.spans[1].is_emphasized());
    }

    #[test]
    fn test_workspace_widget_no_compositor_renders_empty() {
        let widget = WorkspaceWidget::from_optional_provider("workspaces", None);
        let state = widget.state();
        assert!(state.spans.is_empty());
    }
}
