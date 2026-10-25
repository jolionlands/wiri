#[derive(Debug, Clone)]
pub struct Config {
    pub input: InputConfig,
    pub output: Vec<OutputConfig>,
    pub layout: LayoutConfig,
    pub workspace: Vec<WorkspaceConfig>,
    pub window_rules: Vec<WindowRule>,
    pub binds: BindsConfig,
    pub animations: AnimationsConfig,
    /// niri screenshot-path: Pictures default, a strftime template, or null (clipboard only).
    pub screenshot_path: crate::screenshot::path::ScreenshotPath,
    /// Programs to spawn at startup. Each entry is [program, arg1, arg2, …].
    /// Populated from `spawn-at-startup "prog" "arg"` top-level KDL lines.
    pub spawn_at_startup: Vec<Vec<String>>,
    /// Shell command lines to run at startup (`spawn-sh-at-startup "cmd"`), each through
    /// `cmd.exe /d /s /c` like the `spawn-sh` bind.
    pub spawn_sh_at_startup: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            input: InputConfig::default_config(),
            output: vec![OutputConfig {
                name: String::new(),
                position: Position::default(),
                width: 0,
                height: 0,
                scale: 0.0,
                mode: String::new(),
                vrr: false,
                primary: false,
                position_set: false,
                focus_at_startup: false,
                transform: String::new(),
                enable: true,
                layout_override: None,
                mod_key: None,
            }],
            layout: LayoutConfig::default_config(),
            workspace: Vec::new(),
            window_rules: Vec::new(),
            binds: BindsConfig::default(),
            animations: AnimationsConfig::default_config(),
            screenshot_path: crate::screenshot::path::ScreenshotPath::default(),
            spawn_at_startup: Vec::new(),
            spawn_sh_at_startup: Vec::new(),
        }
    }
}

impl Config {
    pub fn default_config() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct InputConfig {
    pub keyboard_layout: String,
    pub repeat_delay: u32,
    pub repeat_rate: u32,
    pub mouse_speed: f64,
    pub mouse_acceleration: f64,
    pub natural_scroll: bool,
    pub tap_to_click: bool,
    pub mouse_sensitivity: f64,
    /// When true, keyboard focus follows the mouse pointer automatically.
    pub focus_follows_mouse: bool,
    /// Modifier prefix for default keybindings. One of: "ctrl-alt" (default),
    /// "alt", "super" (Win key), "win-alt", "ctrl-shift". Custom KDL `binds`
    /// override this entirely.
    pub mod_key: String,
    /// niri's `workspace-auto-back-and-forth`: asking for the workspace you are on
    /// (`focus-workspace 2` while on the second one) goes back to the previous
    /// workspace instead.
    pub workspace_auto_back_and_forth: bool,

    // ---- nested-block fields surfaced as flat config fields ----
    /// Delay (ms) before focus-follows-mouse kicks in after the pointer
    /// settles on a new window. 0 = instant.
    pub focus_delay_ms: u32,
    /// When focus changes programmatically, warp the mouse pointer to the
    /// centre of the newly focused window.
    pub warp_on_focus: bool,
    /// Whether to enable NumLock at startup. Mirrors `input.keyboard.numlock`.
    pub numlock: bool,

    // ---- touch fields (input.touch { ... } block) ----
    /// Enable touch input handling. Default false — wiri only reads touch when
    /// this is true.
    pub touch_enabled: bool,
    /// Convert a tap into a left-click event (mirrors trackpad behaviour).
    pub touch_tap_to_click: bool,
    /// Touch scroll speed multiplier (1.0 = native).
    pub touch_scroll_speed: f64,
}

impl InputConfig {
    pub fn default_config() -> Self {
        Self {
            keyboard_layout: "us".to_string(),
            repeat_delay: 250,
            repeat_rate: 25,
            mouse_speed: 1.0,
            mouse_acceleration: 0.0,
            natural_scroll: false,
            tap_to_click: false,
            mouse_sensitivity: 1.0,
            focus_follows_mouse: false,
            mod_key: "ctrl-alt".to_string(),
            workspace_auto_back_and_forth: false,
            focus_delay_ms: 0,
            warp_on_focus: false,
            numlock: false,
            touch_enabled: false,
            touch_tap_to_click: true,
            touch_scroll_speed: 1.0,
        }
    }

    /// Return a nested keyboard view built from the flat fields.
    pub fn keyboard(&self) -> KeyboardSubConfig {
        KeyboardSubConfig {
            layout: self.keyboard_layout.clone(),
            repeat_delay: self.repeat_delay,
            repeat_rate: self.repeat_rate,
            numlock: self.numlock,
        }
    }

    /// Return a nested mouse view built from the flat fields.
    pub fn mouse(&self) -> MouseSubConfig {
        MouseSubConfig {
            speed: self.mouse_speed,
            acceleration: self.mouse_acceleration,
            sensitivity: self.mouse_sensitivity,
            natural_scroll: self.natural_scroll,
            tap_to_click: self.tap_to_click,
        }
    }

    /// Return a nested touch view built from the flat fields.
    pub fn touch(&self) -> TouchSubConfig {
        TouchSubConfig {
            enabled: self.touch_enabled,
            // swipe defaults — wiri does not yet ship configurable swipe thresholds,
            // so we surface sensible OS-aligned values here.
            swipe_threshold_px: 16,
            swipe_timeout_ms: 250,
            tap_to_click: self.touch_tap_to_click,
            scroll_speed: self.touch_scroll_speed,
        }
    }

    /// Return a nested focus view built from the flat fields.
    pub fn focus(&self) -> FocusSubConfig {
        FocusSubConfig {
            follows_mouse: self.focus_follows_mouse,
            focus_delay_ms: self.focus_delay_ms,
            warp_on_focus: self.warp_on_focus,
        }
    }
}

// ---------------------------------------------------------------------------
// InputConfig sub-structs
// ---------------------------------------------------------------------------

/// Nested keyboard configuration view derived from `InputConfig` flat fields.
#[derive(Debug, Clone, Default)]
pub struct KeyboardSubConfig {
    pub layout: String,
    pub repeat_delay: u32,
    pub repeat_rate: u32,
    /// Numlock-on-start; no flat counterpart in InputConfig yet, defaults false.
    pub numlock: bool,
}

/// Nested mouse configuration view derived from `InputConfig` flat fields.
#[derive(Debug, Clone, Default)]
pub struct MouseSubConfig {
    pub speed: f64,
    pub acceleration: f64,
    pub sensitivity: f64,
    pub natural_scroll: bool,
    pub tap_to_click: bool,
}

/// Nested touch configuration view derived from `InputConfig` flat fields.
#[derive(Debug, Clone, Default)]
pub struct TouchSubConfig {
    pub enabled: bool,
    pub swipe_threshold_px: u32,
    pub swipe_timeout_ms: u32,
    pub tap_to_click: bool,
    pub scroll_speed: f64,
}

/// Nested focus-policy configuration view derived from `InputConfig` flat fields.
#[derive(Debug, Clone, Default)]
pub struct FocusSubConfig {
    pub follows_mouse: bool,
    /// Focus delay in ms; no flat counterpart yet, defaults 0.
    pub focus_delay_ms: u32,
    /// Warp pointer to newly-focused window; no flat counterpart yet, defaults false.
    pub warp_on_focus: bool,
}

#[derive(Debug, Clone)]
pub struct OutputConfig {
    pub name: String,
    /// Unset position leaves the physical desktop arrangement unchanged.
    pub position_set: bool,
    /// Select this output in wiri once at startup, not as the Windows primary display.
    pub focus_at_startup: bool,
    pub position: Position,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub mode: String,
    pub vrr: bool,
    pub primary: bool,
    pub transform: String,
    pub enable: bool,
    /// Per-output `layout { … }` block (niri parity).  Each field is `Option`
    /// so callers can fold only the explicitly-set fields onto the global
    /// `LayoutConfig` for the monitor backing this output.
    ///
    /// Engine wiring lives in `layout::engine` (owned by Agent E); this struct
    /// is consumed by `main.rs` when it computes the effective per-monitor
    /// layout config at engine-startup / config-reload time.
    pub layout_override: Option<LayoutConfigPartial>,
    /// Per-monitor mod-key override (niri parity).  `None` means "use the
    /// global `input.mod-key` value".  When `Some`, this string overrides the
    /// modifier prefix for monitor-scoped IPC actions such as
    /// `focus-monitor-by-prefix`.  Global hotkeys are NOT affected because
    /// Windows registers hotkeys per-thread, not per-display; see the
    /// `build_hotkey_list` comment in `backend/message_loop.rs` for details.
    pub mod_key: Option<String>,
}

/// Optional-per-field mirror of `LayoutConfig` used by per-output overrides
/// (`output "DP-1" { layout { column-width 800 } }`).
///
/// Every field corresponds to a flat field on `LayoutConfig`.  `None` means
/// "field unset on the override; fall through to the global LayoutConfig
/// value".  `Some(x)` means "use this value on this output specifically".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutConfigPartial {
    pub inner_gaps: Option<u32>,
    pub outer_gaps: Option<u32>,
    pub border_width: Option<u32>,
    pub border_color: Option<String>,
    pub border_color_focused: Option<String>,
    pub focus_ring_width: Option<u32>,
    pub dim_unfocused: Option<f32>,
    pub focus_ring_color: Option<String>,
    pub shadow_enable: Option<bool>,
    pub shadow_opacity: Option<f64>,
    pub shadow_offset_x: Option<i32>,
    pub shadow_offset_y: Option<i32>,
    pub shadow_blur: Option<u32>,
    pub shadow_color: Option<String>,
    pub split_ratio: Option<f64>,
    pub auto_balance: Option<bool>,
    pub scroll_step: Option<u32>,
    pub column_width: Option<u32>,
    pub column_width_mode: Option<String>,
    pub strip_frame: Option<bool>,
    pub border_color_urgent: Option<String>,
    pub border_radius: Option<u32>,
    pub border_padding: Option<u32>,
    pub focus_ring_gap: Option<u32>,
    pub focus_ring_inactive_color: Option<String>,
    pub shadow_spread: Option<u32>,
    /// Per-output override for `LayoutConfig::smart_borders`.
    pub smart_borders: Option<bool>,
    pub max_visible_columns: Option<u32>,
    pub snap_on_drag: Option<bool>,
    pub snap_threshold_px: Option<u32>,
    pub status_bar: Option<bool>,
    pub hide_method: Option<String>,
    pub reserve_top: Option<u32>,
    pub reserve_bottom: Option<u32>,
    pub reserve_left: Option<u32>,
    pub reserve_right: Option<u32>,
}

