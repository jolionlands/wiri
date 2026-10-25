//! Window rules at work.
//!
//! The resolver (`window/rules.rs`) says what the rules want of a window; this is where the
//! engine does it: which monitor and workspace a new window opens on, how wide and how tall,
//! whether it takes the focus or starts fullscreen, and what stays true of it while it lives
//! (its opacity and its size limits), looked at again when something the rules depend on
//! changes.
//!
//! Nothing here runs on a timer.  A rule is looked at again when an event says that something
//! it depends on has changed: the title of the window, the foreground window, floating,
//! urgency, a config reload.  And only the windows concerned are looked at, only when some
//! rule depends on that kind of state at all (`RuleFlags`).

use super::*;
use crate::config::types::{MatcherContext, SizeSpec};
use crate::config::WindowRule;
use crate::layout::workspace::ColumnDisplay;
use crate::window::ResolvedWindowRules;
use std::time::{Duration, Instant};

/// What `AppliedState::opacity` holds for a window nothing has made transparent.
pub(super) const NO_ALPHA: u32 = 0xFFFF_FFFF;

/// How long after a window opened with `open-focused false` the focus that Windows hands to
/// it is taken back.  After that a focus report for the window is the user asking for it.
const UNFOCUSED_OPEN_GRACE: Duration = Duration::from_millis(2500);

/// The least height a tile that takes what the others leave gets, however much they ask for.
const LEAST_AUTO_TILE_PX: i32 = 50;

// ---------------------------------------------------------------------------------------------
// What the engine remembers
// ---------------------------------------------------------------------------------------------

/// The kinds of window state the configured rules depend on.  A change of one of them makes
/// the engine look at the rules again for the windows concerned; kinds that no rule looks at
/// cost nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RuleFlags {
    /// A rule has an `app-id` condition: the AppUserModelID of a window is asked for once.
    pub aumid: bool,
    /// A rule depends on which window has the focus (`is-active`, `is-focused`, ...).
    pub focus: bool,
    /// A rule depends on the title.
    pub title: bool,
    /// A rule depends on `is-floating`.
    pub floating: bool,
    /// A rule depends on `is-urgent`.
    pub urgency: bool,
}

impl RuleFlags {
    pub(super) fn of(rules: &[WindowRule]) -> RuleFlags {
        RuleFlags {
            aumid: rules.iter().any(|r| r.uses_app_id()),
            focus: rules.iter().any(|r| r.depends_on_focus()),
            title: rules.iter().any(|r| r.depends_on_title()),
            floating: rules.iter().any(|r| r.depends_on_floating()),
            urgency: rules.iter().any(|r| r.depends_on_urgency()),
        }
    }
}

/// What the rules decided for one window, and the facts about the window that they were
/// decided from (the title is in `tiled_windows`, kept up to date by `window_title_changed`).
#[derive(Debug, Clone)]
pub(super) struct RuleState {
    /// The file name of the window's process (`firefox.exe`).
    pub process_name: Option<String>,
    /// The AppUserModelID of the window, when a rule asks for it and the window has one.
    pub aumid: Option<String>,
    /// The result of the last evaluation.
    pub resolved: ResolvedWindowRules,
}

/// The size limits the rules put on a window, in pixels of the window (the border around it
/// is not counted; 0 = no limit).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RuleLimits {
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

impl RuleLimits {
    pub(super) fn of(rules: &ResolvedWindowRules) -> RuleLimits {
        RuleLimits {
            min_width: rules.min_width.unwrap_or(0),
            max_width: rules.max_width.unwrap_or(0),
            min_height: rules.min_height.unwrap_or(0),
            max_height: rules.max_height.unwrap_or(0),
        }
    }

    pub(super) fn is_none(&self) -> bool {
        *self == RuleLimits::default()
    }

    /// A limit on the window as a limit on its tile: the border comes on top; no limit stays
    /// no limit.
    pub(super) fn for_tile(limit: u32, border_both_sides: u32) -> u32 {
        if limit == 0 {
            0
        } else {
            limit.saturating_add(border_both_sides)
        }
    }

    /// `size` held to the limits (a minimum above a maximum wins).
    pub(super) fn clamp(&self, width: u32, height: u32) -> (u32, u32) {
        let one = |size: u32, min: u32, max: u32| {
            let mut size = size;
            if max > 0 {
                size = size.min(max);
            }
            size.max(min)
        };
        (
            one(width, self.min_width, self.max_width),
            one(height, self.min_height, self.max_height),
        )
    }
}

/// A window that opened with `open-focused false`: the window that had the focus, and until
/// when a focus report for the new window means Windows handing it the focus on its own.
#[derive(Debug, Clone, Copy)]
pub(super) struct UnfocusedOpen {
    pub previous: WindowId,
    pub until: Instant,
}

/// Where the focus of a monitor was before a window opened on it.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct FocusBefore {
    pub window: Option<WindowId>,
}

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// A size from a rule as pixels of a whole tile: a proportion of `view_px` (with `gap_px`
/// between neighbours, as niri counts it), or a fixed size of the window plus the border
/// around it.
pub(super) fn spec_px(spec: SizeSpec, view_px: i32, gap_px: i32, border_px: i32) -> i32 {
    match spec {
        SizeSpec::Proportion(p) => proportion_px(view_px, gap_px, p),
        SizeSpec::Fixed(px) => (px as i32).saturating_add(border_px),
    }
}

/// What a tile asks of the height of its column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HeightSpec {
    /// A height of its own (`default-window-height`), in pixels of the tile.
    pub fixed: Option<i32>,
    /// Its share of what the fixed ones leave, relative to the other tiles.
    pub weight: f32,
    /// Limits in pixels of the tile (0 = none); the minimum wins over the maximum.
    pub min: i32,
    pub max: i32,
}

/// The heights of the visible tiles of one column, top to bottom, that use `available` pixels.
///
/// * A tile with a height of its own gets it (held to its limits); the others share the rest
///   by their weights, held to their limits.  When all ask for a height, the last one takes
///   what is left instead.  When the asks leave less than [`LEAST_AUTO_TILE_PX`] for each
///   of the others, they are cut back in proportion.
/// * Without limits and fixed heights the tiles get their share of `available` (rounded
///   down); the last one takes what the rounding leaves, so that the column is filled.
/// * A column of one tile fills the view, within its limits.
/// * The limits can leave part of the column empty (every tile at its maximum) or ask for
///   more than there is (the minimums add up to more): the heights say so.
pub(super) fn distribute_heights(available: i32, tiles: &[HeightSpec]) -> Vec<i32> {
    let n = tiles.len();
    if n == 0 {
        return Vec::new();
    }
    let available = available.max(0);
    let limit = |height: i32, tile: &HeightSpec| -> i32 {
        let mut height = height;
        if tile.max > 0 {
            height = height.min(tile.max);
        }
        height.max(tile.min.max(0))
    };
    if n == 1 {
        return vec![limit(available, &tiles[0])];
    }

    let mut height: Vec<Option<i32>> = tiles
        .iter()
        .map(|t| t.fixed.map(|f| limit(f.max(0), t)))
        .collect();
    if height.iter().all(Option::is_some) {
        if let Some(last) = height.last_mut() {
            *last = None;
        }
    }
    let takers = height.iter().filter(|h| h.is_none()).count() as i32;
    let asked: i32 = height.iter().flatten().sum();
    let budget = (available - takers * LEAST_AUTO_TILE_PX).max(0);
    if asked > budget && asked > 0 {
        let scale = budget as f64 / asked as f64;
        for h in height.iter_mut().flatten() {
            *h = (*h as f64 * scale).floor() as i32;
        }
    }
    let mut rest = available - height.iter().flatten().sum::<i32>();

    // The others share `rest` by weight.  One that would end up outside its limits is held to
    // the limit and the rest is shared again among those that are left.
    let mut open: Vec<usize> = (0..n).filter(|&i| height[i].is_none()).collect();
    while !open.is_empty() {
        let weights: Vec<f32> = open.iter().map(|&i| tiles[i].weight.max(0.1)).collect();
        let total: f32 = weights.iter().sum();
        let shares: Vec<i32> = weights
            .iter()
            .map(|w| ((rest as f32) * (w / total)).floor() as i32)
            .collect();
        let held: Vec<usize> = (0..open.len())
            .filter(|&k| limit(shares[k], &tiles[open[k]]) != shares[k])
            .collect();
        if held.is_empty() {
            let leftover = rest - shares.iter().sum::<i32>();
            for (k, &i) in open.iter().enumerate() {
                let mut h = shares[k];
                if k + 1 == open.len() && limit(h + leftover, &tiles[i]) == h + leftover {
                    h += leftover;
                }
                height[i] = Some(h);
            }
            break;
        }
        for &k in &held {
            let i = open[k];
            let h = limit(shares[k], &tiles[i]);
            height[i] = Some(h);
            rest -= h;
        }
        open = open
            .iter()
            .enumerate()
            .filter(|(k, _)| !held.contains(k))
            .map(|(_, &i)| i)
            .collect();
    }
    height.into_iter().map(|h| h.unwrap_or(0)).collect()
}

