pub mod hyprland;
pub mod launch;
pub mod niri;
pub mod provider;
pub mod sway;
pub mod window_rules;

pub use hyprland::HyprlandClient;
pub use launch::{format_terminal_command, launch_gui, launch_tui};
pub use niri::NiriClient;
pub use provider::{
    detect_compositor, CompositorType, WorkspaceEvent, WorkspaceInfo, WorkspaceProvider,
};
pub use sway::SwayClient;
pub use window_rules::{
    apply_runtime_rule, generate_hyprland_rules, generate_niri_snippet, generate_sway_rule,
};

/// Create a suitable workspace provider based on the detected running compositor.
pub fn create_workspace_provider() -> Option<Box<dyn WorkspaceProvider>> {
    match detect_compositor() {
        Some(CompositorType::Sway) => Some(Box::new(SwayClient::new())),
        Some(CompositorType::Hyprland) => Some(Box::new(HyprlandClient::new())),
        Some(CompositorType::Niri) => Some(Box::new(NiriClient::new())),
        None => {
            tracing::info!("No supported Wayland compositor detected for workspaces widget");
            None
        }
    }
}
