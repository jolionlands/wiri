# Config Module Documentation

## Overview

The config module (src/config/) handles loading, parsing, watching, and providing access to wiri's configuration. Configuration is in KDL format (Knuth Document Language), similar to niri.

## Module Structure

`
config/
    mod.rs              # Config struct, ConfigLoader, error types
    parse.rs            # KDL parsing with knuffel
    validate.rs         # Configuration validation
    source.rs           # Config file source locations and watching
    types/
        mod.rs          # Re-exports all config types
        input.rs        # InputConfig, keyboard, mouse, touch settings
        output.rs       # OutputConfig, monitor configuration
        layout.rs       # LayoutConfig, gaps, borders, focus ring, shadows
        workspace.rs    # WorkspaceConfig, named workspaces
        window_rule.rs  # WindowRule, WindowRuleMatch, WindowRuleProperties
        binds.rs        # BindsConfig, hotkey -> action mappings
        animation.rs    # AnimationsConfig
`

---

## Configuration File Location

### Search Path (in order)

1. %WIRI_CONFIG% environment variable
2. %APPDATA%/wiri/config.kdl
3. %USERPROFILE%/.config/wiri/config.kdl

### Fallback

If no config file is found, wiri uses built-in defaults and logs a warning.

---

## Root Config Struct

The root Config struct contains all configuration sections.

`ust
#[derive(Debug, Clone, Default)]
#[knuffel(root = true)]
pub struct Config {
    #[knuffel(child, default)]
    pub input: InputConfig,

    #[knuffel(children, name = "output")]
    pub outputs: Vec<OutputConfig>,

    #[knuffel(child, default)]
    pub layout: LayoutConfig,

    #[knuffel(children, name = "workspace")]
    pub workspaces: Vec<WorkspaceConfig>,

    #[knuffel(children, name = "window-rule")]
    pub window_rules: Vec<WindowRule>,

    #[knuffel(child, default)]
    pub binds: BindsConfig,
}
`

---

## Input Configuration

InputConfig handles keyboard, mouse, and touch settings.

### InputConfig

`ust
#[derive(Debug, Clone, Default)]
#[knuffel(child, name = "input")]
pub struct InputConfig {
    #[knuffel(child, default)]
    pub keyboard: KeyboardConfig,

    #[knuffel(child, default)]
    pub mouse: MouseConfig,

    #[knuffel(child, default)]
    pub touch: TouchConfig,

    #[knuffel(child, default)]
    pub focus: FocusConfig,
}
`

### KeyboardConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "keyboard")]
pub struct KeyboardConfig {
    #[knuffel(property = "layout", default = "us".to_string())]
    pub layout: String,

    #[knuffel(property = "options", default = String::new())]
    pub options: String,

    #[knuffel(property = "repeat-rate", default = 30)]
    pub repeat_rate: u32,

    #[knuffel(property = "repeat-delay", default = 250)]
    pub repeat_delay: u32,

    #[knuffel(property = "mod", default = ModKey::Win)]
    pub mod_key: ModKey,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: "us".to_string(),
            options: String::new(),
            repeat_rate: 30,
            repeat_delay: 250,
            mod_key: ModKey::Win,
        }
    }
}
`

### ModKey Enum

`ust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModKey {
    Alt,
    Ctrl,
    Shift,
    #[default]
    Win,
}
`

### MouseConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "mouse")]
pub struct MouseConfig {
    #[knuffel(property = "scroll-speed", default = 1.0)]
    pub scroll_speed: f64,

    #[knuffel(property = "acceleration", default = 1.0)]
    pub acceleration: f64,

    #[knuffel(property = "sensitivity", default = 1.0)]
    pub sensitivity: f64,

    #[knuffel(property = "natural-scroll", default = false)]
    pub natural_scroll: bool,

    #[knuffel(property = "left-handed", default = false)]
    pub left_handed: bool,
}
`

### TouchConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "touch")]
pub struct TouchConfig {
    #[knuffel(property = "enabled", default = true)]
    pub enabled: bool,

    #[knuffel(property = "tap-to-click", default = true)]
    pub tap_to_click: bool,

    #[knuffel(property = "scroll", default = true)]
    pub scroll: bool,

