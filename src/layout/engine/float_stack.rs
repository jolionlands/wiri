//! Event-driven floating layer, without HWND_TOP/TOPMOST requests to foreign threads.
//!
//! A tile can raise itself *after* its foreground report. Repair actual inversions
//! on REORDER as well as layout/focus, by lowering the offender below a normal-band
//! float. Even a late asynchronous completion cannot put a float over another app.

use std::collections::{HashMap, HashSet};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindow, GetWindowLongPtrW, IsIconic, IsWindowVisible, SetWindowPos, GWL_EXSTYLE,
    GW_HWNDPREV, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_TOPMOST,
};

use super::{query_foreground, TilingEngine};
use crate::utils::{OutputId, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StackRepair {
    window: WindowId,
    after: WindowId,
}

/// `floats` is MRU first, `above[i]` contains the relevant windows above floats[i]
/// (nearest first). Missing/hidden/minimized/other-workspace HWNDs are excluded by
/// the caller. Only actual inversions produce requests; a satisfied stack is inert.
fn stack_repairs(
    floats: &[WindowId],
    above: &HashMap<WindowId, Vec<WindowId>>,
    tiles: &HashSet<WindowId>,
) -> Vec<StackRepair> {
    let mut repairs = Vec::new();
    // Lower an older float that overtook its newer neighbour. Relative HWND anchors
    // converge even when different UI threads consume the requests in any order.
    for pair in floats.windows(2) {
        if above.get(&pair[0]).is_some_and(|a| a.contains(&pair[1])) {
            repairs.push(StackRepair {
                window: pair[1],
                after: pair[0],
            });
        }
    }
    // The lowest *actual* float is the safe anchor, including while the MRU repair
    // is in flight. A hung float does not prevent responsive tiles moving below it.
    let lowest = floats.iter().max_by_key(|f| {
        above
            .get(f)
            .map_or(0, |a| a.iter().filter(|w| floats.contains(w)).count())
    });
    if let Some(&lowest) = lowest {
        if let Some(windows) = above.get(&lowest) {
            for &window in windows {
                if tiles.contains(&window) {
                    repairs.push(StackRepair {
                        window,
                        after: lowest,
                    });
                }
            }
        }
    }
    repairs
}

fn hwnd(window: WindowId) -> HWND {
    HWND(window.as_isize() as *mut std::ffi::c_void)
}

fn visible(window: WindowId) -> bool {
    unsafe { IsWindowVisible(hwnd(window)).as_bool() && !IsIconic(hwnd(window)).as_bool() }
}

fn normal_band(window: WindowId) -> bool {
    unsafe { GetWindowLongPtrW(hwnd(window), GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize == 0 }
}

impl TilingEngine {
    /// REORDER is a native-stack notification, not a focus/layout/IPC-state change.
    pub fn reconcile_float_z_order(&self) {
        let outputs: HashSet<_> = self.float_homes.values().map(|h| h.output).collect();
        for output in outputs {
            self.raise_floats(output);
        }
    }

    /// Do not react to old queued foreground reports after the user switched away.
    fn float_stack_has_foreground(&self) -> bool {
        let Some(fg) = query_foreground().filter(|fg| !fg.ignorable) else {
            return false;
        };
        for window in [WindowId::new(fg.hwnd), WindowId::new(fg.root_owner)] {
            let location = if let Some(home) = self.float_homes.get(&window) {
                Some((home.output, home.workspace))
            } else {
                self.locate_window(window)
                    .map(|(output, ws, _)| (output, ws))
            };
            if let Some((output, workspace)) = location {
                if self
                    .monitors
                    .get(&output)
                    .is_some_and(|m| m.active_workspace == workspace)
                    && !self.minimized.contains_key(&window)
                    && visible(window)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Keep workspace floats above tiles without activating anything or raising a
    /// foreign-thread float to HWND_TOP. All submissions remain asynchronous, even
    /// for apparently responsive windows: a responsiveness check would have a race.
    pub(super) fn raise_floats(&self, output_id: OutputId) {
        let Some(monitor) = self.monitors.get(&output_id) else {
            return;
        };
        let mut floats: Vec<_> = self
            .float_homes
            .iter()
            .filter(|(wid, home)| {
                home.output == output_id
                    && home.workspace == monitor.active_workspace
                    && !self.fullscreen_windows.contains(*wid)
                    && !self.minimized.contains_key(*wid)
                    && visible(**wid)
                    && normal_band(**wid)
                    && !crate::backend::cloak::is_cloaked(wid.as_isize())
            })
            .map(|(wid, home)| (home.seq, *wid))
            .collect();
        if floats.is_empty() || !self.float_stack_has_foreground() {
            return;
        }
        floats.sort_by_key(|(seq, wid)| (std::cmp::Reverse(*seq), wid.as_isize()));
        let floats: Vec<_> = floats.into_iter().map(|(_, wid)| wid).collect();
        let tiles: HashSet<_> = monitor
            .workspace()
            .into_iter()
            .flat_map(|ws| &ws.columns)
            .flat_map(|col| &col.tiles)
            .map(|t| t.window_id)
            .filter(|wid| {
                visible(*wid)
                    && !self.minimized.contains_key(wid)
                    && self.applied_state.get(wid).is_some_and(|s| s.visible)
            })
            .collect();
        let mut above = HashMap::new();
        for &float in &floats {
            let mut windows = Vec::new();
            let mut seen = HashSet::new();
            let mut current = hwnd(float);
            // GetWindow reads USER's z-order; no messages to (possibly hung) apps.
            // A bound and cycle check also make concurrent destruction/reordering safe.
            for _ in 0..4096 {
                let Ok(prev) = (unsafe { GetWindow(current, GW_HWNDPREV) }) else {
                    break;
                };
                let window = WindowId::new(prev.0 as isize);
                if prev.0.is_null() || !seen.insert(window) {
                    break;
                }
                if tiles.contains(&window) || floats.contains(&window) {
                    windows.push(window);
                }
                current = prev;
            }
            above.insert(float, windows);
        }
        for repair in stack_repairs(&floats, &above, &tiles) {
            // Recheck anchors before submission; never insert after a topmost HWND.
            // No global TOP request is queued, so switching to an unrelated application
            // while the target thread is slow cannot make a float cover that application.
            if !visible(repair.window) || !visible(repair.after) || !normal_band(repair.after) {
                continue;
            }
            tracing::debug!(
                "Float stack: place {} below {}",
                repair.window,
                repair.after
            );
            unsafe {
                let _ = SetWindowPos(
                    hwnd(repair.window),
                    hwnd(repair.after),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(floats: &[isize], tiles: &[isize], order: &[isize]) -> Vec<StackRepair> {
        let floats: Vec<_> = floats.iter().copied().map(WindowId::new).collect();
        let above = floats
            .iter()
            .map(|&f| {
                let end = order.iter().position(|&w| w == f.as_isize()).unwrap();
                let windows = order[..end]
                    .iter()
                    .rev()
                    .copied()
                    .map(WindowId::new)
                    .collect();
                (f, windows)
            })
            .collect();
        stack_repairs(
            &floats,
            &above,
            &tiles.iter().copied().map(WindowId::new).collect(),
        )
    }
    fn movement(window: isize, after: isize) -> StackRepair {
        StackRepair {
            window: WindowId::new(window),
            after: WindowId::new(after),
        }
    }
    fn apply(order: &mut Vec<isize>, repair: StackRepair) {
        let window = repair.window.as_isize();
        order.retain(|w| *w != window);
        let index = order
            .iter()
            .position(|w| *w == repair.after.as_isize())
            .unwrap();
        order.insert(index + 1, window);
    }

    #[test]
    fn satisfied_layer_is_inert_even_with_unrelated_windows_above_it() {
        assert!(plan(&[1, 2], &[3, 4], &[90, 1, 91, 2, 3, 4]).is_empty());
    }
    #[test]
    fn delayed_tile_raise_lowers_only_the_offending_tile() {
        assert_eq!(plan(&[1, 2], &[3, 4], &[3, 1, 2, 4]), vec![movement(3, 2)]);
    }
    #[test]
    fn tile_between_floats_goes_below_the_actual_lowest_float() {
        assert_eq!(plan(&[1, 2], &[3], &[1, 3, 2]), vec![movement(3, 2)]);
    }
    #[test]
    fn asynchronous_float_order_is_repaired_with_relative_anchors() {
        assert_eq!(plan(&[1, 2], &[3], &[2, 1, 3]), vec![movement(2, 1)]);
    }
    #[test]
    fn no_floats_never_reorders_tiles() {
        assert!(plan(&[], &[3, 4], &[4, 3]).is_empty());
    }
    #[test]
    fn float_only_workspace_repairs_mru_without_a_tile() {
        assert_eq!(
            plan(&[1, 2, 3], &[], &[3, 2, 1]),
            vec![movement(2, 1), movement(3, 2)]
        );
    }
    #[test]
    fn unrelated_and_excluded_workspace_windows_are_never_moved() {
        assert_eq!(plan(&[1], &[3], &[99, 7, 3, 1]), vec![movement(3, 1)]);
    }
    #[test]
    fn repaired_stack_produces_no_feedback_requests() {
        let mut order = vec![3, 1, 2, 4];
        for repair in plan(&[1, 2], &[3, 4], &order) {
            apply(&mut order, repair);
        }
        assert!(plan(&[1, 2], &[3, 4], &order).is_empty());
    }
    #[test]
    fn all_small_stacks_converge_for_either_async_completion_order() {
        fn permutations(values: &mut [isize], start: usize, out: &mut Vec<Vec<isize>>) {
            if start == values.len() {
                out.push(values.to_vec());
                return;
            }
            for i in start..values.len() {
                values.swap(start, i);
                permutations(values, start + 1, out);
                values.swap(start, i);
            }
        }
        let mut orders = Vec::new();
        permutations(&mut [1, 2, 3, 4, 5], 0, &mut orders);
        for original in orders {
            for reverse in [false, true] {
                let mut order = original.clone();
                for _ in 0..12 {
                    let mut repairs = plan(&[1, 2, 3], &[4, 5], &order);
                    if repairs.is_empty() {
                        break;
                    }
                    if reverse {
                        repairs.reverse();
                    }
                    for repair in repairs {
                        apply(&mut order, repair);
                    }
                }
                assert!(
                    plan(&[1, 2, 3], &[4, 5], &order).is_empty(),
                    "{original:?} -> {order:?}"
                );
                assert_eq!(&order[..3], &[1, 2, 3]);
            }
        }
    }
}
