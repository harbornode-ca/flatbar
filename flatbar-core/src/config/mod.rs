//! Configuration schema, parsing, and validation.

pub mod error;
pub mod schema;
pub mod window_rules;

pub use error::ConfigError;
pub use schema::{generate_json_schema, print_schema_json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
pub use window_rules::{WindowMode, WindowRule, WindowRulesConfig};

/// Top-level configuration for Flatbar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub bar: BarConfig,
    #[serde(default)]
    pub widgets: HashMap<String, toml::Value>,
    #[serde(default)]
    pub window_rules: WindowRulesConfig,
}

/// Bar visual and placement settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarConfig {
    #[serde(default = "default_bar_name")]
    pub name: String,
    #[serde(default = "default_bar_height")]
    pub height: u32,
    #[serde(default = "default_background")]
    pub background: String,
    #[serde(default = "default_foreground")]
    pub foreground: String,
    #[serde(default = "default_position")]
    pub position: Position,
    #[serde(default = "default_output")]
    pub output: OutputSpec,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_font_fallback")]
    pub font_fallback: Vec<String>,
    /// Gap (pixels) between spans *within* one widget (icon ↔ text, icon↔icon).
    /// Widget-to-widget spacing is controlled by the bar layout, not this knob.
    #[serde(default = "default_inner_padding")]
    pub inner_padding: u32,
    #[serde(default)]
    pub menu_command: Option<String>,
    #[serde(default = "default_menu_backend")]
    pub menu_backend: MenuBackend,
    #[serde(default)]
    pub layout: LayoutConfig,
}

impl BarConfig {
    pub fn resolved_menu_command(&self) -> &str {
        self.menu_command
            .as_deref()
            .unwrap_or("fuzzel --dmenu -p flatbar")
    }
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            name: default_bar_name(),
            height: default_bar_height(),
            background: default_background(),
            foreground: default_foreground(),
            position: default_position(),
            output: default_output(),
            font_family: default_font_family(),
            font_size: default_font_size(),
            font_fallback: default_font_fallback(),
            inner_padding: default_inner_padding(),
            menu_command: None,
            menu_backend: default_menu_backend(),
            layout: LayoutConfig::default(),
        }
    }
}

/// Menu display backend mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MenuBackend {
    #[default]
    Launcher,
    Popup,
}

fn default_menu_backend() -> MenuBackend {
    MenuBackend::Launcher
}

/// Widget ordering configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LayoutConfig {
    #[serde(default)]
    pub left: Vec<String>,
    #[serde(default)]
    pub center: Vec<String>,
    #[serde(default)]
    pub right: Vec<String>,
}

/// Position of the bar on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    #[default]
    Top,
    Bottom,
}

/// Target output specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputSpec {
    All(String), // Matches "all"
    Named(String),
}

impl Default for OutputSpec {
    fn default() -> Self {
        OutputSpec::All("all".to_string())
    }
}

impl OutputSpec {
    pub fn matches(&self, output_name: &str) -> bool {
        match self {
            OutputSpec::All(s) if s.eq_ignore_ascii_case("all") => true,
            OutputSpec::All(s) => s == output_name,
            OutputSpec::Named(name) => name == output_name || name.eq_ignore_ascii_case("all"),
        }
    }
}

/// Parsed RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Converts to ARGB8888 `u32` for `wl_shm`.
    pub fn to_argb_u32(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// Parse HEX color string like `#1e1e2e` or `#1e1e2eff`.
    pub fn parse_hex(hex: &str) -> Result<Self, String> {
        let hex = hex.trim().trim_start_matches('#');
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).map_err(|e| e.to_string())?;
                let g = u8::from_str_radix(&hex[2..4], 16).map_err(|e| e.to_string())?;
                let b = u8::from_str_radix(&hex[4..6], 16).map_err(|e| e.to_string())?;
                Ok(Self::rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).map_err(|e| e.to_string())?;
                let g = u8::from_str_radix(&hex[2..4], 16).map_err(|e| e.to_string())?;
                let b = u8::from_str_radix(&hex[4..6], 16).map_err(|e| e.to_string())?;
                let a = u8::from_str_radix(&hex[6..8], 16).map_err(|e| e.to_string())?;
                Ok(Self::rgba(r, g, b, a))
            }
            _ => Err(format!(
                "Invalid hex color format: '{hex}'. Expected #RRGGBB or #RRGGBBAA"
            )),
        }
    }
}