/// The alpha (0 to 255) a window should have, or [`NO_ALPHA`] when nothing asks for any: the
/// opacity of its rule times the dimming of an unfocused window.  `applied` is what the window
/// has now; when nothing asks for transparency any more it is given back (255) once.
pub(super) fn wanted_alpha(
    dim_unfocused: f32,
    focused: bool,
    rule: Option<f32>,
    applied: u32,
) -> u32 {
    let dimming = dim_unfocused < 1.0;
    if !dimming && rule.is_none() {
        return if applied != NO_ALPHA { 255 } else { NO_ALPHA };
    }
    let dim = if dimming && !focused {
        dim_unfocused
    } else {
        1.0
    };
    (dim * rule.unwrap_or(1.0) * 255.0).round().clamp(0.0, 255.0) as u32
}

// ---------------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------------

impl TilingEngine {
    // ---- monitors by name ---------------------------------------------------------------

    /// Tell the engine the device name of a monitor (`\\.\DISPLAY1`), which `open-on-output`
    /// and the IPC refer to it by.
    pub fn set_output_name(&mut self, output: OutputId, name: &str) {
        self.output_names.insert(output, name.to_string());
    }

    /// The device name of a monitor, when the engine was told.
    pub fn output_name(&self, output: OutputId) -> Option<&str> {
        self.output_names.get(&output).map(String::as_str)
    }

    /// The monitor an `open-on-output` names: its device name (`\\.\DISPLAY1`) or that name
    /// without the `\\.\` (`DISPLAY1`), whatever the case.  `None` for a name that no monitor
    /// has.
    pub(super) fn find_output_by_name(&self, name: &str) -> Option<OutputId> {
        fn short(name: &str) -> String {
            name.trim().trim_start_matches("\\\\.\\").to_ascii_lowercase()
        }
        let wanted = short(name);
        if wanted.is_empty() {
            return None;
        }
        if let Some((output, _)) = self
            .output_names
            .iter()
            .find(|(output, device)| self.monitors.contains_key(output) && short(device) == wanted)
        {
            return Some(*output);
        }
        // A monitor registered without a name (a test): the id is a hash of the device name.
        let hashed = OutputId::from_name(name.trim());
        self.monitors.contains_key(&hashed).then_some(hashed)
    }

    // ---- what a rule can ask about a window -----------------------------------------------

    fn rule_window_is_focused(&self, window_id: WindowId) -> bool {
        self.os_foreground == Some(window_id)
    }

    /// The window is the one its monitor's focus rests on (a floating window: the one the
    /// OS has in the foreground).
    fn rule_window_is_active(&self, window_id: WindowId) -> bool {
        (self.floating_windows.contains(&window_id) && self.os_foreground == Some(window_id))
            || self
                .monitors
                .iter()
                .any(|(_, m)| m.focus_window == Some(window_id))
    }

    /// The window is the one of its column that was focused last (a tabbed column: the
    /// visible tab).
    fn rule_window_is_active_in_column(&self, window_id: WindowId) -> bool {
        let Some((output, workspace, column)) = self.locate_window(window_id) else {
            return false;
        };
        let Some(monitor) = self.monitors.get(&output) else {
            return false;
        };
        let Some(column) = monitor
            .workspaces
            .get(&workspace)
            .and_then(|w| w.columns.get(column))
        else {
            return false;
        };
        if let ColumnDisplay::Tabbed { active_tab } = column.display {
            return column
                .tiles
                .get(active_tab)
                .map_or(false, |t| t.window_id == window_id);
        }
        match monitor
            .focus_ring
            .iter_mru()
            .find(|w| column.index_of(*w).is_some())
        {
            Some(last_focused) => last_focused == window_id,
            None => column.tiles.first().map_or(false, |t| t.window_id == window_id),
        }
    }

    /// Evaluate the rules for a window that is about to open.
    pub(super) fn resolve_rules_for_open(
        &self,
        window: &WindowInfo,
        process_name: Option<&str>,
        aumid: Option<&str>,
        at_startup: bool,
    ) -> ResolvedWindowRules {
        let window_id = WindowId::new(window.hwnd);
        let foreground = unsafe {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            let fg = GetForegroundWindow();
            !fg.0.is_null() && fg.0 as isize == window.hwnd
        };
        let ctx = MatcherContext {
            class_name: &window.class_name,
            title: &window.title,
            // Win32 has no "instance" in the X11 / Wayland sense; the process name is the
            // closest thing and is matched as `app-id`.
            instance: None,
            process_name,
            is_active: foreground,
            is_floating: self.floating_windows.contains(&window_id),
            is_urgent: self.urgent_windows.contains(&window_id)
                || crate::window::is_window_flashing(windows::Win32::Foundation::HWND(
                    window.hwnd as *mut std::ffi::c_void,
                )),
            at_startup,
            aumid,
            is_focused: foreground,
            is_active_in_column: false,
        };
        crate::window::resolve_window_rules(&self.window_rules, &ctx)
    }

    // ---- where a window opens -------------------------------------------------------------

    /// The height of the view of `output`: what a proportion of a window height is of.
    fn view_height_for(&self, output: OutputId) -> i32 {
        let Some(monitor) = self.monitors.get(&output) else {
            return 0;
        };
        let cfg = self.effective_config(output);
        monitor.work_area.size.h as i32
            - cfg.outer_gaps.0
            - cfg.outer_gaps.2
            - cfg.reserve_top as i32
            - cfg.reserve_bottom as i32
    }

    /// The monitor a new window opens on, and the workspace there when the rules name one:
    /// `open-on-output` (a name no monitor has is ignored, as niri does), else the monitor
    /// the window is on; `open-on-workspace` (a name no workspace has is ignored) or
    /// `open-in-workspace` (the position from the top, the last workspace for one past it).
    pub(super) fn rule_placement(
        &self,
        window: &WindowInfo,
        rules: &ResolvedWindowRules,
    ) -> (OutputId, Option<i32>) {
        let named = rules.open_on_output.as_deref().and_then(|name| {
            let found = self.find_output_by_name(name);
            if found.is_none() {
                debug!("window rule: no monitor is named {:?}; the rule is ignored", name);
            }
            found
        });
        let output = named.unwrap_or_else(|| self.get_target_output(window));
        let workspace = if let Some(name) = rules.open_on_workspace.as_deref() {
            let found = self.workspace_id_by_name(name);
            if found.is_none() {
                debug!("window rule: no workspace is named {:?}; the rule is ignored", name);
            }
            found
        } else if let Some(number) = rules.open_in_workspace {
            self.monitors
                .get(&output)
                .map(|m| m.workspace_id_for_number(number))
        } else {
            None
        };
        (output, workspace)
    }

