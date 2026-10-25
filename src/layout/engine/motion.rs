//! Connect deterministic timelines to the existing layout/backend driver.
use super::{BackendHandle, TilingEngine};
use crate::config::AnimationsConfig;
use crate::layout::animation::{AnimationTarget, RectAnimation};
use crate::utils::{OutputId, Rect, WindowId};
use std::sync::Arc;
use tokio::sync::Notify;

impl TilingEngine {
    pub fn set_animation_notifier(&mut self, wake: Arc<Notify>) {
        self.animation.set_notifier(Arc::clone(&wake));
        self.animation_wake = Some(wake);
    }

    pub(super) fn configure_motion(&mut self, options: &AnimationsConfig) {
        self.animation.cancel_all();
        self.animating_rects.clear();
        self.animation.set_enabled(options.enabled);
        self.animation.set_kind(options.workspace_kind());
        self.animation.set_slowdown(options.slowdown);
        self.motion = options.clone();
        // A reload can interrupt a frame. Reconcile from layout, not cached targets.
        for state in self.applied_state.values_mut() {
            state.rect = Rect::new(0, 0, 0, 0);
        }
    }

    pub(super) fn queue_rect(&mut self, wid: WindowId, from: Rect, to: Rect) -> bool {
        let movement = self.motion.movement_kind();
        let resize = self.motion.resize_kind();
        if self.is_overview()
            || !self.animation.is_enabled()
            || (movement.is_off() && resize.is_off())
            || self.motion.slowdown == 0.0
        {
            self.animating_rects.remove(&wid);
            return false;
        }
        if self.frame_in_progress {
            // Layout supplies the camera's displayed offset on each scalar frame.
            // Keep the independent resize/move timeline instead of dropping it and
            // letting layout snap the HWND to its final size on the first frame.
            if let Some(rect) = self.animating_rects.get_mut(&wid) {
                rect.translate(
                    to.loc.x.saturating_sub(from.loc.x),
                    to.loc.y.saturating_sub(from.loc.y),
                );
                return true;
            }
            return false;
        }
        if let Some(rect) = self.animating_rects.get_mut(&wid) {
            if rect.target != to {
                rect.retarget_kinds(to, movement, resize, self.motion.slowdown);
            }
        } else {
            self.animating_rects.insert(
                wid,
                RectAnimation::new_kinds(from, to, movement, resize, self.motion.slowdown),
            );
        }
        if let Some(wake) = &self.animation_wake {
            wake.notify_one();
        }
        true
    }

    /// One frame only. The caller has no timer pending when this returns false.
    pub fn tick_animations_with_backend(&mut self, delta_ms: u32, backend: &BackendHandle) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER};
        let mut finished = Vec::new();
        let mut frames = Vec::new();
        for (&wid, timeline) in &mut self.animating_rects {
            frames.push((
                wid.as_isize(),
                timeline.tick(delta_ms),
                SWP_NOACTIVATE | SWP_NOZORDER,
            ));
            if timeline.is_finished() {
                finished.push(wid);
            }
        }
        let _ = backend.set_window_positions_batched(&frames);
        for wid in finished {
            self.animating_rects.remove(&wid);
        }

        let mut outputs = Vec::new();
        for (target, value) in self.animation.tick(delta_ms) {
            match target {
                AnimationTarget::ScrollX(output) => {
                    if let Some(ws) = self
                        .monitors
                        .get_mut(&output)
                        .and_then(|m| m.workspace_mut())
                    {
                        ws.scroll_offset.x = value.round() as i32;
                    }
                    outputs.push(output);
                }
                AnimationTarget::WorkspaceX(output) => outputs.push(output),
                AnimationTarget::Opacity(wid) => {
                    if let Some(info) = self.tiled_windows.get(&wid) {
                        self.apply_window_opacity(
                            info.hwnd,
                            (value as f32 / 255.0).clamp(0.0, 1.0),
                        );
                    }
                }
                _ => {}
            }
        }
        outputs.sort_by_key(|o| o.as_u64());
        outputs.dedup();
        self.frame_in_progress = true;
        for output in outputs {
            self.apply_layout_for_monitor(output, backend);
        }
        self.frame_in_progress = false;
        self.has_active_animations()
    }

    /// Camera movements animate the scroll offset, not each column independently.
    pub(super) fn queue_view(&mut self, output: OutputId, from: i32, target: i32) {
        let key = AnimationTarget::ScrollX(output);
        let kind = self.motion.view_kind();
        if kind.is_off() || self.motion.slowdown == 0.0 {
            self.animation.cancel(&key);
            return;
        }
        if from == target {
            return;
        }
        if let Some(ws) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspace_mut())
        {
            ws.scroll_offset.x = from;
        }
        self.animation
            .animate_with_kind(key, from as f64, target as f64, kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Easing;
    #[test]
    fn backend_rect_driver_delivers_frames_and_stops() {
        let mut e = TilingEngine::new(Default::default());
        let options = AnimationsConfig {
            enabled: true,
            legacy_easing: true,
            duration: 100,
            easing: "linear".into(),
            ..Default::default()
        };
        e.configure_motion(&options);
        let wake = Arc::new(Notify::new());
        e.set_animation_notifier(Arc::clone(&wake));
        let wid = WindowId::new(0);
        let from = Rect::new(0, 0, 200, 300);
        let to = Rect::new(400, 80, 600, 700);
        assert!(e.queue_rect(wid, from, to));
        let backend = BackendHandle::default_for_test();
        assert!(e.tick_animations_with_backend(50, &backend));
        assert_eq!(
            e.animating_rects[&wid].current(),
            Rect::new(200, 40, 400, 500)
        );
        assert!(!e.tick_animations_with_backend(50, &backend));
        assert!(!e.has_active_animations());
        assert!(!e.tick_animations_with_backend(16, &backend));
    }
    #[test]
    fn camera_frames_preserve_a_concurrent_resize_timeline() {
        use crate::layout::animation::AnimationKind;
        let mut e = TilingEngine::new(Default::default());
        let kind = AnimationKind::Easing {
            duration_ms: 100,
            curve: Easing::Linear,
        };
        e.configure_motion(&AnimationsConfig {
            enabled: true,
            default_kind: Some(kind),
            ..Default::default()
        });
        let wid = WindowId::new(0);
        let from = Rect::new(1000, 8, 350, 712);
        let target = Rect::new(1000, 8, 650, 712);
        assert!(e.queue_rect(wid, from, target));
        let backend = BackendHandle::default_for_test();
        assert!(e.tick_animations_with_backend(25, &backend));
        let width = e.animating_rects[&wid].current().size.w;
        assert_eq!(width, 425);
        e.frame_in_progress = true;
        assert!(e.queue_rect(wid, target, Rect::new(900, 8, 650, 712)));
        e.frame_in_progress = false;
        assert_eq!(
            e.animating_rects[&wid].current(),
            Rect::new(900, 8, width, 712)
        );
        assert!(!e.tick_animations_with_backend(75, &backend));
        assert!(!e.has_active_animations());
    }

    #[test]
    fn reload_off_cancels_pending_motion() {
        let mut e = TilingEngine::new(Default::default());
        e.update_animation_settings(true, 100, Easing::Linear);
        assert!(e.queue_rect(
            WindowId::new(0),
            Rect::new(0, 0, 100, 100),
            Rect::new(80, 0, 100, 100)
        ));
        e.configure_motion(&AnimationsConfig::default());
        assert!(!e.has_active_animations());
        assert!(!e.queue_rect(
            WindowId::new(0),
            Rect::new(0, 0, 100, 100),
            Rect::new(80, 0, 100, 100)
        ));
    }
}