impl LayoutConfigPartial {
    /// Fold the override's `Some` fields onto a clone of `base`, returning the
    /// effective per-output `LayoutConfig`.  `None` fields fall through to the
    /// base value (i.e. the global `layout { … }` block wins for unset keys).
    pub fn apply_to(&self, base: &LayoutConfig) -> LayoutConfig {
        let mut out = base.clone();
        if let Some(v) = self.inner_gaps {
            out.inner_gaps = v;
        }
        if let Some(v) = self.outer_gaps {
            out.outer_gaps = v;
        }
        if let Some(v) = self.border_width {
            out.border_width = v;
        }
        if let Some(ref v) = self.border_color {
            out.border_color = v.clone();
        }
        if let Some(ref v) = self.border_color_focused {
            // Per-output override: an "accent" / "windows-accent" sentinel
            // is intentionally NOT stored as a literal — the engine reads
            // the mode at apply time.  We keep this partial as a `String`
            // so the parser surface stays uniform, but the engine
            // (`LayoutConfig::from_config`) handles the "accent" branch
            // by reading the same sentinel out of the partial-folded
            // string.  Empty overrides keep the parent value.
            out.border_color_focused = v.clone();
        }
        if let Some(v) = self.focus_ring_width {
            out.focus_ring_width = v;
        }
        if let Some(v) = self.dim_unfocused {
            out.dim_unfocused = v;
        }
        if let Some(ref v) = self.focus_ring_color {
            out.focus_ring_color = v.clone();
        }
        if let Some(v) = self.shadow_enable {
            out.shadow_enable = v;
        }
        if let Some(v) = self.shadow_opacity {
            out.shadow_opacity = v;
        }
        if let Some(v) = self.shadow_offset_x {
            out.shadow_offset_x = v;
        }
        if let Some(v) = self.shadow_offset_y {
            out.shadow_offset_y = v;
        }
        if let Some(v) = self.shadow_blur {
            out.shadow_blur = v;
        }
        if let Some(ref v) = self.shadow_color {
            out.shadow_color = v.clone();
        }
        if let Some(v) = self.split_ratio {
            out.split_ratio = v;
        }
        if let Some(v) = self.auto_balance {
            out.auto_balance = v;
        }
        if let Some(v) = self.scroll_step {
            out.scroll_step = v;
        }
        if let Some(v) = self.column_width {
            out.column_width = v;
        }
        if let Some(ref v) = self.column_width_mode {
            out.column_width_mode = v.clone();
        }
        if let Some(v) = self.strip_frame {
            out.strip_frame = v;
        }
        if let Some(ref v) = self.border_color_urgent {
            out.border_color_urgent = v.clone();
        }
        if let Some(v) = self.border_radius {
            out.border_radius = v;
        }
        if let Some(v) = self.border_padding {
            out.border_padding = v;
        }
        if let Some(v) = self.focus_ring_gap {
            out.focus_ring_gap = v;
        }
        if let Some(ref v) = self.focus_ring_inactive_color {
            out.focus_ring_inactive_color = v.clone();
        }
        if let Some(v) = self.shadow_spread {
            out.shadow_spread = v;
        }
        if let Some(v) = self.smart_borders {
            out.smart_borders = v;
        }
        if let Some(v) = self.max_visible_columns {
            out.max_visible_columns = v;
        }
        if let Some(v) = self.snap_on_drag {
            out.snap_on_drag = v;
        }
        if let Some(v) = self.snap_threshold_px {
            out.snap_threshold_px = v;
        }
        if let Some(v) = self.status_bar {
            out.status_bar = v;
        }
        if let Some(ref v) = self.hide_method {
            out.hide_method = v.clone();
        }
        if let Some(v) = self.reserve_top {
            out.reserve_top = v;
        }
        if let Some(v) = self.reserve_bottom {
            out.reserve_bottom = v;
        }
        if let Some(v) = self.reserve_left {
            out.reserve_left = v;
        }
        if let Some(v) = self.reserve_right {
            out.reserve_right = v;
        }
        out
    }
}

#[derive(Debug, Clone, Default)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

impl Position {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LayoutConfig {
    pub inner_gaps: u32,
    pub outer_gaps: u32,
    pub border_width: u32,
    pub border_color: String,
    pub border_color_focused: String,
    pub focus_ring_width: u32,
    pub dim_unfocused: f32,
    pub focus_ring_color: String,
    pub shadow_enable: bool,
    pub shadow_opacity: f64,
    pub shadow_offset_x: i32,
    pub shadow_offset_y: i32,
    pub shadow_blur: u32,
    pub shadow_color: String,
    pub split_ratio: f64,
    pub auto_balance: bool,
    pub scroll_step: u32,
    pub column_width: u32,
    pub column_width_mode: String,
    /// Proportional mode: split the screen among at most this many columns
    /// (default 3); further windows scroll in.  0 = no limit.
    pub max_visible_columns: u32,
    /// Strip native frame (caption + thick frame) from tiled windows for pixel-perfect tiles.
    /// Default false — some apps render their content area black after style changes
    /// until they receive a paint message. Off by default for compatibility.
    pub strip_frame: bool,