    /// The width in pixels of the column of a new window, when the rules ask for one.
    pub(super) fn open_column_width(
        &self,
        output: OutputId,
        window: &WindowInfo,
        rules: &ResolvedWindowRules,
    ) -> Option<u32> {
        let monitor = self.monitors.get(&output)?;
        let cfg = self.effective_config(output);
        let view = Self::view_width_for_monitor(monitor, cfg);
        let border = 2 * cfg.border_inset();
        let px = match rules.default_column_width {
            Some(Some(spec)) => spec_px(spec, view, cfg.column_gap, border),
            // The empty block: the window picks its own width, that is the one it opened with.
            Some(None) => (window.bounds.size.w as i32 + border).min(view),
            None => return rules.default_width_px,
        };
        Some(px.clamp(Self::MIN_COLUMN_PX, Self::MAX_COLUMN_PX) as u32)
    }

    /// The height in pixels of the tile of a new window, when the rules ask for one.  It
    /// matters in a column of several windows; a column of one always fills the view.
    fn open_tile_height(
        &self,
        output: OutputId,
        window: &WindowInfo,
        rules: &ResolvedWindowRules,
    ) -> Option<u32> {
        let cfg = self.effective_config(output);
        let border = 2 * cfg.border_inset();
        let px = match rules.default_window_height {
            Some(Some(spec)) => spec_px(spec, self.view_height_for(output), cfg.window_gap, border),
            Some(None) => window.bounds.size.h as i32 + border,
            None => return rules.default_height_px,
        };
        Some(px.max(LEAST_AUTO_TILE_PX) as u32)
    }

    /// Remember what the rules ask of a window that is about to open: its size limits and
    /// the height it asks for (the tiles are given them by `sync_min_widths`).
    pub(super) fn remember_open_rules(
        &mut self,
        window_id: WindowId,
        output: OutputId,
        window: &WindowInfo,
        rules: &ResolvedWindowRules,
    ) {
        let limits = RuleLimits::of(rules);
        if limits.is_none() {
            self.rule_limits.remove(&window_id);
        } else {
            self.rule_limits.insert(window_id, limits);
        }
        match self.open_tile_height(output, window, rules) {
            Some(height) => {
                self.rule_heights.insert(window_id, height);
            }
            None => {
                self.rule_heights.remove(&window_id);
            }
        }
    }

    /// Put a window into a column of its own at the end of `workspace` of `output` (the
    /// active one when `None`).  A window that opens on another workspace than the one the
    /// user is on is not looked at: the focus of the monitor stays where it is and the window
    /// goes to the back of the focus history.  Returns false when there is no such monitor.
    pub(super) fn place_new_tile(
        &mut self,
        window_id: WindowId,
        output: OutputId,
        workspace: Option<i32>,
        width_hint: Option<u32>,
    ) -> bool {
        let Some(monitor) = self.monitors.get_mut(&output) else {
            return false;
        };
        let target = workspace.unwrap_or(monitor.active_workspace);
        if target == monitor.active_workspace {
            monitor.add_window_with_width(window_id, width_hint);
        } else {
            monitor
                .workspaces
                .entry(target)
                .or_insert_with(crate::layout::Workspace::new)
                .add_window_to_new_column_with_width(window_id, width_hint);
            monitor.focus_ring.push_back(window_id);
        }
        true
    }

    /// The focus of `output` as it is now.
    pub(super) fn focus_before(&self, output: OutputId) -> FocusBefore {
        FocusBefore {
            window: self.monitors.get(&output).and_then(|m| m.focus_window),
        }
    }

    /// The size and place of a window that opens floating: its own size (or the size the
    /// rules ask for), held to the size limits, centred on the work area of its monitor.  A
    /// window that was there when wiri started stays where it is.
    pub(super) fn open_float_rect(
        &self,
        window_id: WindowId,
        output: OutputId,
        window: &WindowInfo,
        rules: &ResolvedWindowRules,
        at_startup: bool,
    ) -> Rect {
        let Some(work) = self.monitors.get(&output).map(|m| m.work_area) else {
            return window.bounds;
        };
        let mut rect = if at_startup {
            Self::fit_rect_into(window.bounds, work)
        } else {
            self.initial_float_rect(window_id, output)
        };
        let mut width = rect.size.w;
        let mut height = rect.size.h;
        match rules.default_column_width {
            Some(Some(spec)) => width = spec_px(spec, work.size.w as i32, 0, 0).max(1) as u32,
            _ => {
                if let Some(px) = rules.default_width_px {
                    width = px.max(1);
                }
            }
        }
        match rules.default_window_height {
            Some(Some(spec)) => height = spec_px(spec, work.size.h as i32, 0, 0).max(1) as u32,
            _ => {
                if let Some(px) = rules.default_height_px {
                    height = px.max(1);
                }
            }
        }
        let (width, height) = RuleLimits::of(rules).clamp(width, height);
        if width != rect.size.w || height != rect.size.h {
            // Keep the centre of the window where it was.
            let cx = rect.loc.x + rect.size.w as i32 / 2;
            let cy = rect.loc.y + rect.size.h as i32 / 2;
            rect = Rect::new(cx - width as i32 / 2, cy - height as i32 / 2, width, height);
        }
        Self::fit_rect_into(rect, work)
    }

    /// What the rules ask for once a window has been put in its place: the column as wide as
    /// the view, no focus for it, fullscreen.
    pub(super) fn act_on_open_rules(
        &mut self,
        window_id: WindowId,
        rules: &ResolvedWindowRules,
        before: FocusBefore,
        at_startup: bool,
        backend: &BackendHandle,
    ) {
        let floating = self.floating_windows.contains(&window_id);
        if floating {
            // Floats are not laid out: their alpha is set here.
            self.sync_window_opacity(window_id);
        }
        if rules.open_maximized == Some(true) && !floating {
            self.open_maximized_column(window_id);
        }
        if rules.open_maximized_to_edges == Some(true) && !floating {
            self.set_maximized_to_edges(window_id, true, backend);
        }
        if rules.open_focused == Some(false) && !at_startup {
            self.keep_the_focus(window_id, before);
        }
        if rules.open_fullscreen == Some(true) && !self.fullscreen_windows.contains(&window_id) {
            self.toggle_fullscreen_for(window_id, backend);
        }
    }

    /// `open-maximized`: the column of the window opens as wide as the view.
    fn open_maximized_column(&mut self, window_id: WindowId) {
        let Some((output, workspace, column)) = self.locate_window(window_id) else {
            return;
        };
        let Some(view) = self.monitors.get(&output).map(|m| {
            Self::view_width_for_monitor(m, self.effective_config(output)).max(50) as u32
        }) else {
            return;
        };
        if let Some(column) = self
            .monitors
            .get_mut(&output)
            .and_then(|m| m.workspaces.get_mut(&workspace))
            .and_then(|w| w.columns.get_mut(column))
        {
            if !column.maximized {
                column.toggle_maximized(view);
            }
        }
    }

    /// `open-focused false`: the window that had the focus keeps it.  The engine's focus goes
    /// back to it at once, the new window goes to the back of the focus history, and when
    /// Windows gives the new window the focus on its own (the application asked for it) the
    /// first focus report for it hands the focus back (see `sync_os_foreground`).
    fn keep_the_focus(&mut self, window_id: WindowId, before: FocusBefore) {
        let previous = before
            .window
            .or(self.os_foreground)
            .filter(|w| *w != window_id && self.tiled_windows.contains_key(w));
        let Some(previous) = previous else {
            return; // nothing else to have the focus
        };
        if let Some((output, workspace, _)) = self.locate_window(window_id) {
            if let Some(monitor) = self.monitors.get_mut(&output) {
                if monitor.active_workspace == workspace && monitor.focus_window == Some(window_id) {
                    let column = monitor
                        .workspace()
                        .and_then(|w| w.find_window_column(previous));
                    if let Some(column) = column {
                        monitor.focus_window = Some(previous);
                        monitor.focus_column = Some(column);
                    }
                }
                monitor.focus_ring.remove(window_id);
                monitor.focus_ring.push_back(window_id);
            }
        }
        let now = Instant::now();
        self.unfocused_opens.retain(|_, open| open.until > now);
        self.unfocused_opens.insert(
            window_id,
            UnfocusedOpen {
                previous,
                until: now + UNFOCUSED_OPEN_GRACE,
            },
        );
    }

