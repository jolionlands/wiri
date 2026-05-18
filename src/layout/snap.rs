//! Pure snap math for the interactive move-grab (niri-parity).
//!
//! While the user holds Alt and drags a tile, the low-level mouse hook in
//! `src/input/low_level_hook.rs` builds a list of candidate X positions
//! (column edges + mid-screen guide + monitor edges) and calls
//! [`find_snap`] to decide whether the cursor's projected window-left edge
//! is close enough to lock onto one of them.
//!
//! This file deliberately holds **no** Win32 or Windows-API code so it can
//! be unit-tested in isolation and reused by the touch backend in a future
//! pass.

/// The kind of guide a snap target represents.  Carried so the overlay can
/// colour-code the visible snap line (column edges vs the mid-screen guide
/// vs monitor edges) in a future visual pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapKind {
    /// The X edge of an existing tiled column on the active workspace.
    ColumnEdge,
    /// The horizontal centre of the focused monitor's work area.
    MidScreen,
    /// The left or right edge of the focused monitor's work area.
    MonitorEdge,
}

/// Result of a successful snap probe.  `x` is the snap target in screen-
/// space logical pixels; `kind` records which guide produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapTarget {
    pub x: i32,
    pub kind: SnapKind,
}

/// A single snap candidate fed into [`find_snap`].
#[derive(Debug, Clone, Copy)]
pub struct SnapCandidate {
    pub x: i32,
    pub kind: SnapKind,
}

/// Find the snap candidate closest to `cursor_x`, provided the distance is
/// within `threshold_px`.  Returns `None` when the candidate list is empty,
/// the threshold is non-positive, or no candidate is close enough.
///
/// Distance is measured as absolute pixels.  When two candidates tie, the
/// first one in the input slice wins (deterministic).
pub fn find_snap(
    cursor_x: i32,
    snap_candidates: &[SnapCandidate],
    threshold_px: i32,
) -> Option<SnapTarget> {
    if threshold_px <= 0 || snap_candidates.is_empty() {
        return None;
    }

    let mut best: Option<(i32, SnapCandidate)> = None;
    for cand in snap_candidates {
        let dist = (cand.x - cursor_x).abs();
        if dist > threshold_px {
            continue;
        }
        match best {
            Some((best_dist, _)) if dist >= best_dist => {}
            _ => best = Some((dist, *cand)),
        }
    }
    best.map(|(_, c)| SnapTarget { x: c.x, kind: c.kind })
}

/// Build the candidate snap list for the current monitor: every column
/// `left` edge, every column `right` edge, the mid-screen guide, and the
/// monitor's work-area left/right edges.  Pure function — accepts plain
/// integers so it can be tested without engine state.
pub fn build_candidates(
    column_lefts: &[i32],
    column_rights: &[i32],
    work_area_x: i32,
    work_area_width: i32,
) -> Vec<SnapCandidate> {
    let mut out: Vec<SnapCandidate> = Vec::with_capacity(column_lefts.len() * 2 + 3);
    for &x in column_lefts {
        out.push(SnapCandidate { x, kind: SnapKind::ColumnEdge });
    }
    for &x in column_rights {
        out.push(SnapCandidate { x, kind: SnapKind::ColumnEdge });
    }
    let mid = work_area_x + work_area_width / 2;
    out.push(SnapCandidate { x: mid, kind: SnapKind::MidScreen });
    out.push(SnapCandidate { x: work_area_x, kind: SnapKind::MonitorEdge });
    out.push(SnapCandidate {
        x: work_area_x + work_area_width,
        kind: SnapKind::MonitorEdge,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(x: i32) -> SnapCandidate {
        SnapCandidate { x, kind: SnapKind::ColumnEdge }
    }

    #[test]
    fn snap_when_in_range() {
        let cands = vec![cand(100), cand(500), cand(1000)];
        let r = find_snap(110, &cands, 20).expect("should snap");
        assert_eq!(r.x, 100);
        assert_eq!(r.kind, SnapKind::ColumnEdge);
    }

    #[test]
    fn no_snap_out_of_range() {
        let cands = vec![cand(100), cand(500)];
        assert!(find_snap(200, &cands, 20).is_none());
        assert!(find_snap(0, &cands, 20).is_none());
    }

    #[test]
    fn picks_nearest_of_many() {
        let cands = vec![cand(100), cand(120), cand(140), cand(160)];
        let r = find_snap(135, &cands, 50).expect("should snap");
        assert_eq!(r.x, 140);
    }

    #[test]
    fn empty_candidate_list() {
        assert!(find_snap(0, &[], 20).is_none());
        assert!(find_snap(100, &[], 1000).is_none());
    }

    #[test]
    fn threshold_zero_disables_snap() {
        let cands = vec![cand(100)];
        // Even an exact match returns None when threshold is 0 — keeps the
        // user-facing config switch a true on/off.
        assert!(find_snap(100, &cands, 0).is_none());
        assert!(find_snap(100, &cands, -5).is_none());
    }

    #[test]
    fn negative_cursor() {
        let cands = vec![cand(-100), cand(0), cand(50)];
        let r = find_snap(-95, &cands, 20).expect("should snap negative");
        assert_eq!(r.x, -100);
    }

    #[test]
    fn tie_takes_first() {
        let cands = vec![cand(100), cand(140)];
        // Cursor at 120 is equidistant from 100 and 140 — first wins.
        let r = find_snap(120, &cands, 50).expect("should snap");
        assert_eq!(r.x, 100);
    }

    #[test]
    fn mid_screen_kind_preserved() {
        let cands = vec![
            SnapCandidate { x: 100, kind: SnapKind::ColumnEdge },
            SnapCandidate { x: 960, kind: SnapKind::MidScreen },
        ];
        let r = find_snap(955, &cands, 20).expect("should snap to mid");
        assert_eq!(r.kind, SnapKind::MidScreen);
    }

    #[test]
    fn build_candidates_basic() {
        let lefts = vec![10, 510];
        let rights = vec![500, 1000];
        let cands = build_candidates(&lefts, &rights, 0, 1920);
        // 2 lefts + 2 rights + mid + 2 monitor edges = 7
        assert_eq!(cands.len(), 7);
        // Mid-screen at 960.
        assert!(cands.iter().any(|c| c.x == 960 && c.kind == SnapKind::MidScreen));
        // Monitor edges at 0 and 1920.
        assert!(cands.iter().any(|c| c.x == 0 && c.kind == SnapKind::MonitorEdge));
        assert!(cands.iter().any(|c| c.x == 1920 && c.kind == SnapKind::MonitorEdge));
    }

    #[test]
    fn build_candidates_empty_columns() {
        let cands = build_candidates(&[], &[], 100, 800);
        // No columns: still get mid + 2 edges = 3.
        assert_eq!(cands.len(), 3);
        let mid = cands.iter().find(|c| c.kind == SnapKind::MidScreen).unwrap();
        assert_eq!(mid.x, 500);
    }
}