    // ---- nested-block fields surfaced as flat config fields ----
    /// Border colour used when a window is in the "urgent" state (e.g. flashing).
    pub border_color_urgent: String,
    /// Corner radius (px) for borders + window mask. 0 = square.
    pub border_radius: u32,
    /// Inner padding (px) between window content and border.
    pub border_padding: u32,
    /// Gap (px) between window edge and the focus ring.
    pub focus_ring_gap: u32,
    /// Ring colour for non-focused windows that still want a visible ring.
    pub focus_ring_inactive_color: String,
    /// Shadow spread (px). Default 0.
    pub shadow_spread: u32,
    /// Snap a dragged tile to nearby column edges / mid-screen guides while
    /// the user is holding Alt and dragging with the left mouse button
    /// (niri parity).  When false, no snapping happens and the window
    /// follows the cursor pixel-for-pixel.
    pub snap_on_drag: bool,
    /// Maximum cursor-to-candidate distance (in logical pixels) that
    /// triggers a snap.  Smaller = the user has to land closer to a guide.
    /// Defaults to 20 px.  A value of 0 disables snapping even when
    /// `snap_on_drag` is true.
    pub snap_threshold_px: u32,
    /// niri-parity "smart borders": when `true`, suppress the DWM border on
    /// any workspace that has exactly one column with exactly one tile (so a
    /// single full-screen-ish window has no border to delimit it).  Multiple
    /// columns or stacked tiles still show borders normally.  Defaults to
    /// `false` to preserve historical behaviour — opt-in via
    /// `border-smart true` or `border { smart true }` in `config.kdl`.
    pub smart_borders: bool,
    /// Show a thin status bar at the top of the primary monitor.  Displays the
    /// current workspace label (left), focused window title (centre), and the
    /// local time (right).  Opt-in via `status-bar true` inside `layout { }`.
    /// Default `false`.
    pub status_bar: bool,
    /// How the windows of inactive workspaces are hidden: `"hide"` (`SW_HIDE`, the
    /// default: they leave the taskbar and Alt+Tab) or `"cloak"` (shell cloaking: they stay
    /// in the taskbar and Alt+Tab but are not drawn; off by default because it relies on
    /// undocumented shell interfaces).  Set via `hide-method` inside `layout { }`.
    pub hide_method: String,

    // ---- screen-area reservation for external bars / panels ----
    /// Pixels to subtract from the top of each monitor's tiling work area.
    /// Set via `reserve-top N` inside `layout { }`.  Default 0.
    pub reserve_top: u32,
    /// Pixels to subtract from the bottom of each monitor's tiling work area.
    /// Set via `reserve-bottom N` inside `layout { }`.  Default 0.
    pub reserve_bottom: u32,
    /// Pixels to subtract from the left of each monitor's tiling work area.
    /// Set via `reserve-left N` inside `layout { }`.  Default 0.
    pub reserve_left: u32,
    /// Pixels to subtract from the right of each monitor's tiling work area.
    /// Set via `reserve-right N` inside `layout { }`.  Default 0.
    pub reserve_right: u32,
}

impl LayoutConfig {
    pub fn default_config() -> Self {
        Self {
            inner_gaps: 16,
            outer_gaps: 8,
            border_width: 4,
            border_color: "#333333".to_string(),
            border_color_focused: "#0066cc".to_string(),
            focus_ring_width: 3,
            dim_unfocused: 1.0,
            focus_ring_color: "#00aaff".to_string(),
            shadow_enable: false,
            shadow_opacity: 0.5,
            shadow_offset_x: 10,
            shadow_offset_y: 10,
            shadow_blur: 20,
            shadow_color: "#000000".to_string(),
            split_ratio: 0.5,
            auto_balance: false,
            scroll_step: 200,
            column_width: 500,
            column_width_mode: "proportional".to_string(),
            max_visible_columns: 3,
            strip_frame: false,
            border_color_urgent: "#cc3344".to_string(),
            border_radius: 0,
            border_padding: 0,
            focus_ring_gap: 0,
            focus_ring_inactive_color: String::new(),
            shadow_spread: 0,
            snap_on_drag: true,
            snap_threshold_px: 20,
            smart_borders: false,
            status_bar: false,
            hide_method: "hide".to_string(),
            reserve_top: 0,
            reserve_bottom: 0,
            reserve_left: 0,
            reserve_right: 0,
        }
    }

    /// Return gap sizes as an `Edges<i32>` view.
    /// Uses `inner_gaps` for all four sides; per-side overrides are a future addition.
    pub fn gaps_edges(&self) -> Edges<i32> {
        let g = self.inner_gaps as i32;
        Edges {
            left: g,
            right: g,
            top: g,
            bottom: g,
        }
    }

    /// Return a nested border view built from the flat fields.
    pub fn border(&self) -> BorderSubConfig {
        BorderSubConfig {
            enabled: self.border_width > 0,
            width: self.border_width,
            color: self.border_color.clone(),
            color_focused: self.border_color_focused.clone(),
            color_urgent: self.border_color_urgent.clone(),
            radius: self.border_radius,
            padding: self.border_padding,
        }
    }

    /// Return a nested focus-ring view built from the flat fields.
    pub fn focus_ring(&self) -> FocusRingSubConfig {
        FocusRingSubConfig {
            enabled: self.focus_ring_width > 0,
            width: self.focus_ring_width,
            gap: self.focus_ring_gap,
            active_color: self.focus_ring_color.clone(),
            inactive_color: self.focus_ring_inactive_color.clone(),
        }
    }

