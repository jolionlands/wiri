//! Animation options shared by the decoder and the layout timeline.
use crate::layout::animation::{AnimationKind, Easing, Spring};

#[derive(Debug, Clone)]
pub struct AnimationsConfig {
    // Legacy wiri fields remain source-compatible. An absent animations section
    // remains off; a niri-style `animations { ... }` section opts in.
    pub enabled: bool,
    pub duration: u32,
    pub easing: String,
    pub focus_transition: bool,
    pub workspace_transition: bool,
    pub window_move: bool,
    pub window_resize: bool,
    pub popup: bool,
    pub shadow: bool,
    pub slowdown: f64,
    pub legacy_easing: bool,
    pub default_kind: Option<AnimationKind>,
    pub workspace_switch: Option<AnimationKind>,
    pub horizontal_view_movement: Option<AnimationKind>,
    pub window_movement: Option<AnimationKind>,
    pub resize: Option<AnimationKind>,
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self::default_config()
    }
}

impl AnimationsConfig {
    pub fn default_config() -> Self {
        Self {
            enabled: false,
            duration: 200,
            easing: "cubic-bezier".into(),
            focus_transition: true,
            workspace_transition: true,
            window_move: true,
            window_resize: true,
            popup: true,
            shadow: true,
            slowdown: 1.0,
            legacy_easing: false,
            default_kind: None,
            workspace_switch: None,
            horizontal_view_movement: None,
            window_movement: None,
            resize: None,
        }
    }

    pub fn base_kind(&self) -> AnimationKind {
        self.default_kind.unwrap_or_else(|| {
            if self.legacy_easing {
                AnimationKind::Easing {
                    duration_ms: self.duration,
                    curve: Easing::parse(&self.easing).unwrap_or_default(),
                }
            } else {
                AnimationKind::Spring(Spring::default())
            }
        })
    }

    fn kind(
        &self,
        enabled: bool,
        specific: Option<AnimationKind>,
        stiffness: f64,
    ) -> AnimationKind {
        if !self.enabled || !enabled {
            return AnimationKind::Off;
        }
        specific.unwrap_or_else(|| {
            if self.default_kind.is_some() || self.legacy_easing {
                self.base_kind()
            } else {
                AnimationKind::Spring(Spring {
                    stiffness,
                    ..Spring::default()
                })
            }
        })
    }

    pub fn workspace_kind(&self) -> AnimationKind {
        self.kind(self.workspace_transition, self.workspace_switch, 1000.0)
    }

    pub fn view_kind(&self) -> AnimationKind {
        self.kind(self.focus_transition, self.horizontal_view_movement, 800.0)
    }

    pub fn movement_kind(&self) -> AnimationKind {
        self.kind(self.window_move, self.window_movement, 800.0)
    }

    pub fn resize_kind(&self) -> AnimationKind {
        self.kind(self.window_resize, self.resize, 800.0)
    }
}
