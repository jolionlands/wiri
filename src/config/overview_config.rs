//! Niri's overview camera and workspace shadow (plain data, no system settings).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverviewConfig {
    pub zoom: f64,
    pub backdrop_color: [u8; 4],
    /// `overview { backdrop-color }` was written. Without it the overview paints the
    /// layout's `background-color` when there is one (see `TilingEngine::overview_options`).
    pub backdrop_set: bool,
    pub workspace_shadow: WorkspaceShadow,
}

impl Default for OverviewConfig {
    fn default() -> Self {
        Self {
            zoom: 0.5,
            backdrop_color: [0x26, 0x26, 0x26, 255],
            backdrop_set: false,
            workspace_shadow: WorkspaceShadow::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceShadow {
    pub off: bool,
    pub softness: f64,
    pub spread: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub color: [u8; 4],
}

impl Default for WorkspaceShadow {
    fn default() -> Self {
        Self {
            off: false,
            softness: 40.0,
            spread: 10.0,
            offset_x: 0.0,
            offset_y: 10.0,
            color: [0, 0, 0, 0x50],
        }
    }
}