    /// Return a nested shadow view built from the flat fields.
    pub fn shadow(&self) -> ShadowSubConfig {
        ShadowSubConfig {
            enabled: self.shadow_enable,
            offset_x: self.shadow_offset_x,
            offset_y: self.shadow_offset_y,
            blur: self.shadow_blur,
            spread: self.shadow_spread,
            color: self.shadow_color.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// LayoutConfig sub-structs and Edges generic
// ---------------------------------------------------------------------------

/// A four-sided value (e.g. gaps or padding) generic over any `Copy + Default` type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Edges<T: Copy + Default> {
    pub left: T,
    pub right: T,
    pub top: T,
    pub bottom: T,
}

/// Nested border configuration view derived from `LayoutConfig` flat fields.
#[derive(Debug, Clone, Default)]
pub struct BorderSubConfig {
    pub enabled: bool,
    pub width: u32,
    pub color: String,
    pub color_focused: String,
    /// Urgent border color; no flat counterpart yet, defaults to empty string.
    pub color_urgent: String,
    /// Corner radius; no flat counterpart yet, defaults 0.
    pub radius: u32,
    /// Inner padding; no flat counterpart yet, defaults 0.
    pub padding: u32,
}

/// Nested focus-ring configuration view derived from `LayoutConfig` flat fields.
#[derive(Debug, Clone, Default)]
pub struct FocusRingSubConfig {
    pub enabled: bool,
    pub width: u32,
    /// Gap between window edge and focus ring; no flat counterpart yet, defaults 0.
    pub gap: u32,
    pub active_color: String,
    /// Inactive ring color; no flat counterpart yet, defaults to empty string.
    pub inactive_color: String,
}

/// Nested shadow configuration view derived from `LayoutConfig` flat fields.
#[derive(Debug, Clone, Default)]
pub struct ShadowSubConfig {
    pub enabled: bool,
    pub offset_x: i32,
    pub offset_y: i32,
    pub blur: u32,
    /// Shadow spread; no flat counterpart yet, defaults 0.
    pub spread: u32,
    pub color: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceConfig {
    pub name: String,
    pub layout: String,
    pub monitor: String,
    pub follow_on_focus: bool,
}

impl WorkspaceConfig {
    pub fn default_config() -> Self {
        Self {
            name: String::new(),
            layout: "tile".to_string(),
            monitor: String::new(),
            follow_on_focus: true,
        }
    }
}

/// A single matching criterion for a window rule.
///
/// Exact-match variants compare the context field with `==`.
/// Regex variants store the pattern as a `String` and compile it on each call
/// to `Matcher::matches` via `regex::Regex::new`.  Compilation is cheap for
/// window rules because rules are evaluated only when a new window appears —
/// not on every frame.  If a pattern fails to compile a `warn!` is emitted and
/// the variant returns `false` without panicking.
///
/// If hot-path performance ever matters, the patterns can be pre-compiled and
/// stored in a `OnceLock<regex::Regex>` field, but that requires interior
/// mutability (e.g. wrapping the `String` in an `Arc<OnceLock<…>>`).  Left as
/// a future optimisation.
///
/// Flag variants (`IsActive`, `IsFloating`, `IsUrgent`, `AtStartup`) match
/// when the corresponding field in `MatcherContext` is `true`.
#[derive(Debug, Clone)]
pub enum Matcher {
    /// Match on the window class name (exact string).
    ClassName(String),
    /// Match on the window class name (regex pattern).
    ClassNameRegex(String),
    /// Match on the window title (exact string).
    Title(String),
    /// Match on the window title (regex pattern).
    TitleRegex(String),
    /// Match on the window instance name (exact string).
    Instance(String),
    /// Match on the window instance name (regex pattern).
    InstanceRegex(String),
    /// Match on the process executable name (exact string).
    ProcessName(String),
    /// Match on the process executable name (regex pattern).
    ProcessNameRegex(String),
    /// Match only when the window is the active (foreground) window.
    IsActive,
    /// Match only when the window is in floating state.
    IsFloating,
    /// Match only when the window has the urgent flag set.
    IsUrgent,
    /// Match only during the startup phase.
    AtStartup,
    /// niri `app-id="regex"`: the regex is searched in the process's file name
    /// (`firefox.exe`), in that name without `.exe` (`firefox`) and in the window's
    /// AppUserModelID (packaged apps), anywhere in the string.
    AppId(RegexPat),
    /// niri `title="regex"`: the regex is searched anywhere in the window title.
    TitleSearch(RegexPat),
    /// niri `class-name="regex"` / wiri extension: searched in the window class.
    ClassSearch(RegexPat),
    /// wiri extension `process-name="regex"`: searched in the process's file name.
    ProcessSearch(RegexPat),
    /// Match only when the window has the keyboard focus (niri `is-focused`).
    IsFocused,
    /// Match only when the window is the active one of its column (niri `is-active-in-column`).
    IsActiveInColumn,
    /// The opposite of the matcher inside (`is-active=false`).
    Not(Box<Matcher>),
}

/// A regular expression from the config, kept with its source text.
#[derive(Debug, Clone)]
pub struct RegexPat {
    pub source: String,
    pub regex: regex::Regex,
}

impl RegexPat {
    /// Compile `source` (Rust regex syntax, like niri's).
    pub fn new(source: &str) -> std::result::Result<RegexPat, regex::Error> {
        let regex = regex::Regex::new(source)?;
        Ok(RegexPat {
            source: source.to_string(),
            regex,
        })
    }

    /// True when the regex matches anywhere in `text`.
    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }
}

/// niri's preset size: a proportion of the view or a fixed size in pixels
/// (`default-column-width { proportion 0.5; }`, `{ fixed 800; }`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeSpec {
    Proportion(f64),
    Fixed(u32),
}

/// One `match` or `exclude` line of a niri window rule: every condition has to hold.
#[derive(Debug, Clone, Default)]
pub struct MatchGroup {
    pub conds: Vec<Matcher>,
}

impl MatchGroup {
    pub fn matches(&self, ctx: &MatcherContext<'_>) -> bool {
        self.conds.iter().all(|m| m.matches(ctx))
    }
}

/// True when any candidate name of a window matches an `app-id` regex.  Windows has no
/// app-id: the process's file name, that name without its extension and the
/// AppUserModelID of a packaged app stand in for it.
pub fn app_id_matches(pat: &RegexPat, ctx: &MatcherContext<'_>) -> bool {
    if ctx.process_names().iter().flatten().any(|n| pat.is_match(n)) {
        return true;
    }
    if let Some(aumid) = ctx.aumid {
        if pat.is_match(aumid) {
            return true;
        }
    }
    false
}

impl Matcher {
    /// Returns `true` when this matcher's criterion is satisfied by `ctx`.
    ///
    /// For `*Regex` variants the pattern is compiled on each call.  This is
    /// intentional — window rules are evaluated only when windows open, not on
    /// every frame.  If compilation fails a `warn!` is emitted once and the
    /// function returns `false` without panicking.
    pub fn matches(&self, ctx: &MatcherContext<'_>) -> bool {
        match self {
            Matcher::ClassName(s) => ctx.class_name == s.as_str(),
            Matcher::ClassNameRegex(pat) => match regex::Regex::new(pat) {
                Ok(re) => re.is_match(ctx.class_name),
                Err(e) => {
                    tracing::warn!("ClassNameRegex: invalid pattern {:?}: {}", pat, e);
                    false
                }
            },
            Matcher::Title(s) => ctx.title == s.as_str(),
            Matcher::TitleRegex(pat) => match regex::Regex::new(pat) {
                Ok(re) => re.is_match(ctx.title),
                Err(e) => {
                    tracing::warn!("TitleRegex: invalid pattern {:?}: {}", pat, e);
                    false
                }
            },
            Matcher::Instance(s) => ctx.instance.map_or(false, |i| i == s.as_str()),
            Matcher::InstanceRegex(pat) => match regex::Regex::new(pat) {
                Ok(re) => re.is_match(ctx.instance.unwrap_or("")),
                Err(e) => {
                    tracing::warn!("InstanceRegex: invalid pattern {:?}: {}", pat, e);
                    false
                }
            },
            // The process answers to its file name and to that name without `.exe`; Windows
            // file names do not care about case.
            Matcher::ProcessName(s) => ctx
                .process_names()
                .iter()
                .flatten()
                .any(|p| p.eq_ignore_ascii_case(s.as_str())),
            Matcher::ProcessNameRegex(pat) => match regex::Regex::new(pat) {
                Ok(re) => match ctx.process_names() {
                    [None, _] => re.is_match(""),
                    names => names.iter().flatten().any(|p| re.is_match(p)),
                },
                Err(e) => {
                    tracing::warn!("ProcessNameRegex: invalid pattern {:?}: {}", pat, e);
                    false
                }
            },
            Matcher::IsActive => ctx.is_active,
            Matcher::IsFloating => ctx.is_floating,
            Matcher::IsUrgent => ctx.is_urgent,
            Matcher::AtStartup => ctx.at_startup,
            Matcher::AppId(pat) => app_id_matches(pat, ctx),
            Matcher::TitleSearch(pat) => pat.is_match(ctx.title),
            Matcher::ClassSearch(pat) => pat.is_match(ctx.class_name),
            Matcher::ProcessSearch(pat) => {
                ctx.process_names().iter().flatten().any(|p| pat.is_match(p))
            }
            Matcher::IsFocused => ctx.is_focused,
            Matcher::IsActiveInColumn => ctx.is_active_in_column,
            Matcher::Not(inner) => !inner.matches(ctx),
        }
    }

    /// True when this condition, or the one it negates, satisfies `test`.
    pub fn any(&self, test: &dyn Fn(&Matcher) -> bool) -> bool {
        match self {
            Matcher::Not(inner) => inner.any(test),
            other => test(other),
        }
    }

    /// True for the conditions that depend on the window's state (focus, floating, urgency)
    /// rather than on what it is: a rule that has one has to be looked at again when that
    /// state changes.
    pub fn is_dynamic(&self) -> bool {
        match self {
            Matcher::IsActive
            | Matcher::IsFloating
            | Matcher::IsUrgent
            | Matcher::IsFocused
            | Matcher::IsActiveInColumn
            | Matcher::TitleSearch(_)
            | Matcher::Title(_)
            | Matcher::TitleRegex(_) => true,
            Matcher::Not(inner) => inner.is_dynamic(),
            _ => false,
        }
    }
}

/// Context passed to `Matcher::matches` and `WindowRule::matches`.
///
/// The boolean flags (`is_active`, `is_floating`, `is_urgent`, `at_startup`) are
/// populated by `layout::engine::TilingEngine::add_window_with_target` with live
/// state pulled from `GetForegroundWindow`, the engine's `floating_windows` set,
/// the `urgent_windows` set (driven by `backend::hooks::URGENT_WINDOWS` +
/// `FLASHWINFO`), and the `at_startup` flag is set by
/// `add_window_at_startup`.
///
/// Other call sites (IPC, the legacy shim) construct contexts with these flags
/// defaulting to `false`; they're considered best-effort and only used for
/// rule-resolution against external (non-engine) requests.
pub struct MatcherContext<'a> {
    pub class_name: &'a str,
    pub title: &'a str,
    pub instance: Option<&'a str>,
    pub process_name: Option<&'a str>,
    pub is_active: bool,
    pub is_floating: bool,
    pub is_urgent: bool,
    pub at_startup: bool,
    /// The AppUserModelID of a packaged (UWP / MSIX) app, when the window has one.
    pub aumid: Option<&'a str>,
    /// The window has the keyboard focus.
    pub is_focused: bool,
    /// The window is the active one of its column.
    pub is_active_in_column: bool,
}

impl<'a> MatcherContext<'a> {
    /// The names the window's process answers to: its file name (`firefox.exe`) and that
    /// name without the extension (`firefox`), or nothing for a window whose process is not
    /// known.
    pub fn process_names(&self) -> [Option<&'a str>; 2] {
        match self.process_name {
            None => [None, None],
            Some(name) => {
                let stem = match name.rfind('.') {
                    Some(dot) if dot > 0 => Some(&name[..dot]),
                    _ => None,
                };
                [Some(name), stem]
            }
        }
    }

