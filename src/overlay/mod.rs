//! Painted overlays for wiri (workspace indicator, overview banner, etc.)
pub mod workspace_indicator;
pub mod overview_banner;
pub use workspace_indicator::{MonitorBounds, WorkspaceIndicator};
pub use overview_banner::OverviewBanner;