    #[knuffel(property = "acceleration", default = 1.0)]
    pub acceleration: f64,
}
`

### FocusConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "focus")]
pub struct FocusConfig {
    #[knuffel(property = "follow-mouse", default = false)]
    pub follow_mouse: bool,

    #[knuffel(property = "warp-on-focus", default = false)]
    pub warp_on_focus: bool,

    #[knuffel(property = "workspace-auto-back-and-forth", default = false)]
    pub workspace_auto_back_and_forth: bool,
}
`

---

## Output (Monitor) Configuration

OutputConfig defines per-monitor settings.

### OutputConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "output")]
pub struct OutputConfig {
    #[knuffel(argument)]
    pub name: String,

    #[knuffel(property = "position", default = None)]
    pub position: Option<Point<i32, Physical>>,

    #[knuffel(property = "resolution", default = None)]
    pub resolution: Option<Size<u32, Physical>>,

    #[knuffel(property = "refresh-rate", default = None)]
    pub refresh_rate: Option<u32>,

    #[knuffel(property = "scale", default = 1.0)]
    pub scale: f64,

    #[knuffel(property = "vrr", default = false)]
    pub vrr: bool,

    #[knuffel(property = "enable", default = true)]
    pub enable: bool,

    #[knuffel(property = "workspaces", default = None)]
    pub workspaces: Option<Vec<String>>,

    #[knuffel(property = "transform", default = 0)]
    pub transform: u8,
}
`

---

## Layout Configuration

LayoutConfig defines gaps, borders, focus ring, shadows, and animations.

### LayoutConfig

`ust
#[derive(Debug, Clone, Default)]
#[knuffel(child, name = "layout")]
pub struct LayoutConfig {
    #[knuffel(child, default)]
    pub gaps: GapsConfig,

    #[knuffel(child, default)]
    pub borders: BordersConfig,

    #[knuffel(child, default)]
    pub focus_ring: FocusRingConfig,

    #[knuffel(child, default)]
    pub shadows: ShadowsConfig,

    #[knuffel(child, default)]
    pub animations: AnimationsConfig,

    #[knuffel(property = "default-column-width", default = 0)]
    pub default_column_width: i32,

    #[knuffel(child, default)]
    pub workspace_switcher: WorkspaceSwitcherConfig,
}
`

### GapsConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "gaps")]
pub struct GapsConfig {
    #[knuffel(property = "outer", default = 10)]
    pub outer: i32,

    #[knuffel(property = "inner", default = 10)]
    pub inner: i32,

    #[knuffel(property = "top", default = None)]
    pub top: Option<i32>,

    #[knuffel(property = "bottom", default = None)]
    pub bottom: Option<i32>,

    #[knuffel(property = "left", default = None)]
    pub left: Option<i32>,

    #[knuffel(property = "right", default = None)]
    pub right: Option<i32>,
}
`

### BordersConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "borders")]
pub struct BordersConfig {
    #[knuffel(property = "enable", default = true)]
    pub enable: bool,

    #[knuffel(property = "width", default = 2)]
    pub width: i32,

    #[knuffel(property = "color", default = Color::new(0.3, 0.3, 0.3, 1.0))]
    pub color: Color,

    #[knuffel(property = "color-focused", default = Color::new(0.2, 0.5, 0.9, 1.0))]
    pub color_focused: Color,

    #[knuffel(property = "color-urgent", default = Color::new(0.9, 0.3, 0.2, 1.0))]
    pub color_urgent: Color,

    #[knuffel(property = "radius", default = 0)]
    pub radius: i32,

    #[knuffel(property = "padding", default = 0)]
    pub padding: i32,
}
`

### Color Type

`ust
#[derive(Debug, Clone, Copy, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub fn hex(hex: &str) -> Option<Self> { }
}
`

### FocusRingConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "focus-ring")]
pub struct FocusRingConfig {
    #[knuffel(property = "enable", default = true)]
    pub enable: bool,

    #[knuffel(property = "width", default = 3)]
    pub width: i32,

    #[knuffel(property = "color", default = Color::new(0.2, 0.5, 0.9, 1.0))]
    pub color: Color,

    #[knuffel(property = "gap", default = 2)]
    pub gap: i32,

    #[knuffel(property = "left", default = true)]
    pub left: bool,

    #[knuffel(property = "right", default = false)]
    pub right: bool,

    #[knuffel(property = "top", default = false)]
    pub top: bool,

    #[knuffel(property = "bottom", default = false)]
    pub bottom: bool,
}
`

### ShadowsConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "shadows")]
pub struct ShadowsConfig {
    #[knuffel(property = "enable", default = true)]
    pub enable: bool,

    #[knuffel(property = "color", default = Color::new(0.0, 0.0, 0.0, 0.5))]
    pub color: Color,

    #[knuffel(property = "offset-x", default = 0)]
    pub offset_x: i32,

    #[knuffel(property = "offset-y", default = 5)]
    pub offset_y: i32,

    #[knuffel(property = "blur", default = 20)]
    pub blur: i32,

    #[knuffel(property = "spread", default = 0)]
    pub spread: i32,
}
`

### WorkspaceSwitcherConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "workspace-switcher")]
pub struct WorkspaceSwitcherConfig {
    #[knuffel(property = "enable", default = true)]
    pub enable: bool,

    #[knuffel(property = "position", default = WorkspaceSwitcherPosition::BottomRight)]
    pub position: WorkspaceSwitcherPosition,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum WorkspaceSwitcherPosition {
    #[default]
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
    Center,
}
`

---

## Animations Configuration

### AnimationsConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "animations")]
pub struct AnimationsConfig {
    #[knuffel(property = "enable", default = true)]
    pub enable: bool,

    #[knuffel(property = "duration", default = 200)]
    pub duration_ms: u64,

    #[knuffel(property = "curve", default = AnimationCurve::EaseOutCubic)]
    pub curve: AnimationCurve,

    #[knuffel(child, default)]
    pub workspace_switch: AnimationParams,

    #[knuffel(child, default)]
    pub window_focus: AnimationParams,

    #[knuffel(child, default)]
    pub overview: AnimationParams,

    #[knuffel(child, default)]
    pub resize: AnimationParams,
}
`

### AnimationParams

`ust
#[derive(Debug, Clone)]
#[knuffel(child)]
pub struct AnimationParams {
    #[knuffel(property = "enable", default = true)]
    pub enable: bool,

    #[knuffel(property = "duration", default = None)]
    pub duration_ms: Option<u64>,

    #[knuffel(property = "curve", default = None)]
    pub curve: Option<AnimationCurve>,

    #[knuffel(property = "stiffness", default = None)]
    pub stiffness: Option<f64>,

    #[knuffel(property = "damping", default = None)]
    pub damping: Option<f64>,
}
`

### AnimationCurve

`ust
#[derive(Debug, Clone, Copy, Default)]
pub enum AnimationCurve {
    Linear,
    EaseInQuad,
    EaseOutQuad,
    #[default]
    EaseOutCubic,
    EaseInOutQuad,
    EaseInOutCubic,
    EaseOutExpo,
    EaseOutBack,
    Spring { stiffness: f64, damping: f64 },
}
`

---

## Workspace Configuration

### WorkspaceConfig

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "workspace")]
pub struct WorkspaceConfig {
    #[knuffel(argument)]
    pub name: String,

    #[knuffel(property = "layout", default = None)]
    pub layout: Option<WorkspaceLayout>,

    #[knuffel(property = "default", default = false)]
    pub default: bool,

    #[knuffel(property = "output", default = None)]
    pub output: Option<String>,

    #[knuffel(property = "gaps", default = None)]
    pub gaps: Option<GapsConfig>,

    #[knuffel(property = "borders", default = None)]
    pub borders: Option<BordersConfig>,
}
`

### WorkspaceLayout

`ust
#[derive(Debug, Clone, Copy, Default)]
pub enum WorkspaceLayout {
    #[default]
    Horizontal,
    Vertical,
    Grid,
    Single,
}
`

---

## Window Rules

WindowRule defines per-window matching conditions and properties.

### WindowRule

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "window-rule")]
pub struct WindowRule {
    #[knuffel(child)]
    pub match: WindowRuleMatch,