    /// Build a `MatcherContext` from the fields previously threaded through
    /// `resolve_window_rules` (class, title, instance, pid).  All boolean
    /// flags are set to `false` — this constructor is only used by external
    /// (non-engine) call sites that cannot observe live window state.  The
    /// engine itself builds a richer context inline (see
    /// `layout::engine::TilingEngine::add_window_with_target`).
    pub fn from_legacy(
        class_name: &'a str,
        title: &'a str,
        instance: Option<&'a str>,
        process_name: Option<&'a str>,
    ) -> Self {
        Self {
            class_name,
            title,
            instance,
            process_name,
            is_active: false,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: false,
            is_active_in_column: false,
        }
    }

    /// Build a `MatcherContext` from a `crate::backend::WindowInfo`.
    /// Boolean flags default to `false` — callers that want live state should
    /// construct `MatcherContext` directly (see
    /// `layout::engine::TilingEngine::add_window_with_target`).
    pub fn from_window_info(info: &'a crate::backend::WindowInfo) -> Self {
        Self {
            class_name: &info.class_name,
            title: &info.title,
            instance: None,
            process_name: None,
            is_active: false,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: false,
            is_active_in_column: false,
        }
    }
}

/// A window rule loaded from configuration.
///
/// **Matching behaviour** — two modes, never mixed within a single rule:
///
/// 1. **Matcher-based** (`matchers` is non-empty): all entries in `matchers`
///    must be satisfied (AND semantics).  The flat fields (`class`, `title`,
///    `instance`, `process_name`, `pid`) are ignored.
///
/// 2. **Flat-field fallback** (`matchers` is empty): the legacy flat fields
///    are used for matching, preserving full backwards compatibility with
///    existing configs that do not use a `match { }` block.
///
/// Regex matching is supported via `Matcher::*Regex` variants in `match { }` blocks.
#[derive(Debug, Clone, Default)]
pub struct WindowRule {
    // ---- enum-based matchers (populated from `match { }` blocks) ----
    /// When non-empty ALL matchers must pass (AND semantics).
    /// When empty the flat fields below are used instead.
    pub matchers: Vec<Matcher>,

    // ---- flat/legacy matchers (used when `matchers` is empty) ----
    pub class: Option<String>,
    pub title: Option<String>,
    pub instance: Option<String>,
    pub process_name: Option<String>,
    pub pid: Option<u32>,

    // ---- properties (applied when this rule matches) ----
    pub floating: bool,
    pub workspace: Option<String>,
    pub monitor: Option<String>,
    /// Per-window opacity override (0.0 = fully transparent, 1.0 = fully opaque).
    /// `None` means "field unset; fall through to default / earlier rules".
    /// `Some(x)` is an explicit override — including `Some(0.0)` for fully transparent.
    pub opacity: Option<f64>,
    pub blur: bool,
    pub sticky: bool,
    pub scale: Option<f64>,
    pub default_width: Option<u32>,
    pub default_height: Option<u32>,

    // ---- niri-parity `open-*` placement properties (applied on first map) ----
    /// Place the window on the named monitor when it first appears.  Matches
    /// `OutputConfig.name` (the friendly device name from Windows display
    /// settings).
    pub open_on_output: Option<String>,
    /// Place the window on the workspace with this 1-based numeric id when it
    /// first appears.
    pub open_in_workspace: Option<i32>,
    /// When `Some(true)`, start the window fullscreen.  `Some(false)` is
    /// explicit-not-fullscreen; `None` leaves the engine default.
    pub open_fullscreen: Option<bool>,
    /// When `Some(true)`, start the window floating.  Equivalent to the
    /// existing `floating` flag but expresses "on open" semantics for the
    /// niri-style placement pipeline.
    pub open_floating: Option<bool>,
    /// Cap the initial physical size of the window to `(width_px, height_px)`.
    /// Either dimension of `0` means "no cap on that axis".
    pub open_max_bounds: Option<(u32, u32)>,

    // ---- niri window-rule matching: `match` lines (any) and `exclude` lines (none) ----
    /// niri: a window matches the rule when it matches ANY of these (an empty list matches
    /// every window), each group being the AND of its conditions.
    pub match_groups: Vec<MatchGroup>,
    /// niri: a window that matches ANY of these is left out.
    pub exclude_groups: Vec<MatchGroup>,

    // ---- niri properties (applied when the rule matches) ----
    /// `default-column-width`: `Some(Some(size))` for `{ proportion 0.5; }` / `{ fixed 800; }`,
    /// `Some(None)` for an empty block (the window picks its own width), `None` when unset.
    pub default_column_width: Option<Option<SizeSpec>>,
    /// `default-window-height`, the same forms.
    pub default_window_height: Option<Option<SizeSpec>>,
    /// `open-on-workspace "name"`: the workspace (by name) the window opens on.
    pub open_on_workspace: Option<String>,
    /// `open-maximized`: the window's column opens as wide as the view.
    pub open_maximized: Option<bool>,
    /// `open-maximized-to-edges`: the window opens maximized to the edges of the work area.
    pub open_maximized_to_edges: Option<bool>,
    /// `open-focused`: false keeps the focus where it was when the window opens.
    pub open_focused: Option<bool>,
    /// `min-width` / `min-height` / `max-width` / `max-height`: size limits in pixels.
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}

impl WindowRule {
    /// Returns `true` when this rule matches the given `MatcherContext`: the old flat /
    /// `matchers` conditions all hold, and (niri) the window matches any `match` line, if
    /// there are any, and none of the `exclude` lines.
    pub fn matches(&self, ctx: &MatcherContext<'_>) -> bool {
        if !self.legacy_matches(ctx) {
            return false;
        }
        if !self.match_groups.is_empty() && !self.match_groups.iter().any(|g| g.matches(ctx)) {
            return false;
        }
        !self.exclude_groups.iter().any(|g| g.matches(ctx))
    }

    /// True when any condition of the rule depends on state that changes while a window
    /// lives (focus, floating, urgency, the title), so the rule has to be looked at again.
    pub fn has_dynamic_conditions(&self) -> bool {
        self.matchers.iter().any(|m| m.is_dynamic())
            || self.title.is_some()
            || self
                .match_groups
                .iter()
                .chain(self.exclude_groups.iter())
                .any(|g| g.conds.iter().any(|m| m.is_dynamic()))
    }

    /// True when any condition of the rule (in `matchers`, a `match` line or an `exclude`
    /// line) satisfies `test`.
    pub fn any_condition(&self, test: &dyn Fn(&Matcher) -> bool) -> bool {
        self.matchers
            .iter()
            .chain(
                self.match_groups
                    .iter()
                    .chain(self.exclude_groups.iter())
                    .flat_map(|g| g.conds.iter()),
            )
            .any(|m| m.any(test))
    }

    /// The rule looks at the AppUserModelID of the window (`app-id`).
    pub fn uses_app_id(&self) -> bool {
        self.any_condition(&|m| matches!(m, Matcher::AppId(_)))
    }

    /// The rule's outcome depends on which window has the focus.
    pub fn depends_on_focus(&self) -> bool {
        self.any_condition(&|m| {
            matches!(
                m,
                Matcher::IsActive | Matcher::IsFocused | Matcher::IsActiveInColumn
            )
        })
    }

