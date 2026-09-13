//! Widget trait definition, widget state representations, and built-in widget implementations.

pub mod battery;
pub mod bluetooth;
pub mod calendar;
pub mod clipboard;
pub mod command;
pub mod datetime;
pub mod disk;
pub mod menu;
pub mod network;
pub mod notifications;
pub mod performance;
pub mod state;
pub mod stats;
pub mod sysfs;
pub mod tray;
pub mod wifi;
pub mod workspaces;

use std::time::Duration;

pub use battery::{read_battery_info, AggregatedBattery, BatteryStatus, BatteryWidget};
pub use bluetooth::{BluetoothConfig, BluetoothWidget};
pub use calendar::{CalendarHit, CalendarLayout, CalendarModel, FirstDay};
pub use clipboard::ClipboardWidget;
pub use command::{parse_command_output, CommandOutput, CommandWidget, MouseActions};
pub use datetime::{format_time, DatetimeWidget};
pub use disk::{format_bytes, query_statvfs, DiskMountInfo, DiskWidget};
pub use menu::{
    layout_menu, render_menu, MenuItem, MenuItemPlacement, MenuLayout, MenuModel, MenuRenderParams,
};
pub use network::{detect_network_state, NetworkState, NetworkWidget};
pub use notifications::NotificationsWidget;
pub use performance::{PerformanceIcons, PerformanceWidget};
pub use state::{Span, SpanKind, WidgetState};
pub use stats::StatsWidget;
pub use sysfs::FsPathProvider;
pub use tray::{TrayConfig, TrayWidget};
pub use wifi::{
    parse_proc_net_wireless, query_ssid, read_wifi_info, WifiInfo, WifiTier, WifiWidget,
};
pub use workspaces::WorkspaceWidget;

/// A request to open an interactive popup surface from a widget.
#[derive(Debug, Clone, PartialEq)]
pub enum PopupRequest {
    Menu(MenuModel),
    Calendar {
        first_day: FirstDay,
        launch_command: Option<Vec<String>>,
    },
    TrayGrid,
}

/// Shared notification transport shape: `(summary, body, app_name)`.
pub type NotifyFn = std::sync::Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Shared public-IP fetch shape: URL endpoint in, plain-text IP out
/// (`None` on any failure).
pub type PublicIpFn = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The core Widget API contract.
pub trait Widget: 'static + Send + Sync {
    /// Unique identifier for this widget instance.
    fn id(&self) -> &str;

    /// Downcast helper for TrayWidget.
    fn as_tray(&self) -> Option<&crate::widget::TrayWidget> {
        None
    }

    /// Mutable downcast helper for CommandWidget (used to install the wake channel).
    fn as_command_mut(&mut self) -> Option<&mut crate::widget::CommandWidget> {
        None
    }

    /// Mutable downcast helper for ClipboardWidget (used to install the wake channel).
    fn as_clipboard_mut(&mut self) -> Option<&mut crate::widget::ClipboardWidget> {
        None
    }

    /// Mutable downcast helper for NotificationsWidget (used to install the wake channel).
    fn as_notifications_mut(&mut self) -> Option<&mut crate::widget::NotificationsWidget> {
        None
    }

    /// Update interval for polling. Duration of 0 indicates purely event-driven.
    fn update_interval(&self) -> Duration;

    /// Return an immutable snapshot of current visual state.
    fn state(&self) -> WidgetState;

    /// Handle mouse interaction event (click / scroll).
    fn handle_mouse_event(&self, _event: crate::shell::pointer::WidgetMouseEvent) {}

    /// Handle mouse interaction event with precise relative coordinates and span hit info.
    fn handle_mouse_event_at(
        &self,
        event: crate::shell::pointer::WidgetMouseEvent,
        _rel_x: i32,
        _rel_y: i32,
        _span_idx: Option<usize>,
    ) {
        self.handle_mouse_event(event);
    }

    /// Trigger a refresh / re-execution of the widget.
    fn refresh(&self) {}

    /// Optional menu model for popup menus on right-click.
    fn get_menu(&self) -> Option<MenuModel> {
        None
    }

    /// Launch an external app on right-click instead of opening a menu.
    /// Returns `true` when the click was consumed; the pointer dispatcher
    /// checks this before the right-click menu path. The launcher applies
    /// window rules and reports spawn failures as a desktop notification.
    fn right_click_launch(&self) -> bool {
        false
    }

    /// Optional popup request on click (Menu, Calendar, or TrayGrid).
    fn popup_request(&self) -> Option<PopupRequest> {
        self.get_menu().map(PopupRequest::Menu)
    }

    /// Handle action when a menu item is clicked in this widget's popup.
    fn handle_menu_action(&self, _action_id: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyWidget;
    impl Widget for DummyWidget {
        fn id(&self) -> &str {
            "dummy"
        }
        fn update_interval(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn state(&self) -> WidgetState {
            WidgetState {
                spans: vec![Span::text("hello")],
                tooltip: None,
            }
        }
    }

    #[test]
    fn test_widget_contract() {
        let w = DummyWidget;
        assert_eq!(w.id(), "dummy");
        assert_eq!(w.state().spans.len(), 1);
    }
}
