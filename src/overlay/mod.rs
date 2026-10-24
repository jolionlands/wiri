//! Painted overlays for wiri (workspace indicator, overview banner, etc.)
pub mod bindings_cheatsheet;
pub mod overview_banner;
pub mod status_bar;
pub mod thumbnail_overview;
pub mod workspace_indicator;
pub use bindings_cheatsheet::BindingsCheatsheet;
pub use overview_banner::OverviewBanner;
pub use status_bar::{StatusBar, StatusBarContent};
#[cfg(target_os = "windows")]
pub use thumbnail_overview::DwmThumbnailOverview;
pub use thumbnail_overview::{MockThumbnailOverview, ThumbnailOverviewSink};
pub use workspace_indicator::{MonitorBounds, WorkspaceIndicator};
