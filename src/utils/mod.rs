pub use anyhow::{Context, Result};
pub use thiserror::Error;
pub use tracing::{debug, error, info, warn, trace};

pub mod rect;
pub mod id;

pub use rect::{Rect, Point, Size};
pub use id::{OutputId, WindowId};