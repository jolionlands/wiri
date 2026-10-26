pub mod decode;
pub mod diagnostics;
pub mod effects;
pub mod error;
pub mod gestures_config;
pub mod kdl;
pub mod loader;
pub mod overview_config;
pub mod parse;
pub mod recent_windows_config;
pub mod search;
pub mod types;
mod validation;

pub use decode::{Decoded, MAX_INCLUDE_DEPTH};
pub use effects::{
    color_text, parse_css_color, Effects, EffectsPartial, GradientSpace, GradientStyle,
    InsertHintStyle, PaintStyle, RingStyle, ShadowStyle, TabIndicatorStyle, TabPosition,
};
pub use error::{ConfigError, Result};
pub use loader::{
    default_config_dir, default_config_path, failure_summary, ConfigLoader, ReloadOutcome,
};
pub use parse::{
    parse_kdl_config, validate_config_file, validate_kdl_config, ValidationIssue,
    ValidationSeverity,
};
pub use types::*;
