//! Generic workspace provider trait, compositor detection, and data models.

use std::env;
use std::sync::mpsc::Sender;

/// Compositor types supported by Flatbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositorType {
    Sway,
    Hyprland,
    Niri,
}

/// Generic representation of a workspace across any compositor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub focused: bool,
    pub urgent: bool,
    pub monitor: String,
}

/// Events emitted by workspace providers when compositor state changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEvent {
    Full(Vec<WorkspaceInfo>),
    Focus(String),
    Urgent(String),
    Remove(String),
}

/// Compositor-agnostic workspace querying and event subscription contract.
pub trait WorkspaceProvider: Send + Sync {
    /// Subscribe to live workspace events on a dedicated thread, sending events to `tx`.
    fn subscribe(&self, tx: Sender<WorkspaceEvent>) -> Result<(), String>;

    /// Synchronously query current workspace list.
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, String>;

    /// Switch/focus to a workspace by its identifier.
    fn focus(&self, id: &str) -> Result<(), String>;
}

/// Detect the running Wayland compositor from environment variables.
pub fn detect_compositor() -> Option<CompositorType> {
    if env::var("SWAYSOCK").is_ok() || env::var("I3SOCK").is_ok() {
        Some(CompositorType::Sway)
    } else if env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        Some(CompositorType::Hyprland)
    } else if env::var("NIRI_SOCKET").is_ok() {
        Some(CompositorType::Niri)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_detect_compositor_sway() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("SWAYSOCK", "/tmp/sway-ipc.sock");
        env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        env::remove_var("NIRI_SOCKET");

        assert_eq!(detect_compositor(), Some(CompositorType::Sway));
        env::remove_var("SWAYSOCK");
    }

    #[test]
    fn test_detect_compositor_hyprland() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("SWAYSOCK");
        env::remove_var("I3SOCK");
        env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "hyprland_1234");
        env::remove_var("NIRI_SOCKET");

        assert_eq!(detect_compositor(), Some(CompositorType::Hyprland));
        env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
    }

    #[test]
    fn test_detect_compositor_niri() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("SWAYSOCK");
        env::remove_var("I3SOCK");
        env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        env::set_var("NIRI_SOCKET", "/tmp/niri.sock");

        assert_eq!(detect_compositor(), Some(CompositorType::Niri));
        env::remove_var("NIRI_SOCKET");
    }

    #[test]
    fn test_detect_compositor_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("SWAYSOCK");
        env::remove_var("I3SOCK");
        env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        env::remove_var("NIRI_SOCKET");

        assert_eq!(detect_compositor(), None);
    }
}
