//! Core library for the Flatbar Wayland status bar.
#![recursion_limit = "512"]

pub mod config;
pub mod dbus;
pub mod ipc;
pub mod render;
pub mod shell;
pub mod singleton;
pub mod widget;

pub use config::{Config, ConfigError};
pub use shell::{run_flatbar, run_flatbar_with_path};
pub use singleton::{InstanceLock, SingletonError};
pub use widget::{Span, SpanKind, Widget, WidgetState};
