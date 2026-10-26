//! Part-time compositor. The existing input pump owns the only HWND; the existing
//! animation driver supplies frames. No resident window, worker, timer or device.
mod snapshot;
mod win;

use crate::layout::animation::{Animation, AnimationKind};
use crate::utils::Rect;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) use snapshot::capture;
pub(crate) use snapshot::frame as source_frame;
pub use snapshot::Snapshot;
pub use win::{
    attach_message_thread, detach_message_thread, handle_message, preserve_cloak, ready,
};

static INPUT_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Hooks only record activity: never wait for an application or acquire the engine lock.
pub fn input_activity() {
    INPUT_EPOCH.fetch_add(1, Ordering::Relaxed);
}
pub fn input_epoch() -> u64 {
    INPUT_EPOCH.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Effect {
    Open = 1,
    Close = 2,
    Workspace = 3,
    Resize = 4,
}

pub struct Tile {
    pub source: isize,
    pub from: Rect,
    pub to: Rect,
    pub clip: Rect,
    /// Outgoing workspaces must remain hidden when the layer releases its cloak.
    pub outgoing: bool,
    pub snapshot: Option<Snapshot>,
}

pub struct Scene {
    pub id: u64,
    pub effect: Effect,
    pub tiles: Vec<Tile>,
}

pub fn start(scene: Scene) -> bool {
    win::start(scene)
}
pub fn frame(id: u64, progress: f64) {
    win::frame(id, progress);
}
pub fn finish(id: u64) {
    win::finish(id);
}

/// The same scalar timeline as movement, including springs, slowdown and instant axes.
pub struct Timeline {
    value: Animation,
    epoch: u64,
}
impl Timeline {
    pub fn new(kind: AnimationKind, slowdown: f64) -> Self {
        Self {
            value: Animation::with_kind(0.0, 1.0, kind, slowdown),
            epoch: input_epoch(),
        }
    }
    pub fn tick(&mut self, delta_ms: u32) -> f64 {
        self.value.tick(delta_ms)
    }
    pub fn finished(&self) -> bool {
        self.value.is_finished() || self.epoch != input_epoch()
    }
    pub fn interrupted(&self) -> bool {
        self.epoch != input_epoch()
    }
}

/// niri's default open scales 0.5 -> 1; close scales 1 -> 0.8. Workspace
/// coordinates may overshoot with springs; alpha must always stay representable.
pub(super) fn geometry(effect: Effect, from: Rect, to: Rect, p: f64) -> (Rect, u8) {
    let lerp = |a: f64, b: f64| a + (b - a) * p;
    let rect = Rect::new(
        lerp(from.loc.x as f64, to.loc.x as f64).round() as i32,
        lerp(from.loc.y as f64, to.loc.y as f64).round() as i32,
        lerp(from.size.w as f64, to.size.w as f64)
            .round()
            .clamp(1.0, 32768.0) as u32,
        lerp(from.size.h as f64, to.size.h as f64)
            .round()
            .clamp(1.0, 32768.0) as u32,
    );
    let scale = match effect {
        Effect::Open => 0.5 + p * 0.5,
        Effect::Close => 1.0 - p * 0.2,
        _ => 1.0,
    }
    .clamp(0.01, 4.0);
    let w = (rect.size.w as f64 * scale).round().max(1.0) as u32;
    let h = (rect.size.h as f64 * scale).round().max(1.0) as u32;
    let alpha = match effect {
        Effect::Open | Effect::Resize => p,
        Effect::Close => 1.0 - p,
        Effect::Workspace => 1.0,
    };
    (
        Rect::new(
            rect.loc
                .x
                .saturating_add((rect.size.w as i64 - w as i64) as i32 / 2),
            rect.loc
                .y
                .saturating_add((rect.size.h as i64 - h as i64) as i32 / 2),
            w,
            h,
        ),
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Clip destination and crop source proportionally, rather than squashing at an output edge.
pub(super) fn crop(dst: Rect, clip: Rect, width: u32, height: u32) -> Option<(Rect, Rect)> {
    if dst.size.w == 0 || dst.size.h == 0 || width == 0 || height == 0 {
        return None;
    }
    let l = (dst.loc.x as i64).max(clip.loc.x as i64);
    let t = (dst.loc.y as i64).max(clip.loc.y as i64);
    let r = (dst.loc.x as i64 + dst.size.w as i64).min(clip.loc.x as i64 + clip.size.w as i64);
    let b = (dst.loc.y as i64 + dst.size.h as i64).min(clip.loc.y as i64 + clip.size.h as i64);
    if l >= r || t >= b {
        return None;
    }
    let x = |x: i64| ((x - dst.loc.x as i64) * width as i64 / dst.size.w as i64) as i32;
    let y = |y: i64| ((y - dst.loc.y as i64) * height as i64 / dst.size.h as i64) as i32;
    Some((
        Rect::new(l as i32, t as i32, (r - l) as u32, (b - t) as u32),
        Rect::new(
            x(l),
            y(t),
            (x(r) - x(l)).max(1) as u32,
            (y(b) - y(t)).max(1) as u32,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::animation::{Easing, Spring};
    #[test]
    fn niri_open_and_close_are_centred_and_fade() {
        let r = Rect::new(-800, 20, 400, 600);
        assert_eq!(
            geometry(Effect::Open, r, r, 0.0),
            (Rect::new(-700, 170, 200, 300), 0)
        );
        assert_eq!(geometry(Effect::Open, r, r, 1.0), (r, 255));
        assert_eq!(
            geometry(Effect::Close, r, r, 1.0),
            (Rect::new(-760, 80, 320, 480), 0)
        );
        assert_eq!(geometry(Effect::Resize, r, r, 0.5).1, 128);
    }
    #[test]
    fn vertical_slide_keeps_size_and_crop_does_not_squash() {
        let a = Rect::new(-1000, 600, 400, 600);
        let b = Rect::new(-1000, 0, 400, 600);
        let mid = geometry(Effect::Workspace, a, b, 0.5).0;
        assert_eq!(mid, Rect::new(-1000, 300, 400, 600));
        assert_eq!(
            crop(mid, Rect::new(-1366, 0, 1366, 728), 400, 600),
            Some((Rect::new(-1000, 300, 400, 428), Rect::new(0, 0, 400, 428)))
        );
        assert!(crop(a, Rect::new(0, 0, 1, 1), 400, 600).is_none());
        assert!(crop(a, b, 0, 600).is_none());
    }
    #[test]
    fn compositor_uses_configured_timeline_and_stops() {
        let kind = AnimationKind::Easing {
            duration_ms: 150,
            curve: Easing::Linear,
        };
        let mut a = Timeline::new(kind, 10.0);
        assert_eq!(a.tick(750), 0.5);
        assert!(!a.finished());
        assert_eq!(a.tick(750), 1.0);
        assert!(a.finished());
        assert!(Timeline::new(AnimationKind::Off, 1.0).finished());
        let mut spring = Timeline::new(AnimationKind::Spring(Spring::default()), 1.0);
        spring.tick(60_000);
        assert!(spring.finished());
    }
    #[test]
    fn overshoot_cannot_produce_invalid_alpha_or_size() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(geometry(Effect::Open, r, r, 1.1).1, 255);
        assert_eq!(geometry(Effect::Close, r, r, 1.1).1, 0);
        assert!(
            geometry(Effect::Resize, r, Rect::new(0, 0, 1, 1), 2.0)
                .0
                .size
                .w
                > 0
        );
    }
}
