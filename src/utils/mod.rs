pub use anyhow::{Context, Result};
pub use thiserror::Error;
pub use tracing::{debug, error, info, trace, warn};

pub mod id;
pub mod rect;

pub use id::{OutputId, WindowId};
pub use rect::{Point, Rect, Size};
