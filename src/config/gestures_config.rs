//! niri gestures block; presence of a drag block (or an empty block) opts in to Windows
//! drag/capture observation. `hot-corners` is independent of that opt-in.
use super::hot_corner_config::HotCorners;
#[derive(Debug, Clone)]
pub struct EdgeScroll {
    pub trigger: f64,
    pub delay_ms: u32,
    pub max_speed: f64,
}
#[derive(Debug, Clone)]
pub struct GesturesConfig {
    pub enabled: bool,
    pub view: EdgeScroll,
    pub workspace: EdgeScroll,
    /// Global overview hot corners (niri `gestures { hot-corners {} }`).
    pub hot_corners: HotCorners,
}
impl Default for GesturesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hot_corners: HotCorners::default(),
            view: EdgeScroll {
                trigger: 30.0,
                delay_ms: 100,
                max_speed: 1500.0,
            },
            workspace: EdgeScroll {
                trigger: 50.0,
                delay_ms: 100,
                max_speed: 1500.0,
            },
        }
    }
}