    /// A focus report for a window that opened with `open-focused false`.  Within the grace
    /// period it is Windows giving the focus to the new window; the window that had it gets
    /// it back.  Returns true when the report was taken that way.
    pub(super) fn refuse_unwanted_focus(
        &mut self,
        window_id: WindowId,
        backend: &BackendHandle,
    ) -> bool {
        let Some(open) = self.unfocused_opens.remove(&window_id) else {
            return false;
        };
        if Instant::now() >= open.until
            || !self.tiled_windows.contains_key(&open.previous)
            || self.minimized.contains_key(&open.previous)
        {
            return false;
        }
        debug!(
            "window {} opened without the focus (open-focused false): giving it back to {}",
            window_id.as_isize(),
            open.previous.as_isize()
        );
        self.os_foreground = Some(open.previous);
        self.activate_now(open.previous, backend);
        true
    }

    // ---- what stays true of a window while it lives ------------------------------------------

    /// Forget everything the rules decided for a window that is gone.
    pub(super) fn forget_rule_state(&mut self, window_id: WindowId) {
        self.rule_state.remove(&window_id);
        self.rule_limits.remove(&window_id);
        self.rule_heights.remove(&window_id);
        self.unfocused_opens.remove(&window_id);
    }

    /// The opacity of the rule that applies to a window now, if any.
    pub fn rule_opacity(&self, window_id: WindowId) -> Option<f32> {
        self.rule_state
            .get(&window_id)
            .and_then(|s| s.resolved.opacity)
    }

    /// Look at the rules again for one window.  The `open-*` and `default-*` properties are
    /// not applied again; opacity and the size limits are, when they changed.  Returns true
    /// when something changed.
    pub fn reevaluate_window_rules(&mut self, window_id: WindowId, backend: &BackendHandle) -> bool {
        let resolved = {
            let (Some(info), Some(state)) = (
                self.tiled_windows.get(&window_id),
                self.rule_state.get(&window_id),
            ) else {
                return false;
            };
            let ctx = MatcherContext {
                class_name: &info.class_name,
                title: &info.title,
                instance: None,
                process_name: state.process_name.as_deref(),
                is_active: self.rule_window_is_active(window_id),
                is_floating: self.floating_windows.contains(&window_id),
                is_urgent: self.urgent_windows.contains(&window_id)
                    || crate::window::is_window_flashing(windows::Win32::Foundation::HWND(
                        window_id.as_isize() as *mut std::ffi::c_void,
                    )),
                at_startup: false,
                aumid: state.aumid.as_deref(),
                is_focused: self.rule_window_is_focused(window_id),
                is_active_in_column: self.rule_window_is_active_in_column(window_id),
            };
            crate::window::resolve_window_rules(&self.window_rules, &ctx)
        };
        let Some(state) = self.rule_state.get_mut(&window_id) else {
            return false;
        };
        let changed = !state.resolved.same_dynamic_part(&resolved);
        state.resolved = resolved;
        if changed {
            self.apply_dynamic_rules(window_id, backend);
        }
        changed
    }

    /// Look at the rules again for every window (the rules were replaced).
    pub fn reevaluate_all_window_rules(&mut self, backend: &BackendHandle) {
        let windows: Vec<WindowId> = self.rule_state.keys().copied().collect();
        for window_id in windows {
            self.reevaluate_window_rules(window_id, backend);
        }
    }

    /// The opacity and the size limits of a window changed: apply them.
    fn apply_dynamic_rules(&mut self, window_id: WindowId, backend: &BackendHandle) {
        let Some(resolved) = self.rule_state.get(&window_id).map(|s| s.resolved.clone()) else {
            return;
        };
        let limits = RuleLimits::of(&resolved);
        let known = self.rule_limits.get(&window_id).copied().unwrap_or_default();
        if limits != known {
            if limits.is_none() {
                self.rule_limits.remove(&window_id);
            } else {
                self.rule_limits.insert(window_id, limits);
            }
            self.sync_min_widths();
            if self.floating_windows.contains(&window_id) {
                if let Some(rect) = self.float_rect_of(window_id) {
                    // `place_float` holds the rectangle to the new limits.
                    self.place_float(window_id, rect, backend);
                }
            } else if let Some((output, _, _)) = self.locate_window(window_id) {
                self.reveal_focused_column(output);
                self.apply_layout_for_monitor(output, backend);
            }
        }
        self.sync_window_opacity(window_id);
    }

    /// Give a window the alpha its rule (and the dimming of unfocused windows) ask for.  A
    /// tile also gets it in every layout pass; this is for a change that is not followed by
    /// one, and for floating windows, which are not laid out.
    pub(super) fn sync_window_opacity(&mut self, window_id: WindowId) {
        let Some(rule) = self
            .rule_state
            .get(&window_id)
            .map(|s| s.resolved.opacity)
        else {
            return;
        };
        let output = self
            .locate_window(window_id)
            .map(|(output, _, _)| output)
            .or_else(|| self.float_location(window_id).map(|(output, _)| output));
        let dim = output.map_or(1.0, |o| self.effective_config(o).dim_unfocused);
        let focused = output.map_or(false, |o| {
            self.monitors.focused_id() == Some(o)
                && self.monitors.get(&o).and_then(|m| m.focus_window) == Some(window_id)
        });
        let applied = self
            .applied_state
            .get(&window_id)
            .map_or(NO_ALPHA, |s| s.opacity);
        let wanted = wanted_alpha(dim, focused, rule, applied);
        if wanted != NO_ALPHA && wanted != applied {
            self.apply_window_opacity(window_id.as_isize(), wanted as f32 / 255.0);
            self.applied_state
                .entry(window_id)
                .or_insert_with(AppliedState::unset)
                .opacity = wanted;
        }
    }

    // ---- the events that make the rules look again ---------------------------------------------

    /// The title of a managed window changed.  The new title is kept, and the rules that look
    /// at titles are evaluated again for the window.
    pub fn window_title_changed(&mut self, window_id: WindowId, title: &str, backend: &BackendHandle) {
        let Some(info) = self.tiled_windows.get_mut(&window_id) else {
            return;
        };
        if info.title == title {
            return;
        }
        info.title = title.to_string();
        if self.rule_flags.title {
            self.reevaluate_window_rules(window_id, backend);
        }
    }

    /// The window started or stopped asking for attention (it flashes in the taskbar).
    pub fn window_urgency_changed(&mut self, window_id: WindowId, backend: &BackendHandle) {
        if self.rule_flags.urgency {
            self.reevaluate_window_rules(window_id, backend);
        }
    }

    /// The foreground window changed from `previous` to `window_id`: the rules that depend
    /// on focus are evaluated again for the two windows and for the windows of the column of
    /// the new one (`is-active-in-column`).
    pub(super) fn rules_after_focus_change(
        &mut self,
        previous: Option<WindowId>,
        window_id: WindowId,
        backend: &BackendHandle,
    ) {
        if !self.rule_flags.focus || previous == Some(window_id) {
            return;
        }
        let mut concerned: Vec<WindowId> = Vec::new();
        concerned.extend(previous);
        concerned.push(window_id);
        if let Some((output, workspace, column)) = self.locate_window(window_id) {
            if let Some(column) = self
                .monitors
                .get(&output)
                .and_then(|m| m.workspaces.get(&workspace))
                .and_then(|w| w.columns.get(column))
            {
                concerned.extend(column.tiles.iter().map(|t| t.window_id));
            }
        }
        concerned.dedup();
        for wid in concerned {
            self.reevaluate_window_rules(wid, backend);
        }
    }

