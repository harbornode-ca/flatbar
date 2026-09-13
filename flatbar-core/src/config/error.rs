use std::path::{Path, PathBuf};
use thiserror::Error;

/// Configuration errors for flatbar.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Configuration file not found: {path}")]
    FileNotFound {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read configuration file at {path}: {source}")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Configuration parse error in {path}:{line}:{column}: {message}")]
    ParseError {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },

    #[error("Configuration validation error: {message}")]
    ValidationError { message: String },
}

impl ConfigError {
    /// Create a parse error with line/column information from a TOML de error.
    pub fn from_toml_de(path: &Path, source: &str, err: toml::de::Error) -> Self {
        let span = err.span();
        let (line, column) = match span {
            Some(range) => byte_offset_to_line_col(source, range.start),
            None => (1, 1),
        };
        let message = err.message().to_string();

        Self::ParseError {
            path: path.to_path_buf(),
            line,
            column,
            message,
        }
    }
}

fn byte_offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1;
    let mut col = 1;

    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }

    (line, col)
}