    /// The rule's outcome depends on the title of the window.
    pub fn depends_on_title(&self) -> bool {
        self.title.is_some()
            || self.any_condition(&|m| {
                matches!(
                    m,
                    Matcher::Title(_) | Matcher::TitleRegex(_) | Matcher::TitleSearch(_)
                )
            })
    }

    /// The rule's outcome depends on whether the window floats.
    pub fn depends_on_floating(&self) -> bool {
        self.any_condition(&|m| matches!(m, Matcher::IsFloating))
    }

    /// The rule's outcome depends on whether the window is urgent (flashing).
    pub fn depends_on_urgency(&self) -> bool {
        self.any_condition(&|m| matches!(m, Matcher::IsUrgent))
    }

    /// The conditions of the old rule format: `matchers` when there are any (all of them
    /// must pass), otherwise the flat `class` / `title` / `instance` / `process_name` fields.
    fn legacy_matches(&self, ctx: &MatcherContext<'_>) -> bool {
        if !self.matchers.is_empty() {
            // Matcher-based path: every matcher must pass.
            return self.matchers.iter().all(|m| m.matches(ctx));
        }

        // Flat-field fallback path (legacy).
        if let Some(ref rule_class) = self.class {
            if ctx.class_name != rule_class.as_str() {
                return false;
            }
        }
        if let Some(ref rule_title) = self.title {
            if ctx.title != rule_title.as_str() {
                return false;
            }
        }
        if let Some(ref rule_instance) = self.instance {
            match ctx.instance {
                Some(i) if i == rule_instance.as_str() => {}
                _ => return false,
            }
        }
        if let Some(ref rule_proc) = self.process_name {
            match ctx.process_name {
                Some(p) if p == rule_proc.as_str() => {}
                _ => return false,
            }
        }
        if let Some(rule_pid) = self.pid {
            // pid is not in MatcherContext; treat a non-zero context pid as a
            // proxy via process_name. For direct pid matching keep flat path only.
            // We cannot check pid here without extending MatcherContext — leave
            // pid matching unsatisfied when coming from a MatcherContext that has
            // no pid field.  Callers that need pid matching should use the legacy
            // resolve_window_rules_legacy helper.
            let _ = rule_pid;
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct BindsConfig {
    pub hotkeys: Vec<HotkeyBinding>,
    /// niri-style "extend the built-in default keybindings instead of
    /// replacing them".  Default `true` — the user's `binds { … }` block
    /// is folded on top of the shipped defaults so newly-added actions
    /// stay available without a config edit.  Set to `false` (via
    /// `binds { extend-defaults false … }`) to get the pre-2026-05-18
    /// "replace everything" behaviour.
    pub extend_defaults: bool,
}

impl Default for BindsConfig {
    fn default() -> Self {
        Self {
            hotkeys: Vec::new(),
            extend_defaults: true,
        }
    }
}

#[derive(Debug, Clone)]
/// A keybinding from the config file.
/// Maps modifier+key combos to actions.
pub struct HotkeyBinding {
    pub modifiers: Vec<String>,
    pub key: String,
    /// The action name from config (e.g. "focus-column-left", "close-window")
    pub command: String,
    pub args: Vec<String>,
    /// Whether each argument was written in double quotes (parallel to `args`;
    /// missing entries mean "not quoted").  `focus-workspace 2` is an index,
    /// `focus-workspace "2"` the workspace named "2".
    pub args_quoted: Vec<bool>,
    /// niri's `repeat=false` turns this off: holding the key fires the action once.  True
    /// (the default) lets the key's auto-repeat fire it again.
    pub repeat: bool,
    /// Wiri extension `on-release=true` (also `release=true`); pinned niri has no
    /// generic release-bind property. The chord is captured on down, fired on up.
    pub release: bool,
    /// niri's `cooldown-ms=`: after the action fired, the bind is ignored for this long.
    pub cooldown_ms: Option<u32>,
    /// niri's `allow-when-locked=`: accepted; a locked Windows session never delivers hotkeys.
    pub allow_when_locked: bool,
    /// niri's `allow-inhibiting=`: false keeps the bind active during shortcut inhibition.
    pub allow_inhibiting: bool,
    /// niri's `hotkey-overlay-title=`: `Some(Some(text))` is the title, `Some(None)`
    /// (`hotkey-overlay-title=null`) hides the bind from the overlay.
    pub hotkey_overlay_title: Option<Option<String>>,
    /// Properties written on the action itself (`focus=false` of `move-window-to-workspace`,
    /// `show-pointer=false` of `screenshot`), as `(name, value)` text.
    pub action_props: Vec<(String, String)>,
}

impl HotkeyBinding {
    /// Parse the action name to an Action enum.
    /// Delegates to the shared parse_action_name function.  Recognises the
    /// niri-style `spawn-cmd "<command>"` shorthand by returning
    /// `Action::SpawnCmd(...)` so the WM_HOTKEY dispatcher routes through
    /// the Windows shell instead of `Spawn`'s whitespace-split argv path.
    pub fn parse_action(&self) -> Option<crate::input::Action> {
        if self.is_spawn_cmd() {
            if let Some(cmd) = self.args.first() {
                if !cmd.is_empty() {
                    return Some(crate::input::Action::SpawnCmd(cmd.clone()));
                }
            }
            return None;
        }
        crate::input::parse_action_flagged(&self.command, &self.args, &self.args_quoted)
    }

    /// Parse modifier strings to Windows MOD flags.
    ///
    /// Lenient: a modifier it does not know is skipped, and `Mod` is not
    /// understood.  Registering hotkeys must use [`Self::resolve_modifiers`],
    /// which refuses such binds instead of turning them into a bare key.
    pub fn mod_flags(&self) -> u32 {
        crate::input::parse_modifiers(&self.modifiers)
    }

    /// Strict modifier resolution: `Mod` becomes `mod_key` (what `input {
    /// mod-key }` stands for), and any modifier that is not understood is an
    /// error.
    pub fn resolve_modifiers(&self, mod_key: u32) -> Result<u32, crate::input::ModifierError> {
        crate::input::resolve_bind_modifiers(&self.modifiers, mod_key)
    }

    /// The bind as the user wrote it, for log messages (`Mod+Shift+T`).
    pub fn chord_text(&self) -> String {
        let mut parts: Vec<&str> = self.modifiers.iter().map(|m| m.as_str()).collect();
        parts.push(self.key.as_str());
        parts.join("+")
    }

    /// Parse key string to a Windows virtual key code.
    /// Delegates to the shared parse_key_name function.
    pub fn vk_code(&self) -> Option<u32> {
        crate::input::parse_key_name(&self.key)
    }

    /// Returns `true` when this binding's command is the niri-style
    /// `spawn-cmd "<command>"` shorthand (Windows equivalent of
    /// `spawn-sh`).  The dispatcher (owned by Agent E in
    /// `backend/message_loop.rs`) consumes this to spawn the argument
    /// through `cmd.exe /C "<command>"`.
    pub fn is_spawn_cmd(&self) -> bool {
        matches!(
            self.command.as_str(),
            "spawn-cmd" | "spawn_cmd" | "spawn-sh"
        )
    }

    /// When this binding is `spawn-cmd "<command>"`, return the canonical
    /// argv expansion the dispatcher should pass to `CreateProcessW`:
    /// `["cmd.exe", "/C", "<command>"]`.
    ///
    /// Returns `None` for any other command name, or when the bind has no
    /// argument (i.e. `Ctrl+Alt+S spawn-cmd` with no quoted command).
    pub fn shell_command_argv(&self) -> Option<Vec<String>> {
        if !self.is_spawn_cmd() {
            return None;
        }
        let cmd = self.args.first()?;
        if cmd.is_empty() {
            return None;
        }
        Some(vec!["cmd.exe".to_string(), "/C".to_string(), cmd.clone()])
    }
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self {
            key: String::new(),
            modifiers: Vec::new(),
            command: String::new(),
            args: Vec::new(),
            args_quoted: Vec::new(),
            repeat: true,
            release: false,
            cooldown_ms: None,
            allow_when_locked: false,
            allow_inhibiting: true,
            hotkey_overlay_title: None,
            action_props: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AnimationsConfig {
    pub enabled: bool,
    pub duration: u32,
    pub easing: String,
    pub focus_transition: bool,
    pub workspace_transition: bool,
    pub window_move: bool,
    pub window_resize: bool,
    pub popup: bool,
    pub shadow: bool,
}

impl AnimationsConfig {
    pub fn default_config() -> Self {
        Self {
            enabled: false,
            duration: 200,
            easing: "cubic-bezier".to_string(),
            focus_transition: true,
            workspace_transition: true,
            window_move: true,
            window_resize: true,
            popup: true,
            shadow: true,
        }
    }
}

pub fn parse_color(color: &str) -> Option<[u8; 4]> {
    let color = color.trim();
    if color.starts_with('#') {
        let hex = &color[1..];
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some([r, g, b, 255])
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some([r, g, b, a])
            }
            _ => None,
        }
    } else {
        None
    }
}

pub fn parse_modifiers(modifiers: &[String]) -> ModifierMask {
    let mut mask = ModifierMask::empty();
    for m in modifiers {
        match m.to_lowercase().as_str() {
            "ctrl" | "control" => mask |= ModifierMask::CTRL,
            "alt" => mask |= ModifierMask::ALT,
            "shift" => mask |= ModifierMask::SHIFT,
            "super" | "meta" | "win" => mask |= ModifierMask::SUPER,
            "mod1" => mask |= ModifierMask::MOD1,
            "mod2" => mask |= ModifierMask::MOD2,
            "mod3" => mask |= ModifierMask::MOD3,
            "mod4" => mask |= ModifierMask::MOD4,
            "mod5" => mask |= ModifierMask::MOD5,
            _ => {}
        }
    }
    mask
}

bitflags::bitflags! {
    #[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ModifierMask: u32 {
        const CTRL = 1 << 0;
        const ALT = 1 << 1;
        const SHIFT = 1 << 2;
        const SUPER = 1 << 3;
        const MOD1 = 1 << 4;
        const MOD2 = 1 << 5;
        const MOD3 = 1 << 6;
        const MOD4 = 1 << 7;
        const MOD5 = 1 << 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_color_hex6() {
        let c = parse_color("#ff0000").unwrap();
        assert_eq!(c, [255, 0, 0, 255]);
    }

    #[test]
    fn test_parse_color_hex8() {
        let c = parse_color("#ff000080").unwrap();
        assert_eq!(c, [255, 0, 0, 128]);
    }

    #[test]
    fn test_parse_color_hex_short() {
        assert_eq!(parse_color("#fff"), None);
    }

    #[test]
    fn test_parse_color_invalid_hex() {
        assert_eq!(parse_color("#gggggg"), None);
    }

    #[test]
    fn test_parse_color_no_hash() {
        assert_eq!(parse_color("ff0000"), None);
    }

    #[test]
    fn test_parse_color_with_whitespace() {
        let c = parse_color("  #00ff00  ").unwrap();
        assert_eq!(c, [0, 255, 0, 255]);
    }

    fn ctx<'a>(class: &'a str, title: &'a str) -> MatcherContext<'a> {
        MatcherContext::from_legacy(class, title, None, None)
    }

    fn ctx_full<'a>(
        class: &'a str,
        title: &'a str,
        instance: Option<&'a str>,
        process_name: Option<&'a str>,
    ) -> MatcherContext<'a> {
        MatcherContext::from_legacy(class, title, instance, process_name)
    }

    #[test]
    fn test_window_rule_matches_class() {
        let mut rule = WindowRule::default();
        rule.class = Some("Chrome".to_string());
        assert!(rule.matches(&ctx("Chrome", "")));
        assert!(!rule.matches(&ctx("Firefox", "")));
        assert!(!rule.matches(&ctx("", "")));
    }

    #[test]
    fn test_window_rule_matches_title() {
        let mut rule = WindowRule::default();
        rule.title = Some("Calculator".to_string());
        assert!(rule.matches(&ctx("", "Calculator")));
        assert!(!rule.matches(&ctx("", "Notepad")));
    }

    #[test]
    fn test_window_rule_matches_pid() {
        // pid is not in MatcherContext; a flat pid-only rule passes matches()
        // (the pid field is silently skipped). Real pid checks are in the
        // legacy helper in rules.rs.
        let mut rule = WindowRule::default();
        rule.pid = Some(1234);
        // The flat-field path skips pid (no pid field in MatcherContext), so
        // a pid-only rule matches anything — documented limitation.
        assert!(rule.matches(&ctx("", "")));
    }

    #[test]
    fn test_window_rule_matches_multiple_criteria() {
        let mut rule = WindowRule::default();
        rule.class = Some("Chrome".to_string());
        rule.title = Some("Gmail".to_string());
        assert!(rule.matches(&ctx("Chrome", "Gmail")));
        assert!(!rule.matches(&ctx("Chrome", "Drive")));
        assert!(!rule.matches(&ctx("Firefox", "Gmail")));
    }

    #[test]
    fn test_window_rule_matches_empty() {
        let rule = WindowRule::default();
        // Empty rule (no matchers, no flat fields) matches everything.
        assert!(rule.matches(&ctx("Anything", "Whatever")));
    }

    #[test]
    fn test_modifier_mask_flags() {
        assert_eq!(ModifierMask::CTRL.bits(), 1);
        assert_eq!(ModifierMask::ALT.bits(), 2);
        assert_eq!(ModifierMask::SHIFT.bits(), 4);
        assert_eq!(ModifierMask::SUPER.bits(), 8);
    }

    #[test]
    fn test_modifier_mask_combine() {
        let combined = ModifierMask::CTRL | ModifierMask::ALT;
        assert!(combined.contains(ModifierMask::CTRL));
        assert!(combined.contains(ModifierMask::ALT));
        assert!(!combined.contains(ModifierMask::SHIFT));
    }

    #[test]
    fn test_parse_modifiers_config() {
        let mask = parse_modifiers(&["Ctrl".to_string(), "Alt".to_string()]);
        assert!(mask.contains(ModifierMask::CTRL));
        assert!(mask.contains(ModifierMask::ALT));
    }

    #[test]
    fn test_hotkey_binding_parse_action() {
        let mut b = HotkeyBinding::default();
        b.command = "focus-column-left".to_string();
        assert_eq!(
            b.parse_action(),
            Some(crate::input::Action::FocusColumnLeft)
        );
    }

    #[test]
    fn test_hotkey_binding_mod_flags() {
        let mut b = HotkeyBinding::default();
        b.modifiers = vec!["Ctrl".to_string(), "Alt".to_string()];
        assert_eq!(b.mod_flags(), 0x0002 | 0x0001);
    }

    #[test]
    fn test_hotkey_binding_vk_code() {
        let mut b = HotkeyBinding::default();
        b.key = "Left".to_string();
        assert_eq!(b.vk_code(), Some(0x25));
    }

    /// `spawn-cmd "wt.exe"` is recognised and expands to the Windows shell argv.
    #[test]
    fn test_hotkey_binding_spawn_cmd_recognised() {
        let b = HotkeyBinding {
            command: "spawn-cmd".to_string(),
            args: vec!["wt.exe".to_string()],
            ..HotkeyBinding::default()
        };
        assert!(b.is_spawn_cmd());
        assert_eq!(
            b.shell_command_argv(),
            Some(vec![
                "cmd.exe".to_string(),
                "/C".to_string(),
                "wt.exe".to_string()
            ]),
        );
    }

    /// `spawn-sh "echo hi"` (niri-style alias) also works.
    #[test]
    fn test_hotkey_binding_spawn_sh_alias() {
        let b = HotkeyBinding {
            command: "spawn-sh".to_string(),
            args: vec!["echo hi".to_string()],
            ..HotkeyBinding::default()
        };
        assert!(b.is_spawn_cmd());
        assert_eq!(
            b.shell_command_argv(),
            Some(vec![
                "cmd.exe".to_string(),
                "/C".to_string(),
                "echo hi".to_string()
            ]),
        );
    }

    /// Non-spawn-cmd commands return None from `shell_command_argv`.
    #[test]
    fn test_hotkey_binding_non_spawn_cmd_returns_none() {
        let b = HotkeyBinding {
            command: "spawn".to_string(),
            args: vec!["notepad.exe".to_string()],
            ..HotkeyBinding::default()
        };
        assert!(!b.is_spawn_cmd());
        assert_eq!(b.shell_command_argv(), None);
    }

    /// `spawn-cmd` with no argument yields None (parser still records the bind
    /// so the user sees the error in logs).
    #[test]
    fn test_hotkey_binding_spawn_cmd_without_arg_yields_none() {
        let b = HotkeyBinding {
            command: "spawn-cmd".to_string(),
            args: vec![],
            ..HotkeyBinding::default()
        };
        assert!(b.is_spawn_cmd());
        assert!(b.shell_command_argv().is_none());
    }

    /// `spawn-cmd "wt.exe"` → `Action::SpawnCmd("wt.exe")` via parse_action so
    /// the WM_HOTKEY dispatcher can route through the Windows shell.
    #[test]
    fn test_hotkey_binding_spawn_cmd_parse_action_returns_spawn_cmd() {
        let b = HotkeyBinding {
            command: "spawn-cmd".to_string(),
            args: vec!["wt.exe".to_string()],
            ..HotkeyBinding::default()
        };
        assert_eq!(
            b.parse_action(),
            Some(crate::input::Action::SpawnCmd("wt.exe".to_string())),
        );
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.input.keyboard_layout, "us");
        assert!(!config.animations.enabled);
    }

