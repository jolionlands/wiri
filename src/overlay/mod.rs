//! Painted overlays for wiri (workspace indicator, overview banner, etc.)
pub mod workspace_indicator;
pub mod overview_banner;
pub mod thumbnail_overview;
pub use workspace_indicator::{MonitorBounds, WorkspaceIndicator};
pub use overview_banner::OverviewBanner;
pub use thumbnail_overview::{MockThumbnailOverview, ThumbnailOverviewSink};
#[cfg(target_os = "windows")]
pub use thumbnail_overview::DwmThumbnailOverview;
