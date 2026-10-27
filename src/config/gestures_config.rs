//! niri gestures block; presence opts in to Windows drag/capture observation.
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
}
impl Default for GesturesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
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