    #[knuffel(children)]
    pub properties: Vec<WindowRuleProperty>,
}
`

### WindowRuleMatch

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "match")]
pub struct WindowRuleMatch {
    #[knuffel(property = "class-name", default = None)]
    pub class_name: Option<Regex>,

    #[knuffel(property = "title", default = None)]
    pub title: Option<Regex>,

    #[knuffel(property = "app-id", default = None)]
    pub app_id: Option<Regex>,

    #[knuffel(property = "process-name", default = None)]
    pub process_name: Option<Regex>,

    #[knuffel(property = "is-active", default = false)]
    pub is_active: bool,

    #[knuffel(property = "is-focused", default = false)]
    pub is_focused: bool,

    #[knuffel(property = "is-floating", default = false)]
    pub is_floating: bool,

    #[knuffel(property = "is-urgent", default = false)]
    pub is_urgent: bool,

    #[knuffel(property = "at-startup", default = false)]
    pub at_startup: bool,
}
`

### WindowRuleProperty

`ust
#[derive(Debug, Clone)]
#[knuffel(property)]
pub enum WindowRuleProperty {
    DefaultWidth(i32),
    DefaultHeight(i32),
    DefaultX(i32),
    DefaultY(i32),
    OpenMaximized(bool),
    OpenFullscreen(bool),
    OpenFloating(bool),
    OpenFocused(bool),
    OpenOnWorkspace(String),
    OpenOnOutput(String),
    MinWidth(i32),
    MinHeight(i32),
    MaxWidth(i32),
    MaxHeight(i32),
    Opacity(f32),
    Borderless(bool),
    BabaIsFloat(bool),
    ScrollFactor(f64),
    VariableRefreshRate(bool),
    WorkspaceLayout(WorkspaceLayout),
}
`

---

## Key Bindings

BindsConfig defines hotkey to action mappings.

### BindsConfig

`ust
#[derive(Debug, Clone, Default)]
#[knuffel(child, name = "binds")]
pub struct BindsConfig {
    #[knuffel(children, name = "bind")]
    pub normal: Vec<Bind>,

    #[knuffel(children, name = "bind-global")]
    pub global: Vec<Bind>,

    #[knuffel(children, name = "bind-workspace-switcher")]
    pub workspace_switcher: Vec<Bind>,
}
`

### Bind

`ust
#[derive(Debug, Clone)]
#[knuffel(child, name = "bind")]
pub struct Bind {
    #[knuffel(argument)]
    pub key: KeyCombo,

    #[knuffel(child)]
    pub action: Action,
}
`

### KeyCombo

`ust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub modifiers: EnumSet<Modifiers>,
    pub key: VirtualKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(EnumSet<Modifier>);

pub enum Modifier {
    Alt,
    Ctrl,
    Shift,
    Win,
    Lock,
    Mod2,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Result<Self, ParseError>;
}
`

### Action

`ust
#[derive(Debug, Clone)]
pub enum Action {
    CloseWindow,
    MinimizeWindow,
    MaximizeWindow,
    FullscreenWindow,
    ToggleFloating,
    ToggleSticky,
    ToggleMaximize,
    FocusWindowNext,
    FocusWindowPrev,
    FocusColumnLeft,
    FocusColumnRight,
    MoveWindowUp,
    MoveWindowDown,
    MoveColumnLeft,
    MoveColumnRight,
    MoveWindowToWorkspace(i32),
    MoveWindowToNextWorkspace,
    SwitchWorkspaceNext,
    SwitchWorkspacePrev,
    SwitchToWorkspace(i32),
    CreateWorkspace,
    DeleteWorkspace,
    ToggleOverview,
    ScrollLeft,
    ScrollRight,
    InteractiveMove,
    InteractiveResize,
    Spawn(String),
    Screenshot,
    ScreenshotWindow,
    ReloadConfig,
    Restart,
    Quit,
}
`

---

## ConfigLoader

The ConfigLoader manages config file loading and live reload.

### ConfigLoader

`ust
pub struct ConfigLoader {
    config: RwLock<Config>,
    path: PathBuf,
    watcher: RecommendedWatcher,
    reload_callback: RwLock<Option<Box<dyn Fn(Config) + Send + Sync>>>,
}

impl ConfigLoader {
    pub fn new() -> Result<Self, ConfigError>;
    pub fn from_path(path: PathBuf) -> Result<Self, ConfigError>;
    pub fn load(&self) -> Result<(), ConfigError>;
    pub fn get_config(&self) -> Config;
    pub fn set_reload_callback<F>(&self, callback: F)
        where F: Fn(Config) + Send + Sync + 'static;
    pub fn watch(&self) -> Result<(), ConfigError>;
    pub fn stop_watching(&self);
}
`