fn default_bar_name() -> String {
    "main".to_string()
}

fn default_bar_height() -> u32 {
    30
}

fn default_background() -> String {
    "#1e1e2e".to_string()
}

fn default_foreground() -> String {
    "#cdd6f4".to_string()
}

fn default_position() -> Position {
    Position::Top
}

fn default_output() -> OutputSpec {
    OutputSpec::All("all".to_string())
}

fn default_font_family() -> String {
    "sans-serif".to_string()
}

fn default_font_size() -> f32 {
    13.0
}

fn default_inner_padding() -> u32 {
    2
}

fn default_font_fallback() -> Vec<String> {
    vec!["Noto Sans".to_string()]
}

impl Config {
    /// Load and validate configuration from a file path.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::FileNotFound {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
            Err(e) => {
                return Err(ConfigError::IoError {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
        };

        Self::parse_str(&content, path)
    }

    /// Parse configuration from a TOML string with an associated path for error reporting.
    pub fn parse_str(content: &str, path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let config: Config =
            toml::from_str(content).map_err(|err| ConfigError::from_toml_de(path, content, err))?;

        config.validate()?;
        Ok(config)
    }

    /// Validate configuration invariants.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.bar.height == 0 {
            return Err(ConfigError::ValidationError {
                message: "bar.height must be greater than 0".to_string(),
            });
        }

        if self.bar.font_size <= 0.0 {
            return Err(ConfigError::ValidationError {
                message: "bar.font_size must be greater than 0".to_string(),
            });
        }

        Color::parse_hex(&self.bar.background).map_err(|msg| ConfigError::ValidationError {
            message: format!("Invalid bar.background: {msg}"),
        })?;

        Color::parse_hex(&self.bar.foreground).map_err(|msg| ConfigError::ValidationError {
            message: format!("Invalid bar.foreground: {msg}"),
        })?;

        if let Some(ref cmd) = self.bar.menu_command {
            if cmd.trim().is_empty() {
                return Err(ConfigError::ValidationError {
                    message: "bar.menu_command cannot be empty".to_string(),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_valid_minimal_config() {
        let toml_str = r##"
            [bar]
            name = "topbar"
            height = 32
            background = "#000000"
            foreground = "#ffffff"
        "##;
        let config = Config::parse_str(toml_str, PathBuf::from("test.toml")).unwrap();
        assert_eq!(config.bar.name, "topbar");
        assert_eq!(config.bar.height, 32);
        assert_eq!(config.bar.position, Position::Top);
        assert_eq!(
            Color::parse_hex(&config.bar.background).unwrap(),
            Color::rgb(0, 0, 0)
        );
        assert_eq!(
            Color::parse_hex(&config.bar.foreground).unwrap(),
            Color::rgb(255, 255, 255)
        );
    }

    #[test]
    fn test_parse_missing_file_error() {
        let err = Config::load_from_file(PathBuf::from("/nonexistent/config.toml")).unwrap_err();
        match err {
            ConfigError::FileNotFound { path, .. } => {
                assert_eq!(path, PathBuf::from("/nonexistent/config.toml"));
            }
            _ => panic!("Expected FileNotFound, got {err:?}"),
        }
    }

    #[test]
    fn test_parse_malformed_toml_error() {
        let toml_str = "invalid toml syntax [[";
        let err = Config::parse_str(toml_str, PathBuf::from("bad.toml")).unwrap_err();
        match err {
            ConfigError::ParseError { path, line, .. } => {
                assert_eq!(path, PathBuf::from("bad.toml"));
                assert_eq!(line, 1);
            }
            _ => panic!("Expected ParseError, got {err:?}"),
        }
    }

    #[test]
    fn test_validate_invalid_height() {
        let toml_str = r##"
            [bar]
            height = 0
        "##;
        let err = Config::parse_str(toml_str, PathBuf::from("test.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::ValidationError { .. }));
    }

    #[test]
    fn test_validate_invalid_color() {
        let toml_str = r##"
            [bar]
            background = "not-a-color"
        "##;
        let err = Config::parse_str(toml_str, PathBuf::from("test.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::ValidationError { .. }));
    }
}
