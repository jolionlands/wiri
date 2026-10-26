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
    pub window_open: Option<AnimationKind>,
    pub window_close: Option<AnimationKind>,
    pub overview_open_close: Option<AnimationKind>,
    /// Overview defaults to niri's spring even when legacy HWND motion is not opted in.
    pub overview_enabled: bool,
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
            window_open: None,
            window_close: None,
            overview_open_close: None,
            overview_enabled: true,
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

    fn content_kind(&self, specific: Option<AnimationKind>, curve: Easing) -> AnimationKind {
        if !self.enabled {
            return AnimationKind::Off;
        }
        specific.unwrap_or_else(|| {
            if self.legacy_easing || self.default_kind.is_some() {
                self.base_kind()
            } else {
                AnimationKind::Easing {
                    duration_ms: 150,
                    curve,
                }
            }
        })
    }

    pub fn open_kind(&self) -> AnimationKind {
        self.content_kind(self.window_open, Easing::ExpoOut)
    }
    pub fn close_kind(&self) -> AnimationKind {
        self.content_kind(self.window_close, Easing::QuadOut)
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

    pub fn overview_kind(&self) -> AnimationKind {
        if !self.overview_enabled {
            return AnimationKind::Off;
        }
        self.overview_open_close.unwrap_or_else(|| {
            if self.default_kind.is_some() || self.legacy_easing {
                self.base_kind()
            } else {
                AnimationKind::Spring(Spring {
                    stiffness: 800.0,
                    ..Spring::default()
                })
            }
        })
    }

    pub fn resize_kind(&self) -> AnimationKind {
        self.kind(self.window_resize, self.resize, 800.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn easing(duration_ms: u32, curve: Easing) -> AnimationKind {
        AnimationKind::Easing { duration_ms, curve }
    }

    #[test]
    fn open_close_defaults_require_animation_opt_in() {
        let mut cfg = AnimationsConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.window_open, None);
        assert_eq!(cfg.window_close, None);
        assert_eq!(cfg.open_kind(), AnimationKind::Off);
        assert_eq!(cfg.close_kind(), AnimationKind::Off);

        cfg.enabled = true;
        assert_eq!(cfg.open_kind(), easing(150, Easing::ExpoOut));
        assert_eq!(cfg.close_kind(), easing(150, Easing::QuadOut));
    }

    #[test]
    fn open_close_follow_legacy_easing_and_explicit_global_kinds() {
        let mut cfg = AnimationsConfig {
            enabled: true,
            duration: 450,
            easing: "linear".into(),
            ..AnimationsConfig::default()
        };
        // Legacy fields only override the niri defaults when explicitly opted in.
        assert_eq!(cfg.open_kind(), easing(150, Easing::ExpoOut));
        assert_eq!(cfg.close_kind(), easing(150, Easing::QuadOut));
        cfg.legacy_easing = true;
        assert_eq!(cfg.open_kind(), easing(450, Easing::Linear));
        assert_eq!(cfg.close_kind(), easing(450, Easing::Linear));

        for kind in [
            AnimationKind::Off,
            easing(320, Easing::QuadOut),
            AnimationKind::Spring(Spring {
                damping_ratio: 0.7,
                stiffness: 900.0,
                epsilon: 0.001,
            }),
        ] {
            cfg.default_kind = Some(kind);
            assert_eq!(cfg.open_kind(), kind);
            assert_eq!(cfg.close_kind(), kind);
        }
    }

    #[test]
    fn open_close_overrides_are_independent_but_global_off_wins() {
        let spring = AnimationKind::Spring(Spring {
            damping_ratio: 0.7,
            stiffness: 900.0,
            epsilon: 0.001,
        });
        for (open, close) in [
            (AnimationKind::Off, easing(240, Easing::Linear)),
            (spring, AnimationKind::Off),
            (easing(310, Easing::ExpoOut), spring),
        ] {
            let mut cfg = AnimationsConfig {
                enabled: true,
                default_kind: Some(easing(450, Easing::CubicOut)),
                window_open: Some(open),
                window_close: Some(close),
                ..AnimationsConfig::default()
            };
            assert_eq!(cfg.open_kind(), open);
            assert_eq!(cfg.close_kind(), close);

            cfg.enabled = false;
            assert_eq!(cfg.open_kind(), AnimationKind::Off);
            assert_eq!(cfg.close_kind(), AnimationKind::Off);
            assert_eq!(cfg.window_open, Some(open));
            assert_eq!(cfg.window_close, Some(close));

            cfg.enabled = true;
            assert_eq!(cfg.open_kind(), open);
            assert_eq!(cfg.close_kind(), close);
        }
    }

    #[test]
    fn legacy_geometry_flags_do_not_disable_open_close() {
        let cfg = AnimationsConfig {
            enabled: true,
            focus_transition: false,
            workspace_transition: false,
            window_move: false,
            window_resize: false,
            popup: false,
            shadow: false,
            ..AnimationsConfig::default()
        };
        assert_eq!(cfg.open_kind(), easing(150, Easing::ExpoOut));
        assert_eq!(cfg.close_kind(), easing(150, Easing::QuadOut));
        assert_eq!(cfg.workspace_kind(), AnimationKind::Off);
        assert_eq!(cfg.view_kind(), AnimationKind::Off);
        assert_eq!(cfg.movement_kind(), AnimationKind::Off);
        assert_eq!(cfg.resize_kind(), AnimationKind::Off);
    }
}