---

## Error Types

### ConfigError

`ust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to find config file")]
    ConfigNotFound,

    #[error("failed to read config file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("failed to parse KDL: {0}")]
    ParseError(#[from] knuffel::Error),

    #[error("invalid regex in config: {0}")]
    RegexError(#[from] regex::Error),

    #[error("validation error: {0}")]
    ValidationError(String),

    #[error("config file changed during parsing")]
    FileChangedDuringParse,

    #[error("file watcher error: {0}")]
    WatchError(#[from] notify::Error),
}

unsafe impl Send for ConfigError {}
unsafe impl Sync for ConfigError {}
`

---

## Example KDL Configuration

`kdl
// wiri config.kdl

// Input settings
input {
    keyboard {
        layout "us"
        repeat-rate 30
        repeat-delay 250
        mod "Win"
    }
    mouse {
        scroll-speed 1.0
        acceleration 1.0
        sensitivity 1.0
        natural-scroll false
    }
    touch {
        enabled true
        tap-to-click true
    }
    focus {
        follow-mouse false
        warp-on-focus false
        workspace-auto-back-and-forth false
    }
}

// Output (monitor) configuration
output "\\\\.\\DISPLAY1" {
    position x=0 y=0
    resolution width=2560 height=1440
    refresh-rate 144
    scale 1.0
    vrr true
    enable true
}

output "\\\\.\\DISPLAY2" {
    position x=2560 y=0
    resolution width=1920 height=1080
    scale 1.0
    enable true
}

// Layout settings
layout {
    gaps {
        outer 10
        inner 10
    }
    borders {
        enable true
        width 2
        color "#4d4d4d"
        color-focused "#3381d9"
        color-urgent "#e54d34"
    }
    focus-ring {
        enable true
        width 3
        color "#3381d9"
        gap 2
        left true
    }
    shadows {
        enable true
        color "#00000080"
        offset-x 0
        offset-y 5
        blur 20
        spread 0
    }
    animations {
        enable true
        duration 200
        curve ease-out-cubic
    }
}

// Workspace configuration
workspace "1" {
    default true
    layout "horizontal"
}

workspace "2" {
    layout "horizontal"
}

workspace "dev" {
    layout "vertical"
    output "\\\\.\\DISPLAY2"
}

// Window rules
window-rule {
    match class-name="^Firefox$"
    open-maximized true
    opacity 0.95
}

window-rule {
    match class-name="^chrome$"
    title="^Save As.*"
    baba_is_float true
}

window-rule {
    match app-id="^explorer.exe$"
    borderless true
}

window-rule {
    match is-floating=true
    opacity 0.9
}

// Key bindings
binds {
    bind "Mod+Shift+KeyQ" { action Quit }
    bind "Mod+KeyReturn" { action Spawn "wt" }
    bind "Mod+KeyJ" { action FocusColumnLeft }
    bind "Mod+KeyK" { action FocusColumnRight }
    bind "Mod+Shift+KeyJ" { action MoveColumnLeft }
    bind "Mod+Shift+KeyK" { action MoveColumnRight }
    bind "Mod+KeyH" { action ScrollLeft }
    bind "Mod+KeyL" { action ScrollRight }
    bind "Mod+KeyF" { action FullscreenWindow }
    bind "Mod+KeyT" { action ToggleFloating }
    bind "Mod+KeyM" { action MinimizeWindow }
    bind "Mod+Shift+KeyC" { action CloseWindow }
    bind "Mod+Key1" { action SwitchToWorkspace 1 }
    bind "Mod+Key2" { action SwitchToWorkspace 2 }
    bind "Mod+Key3" { action SwitchToWorkspace 3 }
    bind "Mod+Shift+Key1" { action MoveWindowToWorkspace 1 }
    bind "Mod+Shift+Key2" { action MoveWindowToWorkspace 2 }
    bind "Mod+KeySpace" { action InteractiveMove }
    bind "Mod+Shift+KeyR" { action InteractiveResize }
    bind "Mod+KeyR" { action ReloadConfig }
    bind-global "Mod+Shift+KeyS" { action Screenshot }
    bind-global "Mod+Shift+KeyA" { action Spawn "action-center" }
}
`

