//! Opening-only focus protection yields to explicit managed-window selection.
//!
//! Actual Task19 R2 trace: admission 05:10:11.672, typed FocusWindow handling
//! 05:10:13.750, then the admission guard restores the previous window at
//! 05:10:13.864. An intentional focus request must supersede admission intent;
//! an unsolicited foreground report must still be refused during the grace.
//!
//! This composes the production open-rule hook, the engine focus consumer used
//! by IPC, and the later foreground-event consumer. It does not qualify wire
//! decoding, OS activation success, event timing or native desktop behavior.
//! The model bypasses discovery/add_window and its shared hidden-window journal.
//! The entire fixture and every action run under the existing refused-activation
//! guard. Both HWNDs are invalid negative IDs, not Win32 pseudo handles. No
//! window, hook, input stream, message pump or daemon is created.

use super::*;
use crate::backend::activation_attempt_test_support::with_refused_activation_attempts;
use crate::backend::WindowState;
use crate::window::ResolvedWindowRules;
use std::time::Instant;

fn recent_admission_fixture() -> (TilingEngine, BackendHandle, WindowId, WindowId) {
    let mut engine = TilingEngine::new(LayoutConfig::default());
    let backend = BackendHandle::default_for_test();
    let output = OutputId::from_name("recent-admission-focus-model");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 900),
        Rect::new(0, 0, 1200, 860),
    );
    let previous = WindowId::new(-9501);
    let admitted = WindowId::new(-9502);
    for (window, x) in [(previous, 30), (admitted, 430)] {
        let bounds = Rect::new(x, 40, 350, 300);
        engine.tiled_windows.insert(
            window,
            WindowInfo {
                hwnd: window.as_isize(),
                title: "recent admission model".into(),
                class_name: "RecentAdmissionModel".into(),
                process_id: 1,
                bounds,
                state: WindowState::Normal,
                is_visible: true,
            },
        );
        engine.register_float(window, output, 0, bounds);
    }
    engine.record_ipc_foreground(previous);
    let before = engine.focus_before(output);
    engine.act_on_open_rules(
        admitted,
        &ResolvedWindowRules {
            open_focused: Some(false),
            ..ResolvedWindowRules::default()
        },
        before,
        false,
        &backend,
    );
    let pending = engine
        .unfocused_opens
        .get(&admitted)
        .expect("real open-rule hook creates the admission guard");
    assert_eq!(pending.previous, previous);
    assert!(
        pending.until > Instant::now(),
        "fixture must remain within real grace"
    );
    assert_eq!(engine.os_foreground, Some(previous));
    (engine, backend, previous, admitted)
}

#[test]
fn parity_recent_admission_explicit_focus_survives_foreground_report() {
    let (_, attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, _previous, admitted) = recent_admission_fixture();
        // This is the exact engine consumer invoked by server focus-window.
        assert!(engine.focus_action_window(admitted, &backend));
        assert_eq!(engine.os_foreground, Some(admitted));
        // No sleep or synthetic expiry: demonstrate this report still arrives
        // within the original admission grace if a marker remains pending.
        if let Some(pending) = engine.unfocused_opens.get(&admitted) {
            assert!(
                pending.until > Instant::now(),
                "fixture exceeded real grace"
            );
        }
        engine.sync_os_foreground(admitted, &backend);
        assert_eq!(
            engine.os_foreground,
            Some(admitted),
            "explicit focus supersedes open-focused=false admission intent"
        );
    });
    assert!(attempts.iter().all(|hwnd| matches!(*hwnd, -9501 | -9502)));
}

#[test]
fn parity_recent_admission_unsolicited_foreground_is_returned_once() {
    let (_, attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, previous, admitted) = recent_admission_fixture();
        assert!(
            engine.unfocused_opens[&admitted].until > Instant::now(),
            "fixture exceeded real grace"
        );
        engine.sync_os_foreground(admitted, &backend);
        assert_eq!(
            engine.os_foreground,
            Some(previous),
            "unsolicited focus must still respect open-focused=false"
        );
        assert!(!engine.unfocused_opens.contains_key(&admitted));
        // Admission protection is consumed once; do not reject later reports.
        engine.sync_os_foreground(admitted, &backend);
        assert_eq!(engine.os_foreground, Some(admitted));
    });
    assert!(
        attempts.contains(&-9501),
        "rejection dispatches restoration under the guard"
    );
    assert!(attempts.iter().all(|hwnd| matches!(*hwnd, -9501 | -9502)));
}

