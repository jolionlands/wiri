use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to parse KDL: {0}")]
    KdlParse(String),

    /// The config has errors: `file:line:column: error: message` of the first one, and how
    /// many more there are.
    #[error("{0}")]
    Invalid(String),

    #[error("Invalid value for {field}: {message}")]
    InvalidValue { field: String, message: String },

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Unknown config section: {0}")]
    UnknownSection(String),

    #[error("Failed to watch config file: {0}")]
    WatchError(String),

    #[error("Config reload error: {0}")]
    ReloadError(String),
}

pub type Result<T> = std::result::Result<T, ConfigError>;

pub fn parse_error(path: &PathBuf, message: &str) -> ConfigError {
    ConfigError::KdlParse(format!("{}: {}", path.display(), message))
}