---

## File Watching for Live Reload

### FileWatcher

`ust
pub struct FileWatcher {
    watcher: RecommendedWatcher,
    path: PathBuf,
}

impl FileWatcher {
    pub fn new<F>(path: PathBuf, callback: F) -> Result<Self, ConfigError>
        where F: Fn() + Send + Sync + 'static;

    pub fn watch(&self) -> Result<(), ConfigError>;
    pub fn stop(&self);
}
`

### Live Reload Flow

1. ConfigLoader initializes file watcher on config file path
2. When file changes are detected, watcher calls reload callback
3. Callback acquires write lock on config, re-parses file
4. New config replaces old config atomically
5. Other modules receive notification via callback system

### Debouncing

`ust
const RELOAD_DEBOUNCE_MS: u64 = 100;

impl ConfigLoader {
    fn handle_file_change(&self) {
        static LAST_CHANGE: AtomicU64 = AtomicU64::new(0);
        let now = std::time::Instant::now().elapsed().as_millis() as u64;
        if now - LAST_CHANGE.load(Ordering::SeqCst) < RELOAD_DEBOUNCE_MS {
            return;
        }
        LAST_CHANGE.store(now, Ordering::SeqCst);
        self.load().ok();
    }
}
`

---

## Configuration Validation

### Validation Rules

`ust
pub fn validate_config(config: &Config) -> Result<(), ConfigError> {
    for rule in &config.window_rules {
        if let Some(re) = &rule.match.class_name {
            regex::Regex::new(re)?;
        }
    }
    let mut seen_keys: HashSet<KeyCombo> = HashSet::new();
    for bind in &config.binds.normal {
        if !seen_keys.insert(bind.key.clone()) {
            return Err(ConfigError::ValidationError(format!(
                "Duplicate key binding: {:?}", bind.key
            )));
        }
    }
    let mut workspace_names: HashSet<&str> = HashSet::new();
    for ws in &config.workspaces {
        if !workspace_names.insert(&ws.name) {
            return Err(ConfigError::ValidationError(format!(
                "Duplicate workspace name: {}", ws.name
            )));
        }
    }
    let output_names: HashSet<&str> = config.outputs.iter().map(|o| o.name.as_str()).collect();
    for ws in &config.workspaces {
        if let Some(ref output) = ws.output {
            if !output_names.contains(output.as_str()) {
                return Err(ConfigError::ValidationError(format!(
                    "Workspace {} references undefined output: {}",
                    ws.name, output
                )));
            }
        }
    }
    Ok(())
}
`

---

## Integration Points

### With Layout Module

- Layout receives LayoutConfig on initialization
- Layout gaps and borders apply from config
- Focus ring rendering uses FocusRingConfig
- Shadow rendering uses ShadowsConfig

### With Input Module

- InputManager receives InputConfig on initialization
- Keyboard repeat settings applied via Win32 API
- Mouse sensitivity applied to input processing
- Global hotkeys use BindsConfig

### With Window Module

- Window rules resolved against WindowRule list
- Per-window ResolvedWindowRules computed from config
- Border opacity from window rules applied

### With Backend Module

- Output configurations applied via LayoutRequest::ConfigureOutput
- Monitor enumeration matched against OutputConfig list

---

## Implementation Notes

### Regex Matching

`ust
fn matches_rule(window: &WindowHandle, rule: &WindowRule) -> bool {
    let m = &rule.match;
    if let Some(ref re) = m.class_name {
        if !re.is_match(&window.class_name()) { return false; }
    }
    if let Some(ref re) = m.title {
        if !re.is_match(&window.title()) { return false; }
    }
    if let Some(ref re) = m.app_id {
        if let Some(app_id) = window.app_id() {
            if !re.is_match(&app_id) { return false; }
        } else { return false; }
    }
    true
}
`

### Thread Safety

- ConfigLoader uses RwLock<Config> for interior mutability
- Config is read-only after initial load; replaced on reload
- Callbacks are Box<dyn Fn(Config) + Send + Sync>

### Performance Considerations

- Config parsing happens on file change, not per-frame
- Regex patterns compiled once and cached in WindowRuleMatch
- Use Cow<str> to avoid unnecessary allocations where possible