// Additional coverage: resolve a real modeled tile, and keep an unrelated
// admission marker intact when explicit target lookup fails.
fn recent_admission_tiled_fixture() -> (TilingEngine, BackendHandle, OutputId, WindowId, WindowId) {
    let mut cfg = LayoutConfig {
        column_width: 350,
        column_gap: 0,
        window_gap: 0,
        border_width: 0,
        outer_gaps: (0, 0, 0, 0),
        ..LayoutConfig::default()
    };
    cfg.effects.border.off = true;
    cfg.effects.focus_ring.off = true;
    cfg.effects.shadow.on = false;
    let mut engine = TilingEngine::new(cfg);
    let backend = BackendHandle::default_for_test();
    let output = OutputId::from_name("recent-admission-tiled-focus-model");
    engine.register_monitor(
        output,
        Rect::new(0, 0, 1200, 900),
        Rect::new(0, 0, 1200, 860),
    );
    let previous = WindowId::new(-9511);
    let admitted = WindowId::new(-9512);
    for (window, x) in [(previous, 0), (admitted, 350)] {
        engine.tiled_windows.insert(
            window,
            WindowInfo {
                hwnd: window.as_isize(),
                title: "recent admission tiled model".into(),
                class_name: "RecentAdmissionTiledModel".into(),
                process_id: 1,
                bounds: Rect::new(x, 0, 350, 860),
                state: WindowState::Normal,
                is_visible: true,
            },
        );
        let mut column = crate::layout::Column::new();
        column.add_tile(crate::layout::Tile::new(window));
        engine
            .monitors
            .get_mut(&output)
            .unwrap()
            .workspace_mut()
            .unwrap()
            .columns
            .push(column);
    }
    let monitor = engine.monitors.get_mut(&output).unwrap();
    monitor.focus_column = Some(0);
    monitor.focus_window = Some(previous);
    monitor.workspace_mut().unwrap().remember_focus(previous);
    engine.record_ipc_foreground(previous);
    let before = engine.focus_before(output);
    engine.act_on_open_rules(
        admitted,
        &ResolvedWindowRules {
            open_focused: Some(false),
            ..ResolvedWindowRules::default()
        },
        before,
        false,
        &backend,
    );
    assert_eq!(engine.unfocused_opens[&admitted].previous, previous);
    assert!(engine.unfocused_opens[&admitted].until > Instant::now());
    assert!(!engine.is_floating(previous) && !engine.is_floating(admitted));
    (engine, backend, output, previous, admitted)
}

#[test]
fn parity_recent_admission_tiled_explicit_focus_survives_foreground_report() {
    let (_, attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, output, _previous, admitted) = recent_admission_tiled_fixture();
        // This API internally applies the selected column. Both modeled columns
        // fit in the only workspace, so no window is hidden or journaled. Every
        // HWND is invalid; activation is refused before any native focus call.
        assert!(engine.focus_action_window(admitted, &backend));
        assert_eq!(
            engine.monitors.get(&output).unwrap().focus_window,
            Some(admitted)
        );
        if let Some(pending) = engine.unfocused_opens.get(&admitted) {
            assert!(
                pending.until > Instant::now(),
                "fixture exceeded real grace"
            );
        }
        // Native success is not fabricated: this is a subsequent reported fact.
        engine.sync_os_foreground(admitted, &backend);
        assert_eq!(
            engine.os_foreground,
            Some(admitted),
            "explicit tiled focus supersedes open-focused=false admission intent"
        );
        assert_eq!(
            engine.monitors.get(&output).unwrap().focus_window,
            Some(admitted)
        );
    });
    assert!(attempts.iter().all(|hwnd| matches!(*hwnd, -9511 | -9512)));
}

#[test]
fn parity_recent_admission_unknown_explicit_target_preserves_other_admission_guard() {
    let (_, attempts) = with_refused_activation_attempts(|| {
        let (mut engine, backend, previous, admitted) = recent_admission_fixture();
        let until = engine.unfocused_opens[&admitted].until;
        assert!(!engine.focus_action_window(WindowId::new(-9599), &backend));
        assert_eq!(engine.os_foreground, Some(previous));
        let pending = engine
            .unfocused_opens
            .get(&admitted)
            .expect("unknown explicit target cannot cancel another window's admission guard");
        assert_eq!(pending.previous, previous);
        assert_eq!(pending.until, until);
        assert!(
            pending.until > Instant::now(),
            "fixture exceeded real grace"
        );
        engine.sync_os_foreground(admitted, &backend);
        assert_eq!(
            engine.os_foreground,
            Some(previous),
            "failed target lookup must leave unsolicited admission focus protected"
        );
        assert!(!engine.unfocused_opens.contains_key(&admitted));
    });
    assert!(attempts.contains(&-9501));
    assert!(attempts.iter().all(|hwnd| matches!(*hwnd, -9501 | -9502)));
}
