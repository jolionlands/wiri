//! Hot-corner policy from niri e19df12cf8 (gestures.rs / output.rs).
//! Explicit names replace the default top-left selection. The global block lives in
//! `gestures_config::GesturesConfig::hot_corners`; an output may carry its own.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HotCorners {
    pub off: bool,
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_left: bool,
    pub bottom_right: bool,
}

impl HotCorners {
    /// With no named corners niri defaults to top-left. Explicit names replace
    /// that default, rather than adding to it; `off` wins over every name.
    pub fn top_left_enabled(self) -> bool {
        !self.off && (self.top_left || !(self.top_right || self.bottom_left || self.bottom_right))
    }

    /// Enabled corners in top-left, top-right, bottom-left, bottom-right order.
    pub fn enabled(self) -> [bool; 4] {
        if self.off {
            [false; 4]
        } else {
            [
                self.top_left_enabled(),
                self.top_right,
                self.bottom_left,
                self.bottom_right,
            ]
        }
    }

    /// An output block replaces the global block; an absent block inherits it.
    pub fn for_output(self, output: Option<Self>) -> Self {
        output.unwrap_or(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_explicit_names() {
        assert!(HotCorners::default().top_left_enabled());
        for other in [
            HotCorners {
                top_right: true,
                ..HotCorners::default()
            },
            HotCorners {
                bottom_left: true,
                ..HotCorners::default()
            },
            HotCorners {
                bottom_right: true,
                ..HotCorners::default()
            },
        ] {
            assert!(!other.top_left_enabled());
            assert!(HotCorners {
                top_left: true,
                ..other
            }
            .top_left_enabled());
        }
        assert!(!HotCorners {
            off: true,
            top_left: true,
            ..HotCorners::default()
        }
        .top_left_enabled());
    }

    #[test]
    fn output_inherits_or_replaces_the_entire_global_block() {
        let disabled = HotCorners {
            off: true,
            ..HotCorners::default()
        };
        assert_eq!(disabled.for_output(None), disabled);
        assert!(disabled
            .for_output(Some(HotCorners::default()))
            .top_left_enabled());
        assert!(!HotCorners::default()
            .for_output(Some(disabled))
            .top_left_enabled());
    }
}
