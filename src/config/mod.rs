pub mod decode;
pub mod error;
pub mod kdl;
pub mod loader;
pub mod parse;
pub mod types;

pub use decode::{Decoded, MAX_INCLUDE_DEPTH};
pub use error::{ConfigError, Result};
pub use loader::{
    default_config_dir, default_config_path, failure_summary, ConfigLoader, ReloadOutcome,
};
pub use parse::{
    parse_kdl_config, validate_config_file, validate_kdl_config, ValidationIssue,
    ValidationSeverity,
};
pub use types::*;