    /// A window floated or tiled again: the rules that depend on `is-floating` look again.
    pub(super) fn rules_after_float_change(&mut self, window_id: WindowId, backend: &BackendHandle) {
        if self.rule_flags.floating {
            self.reevaluate_window_rules(window_id, backend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{MatchGroup, Matcher, RegexPat};

    fn spec(fixed: Option<i32>, weight: f32, min: i32, max: i32) -> HeightSpec {
        HeightSpec {
            fixed,
            weight,
            min,
            max,
        }
    }

    fn auto() -> HeightSpec {
        spec(None, 1.0, 0, 0)
    }

    // ---- distribute_heights ---------------------------------------------------------------

    /// What the layout did before heights could be asked for: shares by weight, the last
    /// tile takes the rest.
    fn old_heights(available: i32, weights: &[f32]) -> Vec<i32> {
        let total: f32 = weights.iter().sum::<f32>().max(0.1);
        let mut heights: Vec<i32> = weights
            .iter()
            .map(|w| ((available as f32) * (w / total)).floor() as i32)
            .collect();
        let leading: i32 = if heights.len() > 1 {
            heights[..heights.len() - 1].iter().sum()
        } else {
            0
        };
        if let Some(last) = heights.last_mut() {
            *last = (available - leading).max(0);
        }
        heights
    }

    #[test]
    fn without_asks_the_heights_are_what_they_always_were() {
        for available in [0, 1, 99, 100, 777, 1000, 1039] {
            for weights in [
                vec![1.0, 1.0],
                vec![1.0, 1.0, 1.0],
                vec![1.0, 2.0],
                vec![2.5, 1.0, 0.1],
                vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            ] {
                let tiles: Vec<HeightSpec> = weights.iter().map(|w| spec(None, *w, 0, 0)).collect();
                assert_eq!(
                    distribute_heights(available, &tiles),
                    old_heights(available, &weights),
                    "{} px, weights {:?}",
                    available,
                    weights
                );
            }
        }
    }

    #[test]
    fn three_equal_tiles_share_the_column_and_the_last_takes_the_rounding() {
        assert_eq!(distribute_heights(1000, &[auto(), auto(), auto()]), vec![333, 333, 334]);
    }

    #[test]
    fn a_column_of_one_fills_the_view_within_its_limits() {
        assert_eq!(distribute_heights(900, &[auto()]), vec![900]);
        assert_eq!(distribute_heights(900, &[spec(Some(300), 1.0, 0, 0)]), vec![900]);
        assert_eq!(distribute_heights(900, &[spec(None, 1.0, 0, 500)]), vec![500]);
        assert_eq!(distribute_heights(900, &[spec(None, 1.0, 1200, 0)]), vec![1200]);
        assert!(distribute_heights(900, &[]).is_empty());
    }

    #[test]
    fn a_tile_with_a_height_of_its_own_keeps_it_and_the_others_share_the_rest() {
        assert_eq!(
            distribute_heights(1000, &[spec(Some(300), 1.0, 0, 0), auto()]),
            vec![300, 700]
        );
        assert_eq!(
            distribute_heights(1000, &[auto(), spec(Some(300), 1.0, 0, 0), auto()]),
            vec![350, 300, 350]
        );
        // Weights still divide what is left.
        assert_eq!(
            distribute_heights(1000, &[spec(Some(400), 1.0, 0, 0), spec(None, 1.0, 0, 0), spec(None, 2.0, 0, 0)]),
            vec![400, 200, 400]
        );
    }

    #[test]
    fn when_every_tile_asks_for_a_height_the_last_one_takes_what_is_left() {
        assert_eq!(
            distribute_heights(1000, &[spec(Some(300), 1.0, 0, 0), spec(Some(300), 1.0, 0, 0)]),
            vec![300, 700]
        );
    }

    #[test]
    fn asks_that_leave_nothing_for_the_others_are_cut_back() {
        // 2000 px asked of 1000: the other tile still gets its 50.
        assert_eq!(
            distribute_heights(1000, &[spec(Some(2000), 1.0, 0, 0), auto()]),
            vec![950, 50]
        );
    }

    #[test]
    fn a_maximum_holds_a_tile_and_the_others_take_the_room() {
        assert_eq!(
            distribute_heights(1000, &[spec(None, 1.0, 0, 200), auto()]),
            vec![200, 800]
        );
        assert_eq!(
            distribute_heights(1000, &[auto(), spec(None, 1.0, 0, 200), auto()]),
            vec![400, 200, 400]
        );
    }

    #[test]
    fn a_minimum_holds_a_tile_and_the_others_give_way() {
        assert_eq!(
            distribute_heights(1000, &[spec(None, 1.0, 700, 0), auto()]),
            vec![700, 300]
        );
    }

    #[test]
    fn limits_can_leave_the_column_short_or_ask_for_more_than_is_there() {
        // Both tiles stop at their maximum: the column is not filled.
        assert_eq!(
            distribute_heights(1000, &[spec(None, 1.0, 0, 300), spec(None, 1.0, 0, 300)]),
            vec![300, 300]
        );
        // Both need more than half: the minimums win and the column overflows.
        assert_eq!(
            distribute_heights(1000, &[spec(None, 1.0, 700, 0), spec(None, 1.0, 700, 0)]),
            vec![700, 700]
        );
        // A minimum above the maximum: the minimum wins.
        assert_eq!(distribute_heights(1000, &[spec(None, 1.0, 400, 300), auto()]), vec![400, 600]);
    }

    // ---- sizes ------------------------------------------------------------------------------

    #[test]
    fn sizes_from_rules_are_tiles_with_their_border() {
        // niri counts a proportion of the view with the gap between neighbours: two halves
        // and the gap between them fill the view exactly.
        assert_eq!(spec_px(SizeSpec::Proportion(0.5), 1000, 20, 8), 490);
        assert_eq!(2 * 490 + 20, 1000);
        assert_eq!(spec_px(SizeSpec::Proportion(1.0), 1000, 20, 8), 1000);
        // A fixed size is the window's: the border comes on top.
        assert_eq!(spec_px(SizeSpec::Fixed(800), 1000, 20, 8), 808);
    }

    #[test]
    fn limits_of_a_window_become_limits_of_its_tile() {
        assert_eq!(RuleLimits::for_tile(0, 8), 0, "no limit stays none");
        assert_eq!(RuleLimits::for_tile(300, 8), 308);
        let limits = RuleLimits {
            min_width: 200,
            max_width: 600,
            min_height: 0,
            max_height: 400,
        };
        assert_eq!(limits.clamp(100, 1000), (200, 400));
        assert_eq!(limits.clamp(900, 100), (600, 100));
        assert_eq!(limits.clamp(300, 300), (300, 300));
        assert!(RuleLimits::default().is_none());
        assert!(!limits.is_none());
    }

    // ---- opacity ---------------------------------------------------------------------------

    #[test]
    fn the_alpha_is_the_rule_times_the_dimming() {
        // Nothing asks: the window is left alone.
        assert_eq!(wanted_alpha(1.0, true, None, NO_ALPHA), NO_ALPHA);
        assert_eq!(wanted_alpha(1.0, false, None, NO_ALPHA), NO_ALPHA);
        // The rule alone.
        assert_eq!(wanted_alpha(1.0, true, Some(0.5), NO_ALPHA), 128);
        assert_eq!(wanted_alpha(1.0, false, Some(0.5), NO_ALPHA), 128);
        assert_eq!(wanted_alpha(1.0, true, Some(0.0), NO_ALPHA), 0, "fully transparent is a value");
        // Dimming alone: what the config always did.
        assert_eq!(wanted_alpha(0.8, true, None, NO_ALPHA), 255);
        assert_eq!(wanted_alpha(0.8, false, None, NO_ALPHA), 204);
        // Both: they multiply.
        assert_eq!(wanted_alpha(0.8, false, Some(0.5), NO_ALPHA), 102);
        assert_eq!(wanted_alpha(0.8, true, Some(0.5), NO_ALPHA), 128);
    }

    #[test]
    fn a_rule_that_stops_asking_gives_the_alpha_back_once() {
        assert_eq!(wanted_alpha(1.0, true, None, 128), 255, "it was made transparent: opaque again");
        assert_eq!(wanted_alpha(1.0, true, None, 255), 255, "and then nothing more to do");
    }

    // ---- the engine ---------------------------------------------------------------------------

    fn engine() -> TilingEngine {
        let mut engine = TilingEngine::new(LayoutConfig {
            outer_gaps: (0, 0, 0, 0),
            ..LayoutConfig::default()
        });
        engine.register_monitor(
            OutputId::from_name("M"),
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
        );
        engine.set_output_name(OutputId::from_name("M"), "\\\\.\\DISPLAY1");
        engine
    }

    fn window(hwnd: isize, title: &str) -> WindowInfo {
        WindowInfo {
            hwnd,
            title: title.to_string(),
            class_name: "TestClass".to_string(),
            process_id: 1000,
            bounds: Rect::new(100, 100, 700, 500),
            state: crate::backend::WindowState::Normal,
            is_visible: true,
        }
    }

    fn be() -> BackendHandle {
        BackendHandle::default_for_test()
    }

    fn title_rule(pattern: &str, rule: WindowRule) -> WindowRule {
        WindowRule {
            match_groups: vec![MatchGroup {
                conds: vec![Matcher::TitleSearch(RegexPat::new(pattern).unwrap())],
            }],
            ..rule
        }
    }

    /// Where `wid` is: (workspace id, column index), on the one monitor.
    fn place_of(engine: &TilingEngine, wid: isize) -> Option<(i32, usize)> {
        engine
            .locate_window(WindowId::new(wid))
            .map(|(_, workspace, column)| (workspace, column))
    }

    fn tile_of(engine: &TilingEngine, wid: isize) -> crate::layout::Tile {
        let oid = OutputId::from_name("M");
        engine.monitors.get(&oid).unwrap().workspaces.values().flat_map(|w| w.columns.iter()).flat_map(|c| c.tiles.iter()).find(|t| t.window_id == WindowId::new(wid)).cloned().expect("a tile")
    }

    #[test]
    fn a_monitor_is_found_by_its_device_name_with_or_without_the_prefix() {
        let engine = engine();
        let oid = OutputId::from_name("M");
        assert_eq!(engine.find_output_by_name("\\\\.\\DISPLAY1"), Some(oid));
        assert_eq!(engine.find_output_by_name("DISPLAY1"), Some(oid));
        assert_eq!(engine.find_output_by_name("display1"), Some(oid));
        assert_eq!(engine.find_output_by_name("DISPLAY2"), None);
        assert_eq!(engine.find_output_by_name(""), None);
        // A monitor the engine was never told the name of is found by the hash of it.
        assert_eq!(engine.find_output_by_name("M"), Some(oid));
        assert_eq!(engine.output_name(oid), Some("\\\\.\\DISPLAY1"));
    }

    #[test]
    fn open_on_output_picks_the_monitor_and_an_unknown_one_is_ignored() {
        let mut engine = engine();
        let second = OutputId::from_name("Second");
        engine.register_monitor(
            second,
            Rect::new(1920, 0, 1920, 1080),
            Rect::new(1920, 0, 1920, 1040),
        );
        engine.set_output_name(second, "\\\\.\\DISPLAY2");
        engine.set_window_rules(vec![
            title_rule(
                "^second",
                WindowRule {
                    open_on_output: Some("DISPLAY2".to_string()),
                    ..WindowRule::default()
                },
            ),
            title_rule(
                "^nowhere",
                WindowRule {
                    open_on_output: Some("DISPLAY9".to_string()),
                    ..WindowRule::default()
                },
            ),
        ]);
        // Both windows are on the first monitor (their centre is there).
        engine.add_window(window(10, "second one"), &be());
        engine.add_window(window(11, "nowhere"), &be());
        engine.add_window(window(12, "plain"), &be());
        let on = |wid: isize| engine.locate_window(WindowId::new(wid)).map(|(o, _, _)| o);
        assert_eq!(on(10), Some(second), "the rule names the second monitor");
        assert_eq!(on(11), Some(OutputId::from_name("M")), "a monitor that is not there: ignored");
        assert_eq!(on(12), Some(OutputId::from_name("M")));
    }

    #[test]
    fn open_on_workspace_by_name_opens_there_and_leaves_the_focus_alone() {
        let mut engine = engine();
        let oid = OutputId::from_name("M");
        engine.add_window(window(1, "first"), &be());
        engine.monitors.get_mut(&oid).unwrap().workspaces.insert(1, crate::layout::Workspace::new());
        engine.rename_workspace(1, "chat").unwrap();
        engine.set_window_rules(vec![
            title_rule(
                "^slack",
                WindowRule {
                    open_on_workspace: Some("chat".to_string()),
                    ..WindowRule::default()
                },
            ),
            title_rule(
                "^lost",
                WindowRule {
                    open_on_workspace: Some("no such workspace".to_string()),
                    ..WindowRule::default()
                },
            ),
        ]);
        engine.add_window(window(2, "slack"), &be());
        assert_eq!(place_of(&engine, 2), Some((1, 0)), "on the workspace named chat");
        let monitor = engine.monitors.get(&oid).unwrap();
        assert_eq!(monitor.active_workspace, 0, "the user stays where they are");
        assert_eq!(monitor.focus_window, Some(WindowId::new(1)), "and so does the focus");
        assert_eq!(monitor.focus_column, Some(0));
        assert_eq!(
            monitor.focus_ring.current(),
            Some(WindowId::new(1)),
            "the window nobody looked at is not the most recent"
        );
        // A name no workspace has: the rule is ignored, the window opens where the user is.
        engine.add_window(window(3, "lost"), &be());
        assert_eq!(place_of(&engine, 3).map(|(ws, _)| ws), Some(0));
    }

    #[test]
    fn open_in_workspace_is_a_position_from_the_top() {
        let mut engine = engine();
        let oid = OutputId::from_name("M");
        engine.monitors.get_mut(&oid).unwrap().workspaces.insert(1, crate::layout::Workspace::new());
        engine.monitors.get_mut(&oid).unwrap().workspaces.insert(2, crate::layout::Workspace::new());
        engine.set_window_rules(vec![
            title_rule(
                "^two",
                WindowRule {
                    open_in_workspace: Some(2),
                    ..WindowRule::default()
                },
            ),
            title_rule(
                "^older",
                WindowRule {
                    workspace: Some("3".to_string()),
                    ..WindowRule::default()
                },
            ),
        ]);
        engine.add_window(window(1, "two"), &be());
        assert_eq!(place_of(&engine, 1).map(|(ws, _)| ws), Some(1), "the second workspace has id 1");
        engine.add_window(window(2, "older style"), &be());
        assert_eq!(place_of(&engine, 2).map(|(ws, _)| ws), Some(2), "workspace \"3\" is the third");
    }

    #[test]
    fn default_column_width_gives_the_column_its_width() {
        let mut engine = engine();
        engine.set_window_rules(vec![
            title_rule(
                "^half",
                WindowRule {
                    default_column_width: Some(Some(SizeSpec::Proportion(0.5))),
                    ..WindowRule::default()
                },
            ),
            title_rule(
                "^fixed",
                WindowRule {
                    default_column_width: Some(Some(SizeSpec::Fixed(800))),
                    ..WindowRule::default()
                },
            ),
            title_rule(
                "^own",
                WindowRule {
                    default_column_width: Some(None),
                    ..WindowRule::default()
                },
            ),
            title_rule(
                "^older",
                WindowRule {
                    default_width: Some(650),
                    ..WindowRule::default()
                },
            ),
        ]);
        let width_of = |engine: &TilingEngine, wid: isize| {
            let (ws, col) = place_of(engine, wid).unwrap();
            let oid = OutputId::from_name("M");
            engine.monitors.get(&oid).unwrap().workspaces[&ws].columns[col].width
        };
        engine.add_window(window(1, "half"), &be());
        // The view is 1920 wide, the gap 16: (1920 + 16) / 2 - 16.
        assert_eq!(width_of(&engine, 1), Some(952));
        engine.add_window(window(2, "fixed"), &be());
        assert_eq!(width_of(&engine, 2), Some(808), "800 for the window and 4 px of border on each side");
        engine.add_window(window(3, "own"), &be());
        assert_eq!(width_of(&engine, 3), Some(708), "the 700 px it opened with, and its border");
        engine.add_window(window(4, "older style"), &be());
        assert_eq!(width_of(&engine, 4), Some(650), "the pixels of the column, as before");
        engine.add_window(window(5, "plain"), &be());
        assert_eq!(width_of(&engine, 5), Some(500), "the configured width");
    }

    #[test]
    fn default_window_height_is_the_height_of_the_tile() {
        let mut engine = engine();
        engine.set_window_rules(vec![
            title_rule(
                "^fixed",
                WindowRule {
                    default_window_height: Some(Some(SizeSpec::Fixed(300))),
                    ..WindowRule::default()
                },
            ),
            title_rule(
                "^third",
                WindowRule {
                    default_window_height: Some(Some(SizeSpec::Proportion(0.25))),
                    ..WindowRule::default()
                },
            ),
        ]);
        engine.add_window(window(1, "fixed"), &be());
        assert_eq!(tile_of(&engine, 1).preferred_height, Some(308));
        engine.add_window(window(2, "third"), &be());
        // The view is 1040 high, the window gap 12: 1052 / 4 - 12.
        assert_eq!(tile_of(&engine, 2).preferred_height, Some(251));
        engine.add_window(window(3, "plain"), &be());
        assert_eq!(tile_of(&engine, 3).preferred_height, None);
    }

    #[test]
    fn the_layout_gives_a_tile_the_height_it_asks_for_in_a_column_of_two() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^short",
            WindowRule {
                default_window_height: Some(Some(SizeSpec::Fixed(292))),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "tall"), &be());
        engine.add_window(window(2, "short"), &be());
        let oid = OutputId::from_name("M");
        // Put the second window under the first: they share a column now.
        {
            let monitor = engine.monitors.get_mut(&oid).unwrap();
            let ws = monitor.workspace_mut().unwrap();
            let tile = ws.columns.remove(1).tiles.remove(0);
            ws.columns[0].add_tile(tile);
        }
        let cfg = engine.config.clone();
        let monitor = engine.monitors.get(&oid).unwrap();
        let positions = engine.calculate_positions(
            monitor.workspace().unwrap(),
            Rect::new(0, 0, 1920, 1040),
            &cfg,
        );
        let height_of = |wid: isize| {
            positions
                .iter()
                .find(|(w, _)| *w == WindowId::new(wid))
                .map(|(_, r)| r.size.h)
        };
        // 292 px of window and 4 px of border on each side; the other tile has the rest.
        assert_eq!(height_of(2), Some(300));
        assert_eq!(height_of(1), Some(1040 - 12 - 300));
    }

    #[test]
    fn size_limits_hold_the_column_and_the_tile() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^limited",
            WindowRule {
                min_width: Some(300),
                max_width: Some(600),
                min_height: Some(200),
                max_height: Some(700),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "limited"), &be());
        let tile = tile_of(&engine, 1);
        // Tile limits are the window's plus 4 px of border on each side.
        assert_eq!((tile.min_width, tile.max_width), (308, 608));
        assert_eq!((tile.min_height, tile.max_height), (208, 708));
        // The column is 500 wide by default; it is held to the maximum when it is asked for more.
        let oid = OutputId::from_name("M");
        engine.monitors.get_mut(&oid).unwrap().workspace_mut().unwrap().columns[0].set_width(Some(900));
        let cfg = engine.config.clone();
        let positions = engine.calculate_positions(
            engine.monitors.get(&oid).unwrap().workspace().unwrap(),
            Rect::new(0, 0, 1920, 1040),
            &cfg,
        );
        assert_eq!(positions[0].1.size.w, 608, "held to the maximum width");
        assert_eq!(positions[0].1.size.h, 708, "a lone tile fills the view but the maximum height holds it");
        engine.monitors.get_mut(&oid).unwrap().workspace_mut().unwrap().columns[0].set_width(Some(100));
        let positions = engine.calculate_positions(
            engine.monitors.get(&oid).unwrap().workspace().unwrap(),
            Rect::new(0, 0, 1920, 1040),
            &cfg,
        );
        assert_eq!(positions[0].1.size.w, 308, "raised to the minimum width");
    }

    #[test]
    fn a_window_that_moves_to_another_workspace_keeps_its_limits_and_height() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^keeper",
            WindowRule {
                max_width: Some(600),
                default_window_height: Some(Some(SizeSpec::Fixed(292))),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "keeper"), &be());
        engine.move_window_to_workspace(1, &be());
        let tile = tile_of(&engine, 1);
        assert_eq!(tile.max_width, 608);
        assert_eq!(tile.preferred_height, Some(300));
    }

    #[test]
    fn open_maximized_opens_the_column_as_wide_as_the_view() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^big",
            WindowRule {
                open_maximized: Some(true),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "big"), &be());
        engine.add_window(window(2, "small"), &be());
        let oid = OutputId::from_name("M");
        let ws = engine.monitors.get(&oid).unwrap().workspace().unwrap();
        assert!(ws.columns[0].maximized);
        assert_eq!(ws.columns[0].width, Some(1920));
        assert!(!ws.columns[1].maximized);
    }

    #[test]
    fn open_fullscreen_opens_the_window_fullscreen() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^movie",
            WindowRule {
                open_fullscreen: Some(true),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "movie"), &be());
        assert!(engine.is_fullscreen(WindowId::new(1)));
        engine.add_window(window(2, "other"), &be());
        assert!(!engine.is_fullscreen(WindowId::new(2)));
    }

    #[test]
    fn open_floating_opens_a_centred_float_of_the_size_the_rules_ask_for() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^dialog",
            WindowRule {
                open_floating: Some(true),
                default_column_width: Some(Some(SizeSpec::Fixed(600))),
                default_window_height: Some(Some(SizeSpec::Fixed(400))),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "dialog"), &be());
        let wid = WindowId::new(1);
        assert!(engine.floating_windows.contains(&wid));
        assert!(place_of(&engine, 1).is_none(), "a float is in no column");
        let home = engine.float_homes.get(&wid).expect("a home");
        assert_eq!((home.rect.size.w, home.rect.size.h), (600, 400));
        // Centred on the 1920x1040 work area.
        assert_eq!((home.rect.loc.x, home.rect.loc.y), (660, 320));
    }

    #[test]
    fn a_float_that_was_there_at_startup_stays_where_it_is() {
        let mut engine = engine();
        engine.set_window_rules(vec![WindowRule {
            open_floating: Some(true),
            ..WindowRule::default()
        }]);
        engine.add_window_at_startup(window(1, "old"), &be());
        let home = engine.float_homes.get(&WindowId::new(1)).expect("a home");
        assert_eq!((home.rect.loc.x, home.rect.loc.y), (100, 100));
        assert_eq!((home.rect.size.w, home.rect.size.h), (700, 500));
    }

    #[test]
    fn open_focused_false_keeps_the_focus_where_it_was() {
        let mut engine = engine();
        let oid = OutputId::from_name("M");
        engine.set_window_rules(vec![title_rule(
            "^quiet",
            WindowRule {
                open_focused: Some(false),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "work"), &be());
        engine.add_window(window(2, "quiet one"), &be());
        let monitor = engine.monitors.get(&oid).unwrap();
        assert_eq!(monitor.focus_window, Some(WindowId::new(1)));
        assert_eq!(monitor.focus_column, Some(0));
        assert_eq!(monitor.focus_ring.current(), Some(WindowId::new(1)));
        assert!(place_of(&engine, 2).is_some(), "it is in the strip");
        // A window that opens normally takes the focus.
        engine.add_window(window(3, "loud"), &be());
        assert_eq!(engine.monitors.get(&oid).unwrap().focus_window, Some(WindowId::new(3)));
    }

    #[test]
    fn the_focus_windows_hands_to_an_unfocused_window_is_given_back_once() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^quiet",
            WindowRule {
                open_focused: Some(false),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "work"), &be());
        engine.add_window(window(2, "quiet one"), &be());
        assert!(engine.unfocused_opens.contains_key(&WindowId::new(2)));
        // Windows reports the new window as the foreground one: the engine hands it back...
        assert!(engine.refuse_unwanted_focus(WindowId::new(2), &be()));
        assert_eq!(engine.os_foreground, Some(WindowId::new(1)));
        // ...once.  The next report is the user asking for the window.
        assert!(!engine.refuse_unwanted_focus(WindowId::new(2), &be()));
    }

    #[test]
    fn the_focus_of_a_window_that_opened_long_ago_is_not_refused() {
        let mut engine = engine();
        engine.add_window(window(1, "work"), &be());
        engine.add_window(window(2, "quiet one"), &be());
        engine.unfocused_opens.insert(
            WindowId::new(2),
            UnfocusedOpen {
                previous: WindowId::new(1),
                until: Instant::now() - Duration::from_millis(1),
            },
        );
        assert!(!engine.refuse_unwanted_focus(WindowId::new(2), &be()));
        assert!(engine.unfocused_opens.is_empty());
    }

    #[test]
    fn opacity_follows_the_title_of_the_window() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^video",
            WindowRule {
                opacity: Some(0.5),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "Inbox"), &be());
        let wid = WindowId::new(1);
        assert_eq!(engine.rule_opacity(wid), None);
        engine.window_title_changed(wid, "video - player", &be());
        assert_eq!(engine.rule_opacity(wid), Some(0.5));
        assert_eq!(engine.tiled_windows[&wid].title, "video - player");
        engine.window_title_changed(wid, "Inbox again", &be());
        assert_eq!(engine.rule_opacity(wid), None, "back to opaque when the title stops matching");
    }

    #[test]
    fn size_limits_follow_the_title_and_the_layout_follows_the_limits() {
        let mut engine = engine();
        engine.set_window_rules(vec![title_rule(
            "^narrow",
            WindowRule {
                max_width: Some(400),
                ..WindowRule::default()
            },
        )]);
        engine.add_window(window(1, "plain"), &be());
        assert_eq!(tile_of(&engine, 1).max_width, 0);
        engine.window_title_changed(WindowId::new(1), "narrow now", &be());
        assert_eq!(tile_of(&engine, 1).max_width, 408);
        engine.window_title_changed(WindowId::new(1), "plain again", &be());
        assert_eq!(tile_of(&engine, 1).max_width, 0);
    }

    #[test]
    fn a_title_change_costs_nothing_when_no_rule_looks_at_titles() {
        let mut engine = engine();
        engine.set_window_rules(vec![WindowRule {
            class: Some("TestClass".to_string()),
            opacity: Some(0.9),
            ..WindowRule::default()
        }]);
        engine.add_window(window(1, "a"), &be());
        assert!(!engine.rule_flags.title);
        engine.window_title_changed(WindowId::new(1), "b", &be());
        assert_eq!(engine.tiled_windows[&WindowId::new(1)].title, "b", "the title is kept anyway");
        assert_eq!(engine.rule_opacity(WindowId::new(1)), Some(0.9));
    }

    #[test]
    fn opacity_follows_the_focus() {
        // The usual rule: dim what has not the focus.
        let mut engine = engine();
        engine.set_window_rules(vec![WindowRule {
            match_groups: vec![MatchGroup {
                conds: vec![Matcher::Not(Box::new(Matcher::IsFocused))],
            }],
            opacity: Some(0.6),
            ..WindowRule::default()
        }]);
        engine.add_window(window(1, "one"), &be());
        engine.add_window(window(2, "two"), &be());
        assert!(engine.rule_flags.focus);
        let (one, two) = (WindowId::new(1), WindowId::new(2));
        engine.sync_os_foreground(one, &be());
        assert_eq!(engine.rule_opacity(one), None, "the focused window stays opaque");
        assert_eq!(engine.rule_opacity(two), Some(0.6));
        engine.sync_os_foreground(two, &be());
        assert_eq!(engine.rule_opacity(one), Some(0.6));
        assert_eq!(engine.rule_opacity(two), None);
    }

    #[test]
    fn rules_that_look_at_floating_follow_the_toggle() {
        let mut engine = engine();
        engine.set_window_rules(vec![WindowRule {
            match_groups: vec![MatchGroup {
                conds: vec![Matcher::IsFloating],
            }],
            opacity: Some(0.8),
            ..WindowRule::default()
        }]);
        engine.add_window(window(1, "w"), &be());
        let wid = WindowId::new(1);
        assert_eq!(engine.rule_opacity(wid), None);
        engine.toggle_floating_for(wid, &be());
        assert_eq!(engine.rule_opacity(wid), Some(0.8));
        engine.toggle_floating_for(wid, &be());
        assert_eq!(engine.rule_opacity(wid), None);
    }

    #[test]
    fn replacing_the_rules_looks_at_every_window_again() {
        let mut engine = engine();
        engine.add_window(window(1, "a"), &be());
        engine.add_window(window(2, "b"), &be());
        assert_eq!(engine.rule_opacity(WindowId::new(1)), None);
        engine.set_window_rules(vec![WindowRule {
            opacity: Some(0.7),
            ..WindowRule::default()
        }]);
        engine.reevaluate_all_window_rules(&be());
        assert_eq!(engine.rule_opacity(WindowId::new(1)), Some(0.7));
        assert_eq!(engine.rule_opacity(WindowId::new(2)), Some(0.7));
        engine.set_window_rules(vec![]);
        engine.reevaluate_all_window_rules(&be());
        assert_eq!(engine.rule_opacity(WindowId::new(1)), None);
    }

    #[test]
    fn a_window_that_is_gone_leaves_nothing_behind() {
        let mut engine = engine();
        engine.set_window_rules(vec![WindowRule {
            opacity: Some(0.7),
            max_width: Some(500),
            default_window_height: Some(Some(SizeSpec::Fixed(300))),
            open_focused: Some(false),
            ..WindowRule::default()
        }]);
        engine.add_window(window(1, "a"), &be());
        engine.add_window(window(2, "b"), &be());
        engine.remove_window(WindowId::new(2), &be());
        let wid = WindowId::new(2);
        assert!(!engine.rule_state.contains_key(&wid));
        assert!(!engine.rule_limits.contains_key(&wid));
        assert!(!engine.rule_heights.contains_key(&wid));
        assert!(!engine.unfocused_opens.contains_key(&wid));
    }

    #[test]
    fn the_flags_say_which_state_the_rules_depend_on() {
        let none = RuleFlags::of(&[WindowRule {
            class: Some("x".into()),
            ..WindowRule::default()
        }]);
        assert_eq!(none, RuleFlags::default());
        let title = RuleFlags::of(&[title_rule("a", WindowRule::default())]);
        assert!(title.title && !title.focus && !title.aumid);
        let all = RuleFlags::of(&[
            WindowRule {
                match_groups: vec![MatchGroup {
                    conds: vec![
                        Matcher::AppId(RegexPat::new("x").unwrap()),
                        Matcher::Not(Box::new(Matcher::IsActive)),
                        Matcher::IsFloating,
                        Matcher::IsUrgent,
                    ],
                }],
                ..WindowRule::default()
            },
        ]);
        assert!(all.aumid && all.focus && all.floating && all.urgency && !all.title);
    }
}