    // -----------------------------------------------------------------------
    // InputConfig sub-struct accessor tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_input_config_keyboard_view() {
        let mut input = InputConfig::default_config();
        input.repeat_rate = 50;
        input.keyboard_layout = "de".to_string();
        input.numlock = true;
        let kb = input.keyboard();
        assert_eq!(kb.repeat_rate, 50);
        assert_eq!(kb.layout, "de");
        assert_eq!(kb.repeat_delay, input.repeat_delay);
        assert!(kb.numlock); // mirrors the flat field
    }

    #[test]
    fn test_input_config_mouse_view() {
        let mut input = InputConfig::default_config();
        input.mouse_speed = 2.5;
        input.mouse_acceleration = 0.3;
        input.mouse_sensitivity = 1.8;
        input.natural_scroll = true;
        input.tap_to_click = true;
        let m = input.mouse();
        assert!((m.speed - 2.5).abs() < f64::EPSILON);
        assert!((m.acceleration - 0.3).abs() < f64::EPSILON);
        assert!((m.sensitivity - 1.8).abs() < f64::EPSILON);
        assert!(m.natural_scroll);
        assert!(m.tap_to_click);
    }

    #[test]
    fn test_input_config_focus_view() {
        let mut input = InputConfig::default_config();
        input.focus_follows_mouse = true;
        input.focus_delay_ms = 150;
        input.warp_on_focus = true;
        let f = input.focus();
        assert!(f.follows_mouse);
        assert_eq!(f.focus_delay_ms, 150);
        assert!(f.warp_on_focus);
    }

    #[test]
    fn test_input_config_touch_view_defaults_and_overrides() {
        // Defaults: touch disabled.
        let input = InputConfig::default_config();
        let t = input.touch();
        assert!(!t.enabled);
        assert_eq!(t.swipe_threshold_px, 16);
        assert_eq!(t.swipe_timeout_ms, 250);
        assert!(t.tap_to_click);
        assert!((t.scroll_speed - 1.0).abs() < f64::EPSILON);

        // Overrides: enabling the flat fields propagates to the nested view.
        let mut input = InputConfig::default_config();
        input.touch_enabled = true;
        input.touch_tap_to_click = false;
        input.touch_scroll_speed = 2.5;
        let t = input.touch();
        assert!(t.enabled);
        assert!(!t.tap_to_click);
        assert!((t.scroll_speed - 2.5).abs() < f64::EPSILON);
    }

    // -----------------------------------------------------------------------
    // LayoutConfig sub-struct accessor tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_layout_config_gaps_edges() {
        let mut layout = LayoutConfig::default_config();
        layout.inner_gaps = 16;
        let e = layout.gaps_edges();
        assert_eq!(e.left, 16);
        assert_eq!(e.right, 16);
        assert_eq!(e.top, 16);
        assert_eq!(e.bottom, 16);
    }

    #[test]
    fn test_layout_config_border_view() {
        let mut layout = LayoutConfig::default_config();
        layout.border_width = 4;
        layout.border_color = "#333333".to_string();
        layout.border_color_focused = "#0066cc".to_string();
        layout.border_color_urgent = "#ff4444".to_string();
        layout.border_radius = 8;
        layout.border_padding = 2;
        let b = layout.border();
        assert!(b.enabled); // width > 0
        assert_eq!(b.width, 4);
        assert_eq!(b.color, "#333333");
        assert_eq!(b.color_focused, "#0066cc");
        assert_eq!(b.color_urgent, "#ff4444");
        assert_eq!(b.radius, 8);
        assert_eq!(b.padding, 2);
    }

    #[test]
    fn test_layout_config_border_disabled_when_zero_width() {
        let mut layout = LayoutConfig::default_config();
        layout.border_width = 0;
        assert!(!layout.border().enabled);
    }

    #[test]
    fn test_layout_config_focus_ring_view() {
        let mut layout = LayoutConfig::default_config();
        layout.focus_ring_width = 3;
        layout.focus_ring_color = "#00aaff".to_string();
        layout.focus_ring_gap = 6;
        layout.focus_ring_inactive_color = "#404040".to_string();
        let fr = layout.focus_ring();
        assert!(fr.enabled);
        assert_eq!(fr.width, 3);
        assert_eq!(fr.active_color, "#00aaff");
        assert_eq!(fr.gap, 6);
        assert_eq!(fr.inactive_color, "#404040");
    }

    #[test]
    fn test_layout_config_shadow_view() {
        let mut layout = LayoutConfig::default_config();
        layout.shadow_enable = true;
        layout.shadow_offset_x = 5;
        layout.shadow_offset_y = 8;
        layout.shadow_blur = 12;
        layout.shadow_color = "#000000".to_string();
        layout.shadow_spread = 4;
        let s = layout.shadow();
        assert!(s.enabled);
        assert_eq!(s.offset_x, 5);
        assert_eq!(s.offset_y, 8);
        assert_eq!(s.blur, 12);
        assert_eq!(s.spread, 4);
        assert_eq!(s.color, "#000000");
    }

    // Use the previously-dead `ctx_full` helper so it's exercised; this also
    // ensures the instance + process_name path of MatcherContext::from_legacy
    // continues to round-trip through WindowRule::matches.
    #[test]
    fn test_window_rule_matches_instance_and_process_name() {
        let mut rule = WindowRule::default();
        rule.instance = Some("main".to_string());
        rule.process_name = Some("firefox.exe".to_string());
        let ok = ctx_full(
            "Mozilla",
            "About:Mozilla",
            Some("main"),
            Some("firefox.exe"),
        );
        let wrong_proc = ctx_full("Mozilla", "About:Mozilla", Some("main"), Some("chrome.exe"));
        assert!(rule.matches(&ok));
        assert!(!rule.matches(&wrong_proc));
    }

    /// `OutputConfig.mod_key` defaults to `None` and accepts a `Some` override.
    /// This exercises the struct field directly; the config-parser path is
    /// exercised by `test_output_config_mod_key_parsing` in `config/parse.rs`.
    #[test]
    fn test_output_config_mod_key_field_defaults_none() {
        let cfg = Config::default();
        let out = &cfg.output[0];
        assert!(
            out.mod_key.is_none(),
            "OutputConfig.mod_key should default to None (use global input.mod-key)",
        );
    }

    #[test]
    fn test_output_config_mod_key_field_can_be_set() {
        let mut out = OutputConfig {
            name: "DP-1".to_string(),
            position_set: false,
            focus_at_startup: false,
            position: Position::default(),
            width: 1920,
            height: 1080,
            scale: 1.0,
            mode: String::new(),
            vrr: false,
            primary: false,
            transform: "normal".to_string(),
            enable: true,
            layout_override: None,
            mod_key: None,
        };
        assert!(out.mod_key.is_none());
        out.mod_key = Some("super".to_string());
        assert_eq!(out.mod_key.as_deref(), Some("super"));
    }
}
