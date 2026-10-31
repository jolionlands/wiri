//! Maps a parsed KDL document onto [`Config`] and says, with `line:column`, everything it
//! does not understand.
//!
//! The rules (niri's, softened where wiri's older format needs it):
//!
//! * a syntax error, an unknown section or key, a value of the wrong type or outside its
//!   range, an unknown bind action, a bad regular expression and a missing include are
//!   **errors**: the file does not load (on a reload the last good config stays);
//! * a known niri option or trigger without a Windows equivalent is a warning;
//!   unknown modifiers and duplicate chords within one config part are errors;
//! * included config parts override earlier chords using the final Mod mapping;
//! * the old wiri spellings (flat keys under `layout`, `Ctrl+Alt+Left focus-column-left`
//!   binds, `window-rule { class-name "x" }`, ...) keep working next to niri's.

use std::fmt;
use std::path::{Path, PathBuf};

use super::effects::{EffectsPartial, PaintStyle, RingStyle, ShadowStyle};
use super::kdl::{self, Node, Pos, Scalar, Value};
use super::types::*;

// The appearance blocks of `layout {}`: focus ring, border, shadow, insert hint, tab indicator.
#[path = "decode/animations_decode.rs"]
mod animations_decode;
mod effects_decode;
mod gestures_decode;
#[path = "decode/hot_corner_decode.rs"]
mod hot_corner_decode;
#[path = "decode/overview_decode.rs"]
mod overview_decode;
#[path = "decode/recent_windows_decode.rs"]
mod recent_windows_decode;
mod rule_effects_decode;
mod rule_policy;
#[cfg(test)]
mod spacing_parity_tests;

/// How many levels of `include` are followed.
pub const MAX_INCLUDE_DEPTH: u8 = 8;

/// Severity of a config issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationSeverity {
    /// The config is rejected.
    Error,
    /// The config loads; something in it is ignored.
    Warning,
}

impl fmt::Display for ValidationSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationSeverity::Error => write!(f, "error"),
            ValidationSeverity::Warning => write!(f, "warning"),
        }
    }
}

/// One finding: where it is and what is wrong.  `line` and `column` are 1-based (columns
/// count characters); `file` is empty for the text that was handed in, or the path of an
/// included file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub severity: ValidationSeverity,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub file: String,
    pub context: super::diagnostics::DiagnosticContext,
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}",
            self.line, self.column, self.severity, self.message
        )
    }
}

impl ValidationIssue {
    /// `file:line:column: severity: message`, the file being the issue's own or `default_file`.
    pub fn located(&self, default_file: &str) -> String {
        let file = if self.file.is_empty() {
            default_file
        } else {
            self.file.as_str()
        };
        if file.is_empty() {
            self.to_string()
        } else {
            format!("{}:{}", file, self)
        }
    }
}

/// The outcome of decoding a config: the config (defaults where the text said nothing or
/// was wrong), every issue found, and the files that were included.
#[derive(Debug, Clone)]
pub struct Decoded {
    pub config: Config,
    pub issues: Vec<ValidationIssue>,
    pub includes: Vec<PathBuf>,
}

impl Decoded {
    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|i| i.severity == ValidationSeverity::Error)
    }

    pub fn first_error(&self) -> Option<&ValidationIssue> {
        self.issues
            .iter()
            .find(|i| i.severity == ValidationSeverity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Warning)
            .count()
    }
}

/// Decode the text of a config.  `file` names it in the issues (may be empty); `base_dir`
/// is what `include "x.kdl"` is resolved against (`None`: includes are skipped with a warning).
pub fn decode_text(text: &str, file: &str, base_dir: Option<&Path>) -> Decoded {
    run(text, file, base_dir, None)
}

/// Read and decode a config file (and the files it includes).
pub fn decode_file(path: &Path) -> Decoded {
    let label = path.display().to_string();
    match read_config_text(path) {
        Ok(text) => {
            let base = path.parent().map(|p| p.to_path_buf());
            let canon = std::fs::canonicalize(path).ok();
            run(&text, &label, base.as_deref(), canon)
        }
        Err(e) => Decoded {
            config: Config::default(),
            issues: vec![ValidationIssue {
                severity: ValidationSeverity::Error,
                line: 1,
                column: 1,
                message: format!("cannot read the config file: {}", e),
                file: label,
                context: Default::default(),
            }],
            includes: Vec::new(),
        },
    }
}

fn run(text: &str, file: &str, base_dir: Option<&Path>, root: Option<PathBuf>) -> Decoded {
    let mut decoder = Decoder {
        issues: Vec::new(),
        includes: Vec::new(),
        file: file.to_string(),
        base_dir: base_dir.map(|p| p.to_path_buf()),
        stack: Vec::new(),
        depth: 0,
        source: String::new(),
        include_trace: Vec::new(),
        bind_groups: Vec::new(),
    };
    if let Some(root) = root {
        decoder.stack.push(root);
    }
    let mut config = Config::default();
    // Outputs come only from the text; the single default entry returns when it has none.
    config.output.clear();
    decoder.decode_source(text, &mut config);
    decoder.finish_binds(&mut config);
    if config.output.is_empty() {
        config.output = Config::default().output;
    }
    Decoded {
        config,
        issues: decoder.issues,
        includes: decoder.includes,
    }
}

/// The text of a config file: UTF-8, with or without a byte-order mark, or UTF-16 with one
/// (what Windows PowerShell's `Out-File` writes).
pub fn read_config_text(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    decode_config_bytes(&bytes).map_err(|m| std::io::Error::new(std::io::ErrorKind::InvalidData, m))
}

/// See [`read_config_text`].
pub fn decode_config_bytes(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return utf8(&bytes[3..]);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let little = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| {
                if little {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        return String::from_utf16(&units).map_err(|_| "the file is not valid UTF-16".to_string());
    }
    utf8(bytes)
}

fn utf8(bytes: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => Err(format!(
            "the file is not valid UTF-8 (first bad byte at offset {}); save it as UTF-8",
            e.valid_up_to()
        )),
    }
}

// ---------------------------------------------------------------------------------------
// names
// ---------------------------------------------------------------------------------------

/// A key as it is matched: lower case, `_` and `-` interchangeable.
fn norm(name: &str) -> String {
    name.replace('_', "-").to_ascii_lowercase()
}

/// The older flat `border-width` / `focus-ring-width` integers mirror a block's LOGICAL width:
/// a non-zero width is at least one pixel, as in niri's `round_logical_in_physical_max1`
/// (utils/mod.rs:185-191) at scale 1.  The block's own width stays fractional.
fn legacy_width_px(width: f64) -> u32 {
    if width > 0.0 {
        width.round().max(1.0) as u32
    } else {
        0
    }
}

/// The `border` block being built by a `layout {}` (started from its defaults when nothing
/// named it yet).
fn border_style(p: &mut LayoutConfigPartial) -> &mut RingStyle {
    p.effects
        .border
        .get_or_insert_with(RingStyle::border_default)
}

/// The `focus-ring` block being built by a `layout {}`.
fn focus_ring_style(p: &mut LayoutConfigPartial) -> &mut RingStyle {
    p.effects
        .focus_ring
        .get_or_insert_with(RingStyle::focus_ring_default)
}

/// The `shadow` block being built by a `layout {}`.
fn shadow_style(p: &mut LayoutConfigPartial) -> &mut ShadowStyle {
    p.effects.shadow.get_or_insert_with(ShadowStyle::default)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The known name closest to `name`, when it is close enough to be a typo.
fn suggestion(name: &str, known: &[&str]) -> Option<String> {
    let limit = if name.chars().count() <= 4 { 1 } else { 2 };
    let mut best: Option<(usize, &str)> = None;
    for k in known {
        let d = edit_distance(name, k);
        if d <= limit && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, k));
        }
    }
    best.map(|(_, k)| k.to_string())
}

const TOP_LEVEL: &[&str] = &[
    "version",
    "input",
    "output",
    "layout",
    "workspace",
    "window-rule",
    "layer-rule",
    "binds",
    "recent-windows",
    "animations",
    "gestures",
    "environment",
    "switch-events",
    "hotkey-overlay",
    "cursor",
    "overview",
    "spawn-at-startup",
    "spawn-sh-at-startup",
    "accessibility",
    "include",
];

/// niri sections that have no meaning for wiri (yet), with why.
fn top_level_unsupported(name: &str) -> Option<&'static str> {
    match name {
        "clipboard" => Some("Windows has no primary selection"),
        "config-notification" => Some("wiri always shows a balloon for a config error"),
        "prefer-no-csd" => Some("Windows applications draw their own title bars"),
        "blur" => Some("no compositor blur on Windows"),
        "xwayland-satellite" => Some("there is no Xwayland on Windows"),
        _ => None,
    }
}

/// Every action niri can bind (`niri-config/src/binds.rs`), in kebab-case.
pub const NIRI_ACTIONS: &[&str] = &[
    "quit",
    "suspend",
    "power-off-monitors",
    "power-on-monitors",
    "toggle-debug-tint",
    "debug-toggle-opaque-regions",
    "debug-toggle-damage",
    "toggle-touchpad",
    "spawn",
    "spawn-sh",
    "do-screen-transition",
    "screenshot",
    "screenshot-screen",
    "screenshot-window",
    "toggle-keyboard-shortcuts-inhibit",
    "close-window",
    "fullscreen-window",
    "toggle-windowed-fullscreen",
    "focus-window-in-column",
    "focus-window-previous",
    "focus-column-left",
    "focus-column-right",
    "focus-column-first",
    "focus-column-last",
    "focus-column-right-or-first",
    "focus-column-left-or-last",
    "focus-column",
    "focus-window-or-monitor-up",
    "focus-window-or-monitor-down",
    "focus-column-or-monitor-left",
    "focus-column-or-monitor-right",
    "focus-window-down",
    "focus-window-up",
    "focus-window-down-or-column-left",
    "focus-window-down-or-column-right",
    "focus-window-up-or-column-left",
    "focus-window-up-or-column-right",
    "focus-window-or-workspace-down",
    "focus-window-or-workspace-up",
    "focus-window-top",
    "focus-window-bottom",
    "focus-window-down-or-top",
    "focus-window-up-or-bottom",
    "move-column-left",
    "move-column-right",
    "move-column-to-first",
    "move-column-to-last",
    "move-column-left-or-to-monitor-left",
    "move-column-right-or-to-monitor-right",
    "move-column-to-index",
    "move-window-down",
    "move-window-up",
    "move-window-down-or-to-workspace-down",
    "move-window-up-or-to-workspace-up",
    "consume-or-expel-window-left",
    "consume-or-expel-window-right",
    "consume-window-into-column",
    "expel-window-from-column",
    "swap-window-left",
    "swap-window-right",
    "toggle-column-tabbed-display",
    "set-column-display",
    "center-column",
    "center-window",
    "center-visible-columns",
    "focus-workspace-down",
    "focus-workspace-up",
    "focus-workspace",
    "focus-workspace-previous",
    "move-window-to-workspace-down",
    "move-window-to-workspace-up",
    "move-window-to-workspace",
    "move-column-to-workspace-down",
    "move-column-to-workspace-up",
    "move-column-to-workspace",
    "move-workspace-down",
    "move-workspace-up",
    "move-workspace-to-index",
    "move-workspace-to-monitor",
    "set-workspace-name",
    "unset-workspace-name",
    "focus-monitor-left",
    "focus-monitor-right",
    "focus-monitor-down",
    "focus-monitor-up",
    "focus-monitor-previous",
    "focus-monitor-next",
    "focus-monitor",
    "move-window-to-monitor-left",
    "move-window-to-monitor-right",
    "move-window-to-monitor-down",
    "move-window-to-monitor-up",
    "move-window-to-monitor-previous",
    "move-window-to-monitor-next",
    "move-window-to-monitor",
    "move-column-to-monitor-left",
    "move-column-to-monitor-right",
    "move-column-to-monitor-down",
    "move-column-to-monitor-up",
    "move-column-to-monitor-previous",
    "move-column-to-monitor-next",
    "move-column-to-monitor",
    "set-window-width",
    "set-window-height",
    "reset-window-height",
    "switch-preset-column-width",
    "switch-preset-column-width-back",
    "switch-preset-window-width",
    "switch-preset-window-width-back",
    "switch-preset-window-height",
    "switch-preset-window-height-back",
    "maximize-column",
    "maximize-window-to-edges",
    "set-column-width",
    "expand-column-to-available-width",
    "switch-layout",
    "show-hotkey-overlay",
    "move-workspace-to-monitor-left",
    "move-workspace-to-monitor-right",
    "move-workspace-to-monitor-down",
    "move-workspace-to-monitor-up",
    "move-workspace-to-monitor-previous",
    "move-workspace-to-monitor-next",
    "toggle-window-floating",
    "move-window-to-floating",
    "move-window-to-tiling",
    "focus-floating",
    "focus-tiling",
    "switch-focus-between-floating-and-tiling",
    "toggle-window-rule-opacity",
    "set-dynamic-cast-window",
    "set-dynamic-cast-monitor",
    "clear-dynamic-cast-target",
    "toggle-overview",
    "open-overview",
    "close-overview",
];

/// niri actions wiri deliberately does not implement, with why.  Every one of
/// them is a compositor effect or a Wayland protocol wiri has no equivalent of,
/// so the decoder reports the bind as `not implemented by wiri yet` instead of
/// calling it an unknown action.  This list is the only place the gate list is
/// narrowed: [`bindable_niri_actions`] is [`NIRI_ACTIONS`] minus these, and the
/// unit tests below keep the two halves an exact partition, so a new niri
/// action cannot land outside the live completeness gate.
pub const UNIMPLEMENTED_NIRI_ACTIONS: &[&str] = &[
    // Compositor debug switches: there is no damage tracking or debug tint to
    // toggle on Windows.
    "toggle-debug-tint",
    "debug-toggle-opaque-regions",
    "debug-toggle-damage",
    // niri's own screen-transition effect: wiri animates with its own driver.
    "do-screen-transition",
    // niri's windowed-fullscreen mode: Windows fullscreen is `fullscreen-window`.
    "toggle-windowed-fullscreen",
    // wlr-screencopy dynamic cast targets: there is no cast protocol on Windows.
    "set-dynamic-cast-window",
    "set-dynamic-cast-monitor",
    "clear-dynamic-cast-target",
];

/// Every niri action name wiri accepts in a bind, in the order [`NIRI_ACTIONS`]
/// declares them.  This is the one list the live completeness gates enumerate
/// (`scripts/live-phases/lane-c2-binds.ps1`, `h79_*`), so a new action added to
/// the vocabulary is bound and run by the next cycle without anyone editing a
/// script.
pub fn bindable_niri_actions() -> Vec<&'static str> {
    NIRI_ACTIONS
        .iter()
        .copied()
        .filter(|name| !UNIMPLEMENTED_NIRI_ACTIONS.contains(name))
        .collect()
}

/// Properties niri allows on an action itself, per action.
fn action_props(action: &str) -> &'static [&'static str] {
    match action {
        "quit" => &["skip-confirmation"],
        "do-screen-transition" => &["delay-ms"],
        "screenshot" => &["show-pointer"],
        "screenshot-screen" | "screenshot-window" => &["write-to-disk", "show-pointer"],
        "move-window-to-workspace-down"
        | "move-window-to-workspace-up"
        | "move-window-to-workspace"
        | "move-column-to-workspace-down"
        | "move-column-to-workspace-up"
        | "move-column-to-workspace"
        | "move-column-up"
        | "move-column-down"
        | "move-column-to-workspace-above"
        | "move-column-to-workspace-below" => &["focus"],
        "focus-window-previous" | "focus-previous" | "alt-tab" | "focus-history-back" => {
            &["debounce-ms"]
        }
        "move-workspace-to-index"
        | "set-workspace-name"
        | "rename-workspace"
        | "rename-ws"
        | "unset-workspace-name"
        | "move-workspace-to-monitor"
        | "move-workspace-to-monitor-left"
        | "move-workspace-to-monitor-right"
        | "move-workspace-to-monitor-up"
        | "move-workspace-to-monitor-down"
        | "move-workspace-to-monitor-previous"
        | "move-workspace-to-monitor-next" => &["workspace"],
        _ => &[],
    }
}

/// A color as written: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `accent`, `windows-accent`
/// or empty.  Returns the text wiri stores (`#rrggbb[aa]`), or `None` when it is not a color.
/// `accent` / `windows-accent`, which only the border colours resolve (they read the live system
/// accent); a backdrop needs a real colour.
fn is_accent_alias(color: &str) -> bool {
    let text = color.trim().to_ascii_lowercase();
    text == "accent" || text == "windows-accent" || text == "system-accent"
}

fn normalize_color(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t == "accent" || t == "windows-accent" {
        return Some(t.to_string());
    }
    // `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, CSS names, `rgb()`, `rgba()`, `hsl()`, `hsla()`.
    super::effects::parse_css_color(t).map(super::effects::color_text)
}

fn expand_user_path(raw: &str) -> String {
    let mut s = raw.to_string();
    if s == "~" || s.starts_with("~/") || s.starts_with("~\\") {
        if let Ok(home) = std::env::var("USERPROFILE") {
            s = format!("{}{}", home, &s[1..]);
        }
    }
    // %NAME% environment references.
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------
// the decoder
// ---------------------------------------------------------------------------------------

struct LocatedBind {
    bind: HotkeyBinding,
    label: super::diagnostics::SourceLabel,
    includes: Vec<super::diagnostics::SourceLabel>,
}

struct Decoder {
    issues: Vec<ValidationIssue>,
    includes: Vec<PathBuf>,
    /// Label of the file being decoded, for the issues ("" for the text passed in).
    file: String,
    /// What `include "x"` is relative to.
    base_dir: Option<PathBuf>,
    /// Canonical paths of the files being decoded, outermost first (cycle detection).
    stack: Vec<PathBuf>,
    depth: u8,
    source: String,
    include_trace: Vec<super::diagnostics::SourceLabel>,
    bind_groups: Vec<Vec<LocatedBind>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputScope {
    Input,
    Keyboard,
    Mouse,
    /// Inside `input { touchpad { ... } }`.
    Touchpad,
    /// Inside `input { trackpoint { ... } }`.
    Trackpoint,
    /// Inside `input { trackball { ... } }`.
    Trackball,
    Focus,
    Touch,
}

impl InputScope {
    /// The per-device-type scopes that take niri's pointing-device options: `mouse`,
    /// `touchpad`, `trackpoint` and `trackball`.  Windows configures all of them with one
    /// system-wide pointer setup, so they share one set of stored fields.
    fn is_pointer(self) -> bool {
        matches!(
            self,
            Self::Mouse | Self::Touchpad | Self::Trackpoint | Self::Trackball
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Input => "'input'",
            Self::Keyboard => "'input' > 'keyboard'",
            Self::Mouse => "'input' > 'mouse'",
            Self::Touchpad => "'input' > 'touchpad'",
            Self::Trackpoint => "'input' > 'trackpoint'",
            Self::Trackball => "'input' > 'trackball'",
            Self::Focus => "'input' > 'focus'",
            Self::Touch => "'input' > 'touch'",
        }
    }

    /// Which options the section takes, for the "did you mean" hint of an unknown key.
    fn known(self) -> &'static [&'static str] {
        match self {
            Self::Input => &[
                "keyboard",
                "mouse",
                "touchpad",
                "trackpoint",
                "trackball",
                "tablet",
                "focus",
                "touch",
                "focus-follows-mouse",
                "warp-mouse-to-focus",
                "workspace-auto-back-and-forth",
                "mod-key",
                "layout",
                "repeat-delay",
                "repeat-rate",
                "numlock",
            ],
            Self::Keyboard => &[
                "layout",
                "repeat-delay",
                "repeat-rate",
                "numlock",
                "xkb",
                "track-layout",
                "mod-key",
            ],
            Self::Mouse => &[
                "off",
                "natural-scroll",
                "scroll-speed",
                "acceleration",
                "sensitivity",
                "tap-to-click",
                "scroll-factor",
                "accel-profile",
                "scroll-method",
                "scroll-button",
                "scroll-button-lock",
                "left-handed",
                "middle-emulation",
            ],
            Self::Touchpad => &[
                "off",
                "tap",
                "dwt",
                "dwtp",
                "drag",
                "drag-lock",
                "tap-button-map",
                "click-method",
                "disabled-on-external-mouse",
                "natural-scroll",
                "scroll-factor",
                "accel-profile",
                "scroll-method",
                "scroll-button",
                "scroll-button-lock",
                "left-handed",
                "middle-emulation",
            ],
            Self::Trackpoint | Self::Trackball => &[
                "off",
                "natural-scroll",
                "accel-profile",
                "scroll-method",
                "scroll-button",
                "scroll-button-lock",
                "left-handed",
                "middle-emulation",
            ],
            Self::Focus => &["follows-mouse", "focus-delay-ms", "warp-on-focus"],
            Self::Touch => &[
                "enable",
                "off",
                "tap-to-click",
                "scroll-speed",
                "map-to-output",
            ],
        }
    }
}

impl Decoder {
    fn push(&mut self, severity: ValidationSeverity, pos: Pos, message: String) {
        self.issues.push(ValidationIssue {
            severity,
            line: pos.line as usize,
            column: pos.col as usize,
            message,
            file: self.file.clone(),
            context: super::diagnostics::DiagnosticContext {
                source_line: self
                    .source
                    .lines()
                    .nth(pos.line.saturating_sub(1) as usize)
                    .unwrap_or("")
                    .to_string(),
                includes: self.include_trace.clone(),
                ..Default::default()
            },
        });
    }

    fn error(&mut self, pos: Pos, message: impl Into<String>) {
        self.push(ValidationSeverity::Error, pos, message.into());
    }

    fn warn(&mut self, pos: Pos, message: impl Into<String>) {
        self.push(ValidationSeverity::Warning, pos, message.into());
    }

    fn unsupported(&mut self, node: &Node, why: &str) {
        let tail = if why.is_empty() {
            String::new()
        } else {
            format!(" ({})", why)
        };
        self.warn(
            node.pos,
            format!("'{}' is not supported by wiri{}; ignored", node.name, tail),
        );
    }

    fn unknown(&mut self, node: &Node, parent: &str, known: &[&str]) {
        let hint = suggestion(&norm(&node.name), known);
        let message = match hint {
            Some(h) => format!(
                "unknown option '{}' in {} (did you mean '{}'?)",
                node.name, parent, h
            ),
            None => format!("unknown option '{}' in {}", node.name, parent),
        };
        self.error(node.pos, message);
    }

    // ---- argument helpers ------------------------------------------------------------

    fn no_props(&mut self, node: &Node) {
        for p in &node.props {
            self.error(
                p.key_pos,
                format!("'{}' takes no properties; found '{}'", node.name, p.key),
            );
        }
    }

    fn no_args(&mut self, node: &Node) {
        if let Some(a) = node.args.first() {
            self.error(a.pos, format!("'{}' takes no arguments", node.name));
        }
    }

    fn no_children(&mut self, node: &Node) {
        if let Some(first) = node.kids().first() {
            self.error(first.pos, format!("'{}' takes no children", node.name));
        }
    }

    /// A section node: no arguments, no properties.
    fn section(&mut self, node: &Node) {
        self.no_args(node);
        self.no_props(node);
    }

    fn one_arg<'a>(&mut self, node: &'a Node) -> Option<&'a Value> {
        self.no_props(node);
        self.no_children(node);
        match node.args.len() {
            0 => {
                self.error(node.pos, format!("'{}' needs a value", node.name));
                None
            }
            1 => Some(&node.args[0]),
            _ => {
                self.error(
                    node.args[1].pos,
                    format!("'{}' takes one value, not {}", node.name, node.args.len()),
                );
                None
            }
        }
    }

    fn value_bool(&mut self, node: &Node, v: &Value) -> Option<bool> {
        match &v.scalar {
            Scalar::Bool(b) => Some(*b),
            Scalar::Int(0) => Some(false),
            Scalar::Int(1) => Some(true),
            Scalar::Str { text, .. } => match text.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Some(true),
                "false" | "no" | "off" | "0" => Some(false),
                _ => {
                    self.error(
                        v.pos,
                        format!("'{}' expects true or false, found \"{}\"", node.name, text),
                    );
                    None
                }
            },
            _ => {
                self.error(
                    v.pos,
                    format!(
                        "'{}' expects true or false, found a {}",
                        node.name,
                        v.kind()
                    ),
                );
                None
            }
        }
    }

    /// A niri flag: the bare node is on, `false` turns it off.
    fn flag(&mut self, node: &Node) -> Option<bool> {
        self.no_props(node);
        self.no_children(node);
        match node.args.len() {
            0 => Some(true),
            1 => self.value_bool(node, &node.args[0]),
            _ => {
                self.error(
                    node.args[1].pos,
                    format!("'{}' takes at most one value", node.name),
                );
                None
            }
        }
    }

    fn value_number(&mut self, node: &Node, v: &Value, min: f64, max: f64) -> Option<f64> {
        let n = match &v.scalar {
            Scalar::Int(i) => *i as f64,
            Scalar::Float(f) => *f,
            Scalar::Str { text, .. } => match text.trim().parse::<f64>() {
                Ok(f) => f,
                Err(_) => {
                    self.error(
                        v.pos,
                        format!("'{}' expects a number, found \"{}\"", node.name, text),
                    );
                    return None;
                }
            },
            _ => {
                self.error(
                    v.pos,
                    format!("'{}' expects a number, found a {}", node.name, v.kind()),
                );
                return None;
            }
        };
        if !n.is_finite() || n < min || n > max {
            self.error(
                v.pos,
                format!("'{}' must be between {} and {}", node.name, min, max),
            );
            return None;
        }
        Some(n)
    }

    fn arg_number(&mut self, node: &Node, min: f64, max: f64) -> Option<f64> {
        let v = self.one_arg(node)?;
        self.value_number(node, v, min, max)
    }

    fn arg_u32(&mut self, node: &Node, max: u32) -> Option<u32> {
        let n = self.arg_number(node, 0.0, max as f64)?;
        if n.fract() != 0.0 {
            self.error(
                node.args[0].pos,
                format!("'{}' expects an integer", node.name),
            );
            return None;
        }
        Some(n as u32)
    }

    /// An integer-only value, with a range that may be wider than `u32` (niri's `FloatOrInt`
    /// bounds are i32-typed).  A decimal is a type error, matching knuffel's integer scalars.
    fn arg_integer(&mut self, node: &Node, min: f64, max: f64) -> Option<f64> {
        let v = self.one_arg(node)?;
        let n = self.value_number(node, v, min, max)?;
        if n.fract() != 0.0 {
            self.error(v.pos, format!("'{}' expects an integer", node.name));
            return None;
        }
        Some(n)
    }

    /// A niri `i32` value.  Pinned niri decodes `output { position }` x/y as plain `i32`
    /// properties (niri-config/src/output.rs:139-152), so a fractional value is a type error
    /// rather than something to round, and the whole i32 range is accepted.
    fn value_i32(&mut self, node: &Node, v: &Value) -> Option<i32> {
        let n = self.value_number(node, v, i32::MIN as f64, i32::MAX as f64)?;
        if n.fract() != 0.0 {
            self.error(
                v.pos,
                format!("'{}' expects an integer, found {}", node.name, n),
            );
            return None;
        }
        Some(n as i32)
    }

    fn arg_i32(&mut self, node: &Node) -> Option<i32> {
        let v = self.one_arg(node)?;
        self.value_i32(node, v)
    }

    fn arg_text(&mut self, node: &Node) -> Option<String> {
        let v = self.one_arg(node)?;
        self.value_text(node, v)
    }

    fn value_text(&mut self, node: &Node, v: &Value) -> Option<String> {
        match &v.scalar {
            Scalar::Str { text, .. } => Some(text.clone()),
            _ => {
                self.error(
                    v.pos,
                    format!(
                        "'{}' expects text (in quotes), found a {}",
                        node.name,
                        v.kind()
                    ),
                );
                None
            }
        }
    }

    fn value_color(&mut self, node: &Node, v: &Value) -> Option<String> {
        let text = self.value_text(node, v)?;
        match normalize_color(&text) {
            Some(c) => Some(c),
            None => {
                self.error(
                    v.pos,
                    format!(
                        "'{}': \"{}\" is not a color (use #rrggbb or #rrggbbaa)",
                        node.name, text
                    ),
                );
                None
            }
        }
    }

    fn arg_color(&mut self, node: &Node) -> Option<String> {
        let v = self.one_arg(node)?;
        self.value_color(node, v)
    }

    fn set_u32(&mut self, node: &Node, max: u32, slot: &mut Option<u32>) {
        if let Some(v) = self.arg_u32(node, max) {
            *slot = Some(v);
        }
    }

    fn set_i32(&mut self, node: &Node, slot: &mut Option<i32>) {
        if let Some(v) = self.arg_i32(node) {
            *slot = Some(v);
        }
    }

    fn set_f64(&mut self, node: &Node, min: f64, max: f64, slot: &mut Option<f64>) {
        if let Some(v) = self.arg_number(node, min, max) {
            *slot = Some(v);
        }
    }

    fn set_flag(&mut self, node: &Node, slot: &mut Option<bool>) {
        if let Some(v) = self.flag(node) {
            *slot = Some(v);
        }
    }

    fn set_text(&mut self, node: &Node, slot: &mut Option<String>) {
        if let Some(v) = self.arg_text(node) {
            *slot = Some(v);
        }
    }

    fn set_color(&mut self, node: &Node, slot: &mut Option<String>) {
        if let Some(v) = self.arg_color(node) {
            *slot = Some(v);
        }
    }

    // ---- documents and includes ------------------------------------------------------

    fn decode_source(&mut self, text: &str, cfg: &mut Config) {
        let saved = std::mem::replace(&mut self.source, text.to_string());
        match kdl::parse(text) {
            Ok(doc) => {
                let mut structural = super::validation::check(&doc.nodes, &self.file, text);
                for issue in &mut structural {
                    issue.context.includes = self.include_trace.clone();
                }
                self.issues.extend(structural);
                self.decode_nodes(&doc.nodes, cfg);
            }
            Err(e) => self.error(e.pos, e.message),
        }
        self.source = saved;
    }

    fn decode_nodes(&mut self, nodes: &[Node], cfg: &mut Config) {
        for node in nodes {
            let key = norm(&node.name);
            match key.as_str() {
                "version" => {}
                "config-notification" => {
                    self.section(node);
                    for child in node.kids() {
                        if norm(&child.name) == "disable-failed" {
                            if let Some(v) = self.flag(child) {
                                cfg.disable_failed_config_notification = v;
                            }
                        } else {
                            self.unknown(child, "'config-notification'", &["disable-failed"]);
                        }
                    }
                }
                "input" => self.decode_input(node, cfg),
                "output" => self.decode_output(node, cfg),
                "layout" => {
                    self.section(node);
                    let mut partial = LayoutConfigPartial::default();
                    // A second `layout {}` changes the keys it names and leaves the others.
                    partial.effects = EffectsPartial::seeded_from(&cfg.layout.effects);
                    self.decode_layout_into(node, &mut partial);
                    cfg.layout = partial.apply_to(&cfg.layout);
                }
                "workspace" => self.decode_workspace(node, cfg),
                "window-rule" => self.decode_window_rule(node, cfg),
                "layer-rule" => self.decode_layer_rule(node, cfg),
                "binds" => self.decode_binds(node, cfg),
                "recent-windows" => self.decode_recent_windows(node, &mut cfg.recent_windows),
                "hotkey-overlay" => self.decode_hotkey_overlay(node, cfg),
                "cursor" => self.decode_cursor(node, cfg),
                "animations" => self.decode_animations(node, cfg),
                "gestures" => self.decode_gestures(node, &mut cfg.gestures),
                "environment" => self.decode_environment(node, cfg),
                "switch-events" => self.decode_switch_events(node, cfg),
                "spawn-at-startup" => self.decode_spawn_at_startup(node, cfg),
                "spawn-sh-at-startup" => {
                    if let Some(cmd) = self.arg_text(node) {
                        cfg.spawn_sh_at_startup.push(cmd);
                    }
                }
                "screenshot-path" => {
                    if let Some(value) = self.one_arg(node) {
                        use crate::screenshot::path::ScreenshotPath;
                        if matches!(value.scalar, Scalar::Null) {
                            cfg.screenshot_path = ScreenshotPath::ClipboardOnly;
                        } else if let Some(text) = self.value_text(node, value) {
                            if text.is_empty() {
                                self.error(value.pos, "screenshot-path must not be empty");
                            } else {
                                cfg.screenshot_path = ScreenshotPath::Template(text);
                            }
                        }
                    }
                }
                "overview" => self.decode_overview(node, &mut cfg.overview),
                "accessibility" => self.decode_accessibility(node, cfg),
                "debug" => self.decode_debug(node, cfg),
                "include" => self.decode_include(node, cfg),
                other => match top_level_unsupported(other) {
                    Some(why) => self.unsupported(node, why),
                    None => {
                        let mut known: Vec<&str> = TOP_LEVEL.to_vec();
                        known.extend_from_slice(&[
                            "clipboard",
                            "hotkey-overlay",
                            "screenshot-path",
                            "prefer-no-csd",
                            "gestures",
                            "overview",
                            "environment",
                            "debug",
                        ]);
                        let hint = suggestion(other, &known);
                        let message = match hint {
                            Some(h) => {
                                format!("unknown section '{}' (did you mean '{}'?)", node.name, h)
                            }
                            None => format!(
                                "unknown section '{}' (known: {})",
                                node.name,
                                TOP_LEVEL.join(", ")
                            ),
                        };
                        self.error(node.pos, message);
                    }
                },
            }
        }
    }

    /// niri `hotkey-overlay { skip-at-startup; hide-not-bound }`.
    fn decode_hotkey_overlay(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "skip-at-startup" => {
                    if let Some(value) = self.flag(child) {
                        cfg.hotkey_overlay.skip_at_startup = value;
                    }
                }
                "hide-not-bound" => {
                    if let Some(value) = self.flag(child) {
                        cfg.hotkey_overlay.hide_not_bound = value;
                    }
                }
                _ => self.unknown(
                    child,
                    "'hotkey-overlay'",
                    &["skip-at-startup", "hide-not-bound"],
                ),
            }
        }
    }

    /// niri `cursor { hide-when-typing; hide-after-inactive-ms <ms>; xcursor-theme; xcursor-size }`.
    ///
    /// The two hide options have a Windows lever (`SPI_SETMOUSEVANISH` and a cursor swap after
    /// inactivity); the Xcursor theme and size belong to the Windows pointer scheme.
    fn decode_cursor(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "hide-when-typing" => {
                    if let Some(value) = self.flag(child) {
                        cfg.cursor.hide_when_typing = Some(value);
                    }
                }
                "hide-after-inactive-ms" => {
                    if let Some(ms) = self.arg_u32(child, u32::MAX) {
                        cfg.cursor.hide_after_inactive_ms = Some(ms);
                    }
                }
                "xcursor-size" => {
                    // Windows has no Xcursor, but niri still type-checks the value
                    // (`xcursor_size: Option<u8>`, niri-config/src/misc.rs:36-40).
                    if self.arg_u32(child, 255).is_some() {
                        self.unsupported(child, "the cursor theme belongs to Windows");
                    }
                }
                "xcursor-theme" => {
                    self.unsupported(child, "the cursor theme belongs to Windows");
                }
                _ => self.unknown(
                    child,
                    "'cursor'",
                    &[
                        "hide-when-typing",
                        "hide-after-inactive-ms",
                        "xcursor-theme",
                        "xcursor-size",
                    ],
                ),
            }
        }
    }

    fn decode_include(&mut self, node: &Node, cfg: &mut Config) {
        let raw = match node.args.len() {
            0 => {
                self.error(node.pos, "'include' needs the path of a file");
                return;
            }
            _ => {
                if node.args.len() > 1 {
                    self.error(node.args[1].pos, "'include' takes one path");
                }
                match self.value_text(node, &node.args[0]) {
                    Some(t) => t,
                    None => return,
                }
            }
        };
        let mut optional = false;
        for p in &node.props {
            if p.key == "optional" {
                if let Some(b) = self.value_bool(node, &p.value) {
                    optional = b;
                }
            } else {
                self.error(
                    p.key_pos,
                    format!("unexpected property '{}' (only 'optional' is known)", p.key),
                );
            }
        }
        self.no_children(node);

        let expanded = expand_user_path(&raw);
        let candidate = PathBuf::from(&expanded);
        let path = if candidate.is_absolute() {
            candidate
        } else {
            match &self.base_dir {
                Some(base) => base.join(candidate),
                None => {
                    self.warn(
                        node.pos,
                        "include is ignored: there is no directory to resolve it against",
                    );
                    return;
                }
            }
        };
        if self.depth + 1 >= MAX_INCLUDE_DEPTH {
            self.error(
                node.pos,
                format!(
                    "include depth exceeds {} levels (a chain of includes that long is probably a loop)",
                    MAX_INCLUDE_DEPTH
                ),
            );
            return;
        }
        // Watched even when it cannot be read or parsed, so fixing it reloads the config.
        self.includes.push(path.clone());
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if self.stack.contains(&canon) {
            self.error(
                node.pos,
                format!(
                    "cyclic include: {} is already being read (a file includes itself)",
                    path.display()
                ),
            );
            return;
        }
        match read_config_text(&path) {
            Ok(text) => {
                self.include_trace.push(super::diagnostics::SourceLabel::at(
                    &self.file,
                    &self.source,
                    node.pos,
                    "included here",
                ));
                let saved_file = std::mem::replace(&mut self.file, path.display().to_string());
                let saved_base =
                    std::mem::replace(&mut self.base_dir, path.parent().map(|p| p.to_path_buf()));
                self.stack.push(canon);
                self.depth += 1;
                self.decode_source(&text, cfg);
                self.depth -= 1;
                self.stack.pop();
                self.file = saved_file;
                self.base_dir = saved_base;
                self.include_trace.pop();
            }
            Err(e) => {
                if optional && e.kind() == std::io::ErrorKind::NotFound {
                    self.warn(
                        node.pos,
                        format!("optional include {} does not exist", path.display()),
                    );
                } else if e.kind() == std::io::ErrorKind::NotFound {
                    self.error(
                        node.pos,
                        format!("include: file not found: {}", path.display()),
                    );
                } else {
                    self.error(
                        node.pos,
                        format!("include: cannot read {}: {}", path.display(), e),
                    );
                }
            }
        }
    }

    fn decode_environment(&mut self, node: &Node, cfg: &mut Config) {
        use crate::hooks::environment::{names_equal, validate_entry};
        self.section(node);
        let mut seen: Vec<(&str, Pos)> = Vec::new();
        for child in node.kids() {
            let Some(arg) = self.one_arg(child) else {
                continue;
            };
            let value = match &arg.scalar {
                Scalar::Null => None,
                Scalar::Str { text, .. } => Some(text.clone()),
                _ => {
                    self.error(arg.pos, "an environment value must be a string or null");
                    continue;
                }
            };
            if let Err(error) = validate_entry(&child.name, value.as_deref()) {
                self.error(child.pos, error.to_string());
                continue;
            }
            if let Some((_, first)) = seen.iter().find(|(name, _)| names_equal(name, &child.name)) {
                self.error(
                    child.pos,
                    format!(
                        "duplicate Windows environment variable '{}' (case-insensitive)",
                        child.name
                    ),
                );
                let label = super::diagnostics::SourceLabel::at(
                    &self.file,
                    &self.source,
                    *first,
                    "first definition is here",
                );
                self.issues.last_mut().unwrap().context.related.push(label);
                continue;
            }
            seen.push((&child.name, child.pos));
            // Includes are separate parts: the later declaration wins, even
            // when the spelling of the Windows variable name changes case.
            cfg.environment
                .retain(|name, _| !names_equal(name, &child.name));
            cfg.environment.insert(child.name.clone(), value);
        }
    }

    fn decode_switch_events(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        for child in node.kids() {
            let slot = match norm(&child.name).as_str() {
                "lid-close" => &mut cfg.switch_events.lid_close,
                "lid-open" => &mut cfg.switch_events.lid_open,
                "tablet-mode-on" => &mut cfg.switch_events.tablet_mode_on,
                "tablet-mode-off" => &mut cfg.switch_events.tablet_mode_off,
                _ => {
                    self.unknown(
                        child,
                        "'switch-events'",
                        &["lid-close", "lid-open", "tablet-mode-on", "tablet-mode-off"],
                    );
                    continue;
                }
            };
            self.section(child);
            if child.kids().len() != 1 {
                self.error(
                    child.pos,
                    "a switch event requires exactly one spawn action",
                );
                continue;
            }
            let action = &child.kids()[0];
            if norm(&action.name) != "spawn" {
                self.error(action.pos, "switch events support only the spawn action");
                continue;
            }
            self.no_props(action);
            self.no_children(action);
            let mut argv = Vec::new();
            for arg in &action.args {
                match &arg.scalar {
                    Scalar::Str { text, .. } if !text.contains('\0') => argv.push(text.clone()),
                    _ => self.error(arg.pos, "spawn arguments must be strings without NUL"),
                }
            }
            if argv.first().is_none_or(|program| program.is_empty()) {
                self.error(action.pos, "spawn needs a nonempty program");
            } else {
                *slot = Some(argv);
            }
        }
    }

    fn decode_spawn_at_startup(&mut self, node: &Node, cfg: &mut Config) {
        self.no_props(node);
        self.no_children(node);
        if node.args.is_empty() {
            self.error(node.pos, "'spawn-at-startup' needs a program");
            return;
        }
        let argv: Vec<String> = node.args.iter().map(|a| a.to_text()).collect();
        if argv[0].trim().is_empty() {
            self.error(node.args[0].pos, "'spawn-at-startup' needs a program");
            return;
        }
        cfg.spawn_at_startup.push(argv);
    }

    // ---- input -----------------------------------------------------------------------

    fn decode_input(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        for child in node.kids() {
            self.input_key(InputScope::Input, child, cfg);
        }
    }

    fn input_key(&mut self, scope: InputScope, child: &Node, cfg: &mut Config) {
        let key = norm(&child.name);
        let in_input = scope == InputScope::Input;
        match key.as_str() {
            // sections
            "keyboard" if in_input => {
                self.section(child);
                for c in child.kids() {
                    self.input_key(InputScope::Keyboard, c, cfg);
                }
            }
            "mouse" if in_input => {
                self.section(child);
                for c in child.kids() {
                    self.input_key(InputScope::Mouse, c, cfg);
                }
            }
            "focus" if in_input => {
                self.section(child);
                for c in child.kids() {
                    self.input_key(InputScope::Focus, c, cfg);
                }
            }
            "touch" if in_input => {
                self.section(child);
                cfg.input.touch_enabled = true;
                for c in child.kids() {
                    self.input_key(InputScope::Touch, c, cfg);
                }
            }
            "touchpad" if in_input => {
                self.section(child);
                cfg.input.touchpad_enabled = true;
                for c in child.kids() {
                    self.input_key(InputScope::Touchpad, c, cfg);
                }
            }
            "tablet" if in_input => {
                self.section(child);
                cfg.input.tablet_enabled = true;
                for c in child.kids() {
                    if norm(&c.name) == "off" {
                        if let Some(off) = self.flag(c) {
                            cfg.input.tablet_enabled = !off;
                        }
                    } else {
                        self.unsupported(
                            c,
                            "tablet mapping and calibration remain owned by Windows",
                        );
                    }
                }
            }
            "trackpoint" | "trackball" if in_input => {
                // niri's trackpoint/trackball blocks take the same pointing-device options as
                // `mouse`.  Windows has one system-wide pointer configuration, so the shared
                // options go through the same fields; their device-specific effect is the OS's.
                self.section(child);
                let trackpoint = key == "trackpoint";
                for c in child.kids() {
                    self.input_key(
                        if trackpoint {
                            InputScope::Trackpoint
                        } else {
                            InputScope::Trackball
                        },
                        c,
                        cfg,
                    );
                }
            }
            // keyboard
            "layout" | "keyboard-layout"
                if matches!(scope, InputScope::Input | InputScope::Keyboard) =>
            {
                if let Some(v) = self.arg_text(child) {
                    cfg.input.keyboard_layout = v;
                    cfg.input.keyboard_layout_configured = true;
                }
            }
            "repeat-delay" if matches!(scope, InputScope::Input | InputScope::Keyboard) => {
                if let Some(v) = self.arg_u32(child, 65535) {
                    cfg.input.repeat_delay = v;
                    cfg.input.repeat_delay_configured = true;
                }
            }
            "repeat-rate" if matches!(scope, InputScope::Input | InputScope::Keyboard) => {
                if let Some(v) = self.arg_u32(child, 255) {
                    cfg.input.repeat_rate = v;
                    cfg.input.repeat_rate_configured = true;
                }
            }
            "numlock" if matches!(scope, InputScope::Input | InputScope::Keyboard) => {
                if let Some(v) = self.flag(child) {
                    cfg.input.numlock = v;
                    cfg.input.numlock_configured = true;
                }
            }
            "xkb" if matches!(scope, InputScope::Input | InputScope::Keyboard) => {
                self.section(child);
                for option in child.kids() {
                    match norm(&option.name).as_str() {
                        "layout" => {
                            if let Some(name) = self.arg_text(option) {
                                cfg.input.keyboard_layout = name;
                                cfg.input.keyboard_layout_configured = true;
                            }
                        }
                        "variant" | "options" | "model" | "rules" | "file" => self.unsupported(option, "Windows has no XKB; only one already-loaded Windows layout can be selected"),
                        _ => self.unknown(option, "'xkb'", &["layout", "variant", "options", "model", "rules", "file"]),
                    }
                }
            }
            "track-layout" if matches!(scope, InputScope::Input | InputScope::Keyboard) => {
                self.unsupported(child, "Windows controls per-application layout tracking")
            }
            // focus
            "focus-follows-mouse" | "follows-mouse" => {
                for p in &child.props {
                    if p.key == "max-scroll-amount" {
                        let percent = p
                            .value
                            .as_str()
                            .and_then(|s| s.strip_suffix('%'))
                            .and_then(|s| s.parse::<f64>().ok())
                            .filter(|p| p.is_finite() && *p >= 0.0);
                        if let Some(percent) = percent {
                            cfg.input.focus_max_scroll_percent = Some(percent);
                        } else {
                            self.error(p.value.pos, "'max-scroll-amount' expects a non-negative finite percentage, e.g. \"0%\"");
                        }
                    } else {
                        self.error(
                            p.key_pos,
                            format!("unexpected property '{}' on '{}'", p.key, child.name),
                        );
                    }
                }
                self.no_children(child);
                let value = match child.args.len() {
                    0 => Some(true),
                    1 => self.value_bool(child, &child.args[0]),
                    _ => {
                        self.error(child.args[1].pos, "takes at most one value");
                        None
                    }
                };
                if let Some(v) = value {
                    cfg.input.focus_follows_mouse = v;
                }
            }
            "focus-delay-ms"
                if matches!(
                    scope,
                    InputScope::Input | InputScope::Keyboard | InputScope::Focus
                ) =>
            {
                if let Some(v) = self.arg_u32(child, 60000) {
                    cfg.input.focus_delay_ms = v;
                }
            }
            "warp-on-focus" | "warp-mouse-to-focus"
                if matches!(
                    scope,
                    InputScope::Input | InputScope::Keyboard | InputScope::Focus
                ) =>
            {
                for p in &child.props {
                    if p.key == "mode" {
                        match p.value.as_str() {
                            Some("center-xy") => cfg.input.warp_mode = WarpMode::CenterXy,
                            Some("center-xy-always") => {
                                cfg.input.warp_mode = WarpMode::CenterXyAlways
                            }
                            _ => self.error(
                                p.value.pos,
                                "'mode' expects \"center-xy\" or \"center-xy-always\"",
                            ),
                        }
                    } else {
                        self.error(
                            p.key_pos,
                            format!("unexpected property '{}' on '{}'", p.key, child.name),
                        );
                    }
                }
                self.no_children(child);
                let value = match child.args.len() {
                    0 => Some(true),
                    1 => self.value_bool(child, &child.args[0]),
                    _ => {
                        self.error(child.args[1].pos, "takes at most one value");
                        None
                    }
                };
                if let Some(v) = value {
                    cfg.input.warp_on_focus = v;
                }
            }
            "workspace-auto-back-and-forth"
                if matches!(scope, InputScope::Input | InputScope::Keyboard) =>
            {
                if let Some(v) = self.flag(child) {
                    cfg.input.workspace_auto_back_and_forth = v;
                }
            }
            "mod-key" if matches!(scope, InputScope::Input | InputScope::Keyboard) => {
                if let Some(v) = self.arg_text(child) {
                    cfg.input.mod_key = v.trim().to_lowercase().replace('+', "-");
                }
            }
            "mod-key-nested" | "disable-power-key-handling" if in_input => {
                self.unsupported(child, "")
            }
            // mouse
            "natural-scroll" if scope == InputScope::Input || scope.is_pointer() => {
                if let Some(v) = self.flag(child) {
                    cfg.input.natural_scroll = v;
                    if matches!(scope, InputScope::Mouse | InputScope::Input) {
                        cfg.input.mouse_scroll.natural = v;
                    }
                }
            }
            "scroll-speed" | "mouse-speed"
                if matches!(scope, InputScope::Input | InputScope::Mouse) =>
            {
                if let Some(v) = self.arg_number(child, 0.0, 1000.0) {
                    cfg.input.mouse_speed = v;
                }
            }
            "acceleration" | "mouse-acceleration"
                if matches!(scope, InputScope::Input | InputScope::Mouse) =>
            {
                if let Some(v) = self.arg_number(child, -1000.0, 1000.0) {
                    cfg.input.mouse_acceleration = v;
                }
            }
            "sensitivity" | "mouse-sensitivity"
                if matches!(scope, InputScope::Input | InputScope::Mouse) =>
            {
                if let Some(v) = self.arg_number(child, 0.0, 1000.0) {
                    cfg.input.mouse_sensitivity = v;
                }
            }
            "tap-to-click" | "tap" if matches!(scope, InputScope::Input | InputScope::Mouse) => {
                if let Some(v) = self.flag(child) {
                    cfg.input.tap_to_click = v;
                }
            }
            "scroll-factor" if matches!(scope, InputScope::Mouse | InputScope::Touchpad) => {
                if let Some(factor) = self.decode_scroll_factor(child) {
                    if scope == InputScope::Mouse {
                        cfg.input.mouse_scroll_factor = Some(factor);
                    } else {
                        cfg.input.touchpad_scroll_factor = Some(factor);
                    }
                }
            }
            "accel-speed" if scope.is_pointer() => {
                self.unsupported(child, "Windows has one pointer speed for all devices")
            }
            "pinch-sensitivity" if scope == InputScope::Touchpad => {
                // niri: `pinch_sensitivity: Option<FloatOrInt<0, 100>>`
                // (niri-config/src/input.rs:228-230).  Windows has no pinch gesture on a
                // precision touchpad, but the value is still niri's.
                if self.arg_number(child, 0.0, 100.0).is_some() {
                    self.unsupported(child, "Windows handles precision-touchpad pinch itself");
                }
            }
            // pointing-device options shared by mouse, touchpad, trackpoint and trackball
            "off"
            | "accel-profile"
            | "scroll-method"
            | "scroll-button"
            | "scroll-button-lock"
            | "left-handed"
            | "middle-emulation"
            | "dwt"
            | "dwtp"
            | "drag"
            | "drag-lock"
            | "tap-button-map"
            | "click-method"
            | "disabled-on-external-mouse"
            | "tap"
            | "tap-to-click"
                if scope.is_pointer() =>
            {
                self.pointing_option(scope, child, cfg)
            }
            // touch
            "enable" | "enabled" if scope == InputScope::Touch => {
                if let Some(v) = self.flag(child) {
                    cfg.input.touch_enabled = v;
                }
            }
            "off" if scope == InputScope::Touch => {
                if let Some(v) = self.flag(child) {
                    cfg.input.touch_enabled = !v;
                }
            }
            "tap-to-click" | "tap" if scope == InputScope::Touch => {
                if let Some(v) = self.flag(child) {
                    cfg.input.touch_tap_to_click = v;
                }
            }
            "scroll-speed" if scope == InputScope::Touch => {
                if let Some(v) = self.arg_number(child, 0.0, 1000.0) {
                    cfg.input.touch_scroll_speed = v;
                }
            }
            "map-to-output" | "calibration-matrix" if scope == InputScope::Touch => {
                self.unsupported(child, "touch calibration belongs to Windows")
            }
            _ => self.unknown(child, scope.label(), scope.known()),
        }
    }

    fn decode_scroll_factor(&mut self, node: &Node) -> Option<ScrollFactor> {
        let before = self.issues.len();
        if node.ty.is_some() {
            self.error(node.pos, "scroll-factor does not accept a type annotation");
        }
        if node.args.len() > 1 {
            self.error(
                node.args[1].pos,
                "scroll-factor takes at most one base value",
            );
        }
        if node.children.is_some() {
            self.error(node.pos, "scroll-factor takes no child block");
        }
        let mut factor = ScrollFactor::default();
        if let Some(value) = node.args.first() {
            factor.base = self.scroll_factor_number(node, value, 0.0, 100.0);
        }
        let mut horizontal_seen = false;
        let mut vertical_seen = false;
        for prop in &node.props {
            let (seen, axis) = match prop.key.as_str() {
                "horizontal" => (&mut horizontal_seen, &mut factor.horizontal),
                "vertical" => (&mut vertical_seen, &mut factor.vertical),
                _ => {
                    self.error(
                        prop.key_pos,
                        format!("unknown scroll-factor property '{}'", prop.key),
                    );
                    continue;
                }
            };
            if std::mem::replace(seen, true) {
                self.error(
                    prop.key_pos,
                    format!("duplicate scroll-factor property '{}'", prop.key),
                );
            }
            *axis = self.scroll_factor_number(node, &prop.value, -100.0, 100.0);
        }
        (self.issues.len() == before).then_some(factor)
    }

    fn scroll_factor_number(
        &mut self,
        node: &Node,
        value: &Value,
        min: f64,
        max: f64,
    ) -> Option<f64> {
        if value.ty.is_some() {
            self.error(
                value.pos,
                "scroll-factor values do not accept type annotations",
            );
            return None;
        }
        let Some(number) = value.as_f64() else {
            self.error(
                value.pos,
                format!("'{}' requires a numeric value", node.name),
            );
            return None;
        };
        if !number.is_finite() || number < min || number > max {
            self.error(
                value.pos,
                format!("'{}' must be between {min} and {max}", node.name),
            );
            return None;
        }
        Some(number)
    }

    /// One niri pointing-device option, shared by `mouse`, `touchpad`, `trackpoint` and
    /// `trackball`.  Windows configures all pointing devices with one system-wide setup, so the
    /// stored fields are shared too; `crate::input::pointer_settings` decides what reaches the
    /// OS and what is only validated.
    fn pointing_option(&mut self, scope: InputScope, child: &Node, cfg: &mut Config) {
        let key = norm(&child.name);
        match key.as_str() {
            "off" => {
                // A bare `off` means off; `off false` turns the device back on.
                let value = match child.args.len() {
                    0 => Some(true),
                    _ => self.flag(child),
                };
                let Some(value) = value else { return };
                match scope {
                    InputScope::Touchpad => cfg.input.touchpad_enabled = !value,
                    InputScope::Mouse => cfg.input.mouse_off = value,
                    InputScope::Trackpoint => cfg.input.trackpoint_off = value,
                    InputScope::Trackball => cfg.input.trackball_off = value,
                    _ => {}
                }
            }
            "tap" | "tap-to-click" => {
                if let Some(v) = self.flag(child) {
                    cfg.input.tap_to_click = v;
                }
            }
            "accel-profile" => {
                if let Some(v) =
                    self.enum_arg(child, AccelProfile::parse, "\"flat\" or \"adaptive\"")
                {
                    cfg.input.accel_profile = Some(v);
                }
            }
            "scroll-method" => {
                if let Some(v) = self.enum_arg(
                    child,
                    ScrollMethod::parse,
                    "\"none\", \"edge\", \"two-finger\" or \"on-button-down\"",
                ) {
                    cfg.input.scroll_method = Some(v);
                    if scope == InputScope::Mouse {
                        cfg.input.mouse_scroll.method = Some(v);
                    } else {
                        cfg.input.non_mouse_scroll_configured = true;
                    }
                }
            }
            "scroll-button" => {
                if let Some(v) = self.arg_u32(child, u32::MAX) {
                    cfg.input.scroll_button = Some(v);
                    if scope == InputScope::Mouse {
                        cfg.input.mouse_scroll.button = Some(v);
                    } else {
                        cfg.input.non_mouse_scroll_configured = true;
                    }
                    if (1..=5).contains(&v) {
                        self.warn(child.pos, "legacy scroll-button 1..5 maps to left/right/middle/X1/X2; prefer evdev 272..276");
                    } else if v != 0 && !(272..=276).contains(&v) {
                        self.warn(child.pos, "scroll-button is valid evdev syntax but unsupported by the Windows mouse hook; left unapplied");
                    }
                }
            }
            "scroll-button-lock" => {
                if let Some(v) = self.flag(child) {
                    cfg.input.scroll_button_lock = Some(v);
                    if scope == InputScope::Mouse {
                        cfg.input.mouse_scroll.lock = Some(v);
                    } else {
                        cfg.input.non_mouse_scroll_configured = true;
                    }
                }
            }
            "left-handed" => {
                if let Some(v) = self.flag(child) {
                    cfg.input.left_handed = Some(v);
                }
            }
            "middle-emulation" => {
                if let Some(v) = self.flag(child) {
                    cfg.input.middle_emulation = Some(v);
                }
            }
            "click-method" => {
                if let Some(v) = self.enum_arg(
                    child,
                    ClickMethod::parse,
                    "\"button-areas\" or \"clickfinger\"",
                ) {
                    cfg.input.touchpad_click_method = Some(v);
                }
            }
            "disabled-on-external-mouse" => {
                if let Some(v) = self.flag(child) {
                    cfg.input.touchpad_disabled_on_external_mouse = Some(v);
                }
            }
            "dwt" | "dwtp" | "drag" | "drag-lock" if scope == InputScope::Touchpad => {
                let Some(v) = self.flag(child) else { return };
                match key.as_str() {
                    "dwt" => cfg.input.touchpad_dwt = Some(v),
                    "dwtp" => cfg.input.touchpad_dwtp = Some(v),
                    "drag" => cfg.input.touchpad_drag = Some(v),
                    _ => cfg.input.touchpad_drag_lock = Some(v),
                }
            }
            "tap-button-map" if scope == InputScope::Touchpad => {
                self.no_props(child);
                let mut map = cfg.input.touchpad_tap_button_map.unwrap_or_default();
                for c in child.kids() {
                    let slot = match norm(&c.name).as_str() {
                        "left" => &mut map.left,
                        "middle" => &mut map.middle,
                        "right" => &mut map.right,
                        _ => {
                            self.unknown(
                                c,
                                "'input' > 'touchpad' > 'tap-button-map'",
                                &["left", "middle", "right"],
                            );
                            continue;
                        }
                    };
                    // niri's values are finger counts.
                    if let Some(v) = self.arg_number(c, 1.0, 5.0) {
                        *slot = v.round() as u8;
                    }
                }
                cfg.input.touchpad_tap_button_map = Some(map);
            }
            // An option of another device type: niri rejects it (`dwt` on a mouse, say).
            _ => self.unknown(child, scope.label(), scope.known()),
        }
    }

    /// A one-argument option whose text is one of a fixed vocabulary.
    fn enum_arg<T>(
        &mut self,
        node: &Node,
        parse: impl Fn(&str) -> Option<T>,
        what: &str,
    ) -> Option<T> {
        let value = self.one_arg(node)?;
        let text = self.value_text(node, value)?;
        match parse(&text) {
            Some(parsed) => Some(parsed),
            None => {
                self.error(
                    value.pos,
                    format!("'{}' expects {}, found \"{}\"", node.name, what, text),
                );
                None
            }
        }
    }

    // ---- output ----------------------------------------------------------------------

    fn decode_output(&mut self, node: &Node, cfg: &mut Config) {
        self.no_props(node);
        let name = match node.args.first() {
            Some(v) => self.value_text(node, v).unwrap_or_default(),
            None => String::new(),
        };
        if node.args.len() > 1 {
            self.error(node.args[1].pos, "'output' takes one name");
        }
        let mut out = OutputConfig {
            name,
            position: Position::default(),
            width: 0,
            height: 0,
            scale: 0.0,
            mode: String::new(),
            vrr: false,
            primary: false,
            position_set: false,
            focus_at_startup: false,
            hot_corners: None,
            transform: String::new(),
            enable: true,
            layout_override: None,
            mod_key: None,
        };
        for child in node.kids() {
            let key = norm(&child.name);
            match key.as_str() {
                "x" => {
                    if let Some(v) = self.arg_i32(child) {
                        out.position.x = v;
                        out.position_set = true;
                    }
                }
                "y" => {
                    if let Some(v) = self.arg_i32(child) {
                        out.position.y = v;
                        out.position_set = true;
                    }
                }
                "width" => {
                    if let Some(v) = self.arg_u32(child, 100_000) {
                        out.width = v;
                    }
                }
                "height" => {
                    if let Some(v) = self.arg_u32(child, 100_000) {
                        out.height = v;
                    }
                }
                "position" => {
                    // niri: position x=0 y=0; wiri's docs: position 0 0
                    self.no_children(child);
                    let mut x = None;
                    let mut y = None;
                    for p in &child.props {
                        match p.key.as_str() {
                            "x" => x = self.value_i32(child, &p.value),
                            "y" => y = self.value_i32(child, &p.value),
                            other => {
                                let msg = format!("unexpected property '{}' on 'position'", other);
                                self.error(p.key_pos, msg);
                            }
                        }
                    }
                    if child.args.len() == 2 {
                        x = self.value_i32(child, &child.args[0]);
                        y = self.value_i32(child, &child.args[1]);
                    } else if !child.args.is_empty() {
                        self.error(child.args[0].pos, "'position' takes x and y");
                    }
                    if let Some(v) = x {
                        out.position.x = v;
                        out.position_set = true;
                    }
                    if let Some(v) = y {
                        out.position.y = v;
                        out.position_set = true;
                    }
                }
                "scale" => {
                    if let Some(v) = self.arg_number(child, 0.0, 10.0) {
                        if v != 0.0 && !(0.5..=4.0).contains(&v) {
                            self.warn(
                                child.args[0].pos,
                                format!(
                                    "output scale {} is outside 0.5 to 4.0; Windows may not honour it",
                                    v
                                ),
                            );
                        }
                        out.scale = v;
                    }
                }
                "enable" | "enabled" => {
                    if let Some(v) = self.flag(child) {
                        out.enable = v;
                    }
                }
                "off" => {
                    if let Some(v) = self.flag(child) {
                        out.enable = !v;
                    }
                }
                "on" => {
                    if let Some(v) = self.flag(child) {
                        out.enable = v;
                    }
                }
                "mode" => {
                    for p in &child.props {
                        self.warn(
                            p.key_pos,
                            format!(
                                "'mode' property '{}' is not supported by wiri; ignored",
                                p.key
                            ),
                        );
                    }
                    self.no_children(child);
                    if child.args.len() == 1 {
                        if let Some(v) = self.value_text(child, &child.args[0]) {
                            out.mode = v;
                        }
                    } else if child.args.is_empty() {
                        self.error(child.pos, "'mode' needs a value like \"1920x1080@60\"");
                    } else {
                        self.error(child.args[1].pos, "'mode' takes one value");
                    }
                }
                "transform" => {
                    if let Some(v) = self.arg_text(child) {
                        out.transform = v;
                    }
                }
                "vrr" | "variable-refresh-rate" => {
                    for p in &child.props {
                        if p.key != "on-demand" {
                            self.error(
                                p.key_pos,
                                format!("unexpected property '{}' on '{}'", p.key, child.name),
                            );
                        }
                    }
                    self.no_children(child);
                    let value = match child.args.len() {
                        0 => Some(true),
                        1 => self.value_bool(child, &child.args[0]),
                        _ => {
                            self.error(child.args[1].pos, "takes at most one value");
                            None
                        }
                    };
                    if let Some(v) = value {
                        out.vrr = v;
                    }
                }
                "primary" | "focus-at-startup" => {
                    if let Some(v) = self.flag(child) {
                        // Legacy primary means initial wiri focus, never Windows primary.
                        out.primary = v;
                        out.focus_at_startup = v;
                    }
                }
                "mod-key" => {
                    if let Some(v) = self.arg_text(child) {
                        out.mod_key = Some(v.trim().to_string());
                    }
                }
                "layout" => {
                    self.section(child);
                    let mut partial = out.layout_override.take().unwrap_or_default();
                    self.decode_layout_into(child, &mut partial);
                    out.layout_override = Some(partial);
                }
                "hot-corners" => out.hot_corners = Some(self.decode_hot_corners(child)),
                "max-bpc" | "modeline" | "backdrop-color" => self.unsupported(child, ""),
                // niri deprecated this per-output key in favour of `layout { background-color }`;
                // wiri has one backdrop surface (the overview), so it maps to the same place.
                "background-color" => {
                    if let Some(color) = self.arg_color(child) {
                        self.warn(
                            child.pos,
                            "'background-color' in an output block is deprecated (niri moved it to 'layout'); mapped to the layout-level colour",
                        );
                        cfg.layout.background_color = Some(color);
                    }
                }
                _ => self.unknown(
                    child,
                    "'output'",
                    &[
                        "off",
                        "scale",
                        "transform",
                        "position",
                        "mode",
                        "x",
                        "y",
                        "width",
                        "height",
                        "enable",
                        "primary",
                        "layout",
                        "mod-key",
                        "variable-refresh-rate",
                        "focus-at-startup",
                        "hot-corners",
                        "background-color",
                    ],
                ),
            }
        }
        cfg.output.push(out);
    }

    // ---- layout ----------------------------------------------------------------------

    fn decode_layout_into(&mut self, node: &Node, p: &mut LayoutConfigPartial) {
        for child in node.kids() {
            let key = norm(&child.name);
            match key.as_str() {
                // gaps: niri's `gaps 16`, wiri's `gaps { inner 16; outer 8 }`
                "gaps" => {
                    if child.children.is_some() {
                        self.section(child);
                        for c in child.kids() {
                            match norm(&c.name).as_str() {
                                "inner" => self.set_f64(c, 0.0, 65535.0, &mut p.inner_gaps),
                                "outer" => self.set_f64(c, 0.0, 65535.0, &mut p.outer_gaps),
                                _ => self.unknown(c, "'gaps'", &["inner", "outer"]),
                            }
                        }
                    } else if let Some(v) = self.arg_number(child, 0.0, 65535.0) {
                        p.inner_gaps = Some(v);
                        p.outer_gaps = Some(v);
                    }
                }
                "inner-gaps" | "inner" => self.set_f64(child, 0.0, 65535.0, &mut p.inner_gaps),
                "outer-gaps" | "outer" => self.set_f64(child, 0.0, 65535.0, &mut p.outer_gaps),
                "struts" => {
                    self.section(child);
                    for c in child.kids() {
                        let side = norm(&c.name);
                        let strut = self.arg_number(c, -65535.0, 65535.0);
                        let value = strut.map(InsetValue::Logical);
                        match side.as_str() {
                            "left" => p.struts.left = value.or(p.struts.left),
                            "right" => p.struts.right = value.or(p.struts.right),
                            "top" => p.struts.top = value.or(p.struts.top),
                            "bottom" => p.struts.bottom = value.or(p.struts.bottom),
                            _ => self.unknown(c, "'struts'", &["left", "right", "top", "bottom"]),
                        }
                    }
                }
                "border" | "borders" => self.decode_border_block(child, p),
                "focus-ring" => self.decode_focus_ring_block(child, p),
                "shadow" | "shadows" => {
                    // legacy `shadow true` (a flag) or a block
                    if child.children.is_none() && !child.args.is_empty() {
                        self.set_flag(child, &mut p.shadow_enable);
                        if let Some(on) = p.shadow_enable {
                            shadow_style(p).on = on;
                        }
                    } else {
                        self.decode_shadow_block(child, p);
                    }
                }
                "insert-hint" => {
                    let mut style = p.effects.insert_hint.take().unwrap_or_default();
                    self.decode_insert_hint(child, &mut style);
                    p.effects.insert_hint = Some(style);
                }
                "tab-indicator" => {
                    let mut style = p.effects.tab_indicator.take().unwrap_or_default();
                    self.decode_tab_indicator(child, &mut style);
                    p.effects.tab_indicator = Some(style);
                }
                // wiri's flat keys
                "border-width" | "width" => {
                    self.set_u32(child, 4096, &mut p.border_width);
                    if let Some(w) = p.border_width {
                        let b = border_style(p);
                        b.width = f64::from(w);
                        b.off = w == 0;
                    }
                }
                "border-color" | "color" | "border-color-inactive" => {
                    self.set_color(child, &mut p.border_color);
                    if let Some(c) = p.border_color.clone() {
                        border_style(p).inactive = PaintStyle::Color(c);
                    }
                }
                "border-color-focused" | "color-focused" => {
                    self.set_color(child, &mut p.border_color_focused);
                    if let Some(c) = p.border_color_focused.clone() {
                        border_style(p).active = PaintStyle::Color(c);
                    }
                }
                "border-color-urgent" | "color-urgent" => {
                    self.set_color(child, &mut p.border_color_urgent);
                    if let Some(c) = p.border_color_urgent.clone() {
                        border_style(p).urgent = PaintStyle::Color(c);
                    }
                }
                "border-radius" | "radius" => {
                    self.set_u32(child, 4096, &mut p.border_radius);
                    if let Some(r) = p.border_radius {
                        p.effects.corner_radius = Some(r);
                    }
                }
                "border-padding" | "padding" => self.set_u32(child, 4096, &mut p.border_padding),
                "focus-ring-width" => {
                    self.set_u32(child, 4096, &mut p.focus_ring_width);
                    if let Some(w) = p.focus_ring_width {
                        let r = focus_ring_style(p);
                        r.width = f64::from(w);
                        r.off = w == 0;
                    }
                }
                "focus-ring-color" => {
                    self.set_color(child, &mut p.focus_ring_color);
                    if let Some(c) = p.focus_ring_color.clone() {
                        focus_ring_style(p).active = PaintStyle::Color(c);
                    }
                }
                "focus-ring-gap" | "gap" => self.set_u32(child, 4096, &mut p.focus_ring_gap),
                "focus-ring-inactive-color" | "inactive-color" => {
                    self.set_color(child, &mut p.focus_ring_inactive_color);
                    if let Some(c) = p.focus_ring_inactive_color.clone() {
                        focus_ring_style(p).inactive = PaintStyle::Color(c);
                    }
                }
                "dim-unfocused" => {
                    if let Some(v) = self.arg_number(child, 0.0, 1.0) {
                        p.dim_unfocused = Some((v as f32).clamp(0.1, 1.0));
                    }
                }
                "shadow-enable" | "shadows-enable" => {
                    self.set_flag(child, &mut p.shadow_enable);
                    if let Some(on) = p.shadow_enable {
                        shadow_style(p).on = on;
                    }
                }
                "shadow-opacity" | "opacity" => {
                    self.set_f64(child, 0.0, 1.0, &mut p.shadow_opacity);
                    if let Some(o) = p.shadow_opacity {
                        let st = shadow_style(p);
                        if let Some(mut rgba) = super::effects::parse_css_color(&st.color) {
                            rgba[3] = (o * 255.0).round() as u8;
                            st.color = super::effects::color_text(rgba);
                        }
                    }
                }
                "shadow-offset-x" => {
                    self.set_i32(child, &mut p.shadow_offset_x);
                    if let Some(v) = p.shadow_offset_x {
                        shadow_style(p).offset_x = v as f64;
                    }
                }
                "shadow-offset-y" => {
                    self.set_i32(child, &mut p.shadow_offset_y);
                    if let Some(v) = p.shadow_offset_y {
                        shadow_style(p).offset_y = v as f64;
                    }
                }
                "shadow-blur" => {
                    self.set_u32(child, 4096, &mut p.shadow_blur);
                    if let Some(v) = p.shadow_blur {
                        shadow_style(p).softness = v as f64;
                    }
                }
                "shadow-color" => {
                    self.set_color(child, &mut p.shadow_color);
                    if let Some(c) = p.shadow_color.clone() {
                        shadow_style(p).color = c;
                    }
                }
                "shadow-spread" | "spread" => {
                    self.set_u32(child, 4096, &mut p.shadow_spread);
                    if let Some(v) = p.shadow_spread {
                        shadow_style(p).spread = v as f64;
                    }
                }
                "scroll-step" => self.set_u32(child, 100_000, &mut p.scroll_step),
                "column-width" => self.set_u32(child, 100_000, &mut p.column_width),
                "column-width-mode" => self.set_text(child, &mut p.column_width_mode),
                "default-column-display" => {
                    let mut value = None;
                    self.set_text(child, &mut value);
                    if let Some(value) = value {
                        if matches!(value.as_str(), "normal" | "tabbed") {
                            p.default_column_display = Some(value);
                        } else {
                            self.error(child.pos, "expected column display 'normal' or 'tabbed'");
                        }
                    }
                }
                "max-visible-columns" => self.set_u32(child, 1000, &mut p.max_visible_columns),
                "split-ratio" => self.set_f64(child, 0.0, 1.0, &mut p.split_ratio),
                "auto-balance" => self.set_flag(child, &mut p.auto_balance),
                "strip-frame" => self.set_flag(child, &mut p.strip_frame),
                "snap-on-drag" => self.set_flag(child, &mut p.snap_on_drag),
                "snap-threshold-px" => self.set_u32(child, 4096, &mut p.snap_threshold_px),
                "smart-borders" | "border-smart" | "smart" => {
                    self.set_flag(child, &mut p.smart_borders)
                }
                "status-bar" => self.set_flag(child, &mut p.status_bar),
                "hide-method" => self.set_text(child, &mut p.hide_method),
                "reserve-top" => {
                    let value = self.arg_i32(child).map(InsetValue::Physical);
                    p.struts.top = value.or(p.struts.top);
                }
                "reserve-bottom" => {
                    let value = self.arg_i32(child).map(InsetValue::Physical);
                    p.struts.bottom = value.or(p.struts.bottom);
                }
                "reserve-left" => {
                    let value = self.arg_i32(child).map(InsetValue::Physical);
                    p.struts.left = value.or(p.struts.left);
                }
                "reserve-right" => {
                    let value = self.arg_i32(child).map(InsetValue::Physical);
                    p.struts.right = value.or(p.struts.right);
                }
                "center-focused-column" => {
                    if let Some(mode) = self.arg_text(child) {
                        if matches!(mode.as_str(), "never" | "always" | "on-overflow") {
                            p.center_focused_column = Some(mode);
                        } else {
                            self.error(child.pos, "expected never, always, or on-overflow");
                        }
                    }
                }
                "always-center-single-column" => {
                    self.set_flag(child, &mut p.always_center_single_column)
                }
                "default-column-width" => {
                    if let Some(size) = self.decode_size_block(child) {
                        p.default_column_width = Some(size);
                    }
                }
                "preset-column-widths" => {
                    if let Some(sizes) = self.decode_size_list(child, false) {
                        p.preset_column_widths = Some(sizes);
                    }
                }
                "preset-window-heights" => {
                    if let Some(sizes) = self.decode_size_list(child, true) {
                        p.preset_window_heights = Some(sizes);
                    }
                }
                "empty-workspace-above-first" => {
                    self.set_flag(child, &mut p.empty_workspace_above_first)
                }
                "fullscreen-backdrop-color" => {
                    if let Some(color) = self.arg_color(child) {
                        p.fullscreen_backdrop_color = Some(color);
                    }
                }
                // niri layout options wiri does not implement yet
                // niri: the colour behind the workspaces.  Wiri paints it behind the overview's
                // workspaces (Windows owns the desktop behind tiled windows).
                "background-color" => {
                    if let Some(color) = self.arg_color(child) {
                        if is_accent_alias(&color) {
                            self.error(
                                child.pos,
                                "'background-color' needs a colour like \"#101820\"; the accent alias is for border colours",
                            );
                        } else {
                            p.background_color = Some(color);
                        }
                    }
                }
                _ => self.unknown(
                    child,
                    "'layout'",
                    &[
                        "gaps",
                        "struts",
                        "border",
                        "focus-ring",
                        "shadow",
                        "inner-gaps",
                        "outer-gaps",
                        "border-width",
                        "border-color",
                        "border-color-focused",
                        "focus-ring-width",
                        "focus-ring-color",
                        "dim-unfocused",
                        "column-width",
                        "column-width-mode",
                        "max-visible-columns",
                        "scroll-step",
                        "hide-method",
                        "snap-on-drag",
                        "strip-frame",
                        "status-bar",
                        "center-focused-column",
                        "preset-column-widths",
                        "default-column-width",
                        "background-color",
                    ],
                ),
            }
        }
    }

    fn decode_border_block(&mut self, node: &Node, p: &mut LayoutConfigPartial) {
        self.section(node);
        let mut style = p
            .effects
            .border
            .take()
            .unwrap_or_else(RingStyle::border_default);
        for c in node.kids() {
            if self.ring_key(c, &mut style, true) {
                // The older flat fields follow the block (the engine's DWM border colours and
                // the per-output overrides read them).
                match norm(&c.name).as_str() {
                    "width" => p.border_width = Some(legacy_width_px(style.width)),
                    "off" | "on" => {
                        if style.off {
                            p.border_width = Some(0);
                        }
                    }
                    "active-color" | "color-focused" => {
                        if let PaintStyle::Color(v) = &style.active {
                            p.border_color_focused = Some(v.clone());
                        }
                    }
                    "inactive-color" | "color" => {
                        if let PaintStyle::Color(v) = &style.inactive {
                            p.border_color = Some(v.clone());
                        }
                    }
                    "urgent-color" | "color-urgent" => {
                        if let PaintStyle::Color(v) = &style.urgent {
                            p.border_color_urgent = Some(v.clone());
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match norm(&c.name).as_str() {
                "radius" => {
                    self.set_u32(c, 4096, &mut p.border_radius);
                    if let Some(r) = p.border_radius {
                        p.effects.corner_radius = Some(r);
                    }
                }
                "padding" => self.set_u32(c, 4096, &mut p.border_padding),
                "smart" => self.set_flag(c, &mut p.smart_borders),
                _ => self.unknown(
                    c,
                    "'border'",
                    &[
                        "off",
                        "on",
                        "width",
                        "active-color",
                        "inactive-color",
                        "urgent-color",
                        "active-gradient",
                        "inactive-gradient",
                        "urgent-gradient",
                        "radius",
                    ],
                ),
            }
        }
        p.effects.border = Some(style);
    }

    fn decode_focus_ring_block(&mut self, node: &Node, p: &mut LayoutConfigPartial) {
        self.section(node);
        let mut style = p
            .effects
            .focus_ring
            .take()
            .unwrap_or_else(RingStyle::focus_ring_default);
        for c in node.kids() {
            if self.ring_key(c, &mut style, false) {
                match norm(&c.name).as_str() {
                    "width" => p.focus_ring_width = Some(legacy_width_px(style.width)),
                    "off" | "on" => {
                        if style.off {
                            p.focus_ring_width = Some(0);
                        }
                    }
                    "active-color" | "color" => {
                        if let PaintStyle::Color(v) = &style.active {
                            p.focus_ring_color = Some(v.clone());
                        }
                    }
                    "inactive-color" => {
                        if let PaintStyle::Color(v) = &style.inactive {
                            p.focus_ring_inactive_color = Some(v.clone());
                        }
                    }
                    "urgent-color" => {
                        if let PaintStyle::Color(v) = &style.urgent {
                            p.border_color_urgent = Some(v.clone());
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match norm(&c.name).as_str() {
                "gap" => self.set_u32(c, 4096, &mut p.focus_ring_gap),
                _ => self.unknown(
                    c,
                    "'focus-ring'",
                    &[
                        "off",
                        "on",
                        "width",
                        "active-color",
                        "inactive-color",
                        "urgent-color",
                        "active-gradient",
                        "inactive-gradient",
                        "urgent-gradient",
                    ],
                ),
            }
        }
        p.effects.focus_ring = Some(style);
    }

    fn decode_shadow_block(&mut self, node: &Node, p: &mut LayoutConfigPartial) {
        let mut style = p.effects.shadow.take().unwrap_or_default();
        self.decode_shadow_style(node, &mut style, p);
        p.effects.shadow = Some(style);
    }

    // ---- animations ------------------------------------------------------------------

    fn decode_animations(&mut self, node: &Node, cfg: &mut Config) {
        self.decode_animation_options(node, &mut cfg.animations);
    }

    // ---- workspaces ------------------------------------------------------------------

    fn decode_workspace(&mut self, node: &Node, cfg: &mut Config) {
        self.no_props(node);
        let name = match node.args.first() {
            Some(v) => {
                if node.args.len() > 1 {
                    self.error(node.args[1].pos, "'workspace' takes one name");
                }
                // A number is a name too (`workspace 2` is the workspace called "2").
                v.to_text()
            }
            None => {
                self.error(node.pos, "'workspace' needs a name");
                return;
            }
        };
        if name.is_empty() {
            self.error(node.pos, "workspace name must not be empty");
            return;
        }
        let mut ws = WorkspaceConfig {
            name,
            ..Default::default()
        };
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "layout" => {
                    if child.args.is_empty() && child.children.is_some() {
                        self.section(child);
                        let mut partial = LayoutConfigPartial::default();
                        self.decode_layout_into(child, &mut partial);
                        for option in child.kids() {
                            if matches!(
                                norm(&option.name).as_str(),
                                "empty-workspace-above-first" | "insert-hint"
                            ) {
                                self.error(option.pos, format!("'{}' is an output-level option and is not allowed in workspace layout", option.name));
                            }
                        }
                        ws.layout_override = Some(partial);
                    } else if let Some(v) = self.arg_text(child) {
                        ws.layout = v;
                    }
                }
                "monitor" => {
                    if let Some(v) = self.arg_text(child) {
                        ws.monitor = v;
                    }
                }
                "open-on-output" => {
                    if let Some(v) = self.arg_text(child) {
                        if v.is_empty() {
                            self.error(child.pos, "open-on-output must not be empty");
                        } else {
                            // Declaration order is the preference order; `monitor` keeps
                            // the first one as the legacy single-output fallback.
                            if ws.monitor.is_empty() {
                                ws.monitor = v.clone();
                            }
                            ws.open_on_outputs.push(v);
                        }
                    }
                }
                "follow-on-focus" => {
                    if let Some(v) = self.flag(child) {
                        ws.follow_on_focus = v;
                    }
                }
                _ => self.unknown(
                    child,
                    "'workspace'",
                    &["layout", "monitor", "open-on-output", "follow-on-focus"],
                ),
            }
        }
        cfg.workspace.push(ws);
    }

    // ---- window rules ----------------------------------------------------------------

    /// `accessibility` — screen-reader announcements (wiri extension: niri's Accessibility
    /// support has no config key and is always on).  A bare node means on, `false` turns it
    /// off, and the section form accepts the same two spellings.
    /// niri `debug { … }`. `strict-new-window-focus-policy` is honoured (see
    /// `layout::engine::rules::new_window_activation`); every other option configures compositor
    /// internals (DRM, PipeWire, frame queueing, ...) and is reported, not an error.
    fn decode_debug(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "strict-new-window-focus-policy" => {
                    if let Some(value) = self.flag(child) {
                        cfg.debug.strict_new_window_focus_policy = value;
                    }
                }
                _ => self.unsupported(
                    child,
                    "a compositor debug option with no Windows counterpart",
                ),
            }
        }
    }

    fn decode_accessibility(&mut self, node: &Node, cfg: &mut Config) {
        let kids = node.kids();
        if kids.is_empty() {
            if let Some(value) = self.flag(node) {
                cfg.accessibility = value;
            }
            return;
        }
        self.section(node);
        for child in kids {
            match norm(&child.name).as_str() {
                "off" | "disable" | "disabled" => {
                    if let Some(value) = self.flag(child) {
                        cfg.accessibility = !value;
                    }
                }
                "enabled" | "announcements" => {
                    if let Some(value) = self.flag(child) {
                        cfg.accessibility = value;
                    }
                }
                _ => self.unknown(
                    child,
                    "'accessibility'",
                    &["off", "enabled", "announcements"],
                ),
            }
        }
    }

    /// `layer-rule { match namespace="…"; block-out-from "…"; }`
    ///
    /// Windows has no layer shell, so the only layer surfaces a rule can reach are wiri's
    /// own overlay windows: `namespace` is the overlay's name (`workspace-indicator`,
    /// `overview`, `status-bar`, `hotkeys`, `recent-windows`, `screenshot-ui`, `decor`,
    /// `animation`, `overview-banner`) and `block-out-from` decides whether it is kept out
    /// of screen capture.  Anything else in a layer rule is a warning, not an error, so an
    /// niri config still loads.
    fn decode_layer_rule(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        let mut rule = WindowRule::default();
        for child in node.kids() {
            let key = norm(&child.name);
            match key.as_str() {
                "match" => {
                    if let Some(group) = self.decode_layer_match_group(child) {
                        rule.match_groups.push(group);
                    }
                }
                "exclude" => {
                    if let Some(group) = self.decode_layer_match_group(child) {
                        rule.exclude_groups.push(group);
                    }
                }
                "block-out-from" => self.decode_block_out_from(child, &mut rule),
                _ => self.unsupported(child, "layer rules"),
            }
        }
        cfg.layer_rules.push(rule);
    }

    /// A layer rule's `match`/`exclude` line: `namespace` (a regular expression, as in
    /// niri) and `at-startup`.
    fn decode_layer_match_group(&mut self, node: &Node) -> Option<MatchGroup> {
        let mut group = MatchGroup::default();
        let mut ok = true;
        for argument in &node.args {
            self.error(
                argument.pos,
                format!(
                    "'{}' takes properties like namespace=\"…\", not arguments",
                    node.name
                ),
            );
            ok = false;
        }
        for property in &node.props {
            let key = norm(&property.key);
            let value = &property.value;
            match key.as_str() {
                "namespace" | "title" => {
                    let Some(text) = self.value_text(node, value) else {
                        ok = false;
                        continue;
                    };
                    match RegexPat::new(&text) {
                        // wiri's overlays answer to their name as the title, so a layer
                        // namespace is matched the same way a window title is.
                        Ok(pattern) => group.conds.push(Matcher::TitleSearch(pattern)),
                        Err(error) => {
                            self.error(
                                value.pos,
                                format!(
                                    "invalid regular expression for '{}': {}",
                                    property.key, error
                                ),
                            );
                            ok = false;
                        }
                    }
                }
                "at-startup" => match self.value_bool(node, value) {
                    Some(true) => group.conds.push(Matcher::AtStartup),
                    Some(false) => group.conds.push(Matcher::Not(Box::new(Matcher::AtStartup))),
                    None => ok = false,
                },
                _ => {
                    self.warn(
                        property.key_pos,
                        format!(
                            "'{}' is not supported by wiri (layer rules match wiri's own \
                             overlays only); ignored",
                            property.key
                        ),
                    );
                }
            }
        }
        ok.then_some(group)
    }

    /// `block-out-from "screencast"` / `"screen-capture"`, in a window rule or a layer rule.
    fn decode_block_out_from(&mut self, node: &Node, rule: &mut WindowRule) {
        let Some(value) = self.arg_text(node) else {
            return;
        };
        match crate::window::block_out::BlockOutFrom::parse(&value) {
            Some(blocked) => rule.block_out_from = Some(blocked),
            None => self.error(
                node.args[0].pos,
                format!(
                    "'block-out-from' expects \"screencast\" or \"screen-capture\", \
                     found \"{value}\""
                ),
            ),
        }
    }

    fn decode_window_rule(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        let mut rule = WindowRule::default();
        for child in node.kids() {
            let key = norm(&child.name);
            match key.as_str() {
                "match" => {
                    if let Some(g) = self.decode_match_group(child) {
                        rule.match_groups.push(g);
                    }
                }
                "exclude" => {
                    if let Some(g) = self.decode_match_group(child) {
                        rule.exclude_groups.push(g);
                    }
                }
                // the old flat matchers
                "class-name" | "class" => self.set_text(child, &mut rule.class),
                "title" => self.set_text(child, &mut rule.title),
                "instance" => self.set_text(child, &mut rule.instance),
                "process-name" => self.set_text(child, &mut rule.process_name),
                // wiri properties
                "float" | "floating" => {
                    if let Some(v) = self.flag(child) {
                        rule.floating = v;
                    }
                }
                "workspace" => self.set_text(child, &mut rule.workspace),
                "monitor" => self.set_text(child, &mut rule.monitor),
                "default-width" => self.set_u32(child, 100_000, &mut rule.default_width),
                "default-height" => self.set_u32(child, 100_000, &mut rule.default_height),
                "blur" => {
                    if let Some(v) = self.flag(child) {
                        rule.blur = v;
                    }
                }
                "sticky" => {
                    if let Some(v) = self.flag(child) {
                        rule.sticky = v;
                    }
                }
                "opacity" => {
                    // niri: `opacity: Option<f32>` with no parse-time bound
                    // (niri-config/src/window_rule.rs:61-62); the clamp to 0..=1 happens when
                    // the rule is resolved (src/layout/rules.rs).
                    if let Some(v) = self.arg_number(child, f32::MIN as f64, f32::MAX as f64) {
                        rule.opacity = Some(v);
                    }
                }
                "open-max-bounds" => {
                    if let Some(text) = self.arg_text(child) {
                        let parts: Vec<&str> = text
                            .split(|c: char| c == 'x' || c == 'X' || c == ',' || c.is_whitespace())
                            .filter(|s| !s.is_empty())
                            .collect();
                        let parsed = if parts.len() == 2 {
                            match (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                                (Ok(w), Ok(h)) => Some((w, h)),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        match parsed {
                            Some(b) => rule.open_max_bounds = Some(b),
                            None => self.error(
                                child.args[0].pos,
                                "'open-max-bounds' expects \"<width>x<height>\"",
                            ),
                        }
                    }
                }
                // niri properties
                "open-on-output" => self.set_text(child, &mut rule.open_on_output),
                "open-on-workspace" => self.set_text(child, &mut rule.open_on_workspace),
                "open-in-workspace" => {
                    if let Some(v) = self.arg_number(child, 0.0, 1_000_000.0) {
                        rule.open_in_workspace = Some(v.round() as i32);
                    }
                }
                "open-fullscreen" => self.set_flag(child, &mut rule.open_fullscreen),
                "open-floating" => self.set_flag(child, &mut rule.open_floating),
                "open-maximized" => self.set_flag(child, &mut rule.open_maximized),
                "open-maximized-to-edges" => {
                    self.set_flag(child, &mut rule.open_maximized_to_edges)
                }
                "open-focused" => self.set_flag(child, &mut rule.open_focused),
                // niri: `min_width .. max_height: Option<u16>`
                // (niri-config/src/window_rule.rs:43-49).
                "min-width" => self.set_u32(child, 65535, &mut rule.min_width),
                "min-height" => self.set_u32(child, 65535, &mut rule.min_height),
                "max-width" => self.set_u32(child, 65535, &mut rule.max_width),
                "max-height" => self.set_u32(child, 65535, &mut rule.max_height),
                "default-column-width" => {
                    if let Some(v) = self.decode_size_block(child) {
                        rule.default_column_width = Some(v);
                    }
                }
                "default-window-height" => {
                    if let Some(v) = self.decode_size_block(child) {
                        rule.default_window_height = Some(v);
                    }
                }
                "focus-ring"
                | "border"
                | "shadow"
                | "tab-indicator"
                | "geometry-corner-radius"
                | "clip-to-geometry" => {
                    self.rule_effect_key(child, &mut rule.effects);
                }
                "default-column-display"
                | "default-floating-position"
                | "on-xdg-activate"
                | "scroll-factor" => {
                    self.rule_policy_key(child, &mut rule.policy);
                }
                "draw-border-with-background"
                | "baba-is-float"
                | "variable-refresh-rate"
                | "pinch-sensitivity"
                | "tiled-state"
                | "background-effect"
                | "popups" => self.unsupported(child, "window rules"),
                "block-out-from" => self.decode_block_out_from(child, &mut rule),
                _ => self.unknown(
                    child,
                    "'window-rule'",
                    &[
                        "match",
                        "exclude",
                        "open-on-output",
                        "open-on-workspace",
                        "open-maximized",
                        "open-fullscreen",
                        "open-floating",
                        "open-focused",
                        "default-column-width",
                        "default-window-height",
                        "min-width",
                        "max-width",
                        "min-height",
                        "max-height",
                        "opacity",
                        "float",
                        "workspace",
                    ],
                ),
            }
        }
        cfg.window_rules.push(rule);
    }

    /// `default-column-width { proportion 0.5; }` / `{ fixed 800; }` / `{}`.
    /// A niri size list: `{ proportion 0.5; fixed 800; }`, as used by
    /// `preset-column-widths` and `preset-window-heights`.  `heights` only changes what an
    /// error message calls the list; proportions are read the same way (a share of the view
    /// for a column width, a share of the column for a window height).
    fn decode_size_list(&mut self, node: &Node, heights: bool) -> Option<Vec<SizeSpec>> {
        self.section(node);
        let mut sizes = Vec::new();
        for child in node.kids() {
            match norm(&child.name).as_str() {
                "proportion" => {
                    // niri: `Proportion(f64)` with no parse-time range
                    // (niri-config/src/layout.rs:127-152).
                    if let Some(v) = self.arg_number(child, f64::MIN, f64::MAX) {
                        sizes.push(SizeSpec::Proportion(v));
                    }
                }
                "fixed" => {
                    // niri: `Fixed(i32)` (niri-config/src/layout.rs:127-152), so a decimal is
                    // a type error.  Wiri stores a `u32`, so a negative fixed size is reported
                    // rather than wrapped.
                    if let Some(v) = self.arg_integer(child, 0.0, u32::MAX as f64) {
                        sizes.push(SizeSpec::Fixed(v as u32));
                    }
                }
                _ => {
                    let key = if heights {
                        "'preset-window-heights'"
                    } else {
                        "'preset-column-widths'"
                    };
                    self.unknown(child, key, &["proportion", "fixed"]);
                }
            }
        }
        Some(sizes)
    }

    fn decode_size_block(&mut self, node: &Node) -> Option<Option<SizeSpec>> {
        self.no_args(node);
        self.no_props(node);
        let kids = node.kids();
        if kids.is_empty() {
            return Some(None);
        }
        if kids.len() > 1 {
            self.error(
                kids[1].pos,
                "only one of 'proportion' or 'fixed' is allowed",
            );
        }
        let first = &kids[0];
        match norm(&first.name).as_str() {
            "proportion" => {
                let v = self.arg_number(first, f64::MIN, f64::MAX)?;
                Some(Some(SizeSpec::Proportion(v)))
            }
            "fixed" => {
                let v = self.arg_integer(first, 0.0, u32::MAX as f64)?;
                Some(Some(SizeSpec::Fixed(v as u32)))
            }
            _ => {
                self.unknown(first, node.name.as_str(), &["proportion", "fixed"]);
                None
            }
        }
    }

    /// A `match` or `exclude` node: niri's properties (`app-id=`, `title=`, `is-active=`, ...)
    /// and/or wiri's older children (`class-name "x"`, `is-active`).
    fn decode_match_group(&mut self, node: &Node) -> Option<MatchGroup> {
        let mut group = MatchGroup::default();
        let mut ok = true;
        for a in &node.args {
            self.error(
                a.pos,
                format!(
                    "'{}' takes properties like app-id=\"...\" or title=\"...\", not arguments",
                    node.name
                ),
            );
            ok = false;
        }
        for prop in &node.props {
            let key = norm(&prop.key);
            let value = &prop.value;
            match key.as_str() {
                "app-id" | "title" | "class-name" | "class" | "process-name" => {
                    let text = match self.value_text(node, value) {
                        Some(t) => t,
                        None => {
                            ok = false;
                            continue;
                        }
                    };
                    match RegexPat::new(&text) {
                        Ok(pat) => group.conds.push(match key.as_str() {
                            "app-id" => Matcher::AppId(pat),
                            "title" => Matcher::TitleSearch(pat),
                            "process-name" => Matcher::ProcessSearch(pat),
                            _ => Matcher::ClassSearch(pat),
                        }),
                        Err(e) => {
                            self.error(
                                value.pos,
                                format!("invalid regular expression for '{}': {}", prop.key, e),
                            );
                            ok = false;
                        }
                    }
                }
                "is-active"
                | "is-focused"
                | "is-active-in-column"
                | "is-floating"
                | "is-urgent"
                | "at-startup" => {
                    let wanted = match self.value_bool(node, value) {
                        Some(b) => b,
                        None => {
                            ok = false;
                            continue;
                        }
                    };
                    let base = match key.as_str() {
                        "is-active" => Matcher::IsActive,
                        "is-focused" => Matcher::IsFocused,
                        "is-active-in-column" => Matcher::IsActiveInColumn,
                        "is-floating" => Matcher::IsFloating,
                        "is-urgent" => Matcher::IsUrgent,
                        _ => Matcher::AtStartup,
                    };
                    group.conds.push(if wanted {
                        base
                    } else {
                        Matcher::Not(Box::new(base))
                    });
                }
                "is-window-cast-target" => {
                    self.warn(
                        prop.key_pos,
                        "'is-window-cast-target' is not supported by wiri (Windows has no screencast target); ignored",
                    );
                }
                _ => {
                    self.error(
                        prop.key_pos,
                        format!(
                            "unknown match property '{}' (known: app-id, title, is-active, is-focused, is-floating, is-urgent, at-startup, is-active-in-column)",
                            prop.key
                        ),
                    );
                    ok = false;
                }
            }
        }
        // wiri's older form: `match { class-name "Firefox"; is-active }`
        for child in node.kids() {
            let key = norm(&child.name);
            match key.as_str() {
                "class-name" | "class" => {
                    if let Some(t) = self.arg_text(child) {
                        group.conds.push(Matcher::ClassName(t));
                    }
                }
                "title" => {
                    if let Some(t) = self.arg_text(child) {
                        group.conds.push(Matcher::Title(t));
                    }
                }
                "instance" => {
                    if let Some(t) = self.arg_text(child) {
                        group.conds.push(Matcher::Instance(t));
                    }
                }
                "process-name" => {
                    if let Some(t) = self.arg_text(child) {
                        group.conds.push(Matcher::ProcessName(t));
                    }
                }
                "class-name-regex" | "class-regex" | "title-regex" | "instance-regex"
                | "process-name-regex" | "process-regex" => {
                    if let Some(t) = self.arg_text(child) {
                        match regex::Regex::new(&t) {
                            Ok(_) => group.conds.push(match key.as_str() {
                                "class-name-regex" | "class-regex" => Matcher::ClassNameRegex(t),
                                "title-regex" => Matcher::TitleRegex(t),
                                "instance-regex" => Matcher::InstanceRegex(t),
                                _ => Matcher::ProcessNameRegex(t),
                            }),
                            Err(e) => {
                                let at = child.args.first().map(|a| a.pos).unwrap_or(child.pos);
                                self.error(
                                    at,
                                    format!(
                                        "invalid regular expression for '{}': {}",
                                        child.name, e
                                    ),
                                );
                                ok = false;
                            }
                        }
                    }
                }
                "is-active" | "is-floating" | "is-urgent" | "at-startup" | "is-focused" => {
                    if let Some(wanted) = self.flag(child) {
                        let base = match key.as_str() {
                            "is-active" => Matcher::IsActive,
                            "is-floating" => Matcher::IsFloating,
                            "is-urgent" => Matcher::IsUrgent,
                            "is-focused" => Matcher::IsFocused,
                            _ => Matcher::AtStartup,
                        };
                        group.conds.push(if wanted {
                            base
                        } else {
                            Matcher::Not(Box::new(base))
                        });
                    }
                }
                _ => {
                    self.unknown(
                        child,
                        &format!("'{}'", node.name),
                        &[
                            "class-name",
                            "title",
                            "process-name",
                            "is-active",
                            "is-floating",
                        ],
                    );
                    ok = false;
                }
            }
        }
        if ok {
            Some(group)
        } else {
            None
        }
    }

    // ---- binds -----------------------------------------------------------------------

    fn decode_binds(&mut self, node: &Node, cfg: &mut Config) {
        self.section(node);
        let mut parsed = Vec::new();
        for child in node.kids() {
            if norm(&child.name) == "extend-defaults" {
                if let Some(v) = self.flag(child) {
                    cfg.binds.extend_defaults = v;
                }
                continue;
            }
            if let Some(bind) = self.decode_bind(child) {
                parsed.push(LocatedBind {
                    bind,
                    label: super::diagnostics::SourceLabel::at(
                        &self.file,
                        &self.source,
                        child.pos,
                        "first definition is here",
                    ),
                    includes: self.include_trace.clone(),
                });
            }
        }
        self.bind_groups.push(parsed);
    }

    fn finish_binds(&mut self, cfg: &mut Config) {
        use crate::input::keynames::ChordIdentity;
        let prefix = crate::input::mod_key_flags(&cfg.input.mod_key)
            .unwrap_or(crate::input::DEFAULT_MOD_KEY);
        let mut merged: Vec<(ChordIdentity, HotkeyBinding)> = Vec::new();
        for group in std::mem::take(&mut self.bind_groups) {
            let mut seen: std::collections::HashMap<
                ChordIdentity,
                super::diagnostics::SourceLabel,
            > = std::collections::HashMap::new();
            for located in group {
                let Ok(id) = ChordIdentity::new(&located.bind.modifiers, &located.bind.key, prefix)
                else {
                    continue;
                };
                if let Some(first) = seen.get(&id) {
                    let label = located.label;
                    self.issues.push(ValidationIssue {
                        severity: ValidationSeverity::Error, file: label.file, line: label.line, column: label.column,
                        message: format!("duplicate chord {}", located.bind.chord_text()),
                        context: super::diagnostics::DiagnosticContext {
                            source_line: label.source_line, related: vec![first.clone()], includes: located.includes,
                            help: "remove one binding; included configuration parts may override a chord".into(),
                        },
                    });
                    continue;
                }
                seen.insert(id, located.label);
                // Includes override only matching chords, not the entire table.
                merged.retain(|(old, _)| *old != id);
                merged.push((id, located.bind));
            }
        }
        cfg.binds.hotkeys = merged.into_iter().map(|(_, bind)| bind).collect();
    }

    fn decode_bind_release(&mut self, node: &Node, value: &Value, bind: &mut HotkeyBinding) {
        if let Some(release) = self.value_bool(node, value) {
            bind.release = release;
        }
    }

    fn decode_bind(&mut self, node: &Node) -> Option<HotkeyBinding> {
        // chord
        let parts: Vec<&str> = node.name.split('+').collect();
        let key = parts.last().map(|k| k.trim()).unwrap_or("");
        if key.is_empty() {
            self.error(
                node.pos,
                format!("bind '{}' has no key (for + write Plus)", node.name),
            );
            return None;
        }
        let modifiers: Vec<String> = parts[..parts.len() - 1]
            .iter()
            .map(|m| m.trim().to_string())
            .collect();

        // the action
        let kids = node.kids();
        let niri_form = !kids.is_empty();
        let mut bind = HotkeyBinding {
            modifiers,
            key: key.to_string(),
            ..HotkeyBinding::default()
        };
        let action_pos: Pos;
        let mut ok = true;
        if niri_form {
            for a in &node.args {
                self.error(
                    a.pos,
                    "a bind with a { } block takes no arguments; put the action inside the braces",
                );
                ok = false;
            }
            for extra in kids.iter().skip(1) {
                self.error(extra.pos, "only one action is allowed per bind");
                ok = false;
            }
            let action = &kids[0];
            action_pos = action.pos;
            bind.command = action.name.clone();
            for a in &action.args {
                bind.args.push(a.to_text());
                bind.args_quoted.push(a.is_quoted_str());
            }
            let allowed = action_props(&action.name);
            for p in &action.props {
                if allowed.contains(&p.key.as_str()) {
                    if p.key == "workspace" {
                        // Preserve the quote bit: workspace="2" names a workspace.
                        bind.args.push("--workspace".to_string());
                        bind.args_quoted.push(false);
                        bind.args.push(p.value.to_text());
                        bind.args_quoted.push(p.value.is_quoted_str());
                    } else {
                        bind.action_props.push((p.key.clone(), p.value.to_text()));
                    }
                } else {
                    self.error(
                        p.key_pos,
                        format!("unexpected property '{}' on '{}'", p.key, action.name),
                    );
                    ok = false;
                }
            }
            if let Some(grand) = action.kids().first() {
                self.error(grand.pos, format!("'{}' takes no children", action.name));
                ok = false;
            }
        } else {
            // wiri's older line: `Ctrl+Alt+Left focus-column-left`, `Ctrl+Alt+1 switch-workspace 1`
            match node.args.first() {
                Some(first) => match first.as_str() {
                    Some(name) => {
                        action_pos = first.pos;
                        bind.command = name.to_string();
                        for a in node.args.iter().skip(1) {
                            bind.args.push(a.to_text());
                            bind.args_quoted.push(a.is_quoted_str());
                        }
                    }
                    None => {
                        self.error(first.pos, "the action of a bind must be a name");
                        return None;
                    }
                },
                None => {
                    self.error(
                        node.pos,
                        format!(
                            "bind {} has no action (write {} {{ action-name; }})",
                            node.name, node.name
                        ),
                    );
                    return None;
                }
            }
        }

        // bind properties
        for p in &node.props {
            match p.key.as_str() {
                "repeat" => {
                    if let Some(v) = self.value_bool(node, &p.value) {
                        bind.repeat = v;
                    }
                }
                // Compatibility extension: pinned niri has no generic release property.
                "on-release" | "release" => {
                    self.decode_bind_release(node, &p.value, &mut bind);
                }
                "cooldown-ms" => {
                    // niri: `cooldown = Duration::from_millis(u64)`
                    // (niri-config/src/binds.rs:848-852): any whole millisecond count.
                    if let Some(v) = self.value_number(node, &p.value, 0.0, u32::MAX as f64) {
                        if v.fract() == 0.0 {
                            bind.cooldown_ms = Some(v as u32);
                        } else {
                            self.error(p.value.pos, "cooldown-ms expects an integer");
                        }
                    }
                }
                "allow-when-locked" => {
                    if let Some(v) = self.value_bool(node, &p.value) {
                        bind.allow_when_locked = v;
                    }
                }
                "allow-inhibiting" => {
                    if let Some(v) = self.value_bool(node, &p.value) {
                        bind.allow_inhibiting = v;
                    }
                }
                "hotkey-overlay-title" => match &p.value.scalar {
                    Scalar::Null => bind.hotkey_overlay_title = Some(None),
                    Scalar::Str { text, .. } => {
                        // niri paints the title with Pango's set_markup (wiki
                        // Configuration:-Key-Bindings, "Custom Hotkey Overlay Titles"), so
                        // an invalid title is not partially styled: it is shown as the
                        // whole original string.  Say so once, here at load, instead of on
                        // every paint; the title itself is kept verbatim so the overlay can
                        // fall back and a later reload can succeed.
                        if let Err(error) = crate::overlay::pango_markup::parse(
                            text,
                            &crate::overlay::pango_markup::Style::overlay_body(),
                        ) {
                            self.warn(
                                p.value.pos,
                                format!(
                                    "hotkey-overlay-title \"{}\" is not valid Pango markup ({}) so it is shown literally; escape '&' as '&amp;' and '<' as '&lt;'",
                                    text, error
                                ),
                            );
                        }
                        bind.hotkey_overlay_title = Some(Some(text.clone()))
                    }
                    _ => {
                        self.error(p.value.pos, "'hotkey-overlay-title' expects text or null");
                        ok = false;
                    }
                },
                other => {
                    self.error(
                        p.key_pos,
                        format!(
                            "unexpected property '{}' (binds take repeat, cooldown-ms, allow-when-locked, allow-inhibiting, hotkey-overlay-title)",
                            other
                        ),
                    );
                    ok = false;
                }
            }
        }
        if !ok {
            return None;
        }

        let chord = bind.chord_text();

        // modifiers
        if let Err(e) = bind.resolve_modifiers(crate::input::DEFAULT_MOD_KEY) {
            self.error(
                node.pos,
                format!("bind {}: {}; the bind is ignored", chord, e),
            );
            return None;
        }

        // key
        if bind.vk_code().is_none() && crate::input::pointer::parse_trigger(&bind.key).is_none() {
            let lower = bind.key.to_ascii_lowercase();
            if lower.starts_with("tabletstylusbutton") {
                // niri's stylus buttons are 1-3; anything else is a typo, not a wiri gap.
                self.error(
                    node.pos,
                    format!(
                        "bind {}: '{}' is not a stylus button (wiri decodes TabletStylusButton1, TabletStylusButton2 and TabletStylusButton3); the bind is ignored",
                        chord, bind.key
                    ),
                );
                return None;
            }
            let niri_only = lower.starts_with("mouse")
                || lower.starts_with("wheelscroll")
                || lower.starts_with("touchpadscroll");
            if niri_only {
                self.warn(
                    node.pos,
                    format!(
                        "bind {}: mouse and wheel triggers are not supported by wiri yet; the bind is ignored",
                        chord
                    ),
                );
            } else if lower.starts_with("xf86") || lower.starts_with("iso_") {
                self.warn(
                    node.pos,
                    format!(
                        "bind {}: the key '{}' has no Windows virtual key; the bind is ignored",
                        chord, bind.key
                    ),
                );
            } else {
                self.error(
                    node.pos,
                    format!("bind {}: unknown key '{}'", chord, bind.key),
                );
            }
            return None;
        }

        // action
        if bind.parse_action().is_none() {
            let name = bind.command.clone();
            if crate::input::action_name_known(&name) {
                self.error(
                    action_pos,
                    format!(
                        "bind {}: bad arguments for '{}'{}",
                        chord,
                        name,
                        if bind.args.is_empty() {
                            " (it needs one)".to_string()
                        } else {
                            format!(" ({})", bind.args.join(" "))
                        }
                    ),
                );
            } else if NIRI_ACTIONS.contains(&name.as_str()) {
                self.warn(
                    action_pos,
                    format!(
                        "bind {}: the action '{}' is not implemented by wiri yet; the bind is ignored",
                        chord, name
                    ),
                );
            } else {
                let hint = suggestion(&name, NIRI_ACTIONS);
                let message = match hint {
                    Some(h) => format!(
                        "bind {}: unknown action '{}' (did you mean '{}'?)",
                        chord, name, h
                    ),
                    None => format!("bind {}: unknown action '{}'", chord, name),
                };
                self.error(action_pos, message);
            }
            return None;
        }
        Some(bind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(text: &str) -> Decoded {
        decode_text(text, "", None)
    }

    fn errors(d: &Decoded) -> Vec<String> {
        d.issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Error)
            .map(|i| i.to_string())
            .collect()
    }

    fn warnings(d: &Decoded) -> Vec<String> {
        d.issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Warning)
            .map(|i| i.to_string())
            .collect()
    }

    /// The vocabulary the live completeness gates enumerate is exactly the two
    /// halves below: nothing may be silently outside them, and nothing may be in
    /// both.  A new niri action therefore cannot land without either being bound
    /// by the gate or being listed (with a reason) as not implemented.
    #[test]
    fn the_bindable_action_list_partitions_the_niri_vocabulary() {
        let bindable = super::bindable_niri_actions();
        for (i, name) in super::NIRI_ACTIONS.iter().enumerate() {
            assert!(
                !super::NIRI_ACTIONS[..i].contains(name),
                "{name} is listed twice in NIRI_ACTIONS"
            );
        }
        for name in &bindable {
            assert!(
                super::NIRI_ACTIONS.contains(name),
                "{name} is bindable but not a niri action"
            );
            assert!(
                !super::UNIMPLEMENTED_NIRI_ACTIONS.contains(name),
                "{name} is both bindable and unimplemented"
            );
        }
        for name in super::UNIMPLEMENTED_NIRI_ACTIONS {
            assert!(
                super::NIRI_ACTIONS.contains(name),
                "{name} is listed as unimplemented but is not a niri action"
            );
        }
        assert_eq!(
            bindable.len() + super::UNIMPLEMENTED_NIRI_ACTIONS.len(),
            super::NIRI_ACTIONS.len(),
            "the gate list and the unimplemented list must add up to the vocabulary"
        );
    }

    /// The same check the `h79_action_names_all_parse` live gate makes, one bind
    /// per name with the arguments the parser itself supplies.
    #[test]
    fn every_bindable_niri_action_binds_without_a_diagnostic() {
        for name in super::bindable_niri_actions() {
            let args = crate::input::sample_action_args(name)
                .unwrap_or_else(|| panic!("{name} is bindable but no argument probe parses"));
            let mut line = format!("Ctrl+Alt+Shift+F7 {name}");
            for word in &args {
                line.push(' ');
                line.push_str(word);
            }
            line.push(';');
            let decoded = load(&format!("binds {{ {line} }}"));
            assert_eq!(
                decoded.config.binds.hotkeys.len(),
                1,
                "{name} did not bind: {line}"
            );
            assert!(
                decoded.issues.is_empty(),
                "{name} produced diagnostics: {:?}",
                decoded.issues
            );
        }
    }

    /// A stale exclusion would hide a newly implemented action from the gate.
    #[test]
    fn every_unimplemented_niri_action_is_really_unimplemented() {
        for name in super::UNIMPLEMENTED_NIRI_ACTIONS {
            assert!(
                !crate::input::action_name_known(name),
                "{name} is implemented now; move it out of UNIMPLEMENTED_NIRI_ACTIONS"
            );
        }
    }

    fn clean(text: &str) -> Config {
        let d = load(text);
        assert!(d.issues.is_empty(), "unexpected issues: {:?}", d.issues);
        d.config
    }

    #[test]
    fn an_empty_config_is_the_default() {
        let cfg = clean("");
        assert_eq!(cfg.input.keyboard_layout, "us");
        assert_eq!(cfg.output.len(), 1);
        assert!(cfg.binds.hotkeys.is_empty());
    }

    #[test]
    fn debug_strict_new_window_focus_policy_is_a_flag_and_other_debug_options_are_reported() {
        // niri:niri-config/src/debug.rs, wiki:Configuration:-Debug-Options (25.01).
        assert!(!clean("").debug.strict_new_window_focus_policy);
        assert!(
            clean("debug { strict-new-window-focus-policy; }")
                .debug
                .strict_new_window_focus_policy
        );
        assert!(
            clean("debug { strict-new-window-focus-policy true; }")
                .debug
                .strict_new_window_focus_policy
        );
        assert!(
            !clean("debug { strict-new-window-focus-policy false; }")
                .debug
                .strict_new_window_focus_policy
        );
        // Every other niri debug option configures compositor internals: a warning, the
        // strict flag next to it still applies, and the config still loads.
        let d = load("debug {\n    disable-direct-scanout\n    strict-new-window-focus-policy\n}");
        assert!(!d.has_errors(), "{:?}", d.issues);
        assert!(d.config.debug.strict_new_window_focus_policy);
        assert_eq!(warnings(&d).len(), 1, "{:?}", d.issues);
        assert!(warnings(&d)[0].contains("disable-direct-scanout"));
        // A value that is not a boolean is an error, not a silent default.
        assert!(load("debug { strict-new-window-focus-policy \"maybe\"; }").has_errors());
    }

    #[test]
    fn accessibility_is_on_by_default_and_can_be_turned_off() {
        assert!(clean("").accessibility);
        assert!(!clean("accessibility false").accessibility);
        assert!(!clean("accessibility off").accessibility);
        assert!(!clean("accessibility { off; }").accessibility);
        assert!(clean("accessibility { enabled true; }").accessibility);
        assert!(clean("accessibility true").accessibility);
        assert!(load("accessibility { maybe; }").has_errors());
    }

    #[test]
    fn block_out_from_is_decoded_in_window_and_layer_rules() {
        use crate::window::block_out::BlockOutFrom;
        let cfg = clean(
            "window-rule {\n    match title=\"video\"\n    block-out-from \"screencast\"\n}\n\
             layer-rule {\n    match namespace=\"workspace-indicator\"\n    block-out-from \"screen-capture\"\n}\n",
        );
        assert_eq!(cfg.window_rules.len(), 1);
        assert_eq!(
            cfg.window_rules[0].block_out_from,
            Some(BlockOutFrom::Screencast)
        );
        assert_eq!(cfg.layer_rules.len(), 1);
        assert_eq!(
            cfg.layer_rules[0].block_out_from,
            Some(BlockOutFrom::ScreenCapture)
        );
        // The layer rule's namespace is the overlay name it matches.
        assert_eq!(cfg.layer_rules[0].match_groups.len(), 1);
        // A value niri does not have is an error that names both spellings.
        let bad = load("window-rule { block-out-from \"screen\" }");
        assert!(bad.has_errors());
        assert!(
            errors(&bad)
                .iter()
                .any(|e| e.contains("screencast") && e.contains("screen-capture")),
            "{:?}",
            errors(&bad)
        );
        // Without the key, nothing is blocked out.
        assert_eq!(
            clean("window-rule { opacity 0.5; }").window_rules[0].block_out_from,
            None
        );
    }

    #[test]
    fn a_layer_rule_warns_about_niri_properties_wiri_cannot_use() {
        let warnings = warnings(&load(
            "layer-rule { match namespace=\"bar\"; shadow { on; } }",
        ));
        assert!(
            warnings.iter().any(|w| w.contains("shadow")),
            "{warnings:?}"
        );
        assert!(errors(&load(
            "layer-rule { match namespace=\"bar\"; shadow { on; } }"
        ))
        .is_empty());
    }

    #[test]
    fn screenshot_path_is_a_template_or_null() {
        use crate::screenshot::path::ScreenshotPath;
        assert_eq!(clean("").screenshot_path, ScreenshotPath::Pictures);
        assert_eq!(
            clean("screenshot-path null").screenshot_path,
            ScreenshotPath::ClipboardOnly
        );
        assert_eq!(
            clean("screenshot-path \"~/Shots/%Y-%m-%d.png\"").screenshot_path,
            ScreenshotPath::Template("~/Shots/%Y-%m-%d.png".into())
        );
        assert!(load("screenshot-path \"\"").has_errors());
        assert!(load("screenshot-path true").has_errors());
    }

    #[test]
    fn syntax_errors_carry_line_and_column() {
        let d = load("input {\n    repeat-delay 250\n    layout \"us\n}\n");
        assert!(d.has_errors());
        let first = d.first_error().unwrap();
        assert_eq!(first.line, 3);
        assert!(first.message.contains("never closed"), "{}", first.message);
    }

    #[test]
    fn unknown_keys_are_errors_with_a_suggestion() {
        let d = load("layout {\n    gaps 8\n    boarder { width 2 }\n}\ninput {\n    repeat-delay 300\n    repeat-rat 25\n}\n");
        let errs = errors(&d);
        assert_eq!(errs.len(), 2, "{:?}", errs);
        assert!(
            errs[0].starts_with("3:5: error: unknown option 'boarder' in 'layout'"),
            "{}",
            errs[0]
        );
        assert!(errs[0].contains("did you mean 'border'"), "{}", errs[0]);
        assert!(
            errs[1].starts_with("7:5: error: unknown option 'repeat-rat'"),
            "{}",
            errs[1]
        );
        assert!(errs[1].contains("repeat-rate"), "{}", errs[1]);
        let d = load("telemetry { on }\n");
        assert_eq!(errors(&d).len(), 1);
        assert!(errors(&d)[0].contains("unknown section 'telemetry'"));
        let d = load("layuot { gaps 8 }\n");
        assert!(
            errors(&d)[0].contains("did you mean 'layout'"),
            "{:?}",
            errors(&d)
        );
    }

    #[test]
    fn bad_values_are_errors_at_the_value() {
        let d = load(
            "layout {\n    inner-gaps \"eight\"\n    border-color \"not-a-color\"\n    scroll-step -5\n}\n",
        );
        let errs = errors(&d);
        assert_eq!(errs.len(), 3, "{:?}", errs);
        assert!(
            errs[0].starts_with("2:16: error: 'inner-gaps' expects a number"),
            "{}",
            errs[0]
        );
        assert!(errs[1].contains("not a color"), "{}", errs[1]);
        assert!(errs[2].contains("must be between"), "{}", errs[2]);
        let d = load("input { repeat-delay true }\n");
        assert!(
            errors(&d)[0].contains("expects a number, found a boolean"),
            "{:?}",
            errors(&d)
        );
    }

    #[test]
    fn niri_input_syntax() {
        let cfg = clean(
            "input {\n    keyboard {\n        repeat-delay 300\n        repeat-rate 40\n        numlock\n    }\n    mouse {\n        natural-scroll\n    }\n    focus-follows-mouse\n    warp-mouse-to-focus\n    workspace-auto-back-and-forth\n    mod-key \"Super\"\n}\n",
        );
        assert_eq!(cfg.input.repeat_delay, 300);
        assert_eq!(cfg.input.repeat_rate, 40);
        assert!(cfg.input.numlock);
        assert!(cfg.input.natural_scroll);
        assert!(cfg.input.focus_follows_mouse);
        assert!(cfg.input.warp_on_focus);
        assert!(cfg.input.workspace_auto_back_and_forth);
        assert_eq!(cfg.input.mod_key, "super");
        // `false` turns a flag off.
        let cfg = clean("input { focus-follows-mouse false\n natural-scroll false }\n");
        assert!(!cfg.input.focus_follows_mouse);
        assert!(!cfg.input.natural_scroll);
    }

    #[test]
    fn niri_input_options_wiri_lacks_are_warnings() {
        let d = load("input {\n    touchpad {\n        tap\n        dwt\n    }\n    keyboard { xkb { layout \"us\" } }\n    mod-key-nested \"Alt\"\n}\n");
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        // Lane P accepts `tap` and `dwt` inside the touchpad block and C2 requests a configured
        // xkb layout natively, so only `mod-key-nested` is still reported.
        assert_eq!(warnings(&d).len(), 1, "{:?}", warnings(&d));
        assert!(d.config.input.touchpad_enabled);
        assert!(d.config.input.keyboard_layout_configured);
        assert_eq!(d.config.input.tap_to_click, true);
        assert_eq!(d.config.input.touchpad_dwt, Some(true));
        assert!(warnings(&d)[0].contains("'mod-key-nested' is not supported"));
    }

    #[test]
    fn pointing_device_options_are_stored_per_niri_spelling() {
        let cfg = clean(
            "input {\n    mouse {\n        accel-profile \"flat\"\n        left-handed\n        middle-emulation false\n        scroll-method \"on-button-down\"\n        scroll-button 274\n        scroll-button-lock\n    }\n    touchpad {\n        dwt\n        dwtp false\n        drag true\n        drag-lock\n        click-method \"clickfinger\"\n        disabled-on-external-mouse\n        tap-button-map {\n            left 1\n            middle 3\n            right 2\n        }\n    }\n    trackpoint { off }\n    trackball { off false }\n}\n",
        );
        assert_eq!(cfg.input.accel_profile, Some(AccelProfile::Flat));
        assert_eq!(cfg.input.left_handed, Some(true));
        assert_eq!(cfg.input.middle_emulation, Some(false));
        assert_eq!(cfg.input.scroll_method, Some(ScrollMethod::OnButtonDown));
        assert_eq!(cfg.input.scroll_button, Some(274));
        assert_eq!(cfg.input.scroll_button_lock, Some(true));
        assert!(
            cfg.input.touchpad_enabled,
            "a touchpad block enables wiri's reader"
        );
        assert_eq!(cfg.input.touchpad_dwt, Some(true));
        assert_eq!(cfg.input.touchpad_dwtp, Some(false));
        assert_eq!(cfg.input.touchpad_drag, Some(true));
        assert_eq!(cfg.input.touchpad_drag_lock, Some(true));
        assert_eq!(
            cfg.input.touchpad_click_method,
            Some(ClickMethod::Clickfinger)
        );
        assert_eq!(cfg.input.touchpad_disabled_on_external_mouse, Some(true));
        assert_eq!(
            cfg.input.touchpad_tap_button_map,
            Some(TapButtonMap {
                left: 1,
                middle: 3,
                right: 2
            })
        );
        assert!(cfg.input.trackpoint_off);
        assert!(!cfg.input.trackball_off, "off false leaves the device on");
        // Nothing configured: every pointing option stays untouched for the OS.
        let untouched = clean("");
        assert_eq!(untouched.input.accel_profile, None);
        assert_eq!(untouched.input.left_handed, None);
        assert_eq!(untouched.input.touchpad_dwt, None);
        assert_eq!(untouched.input.touchpad_disabled_on_external_mouse, None);
    }

    #[test]
    fn parity_axis_scroll_factor_accepts_property_only_mouse_axes() {
        let decoded = load("input { mouse { scroll-factor horizontal=2 vertical=-1 } }");
        assert!(
            decoded.issues.is_empty(),
            "valid niri axis factors must load without diagnostics: {:?}",
            decoded.issues
        );
    }

    #[test]
    fn parity_axis_scroll_factor_accepts_touchpad_axes() {
        let decoded = load("input { touchpad { scroll-factor horizontal=-1.5 vertical=0.5 } }");
        assert!(
            decoded.issues.is_empty(),
            "valid niri touchpad factors must load without diagnostics: {:?}",
            decoded.issues
        );
    }

    #[test]
    fn parity_axis_scroll_factor_stores_and_resolves_defaults_base_and_overrides() {
        for (syntax, base, horizontal, vertical, expected) in [
            ("scroll-factor", None, None, None, (1.0, 1.0)),
            ("scroll-factor 2", Some(2.0), None, None, (2.0, 2.0)),
            (
                "scroll-factor horizontal=2",
                None,
                Some(2.0),
                None,
                (2.0, 1.0),
            ),
            (
                "scroll-factor vertical=-1.5",
                None,
                None,
                Some(-1.5),
                (1.0, -1.5),
            ),
            (
                "scroll-factor 2 vertical=-1",
                Some(2.0),
                None,
                Some(-1.0),
                (2.0, -1.0),
            ),
            (
                "scroll-factor 0 horizontal=100 vertical=-100",
                Some(0.0),
                Some(100.0),
                Some(-100.0),
                (100.0, -100.0),
            ),
        ] {
            for scope in ["mouse", "touchpad"] {
                let config = clean(&format!("input {{ {scope} {{ {syntax} }} }}"));
                let stored = if scope == "mouse" {
                    config.input.mouse_scroll_factor
                } else {
                    config.input.touchpad_scroll_factor
                };
                let factor = stored.expect("explicit identity still preserves presence");
                assert_eq!(
                    (factor.base, factor.horizontal, factor.vertical),
                    (base, horizontal, vertical)
                );
                assert_eq!(factor.h_v_factors(), expected);
            }
        }
    }

    #[test]
    fn parity_axis_scroll_factor_keeps_mouse_and_touchpad_settings_independent() {
        let mouse = "mouse { scroll-factor 2 vertical=-1 }";
        let touchpad = "touchpad { scroll-factor 4 horizontal=-3 }";
        for text in [
            format!("input {{ {mouse}; {touchpad} }}"),
            format!("input {{ {touchpad}; {mouse} }}"),
        ] {
            let config = clean(&text);
            assert_eq!(
                config.input.mouse_scroll_factor.unwrap().h_v_factors(),
                (2.0, -1.0)
            );
            assert_eq!(
                config.input.touchpad_scroll_factor.unwrap().h_v_factors(),
                (-3.0, 4.0)
            );
        }
    }

    #[test]
    fn parity_axis_scroll_factor_rejects_extra_base_arguments() {
        let decoded = load("input { mouse { scroll-factor 2 3 } }");
        assert!(
            decoded.has_errors(),
            "scroll-factor permits at most one base argument"
        );
    }

    #[test]
    fn parity_axis_scroll_factor_rejects_numeric_strings() {
        let decoded = load("input { mouse { scroll-factor \"2\" } }");
        assert!(
            decoded.has_errors(),
            "niri scroll factors require numbers, not numeric strings"
        );
    }

    #[test]
    fn parity_axis_scroll_factor_rejects_unknown_properties() {
        let decoded = load("input { mouse { scroll-factor 2 sideways=3 } }");
        assert!(
            decoded.has_errors(),
            "unknown scroll-factor properties must not be silently ignored"
        );
    }

    #[test]
    fn parity_axis_scroll_factor_rejects_children() {
        let decoded = load("input { mouse { scroll-factor 2 { ignored; } } }");
        assert!(decoded.has_errors(), "scroll-factor has no child nodes");
    }

    #[test]
    fn parity_axis_scroll_factor_rejects_bad_types_ranges_and_duplicate_properties() {
        for scope in ["mouse", "touchpad"] {
            for syntax in [
                "scroll-factor -0.01",
                "scroll-factor 100.01",
                "scroll-factor horizontal=-100.01",
                "scroll-factor vertical=100.01",
                "scroll-factor horizontal=\"2\"",
                "scroll-factor vertical=true",
                "scroll-factor null",
                "scroll-factor horizontal=1 horizontal=2",
                "scroll-factor (f64)2",
                "scroll-factor {}",
            ] {
                let decoded = load(&format!("input {{ {scope} {{ {syntax} }} }}"));
                assert!(
                    decoded.has_errors(),
                    "invalid factor loaded: {scope} {syntax}"
                );
            }
        }
    }

    #[test]
    fn parity_button_scroll_accepts_canonical_evdev_middle() {
        let cfg = clean(
            "input { mouse { scroll-method \"on-button-down\"; scroll-button 274; scroll-button-lock } }",
        );
        assert_eq!(cfg.input.scroll_method, Some(ScrollMethod::OnButtonDown));
        assert_eq!(cfg.input.scroll_button, Some(274));
        assert_eq!(cfg.input.scroll_button_lock, Some(true));
    }

    #[test]
    fn parity_button_scroll_accepts_zero_to_disable() {
        let cfg = clean("input { mouse { scroll-method \"on-button-down\"; scroll-button 0 } }");
        assert_eq!(cfg.input.scroll_button, Some(0));
    }

    #[test]
    fn parity_button_scroll_mouse_scope_survives_both_device_block_orders() {
        let mouse = "mouse { scroll-method \"on-button-down\"; scroll-button 274; scroll-button-lock; natural-scroll }";
        let touchpad = "touchpad { scroll-method \"two-finger\"; scroll-button 276; scroll-button-lock false; natural-scroll false }";
        for text in [
            format!("input {{ {mouse}; {touchpad} }}"),
            format!("input {{ {touchpad}; {mouse} }}"),
        ] {
            let cfg = clean(&text);
            assert_eq!(
                cfg.input.mouse_scroll.method,
                Some(ScrollMethod::OnButtonDown)
            );
            assert_eq!(cfg.input.mouse_scroll.button, Some(274));
            assert_eq!(cfg.input.mouse_scroll.lock, Some(true));
            assert!(cfg.input.mouse_scroll.natural);
            assert!(cfg.input.non_mouse_scroll_configured);
        }
    }

    #[test]
    fn parity_button_scroll_never_completes_partial_mouse_settings_from_touchpad() {
        for (mouse, want_method, want_button, want_lock) in [
            (
                "scroll-method \"on-button-down\"",
                Some(ScrollMethod::OnButtonDown),
                None,
                None,
            ),
            ("scroll-button 274", None, Some(274), None),
            ("scroll-button-lock", None, None, Some(true)),
        ] {
            let other = "touchpad { scroll-method \"on-button-down\"; scroll-button 276; scroll-button-lock }";
            for text in [
                format!("input {{ mouse {{ {mouse} }}; {other} }}"),
                format!("input {{ {other}; mouse {{ {mouse} }} }}"),
            ] {
                let cfg = clean(&text);
                assert_eq!(cfg.input.mouse_scroll.method, want_method);
                assert_eq!(cfg.input.mouse_scroll.button, want_button);
                assert_eq!(cfg.input.mouse_scroll.lock, want_lock);
            }
        }
        let cfg =
            clean("input { touchpad { scroll-method \"on-button-down\"; scroll-button 274 } }");
        assert_eq!(cfg.input.mouse_scroll, MouseScrollConfig::default());
    }

    #[test]
    fn parity_button_scroll_accepts_all_u32_codes_and_reports_unsupported_or_legacy() {
        for code in [8, 99, 277, u32::MAX] {
            let decoded = load(&format!("input {{ mouse {{ scroll-button {code} }} }}"));
            assert!(errors(&decoded).is_empty());
            assert_eq!(decoded.config.input.mouse_scroll.button, Some(code));
            assert_eq!(warnings(&decoded).len(), 1);
            assert!(warnings(&decoded)[0].contains("unsupported"));
        }
        for code in 1..=5 {
            let decoded = load(&format!("input {{ mouse {{ scroll-button {code} }} }}"));
            assert!(errors(&decoded).is_empty());
            assert_eq!(warnings(&decoded).len(), 1);
            assert!(warnings(&decoded)[0].contains("legacy"));
        }
    }

    #[test]
    fn pointing_device_options_reject_bad_values() {
        let cases = [
            "input { mouse { accel-profile \"bogus\" } }",
            "input { mouse { scroll-method \"bogus\" } }",
            "input { mouse { scroll-button -1 } }",
            "input { mouse { scroll-button 1.5 } }",
            "input { mouse { scroll-button 4294967296 } }",
            "input { touchpad { click-method \"bogus\" } }",
            "input { touchpad { tap-button-map { left 0 } } }",
            "input { touchpad { tap-button-map { thumb 1 } } }",
            // `dwt` is a touchpad option; niri rejects it on a mouse.
            "input { mouse { dwt } }",
            // A device type wiri does not have.
            "input { joystick { off } }",
        ];
        for case in cases {
            let d = load(case);
            assert!(d.has_errors(), "{case:?} should be rejected");
        }
    }

    #[test]
    fn touchpad_off_and_mouse_off_disable_their_device_type() {
        let cfg = clean("input {\n    touchpad { off }\n    mouse { off }\n}");
        assert!(!cfg.input.touchpad_enabled);
        assert!(cfg.input.mouse_off);
        // A bare touchpad block is wiri's opt-in for reading its raw contacts.
        assert!(clean("input { touchpad { dwt } }").input.touchpad_enabled);
    }

    #[test]
    fn hotkey_overlay_options_are_parsed() {
        let cfg = clean("hotkey-overlay {\n    skip-at-startup\n    hide-not-bound true\n}");
        assert!(cfg.hotkey_overlay.skip_at_startup);
        assert!(cfg.hotkey_overlay.hide_not_bound);
        let defaults = clean("");
        assert!(!defaults.hotkey_overlay.skip_at_startup);
        assert!(!defaults.hotkey_overlay.hide_not_bound);
        // `false` turns the startup sheet back on.
        assert!(
            !clean("hotkey-overlay { skip-at-startup false }")
                .hotkey_overlay
                .skip_at_startup
        );
        assert!(load("hotkey-overlay { skip-at-start }").has_errors());
    }

    #[test]
    fn the_layout_background_color_is_parsed_and_the_overview_backdrop_stays_in_its_section() {
        let cfg = clean(
            "layout { background-color \"#101820\" }
overview { backdrop-color \"#000000\" }",
        );
        assert_eq!(cfg.layout.background_color.as_deref(), Some("#101820"));
        assert_eq!(cfg.overview.backdrop_color, [0, 0, 0, 255]);
        assert!(
            cfg.overview.backdrop_set,
            "an explicit overview colour is remembered as such"
        );
        assert!(
            !clean("layout { background-color \"#101820\" }")
                .overview
                .backdrop_set
        );
        // niri moved the deprecated per-output key to `layout`; wiri maps it the same way and
        // says that it is deprecated.
        let d = load(
            "output \"DISPLAY1\" {
    background-color \"#202020\"
}",
        );
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        assert_eq!(d.config.layout.background_color.as_deref(), Some("#202020"));
        assert_eq!(warnings(&d).len(), 1, "{:?}", warnings(&d));
        assert!(warnings(&d)[0].contains("deprecated"));
        // Short form, alpha and a real colour are all accepted.
        let cfg = clean("layout { background-color \"#123\" }");
        assert_eq!(cfg.layout.background_color.as_deref(), Some("#112233"));
        assert!(load("layout { background-color \"not a colour\" }").has_errors());
        // The accent alias reads the system accent, which only the border colours paint.
        assert!(load("layout { background-color \"accent\" }").has_errors());
    }

    #[test]
    fn wiri_input_spellings_still_load() {
        let cfg = clean(
            "input {\n    layout \"de\"\n    repeat_delay=600\n    keyboard { layout \"us\"; repeat-rate 30 }\n    mouse {\n        natural-scroll false\n        scroll-speed 2.0\n        acceleration 0.5\n        sensitivity 1.5\n        tap-to-click true\n    }\n    focus {\n        follows-mouse true\n        focus-delay-ms 999\n        warp-on-focus true\n    }\n    touch {\n        enable true\n        tap-to-click false\n        scroll-speed 1.75\n    }\n    numlock true\n}\n",
        );
        assert_eq!(
            cfg.input.keyboard_layout, "us",
            "the keyboard block runs after the flat key"
        );
        assert_eq!(cfg.input.repeat_delay, 600);
        assert_eq!(cfg.input.repeat_rate, 30);
        assert!(!cfg.input.natural_scroll);
        assert!((cfg.input.mouse_speed - 2.0).abs() < 1e-9);
        assert!((cfg.input.mouse_acceleration - 0.5).abs() < 1e-9);
        assert!((cfg.input.mouse_sensitivity - 1.5).abs() < 1e-9);
        assert!(cfg.input.tap_to_click);
        assert!(cfg.input.focus_follows_mouse);
        assert_eq!(cfg.input.focus_delay_ms, 999);
        assert!(cfg.input.warp_on_focus);
        assert!(cfg.input.touch_enabled);
        assert!(!cfg.input.touch_tap_to_click);
        assert!((cfg.input.touch_scroll_speed - 1.75).abs() < 1e-9);
        assert!(cfg.input.numlock);
        // Legacy spellings and quoted booleans remain valid within one section.
        let cfg = clean(
            "input { natural-scroll \"true\"; repeat-delay 500; mouse { natural-scroll true; } }\n",
        );
        assert!(cfg.input.natural_scroll);
        assert_eq!(cfg.input.repeat_delay, 500);
    }

    #[test]
    fn niri_layout_syntax() {
        let cfg = clean(
            "layout {\n    gaps 12\n    struts { left 4; right 5; top 6; bottom 7; }\n    border { width 3; active-color \"#ffc87f\"; inactive-color \"#505050\"; urgent-color \"#9b0000\"; }\n    focus-ring { width 5; active-color \"#7fc8ff\"; inactive-color \"#404040\"; }\n    shadow { on; softness 20; spread 4; offset x=2 y=6; color \"#0007\"; }\n}\n",
        );
        assert_eq!(cfg.layout.inner_gaps, 12.0);
        assert_eq!(cfg.layout.outer_gaps, 12.0);
        assert_eq!(
            (
                cfg.layout.struts.left,
                cfg.layout.struts.right,
                cfg.layout.struts.top,
                cfg.layout.struts.bottom
            ),
            (
                InsetValue::Logical(4.0),
                InsetValue::Logical(5.0),
                InsetValue::Logical(6.0),
                InsetValue::Logical(7.0)
            )
        );
        assert_eq!(cfg.layout.border_width, 3);
        assert_eq!(cfg.layout.border_color_focused, "#ffc87f");
        assert_eq!(cfg.layout.border_color, "#505050");
        assert_eq!(cfg.layout.border_color_urgent, "#9b0000");
        assert_eq!(cfg.layout.focus_ring_width, 5);
        assert_eq!(cfg.layout.focus_ring_color, "#7fc8ff");
        assert_eq!(cfg.layout.focus_ring_inactive_color, "#404040");
        assert!(cfg.layout.shadow_enable);
        assert_eq!(cfg.layout.shadow_blur, 20);
        assert_eq!(cfg.layout.shadow_spread, 4);
        assert_eq!(
            (cfg.layout.shadow_offset_x, cfg.layout.shadow_offset_y),
            (2, 6)
        );
        assert_eq!(cfg.layout.shadow_color, "#00000077", "#rgba is expanded");
    }

    #[test]
    fn strip_policy_mixed_presets_and_signed_struts_decode() {
        let cfg = clean("layout {\n center-focused-column \"on-overflow\"\n always-center-single-column\n default-column-width { proportion 0.5; }\n preset-column-widths { fixed 300; proportion 0.5; fixed 800; }\n gaps 12\n struts { left -20; right 30; top 10; bottom -5; }\n}\n");
        assert_eq!(cfg.layout.center_focused_column, "on-overflow");
        assert!(cfg.layout.always_center_single_column);
        assert_eq!(
            cfg.layout.default_column_width,
            Some(Some(SizeSpec::Proportion(0.5)))
        );
        assert_eq!(
            cfg.layout.preset_column_widths,
            vec![
                SizeSpec::Fixed(300),
                SizeSpec::Proportion(0.5),
                SizeSpec::Fixed(800)
            ]
        );
        assert_eq!(
            (
                cfg.layout.struts.left,
                cfg.layout.struts.right,
                cfg.layout.struts.top,
                cfg.layout.struts.bottom
            ),
            (
                InsetValue::Logical(-20.0),
                InsetValue::Logical(30.0),
                InsetValue::Logical(10.0),
                InsetValue::Logical(-5.0)
            )
        );
        assert_eq!(cfg.layout.inner_gaps, 12.0);
    }

    #[test]
    fn invalid_strip_policy_and_preset_size_are_errors() {
        let d = load(
            "layout { center-focused-column \"sometimes\"; preset-column-widths { fixed -10; } }\n",
        );
        assert_eq!(errors(&d).len(), 2);
    }

    #[test]
    fn empty_default_column_width_leaves_the_size_to_the_window() {
        let cfg = clean("layout { default-column-width {} }\n");
        assert_eq!(cfg.layout.default_column_width, Some(None));
    }

    #[test]
    fn the_layout_options_that_were_missing_are_decoded() {
        let d = load("layout {\n    center-focused-column \"never\"\n    preset-column-widths { proportion 0.5; }\n    tab-indicator { off }\n    empty-workspace-above-first\n    preset-window-heights { proportion 0.25; fixed 400; }\n    fullscreen-backdrop-color \"#101010\"\n}\n");
        assert!(errors(&d).is_empty());
        assert!(
            warnings(&d).is_empty(),
            "all four are implemented now: {:?}",
            warnings(&d)
        );
        assert!(d.config.layout.effects.tab_indicator.off);
        assert_eq!(d.config.layout.center_focused_column, "never");
        assert_eq!(
            d.config.layout.preset_column_widths,
            vec![SizeSpec::Proportion(0.5)]
        );
        assert!(d.config.layout.empty_workspace_above_first);
        assert_eq!(
            d.config.layout.preset_window_heights,
            vec![SizeSpec::Proportion(0.25), SizeSpec::Fixed(400)]
        );
        assert_eq!(
            d.config.layout.fullscreen_backdrop_color.as_deref(),
            Some("#101010")
        );
    }

    #[test]
    fn a_bad_fullscreen_backdrop_colour_is_an_error() {
        let d = load("layout { fullscreen-backdrop-color \"not a colour\" }\n");
        assert_eq!(errors(&d).len(), 1);
        assert_eq!(d.config.layout.fullscreen_backdrop_color, None);
    }

    #[test]
    fn a_bad_preset_window_height_is_an_error_not_a_silent_drop() {
        // niri keeps `proportion` unbounded and `fixed` an integer `i32`
        // (niri-config/src/layout.rs:157-162): a decimal fixed size is a type error, not a
        // value to round.
        let d = load("layout { preset-window-heights { fixed 0.5 } }\n");
        assert_eq!(errors(&d).len(), 1);
        let zero = load("layout { preset-window-heights { fixed 0 } }\n");
        assert_eq!(errors(&zero).len(), 0, "niri accepts a zero fixed size");
    }

    #[test]
    fn border_off_and_focus_ring_off() {
        let cfg = clean("layout { border { off }\n focus-ring { off } }\n");
        assert_eq!(cfg.layout.border_width, 0);
        assert_eq!(cfg.layout.focus_ring_width, 0);
    }

    #[test]
    fn wiri_layout_spellings_still_load() {
        let cfg = clean(
            "layout {\n    gaps { inner 10; outer 6 }\n    borders { width 4; color \"#4d4d4d\"; color-focused \"#3381d9\"; color-urgent \"#cc3344\"; radius 8 }\n    focus-ring { width 3; color \"#3381d9\"; gap 6; inactive-color \"#404040\" }\n    shadows { enable true; opacity 0.75; offset-x 5; offset-y 5; blur 15; spread 5 }\n    column-width-mode \"proportional\"\n    max-visible-columns 4\n    column-width 700\n    scroll-step 300\n    hide-method hide\n    dim-unfocused 0.92\n    snap-on-drag false\n    snap-threshold-px 32\n    smart-borders true\n    reserve-top 30\n}\n",
        );
        assert_eq!((cfg.layout.inner_gaps, cfg.layout.outer_gaps), (10.0, 6.0));
        assert_eq!(cfg.layout.border_width, 4);
        assert_eq!(cfg.layout.border_color, "#4d4d4d");
        assert_eq!(cfg.layout.border_color_focused, "#3381d9");
        assert_eq!(cfg.layout.border_color_urgent, "#cc3344");
        assert_eq!(cfg.layout.border_radius, 8);
        assert_eq!(cfg.layout.focus_ring_width, 3);
        assert_eq!(cfg.layout.focus_ring_color, "#3381d9");
        assert_eq!(cfg.layout.focus_ring_gap, 6);
        assert_eq!(cfg.layout.focus_ring_inactive_color, "#404040");
        assert!(cfg.layout.shadow_enable);
        assert!((cfg.layout.shadow_opacity - 0.75).abs() < 1e-9);
        assert_eq!(
            (cfg.layout.shadow_offset_x, cfg.layout.shadow_offset_y),
            (5, 5)
        );
        assert_eq!(cfg.layout.shadow_blur, 15);
        assert_eq!(cfg.layout.shadow_spread, 5);
        assert_eq!(cfg.layout.column_width_mode, "proportional");
        assert_eq!(cfg.layout.max_visible_columns, 4);
        assert_eq!(cfg.layout.column_width, 700);
        assert_eq!(cfg.layout.scroll_step, 300);
        assert_eq!(cfg.layout.hide_method, "hide");
        assert!((cfg.layout.dim_unfocused - 0.92).abs() < 1e-6);
        assert!(!cfg.layout.snap_on_drag);
        assert_eq!(cfg.layout.snap_threshold_px, 32);
        assert!(cfg.layout.smart_borders);
        assert_eq!(cfg.layout.struts.top, InsetValue::Physical(30));
        // The older flat spellings.
        let cfg = clean("layout {\n    inner_gaps 12\n    border-width 3\n    border-color \"#333333\"\n    focus-ring-width 4\n    shadow-enable true\n    hide-method \"cloak\"\n}\n");
        assert_eq!(cfg.layout.inner_gaps, 12.0);
        assert_eq!(cfg.layout.border_width, 3);
        assert_eq!(cfg.layout.focus_ring_width, 4);
        assert!(cfg.layout.shadow_enable);
        assert_eq!(cfg.layout.hide_method, "cloak");
    }

    #[test]
    fn an_output_block_in_both_spellings() {
        let cfg = clean(
            "output \"DP-1\" {\n    x 0\n    y 0\n    width 2560\n    height 1440\n    scale 1.5\n    enable true\n    mod-key \"super\"\n    layout { column-width 800; inner-gaps 20 }\n}\noutput \"eDP-1\" {\n    mode \"1920x1080@60\"\n    scale 2\n    transform \"90\"\n    position x=1280 y=0\n    variable-refresh-rate\n    focus-at-startup\n}\noutput \"HDMI-1\" { off }\n",
        );
        assert_eq!(cfg.output.len(), 3);
        let a = &cfg.output[0];
        assert_eq!((a.name.as_str(), a.width, a.height), ("DP-1", 2560, 1440));
        assert!((a.scale - 1.5).abs() < 1e-9);
        assert_eq!(a.mod_key.as_deref(), Some("super"));
        let lo = a.layout_override.as_ref().expect("layout override");
        assert_eq!(lo.column_width, Some(800));
        assert_eq!(lo.inner_gaps, Some(20.0));
        assert!(lo.scroll_step.is_none());
        let b = &cfg.output[1];
        assert_eq!(b.mode, "1920x1080@60");
        assert_eq!(b.transform, "90");
        assert_eq!((b.position.x, b.position.y), (1280, 0));
        assert!(b.vrr && b.primary);
        assert!(!cfg.output[2].enable);
        // wiri's docs spelling: position with two numbers
        let cfg = clean("output \"DP-2\" { position 1920 0 }\n");
        assert_eq!(cfg.output[0].position.x, 1920);
        // No output block: one default entry.
        assert_eq!(clean("").output.len(), 1);
        let d = load("output \"DP-1\" { scale 8.0 }\n");
        assert!(errors(&d).is_empty());
        assert_eq!(
            warnings(&d).len(),
            1,
            "an out-of-range scale warns but is kept"
        );
        assert!((d.config.output[0].scale - 8.0).abs() < 1e-9);
    }

    #[test]
    fn workspaces_with_and_without_a_block() {
        let cfg = clean("workspace \"mail\"\nworkspace \"web\" {\n}\nworkspace \"chat\"\nworkspace \"Main\" {\n    layout \"tile\"\n    monitor \"DP-1\"\n    follow-on-focus true\n}\nworkspace \"dev\" { open-on-output \"HDMI-1\" }\n");
        let names: Vec<&str> = cfg.workspace.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["mail", "web", "chat", "Main", "dev"],
            "in the order they are declared"
        );
        assert_eq!(cfg.workspace[3].monitor, "DP-1");
        assert_eq!(cfg.workspace[4].open_on_outputs, vec!["HDMI-1"]);
        let d = load("workspace { }\n");
        assert!(errors(&d)[0].contains("needs a name"));
    }

    #[test]
    fn workspace_output_preferences_preserve_order_and_legacy_monitor() {
        let cfg = clean("workspace \"dev\" {\n    open-on-output \"DP-2\"\n    open-on-output \"HDMI-1\"\n}\nworkspace \"legacy\" {\n    monitor \"DP-1\"\n    open-on-output \"HDMI-2\"\n    open-on-output \"DP-3\"\n}\nworkspace \"plain\"\n");
        assert_eq!(cfg.workspace[0].open_on_outputs, vec!["DP-2", "HDMI-1"]);
        assert_eq!(cfg.workspace[0].monitor, "DP-2");
        assert_eq!(cfg.workspace[1].open_on_outputs, vec!["HDMI-2", "DP-3"]);
        assert_eq!(cfg.workspace[1].monitor, "DP-1");
        assert!(cfg.workspace[2].open_on_outputs.is_empty());
        assert!(cfg.workspace[2].monitor.is_empty());
    }

    #[test]
    fn animations_in_both_spellings() {
        let cfg = clean("animations {\n    enable true\n    duration 250\n    easing \"ease-out-cubic\"\n    workspace-transition false\n}\n");
        assert!(cfg.animations.enabled);
        assert_eq!(cfg.animations.duration, 250);
        assert_eq!(cfg.animations.easing, "ease-out-cubic");
        assert!(!cfg.animations.workspace_transition);
        let cfg = clean("animations { off }\n");
        assert!(!cfg.animations.enabled);
        let d = load("animations {\n    slowdown 3.0\n    workspace-switch { spring damping-ratio=1.0 stiffness=1000 epsilon=0.0001 }\n}\n");
        assert!(errors(&d).is_empty());
        assert!(warnings(&d).is_empty(), "spring and slowdown now act");
        assert_eq!(d.config.animations.slowdown, 3.0);
    }

    #[test]
    fn spawn_at_startup_keeps_every_argument() {
        let cfg = clean("spawn-at-startup \"wt.exe\" \"-d\" \"C:\\My Projects\"\nspawn-at-startup notepad.exe\nspawn-sh-at-startup \"echo hi > %TEMP%\\\\x.txt\"\n");
        assert_eq!(
            cfg.spawn_at_startup,
            vec![
                vec![
                    "wt.exe".to_string(),
                    "-d".to_string(),
                    "C:\\My Projects".to_string()
                ],
                vec!["notepad.exe".to_string()],
            ]
        );
        assert_eq!(
            cfg.spawn_sh_at_startup,
            vec!["echo hi > %TEMP%\\x.txt".to_string()]
        );
        let d = load("spawn-at-startup\n");
        assert!(errors(&d)[0].contains("needs a program"));
    }

    #[test]
    fn set_column_display_binds_parse_both_modes() {
        let cfg = clean(
            "binds {\n    Mod+N { set-column-display \"normal\"; }\n    Mod+T { set-column-display \"tabbed\"; }\n    Mod+U { set-column-display normal; }\n    Mod+V { set-column-display tabbed; }\n}\n",
        );
        let binds = &cfg.binds.hotkeys;
        assert_eq!(binds.len(), 4);
        assert_eq!(
            binds[0].parse_action(),
            Some(crate::input::Action::SetColumnDisplay { tabbed: false })
        );
        assert_eq!(
            binds[1].parse_action(),
            Some(crate::input::Action::SetColumnDisplay { tabbed: true })
        );
        assert_eq!(
            binds[2].parse_action(),
            Some(crate::input::Action::SetColumnDisplay { tabbed: false })
        );
        assert_eq!(
            binds[3].parse_action(),
            Some(crate::input::Action::SetColumnDisplay { tabbed: true })
        );
    }

    #[test]
    fn set_column_display_bind_errors_are_invalid_arguments_not_unsupported_actions() {
        for action in [
            "set-column-display",
            "set-column-display \"stacked\"",
            "set-column-display \"normal\" \"tabbed\"",
        ] {
            let decoded = load(&format!("binds {{\n    Mod+T {{ {action}; }}\n}}\n"));
            assert_eq!(errors(&decoded).len(), 1, "{action}");
            assert!(warnings(&decoded).is_empty(), "{action}");
            assert!(decoded.config.binds.hotkeys.is_empty(), "{action}");
        }
    }

    #[test]
    fn the_cursor_section_decodes_the_two_options_windows_has_and_warns_about_xcursor() {
        let d = load("cursor {\n    hide-when-typing\n    hide-after-inactive-ms 400\n    xcursor-theme \"Adwaita\"\n    xcursor-size 24\n}\n");
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        // The Xcursor theme and size are the Windows pointer scheme, not a wiri setting.
        assert_eq!(warnings(&d).len(), 2, "{:?}", warnings(&d));
        assert_eq!(d.config.cursor.hide_when_typing, Some(true));
        assert_eq!(d.config.cursor.hide_after_inactive_ms, Some(400));
        // Not writing the section leaves both unset: wiri touches no cursor setting at all.
        let untouched = clean("");
        assert_eq!(untouched.cursor.hide_when_typing, None);
        assert_eq!(untouched.cursor.hide_after_inactive_ms, None);
        // `false` is an explicit "off", which is not the same as "not configured".
        assert_eq!(
            clean("cursor { hide-when-typing false; }")
                .cursor
                .hide_when_typing,
            Some(false)
        );
        assert_eq!(
            clean("cursor { hide-after-inactive-ms 0; }")
                .cursor
                .hide_after_inactive_ms,
            Some(0)
        );
        // A value that is not a whole number of milliseconds is an error, not a silent 0.
        let bad = load("cursor {\n    hide-after-inactive-ms 12.5\n}\n");
        assert_eq!(errors(&bad).len(), 1, "{:?}", errors(&bad));
        let unknown = load("cursor {\n    hide-after-inactive 400\n}\n");
        assert_eq!(errors(&unknown).len(), 1, "{:?}", errors(&unknown));
        assert!(
            errors(&unknown)[0].contains("hide-after-inactive"),
            "{:?}",
            errors(&unknown)
        );
    }

    #[test]
    fn tablet_stylus_button_binds_decode_like_mouse_binds() {
        let cfg = clean(
            "binds {\n    Mod+TabletStylusButton1 { close-window; }\n    Mod+TabletStylusButton2 { refresh; }\n    Mod+TabletStylusButton3 cooldown-ms=80 { close-window; }\n}\n",
        );
        let keys: Vec<&str> = cfg.binds.hotkeys.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "TabletStylusButton1",
                "TabletStylusButton2",
                "TabletStylusButton3"
            ]
        );
        assert_eq!(
            cfg.binds.hotkeys[2].cooldown_ms,
            Some(80),
            "a stylus bind takes the same properties as any other"
        );
        for bind in &cfg.binds.hotkeys {
            assert!(
                bind.vk_code().is_none(),
                "{} is a pointer trigger, not a key",
                bind.key
            );
            assert!(
                crate::input::pointer::parse_trigger(&bind.key).is_some(),
                "{} must be a pointer trigger",
                bind.key
            );
        }
        // A stylus name that does not exist is still reported, with the place.
        let bad = load("binds {\n    Mod+TabletStylusButton4 { close-window; }\n}\n");
        assert_eq!(errors(&bad).len(), 1, "{:?}", errors(&bad));
    }

    #[test]
    fn niri_only_sections_are_warnings() {
        let d = load("prefer-no-csd\nhotkey-overlay { skip-at-startup }\nscreenshot-path \"~/Pictures/x.png\"\ncursor { xcursor-size 24 }\nlayer-rule { match namespace=\"x\" }\n");
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        // screenshot-path, hotkey-overlay (Lane P) and layer-rule (Lane A11Y: decoded into the
        // rules wiri's own overlays follow; Windows has no layer shell) are implemented.
        assert_eq!(warnings(&d).len(), 2, "{:?}", warnings(&d));
        assert!(d.config.hotkey_overlay.skip_at_startup);
    }

    #[test]
    fn niri_binds_block() {
        let cfg = clean(
            "binds {\n    Mod+T hotkey-overlay-title=\"Open a Terminal: wt\" { spawn \"wt.exe\"; }\n    Mod+Left repeat=false cooldown-ms=150 { focus-column-left; }\n    Mod+1 { focus-workspace 1; }\n    Mod+Shift+2 { focus-workspace \"2\"; }\n    Mod+R { set-column-width \"+10%\"; }\n    Mod+Q { close-window; }\n    Mod+W hotkey-overlay-title=null { quit skip-confirmation=true; }\n}\n",
        );
        let b = &cfg.binds.hotkeys;
        assert_eq!(b.len(), 7);
        assert_eq!(b[0].modifiers, vec!["Mod"]);
        assert_eq!(b[0].key, "T");
        assert_eq!(b[0].command, "spawn");
        assert_eq!(b[0].args, vec!["wt.exe"]);
        assert_eq!(
            b[0].hotkey_overlay_title,
            Some(Some("Open a Terminal: wt".to_string()))
        );
        assert!(b[0].repeat);
        assert!(!b[1].repeat);
        assert_eq!(b[1].cooldown_ms, Some(150));
        assert_eq!(b[2].args, vec!["1"]);
        assert_eq!(
            b[2].args_quoted,
            vec![false],
            "focus-workspace 1 is an index"
        );
        assert_eq!(
            b[3].args_quoted,
            vec![true],
            "focus-workspace \"2\" is a name"
        );
        assert_eq!(b[4].args, vec!["+10%"]);
        assert_eq!(b[6].hotkey_overlay_title, Some(None));
        assert_eq!(
            b[6].action_props,
            vec![("skip-confirmation".to_string(), "true".to_string())]
        );
        for bind in b {
            assert!(
                bind.parse_action().is_some(),
                "{} does not resolve",
                bind.chord_text()
            );
        }
    }

    // Pinned niri applies a title with Pango set_markup (wiki
    // Configuration:-Key-Bindings, "Custom Hotkey Overlay Titles";
    // niri-config/src/binds.rs:873-875 keeps the string verbatim).  A bad title is a
    // *warning* at load and stays in the config as written, because the overlay paints the
    // whole original string when the markup is invalid.
    #[test]
    fn invalid_hotkey_overlay_markup_warns_once_and_keeps_the_title() {
        let d = load(
            "binds {\n    Mod+F9 hotkey-overlay-title=\"<b>R&D</b>\" { spawn \"notepad.exe\"; }\n}\n",
        );
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        assert_eq!(warnings(&d).len(), 1, "{:?}", warnings(&d));
        assert!(
            warnings(&d)[0].contains("is not valid Pango markup"),
            "{:?}",
            warnings(&d)
        );
        assert!(warnings(&d)[0].contains("R&D"), "{:?}", warnings(&d));
        assert_eq!(
            d.config.binds.hotkeys[0].hotkey_overlay_title,
            Some(Some("<b>R&D</b>".to_string()))
        );
        // Valid markup, entities and a plain title say nothing.
        let d = load(
            "binds {\n    Mod+F8 hotkey-overlay-title=\"<b>R&amp;D</b>\" { spawn \"notepad.exe\"; }\n    Mod+F7 hotkey-overlay-title=\"Plain\" { spawn \"notepad.exe\"; }\n}\n",
        );
        assert!(d.issues.is_empty(), "{:?}", d.issues);
    }

    #[test]
    fn wiri_line_binds_still_load() {
        let cfg = clean("binds {\n    Ctrl+Alt+Left focus-column-left\n    Ctrl+Alt+1 switch-workspace 1\n    Ctrl+Alt+Return spawn-cmd \"start wt.exe || start powershell.exe\"\n    Ctrl+Alt+Y exec cmd.exe\n    Escape overview-toggle\n    extend-defaults false\n}\n");
        let b = &cfg.binds.hotkeys;
        assert_eq!(b.len(), 5);
        assert_eq!(b[0].modifiers, vec!["Ctrl", "Alt"]);
        assert_eq!(b[0].key, "Left");
        assert_eq!(b[0].command, "focus-column-left");
        assert_eq!(b[1].args, vec!["1"]);
        assert_eq!(b[2].command, "spawn-cmd");
        assert_eq!(b[2].args, vec!["start wt.exe || start powershell.exe"]);
        assert_eq!(b[3].command, "exec");
        assert_eq!(b[4].key, "Escape");
        assert!(b[4].modifiers.is_empty());
        assert!(!cfg.binds.extend_defaults);
        // Both forms side by side, and quoted flags in the line form.
        let cfg = clean("binds {\n    Mod+F1 set-column-width \"50%\"\n    Mod+3 focus-workspace \"2\"\n    Mod+2 focus-workspace 2\n    Mod+T { toggle-window-floating; }\n}\n");
        assert_eq!(cfg.binds.hotkeys[1].args_quoted, vec![true]);
        assert_eq!(cfg.binds.hotkeys[2].args_quoted, vec![false]);
    }

    #[test]
    fn workspace_action_properties_preserve_numeric_quotation_and_zero_indices() {
        use crate::input::{Action, WorkspaceAction, WorkspaceRef};
        for (value, quoted, reference) in [
            ("\"2\"", true, WorkspaceRef::Name("2".into())),
            ("2", false, WorkspaceRef::Index(2)),
            ("\"0\"", true, WorkspaceRef::Name("0".into())),
            ("0", false, WorkspaceRef::Index(1)),
            ("\"--bogus\"", true, WorkspaceRef::Name("--bogus".into())),
        ] {
            let cfg = clean(&format!(
                "binds {{\n    Mod+F1 {{ move-workspace-to-index 0 workspace={value}; }}\n}}\n"
            ));
            let bind = &cfg.binds.hotkeys[0];
            assert_eq!(bind.args, vec!["0", "--workspace", reference_word(value)]);
            assert_eq!(bind.args_quoted, vec![false, false, quoted]);
            assert_eq!(
                bind.parse_action(),
                Some(Action::WorkspaceAction(WorkspaceAction::MoveToIndex {
                    index: 1,
                    workspace: Some(reference),
                })),
                "workspace={value}"
            );
        }

        fn reference_word(value: &str) -> &str {
            value.trim_matches('"')
        }
    }

    #[test]
    fn bind_problems_that_cost_one_bind() {
        // Unknown modifiers reject the config rather than weakening the chord.
        // AltGr, pointer triggers and media keys have real Windows equivalents.
        let d = load("binds {\n    Hyper+F9 { close-window; }\n    Mod5+F8 { close-window; }\n    Mod+MouseLeft { close-window; }\n    XF86AudioMute { spawn \"x\"; }\n    Mod+F7 { refresh; }\n}\n");
        assert_eq!(errors(&d).len(), 1, "{:?}", errors(&d));
        assert!(errors(&d)[0].contains("unknown modifier 'Hyper'"));
        assert!(warnings(&d).is_empty(), "{:?}", warnings(&d));
        assert_eq!(d.config.binds.hotkeys.len(), 4);
        assert_eq!(d.config.binds.hotkeys[0].key, "F8");
        assert_eq!(d.config.binds.hotkeys[1].key, "MouseLeft");
        // An action wiri lacks but niri has: a warning, not an error.
        let d = load("binds { Mod+X { toggle-debug-tint; } }\n");
        assert!(errors(&d).is_empty());
        assert!(
            warnings(&d)[0].contains("not implemented by wiri yet"),
            "{:?}",
            warnings(&d)
        );
        assert!(d.config.binds.hotkeys.is_empty());
    }

    #[test]
    fn bind_problems_that_reject_the_config() {
        let d = load("binds {\n    Mod+Retrun { close-window; }\n    Mod+T { clsoe-window; }\n    Mod+Y { spawn; }\n    Mod+U { close-window; quit; }\n    Mod+I\n    Mod+O repeat=maybe { close-window; }\n    Mod+P bogus=1 { close-window; }\n    Mod+L { close-window now=1; }\n}\n");
        let e = errors(&d);
        assert_eq!(e.len(), 8, "{:?}", e);
        assert!(
            e[0].starts_with("2:5:") && e[0].contains("unknown key 'Retrun'"),
            "{}",
            e[0]
        );
        assert!(
            e[1].contains("unknown action 'clsoe-window'") && e[1].contains("close-window"),
            "{}",
            e[1]
        );
        assert!(e[2].contains("bad arguments for 'spawn'"), "{}", e[2]);
        assert!(e[3].contains("only one action"), "{}", e[3]);
        assert!(e[4].contains("has no action"), "{}", e[4]);
        assert!(e[5].contains("expects true or false"), "{}", e[5]);
        assert!(e[6].contains("unexpected property 'bogus'"), "{}", e[6]);
        assert!(e[7].contains("unexpected property 'now'"), "{}", e[7]);
    }

    #[test]
    fn duplicate_bind_aliases_are_errors_with_both_locations() {
        let d =
            load("binds {\n    Control+Return { close-window; }\n    ctrl+enter { quit; }\n}\n");
        assert_eq!(errors(&d).len(), 1, "{:?}", errors(&d));
        assert!(warnings(&d).is_empty());
        assert!(errors(&d)[0].contains("duplicate chord"));
        assert_eq!(d.issues[0].context.related[0].line, 2);
        assert_eq!(d.config.binds.hotkeys.len(), 1);
        assert_eq!(d.config.binds.hotkeys[0].command, "close-window");
    }

    #[test]
    fn duplicate_chords_use_final_mod_even_after_binds() {
        let d = load("binds { Mod+Return { close-window; }\nSuper+Enter { quit; } }\ninput { mod-key \"super\"; }");
        assert_eq!(d.error_count(), 1, "{:?}", d.issues);
        assert!(d.issues[0].message.contains("duplicate chord"));
    }

    #[test]
    fn niri_window_rules() {
        let cfg = clean(
            "window-rule {\n    match app-id=r#\"firefox$\"# title=\"^Picture-in-Picture$\"\n    match app-id=\"notepad\"\n    exclude title=\"Private\"\n    open-on-output \"HDMI-1\"\n    open-on-workspace \"chat\"\n    open-floating true\n    open-fullscreen false\n    open-maximized true\n    open-focused false\n    default-column-width { proportion 0.5; }\n    default-window-height { fixed 400; }\n    min-width 300\n    max-height 900\n    opacity 0.9\n}\nwindow-rule {\n    match is-active=false\n    default-column-width {}\n}\n",
        );
        assert_eq!(cfg.window_rules.len(), 2);
        let r = &cfg.window_rules[0];
        assert_eq!(r.match_groups.len(), 2);
        assert_eq!(r.match_groups[0].conds.len(), 2);
        assert_eq!(r.exclude_groups.len(), 1);
        assert_eq!(r.open_on_output.as_deref(), Some("HDMI-1"));
        assert_eq!(r.open_on_workspace.as_deref(), Some("chat"));
        assert_eq!(r.open_floating, Some(true));
        assert_eq!(r.open_fullscreen, Some(false));
        assert_eq!(r.open_maximized, Some(true));
        assert_eq!(r.open_focused, Some(false));
        assert_eq!(
            r.default_column_width,
            Some(Some(SizeSpec::Proportion(0.5)))
        );
        assert_eq!(r.default_window_height, Some(Some(SizeSpec::Fixed(400))));
        assert_eq!(
            (r.min_width, r.max_height, r.opacity),
            (Some(300), Some(900), Some(0.9))
        );
        let r2 = &cfg.window_rules[1];
        assert_eq!(
            r2.default_column_width,
            Some(None),
            "an empty block: the window picks"
        );
        assert!(matches!(&r2.match_groups[0].conds[0], Matcher::Not(_)));
    }

    #[test]
    fn window_rule_matching_is_or_across_match_lines_and_exclude() {
        let cfg = clean("window-rule {\n    match app-id=\"^firefox\" title=\"Picture\"\n    match app-id=\"^notepad$\"\n    exclude title=\"Private\"\n    open-floating true\n}\n");
        let rule = &cfg.window_rules[0];
        let ctx = |process: &'static str, title: &'static str| MatcherContext {
            class_name: "C",
            title,
            instance: None,
            process_name: Some(process),
            is_active: false,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: false,
            is_active_in_column: false,
        };
        assert!(rule.matches(&ctx("firefox.exe", "Picture-in-Picture")));
        assert!(
            !rule.matches(&ctx("firefox.exe", "Mozilla")),
            "one match line is an AND of its properties"
        );
        assert!(
            rule.matches(&ctx("notepad.exe", "anything")),
            "the second match line is an alternative"
        );
        assert!(
            !rule.matches(&ctx("notepad.exe", "Private notes")),
            "exclude wins"
        );
        assert!(!rule.matches(&ctx("code.exe", "Picture")));
        // A rule with no match line matches everything that is not excluded.
        let cfg = clean("window-rule { exclude app-id=\"^x$\" }\n");
        assert!(cfg.window_rules[0].matches(&ctx("firefox.exe", "t")));
        assert!(!cfg.window_rules[0].matches(&ctx("x.exe", "t")));
    }

    #[test]
    fn app_id_looks_at_the_process_name_with_and_without_exe_and_the_aumid() {
        let cfg = clean("window-rule {\n    match app-id=\"^firefox$\"\n}\nwindow-rule {\n    match app-id=\"Microsoft.WindowsTerminal\"\n}\n");
        let mut ctx = MatcherContext::from_legacy("C", "T", None, Some("firefox.exe"));
        assert!(cfg.window_rules[0].matches(&ctx), "the name without .exe");
        ctx.process_name = Some("firefox-bin.exe");
        assert!(!cfg.window_rules[0].matches(&ctx));
        ctx.process_name = Some("WindowsTerminal.exe");
        ctx.aumid = Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App");
        assert!(cfg.window_rules[1].matches(&ctx), "the AppUserModelID");
    }

    #[test]
    fn wiri_window_rule_spellings_still_load() {
        let cfg = clean("window-rule {\n    class-name \"Firefox\"\n    floating true\n    opacity 0.9\n    default-width 1200\n    default-height 800\n    workspace \"Main\"\n    open-in-workspace 4\n    open-max-bounds \"1600x900\"\n    blur true\n    sticky true\n}\nwindow-rule {\n    match {\n        class-name \"Notepad\"\n        process-name-regex \"(?i)^(steam|discord)\\\\.exe$\"\n        is-active\n    }\n    float true\n}\n");
        let r = &cfg.window_rules[0];
        assert_eq!(r.class.as_deref(), Some("Firefox"));
        assert!(r.floating);
        assert_eq!(r.opacity, Some(0.9));
        assert_eq!((r.default_width, r.default_height), (Some(1200), Some(800)));
        assert_eq!(r.workspace.as_deref(), Some("Main"));
        assert_eq!(r.open_in_workspace, Some(4));
        assert_eq!(r.open_max_bounds, Some((1600, 900)));
        assert!(r.blur && r.sticky);
        let r = &cfg.window_rules[1];
        assert!(r.floating);
        assert_eq!(r.match_groups.len(), 1);
        assert_eq!(r.match_groups[0].conds.len(), 3);
        let ctx = MatcherContext {
            class_name: "Notepad",
            title: "t",
            instance: None,
            process_name: Some("steam.exe"),
            is_active: true,
            is_floating: false,
            is_urgent: false,
            at_startup: false,
            aumid: None,
            is_focused: true,
            is_active_in_column: false,
        };
        assert!(r.matches(&ctx));
        // niri parses `opacity` as an unbounded f32 (niri-config/src/window_rule.rs:69) and the
        // clamp to 0..=1 happens when the rule is resolved (src/layout/rules.rs), so 2.5 is
        // stored as it was written.
        let clamped = clean("window-rule { class-name \"Bright\"\n opacity 2.5 }\n");
        assert_eq!(clamped.window_rules[0].opacity, Some(2.5));
        // A value that is not a number is still an error and must not alter the rule.
        let d = load("window-rule { class-name \"Bright\"\n opacity \"x\" }\n");
        assert_eq!(errors(&d).len(), 1);
        assert_eq!(d.config.window_rules[0].opacity, None);
        assert!(warnings(&d).is_empty());
    }

    #[test]
    fn window_rule_problems() {
        let d = load("window-rule {\n    match app-id=\"(\"\n    open-floating maybe\n    default-column-width { proportion 0.5; fixed 3 }\n    mach app-id=\"x\"\n    match bogus=\"x\"\n    match \"positional\"\n}\n");
        let e = errors(&d);
        assert!(
            e.iter().any(|m| m.contains("invalid regular expression")),
            "{:?}",
            e
        );
        assert!(
            e.iter().any(|m| m.contains("expects true or false")),
            "{:?}",
            e
        );
        assert!(e.iter().any(|m| m.contains("only one of")), "{:?}", e);
        assert!(
            e.iter()
                .any(|m| m.contains("unknown option 'mach'") && m.contains("match")),
            "{:?}",
            e
        );
        assert!(
            e.iter()
                .any(|m| m.contains("unknown match property 'bogus'")),
            "{:?}",
            e
        );
        assert!(e.iter().any(|m| m.contains("takes properties")), "{:?}", e);
        let d = load("window-rule { match is-window-cast-target=true\n focus-ring { off }\n geometry-corner-radius 8 }\n");
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        assert_eq!(warnings(&d).len(), 1); // Only the compositor-only match is unsupported.
    }

    #[test]
    fn keyboard_options_only_acquire_when_explicit() {
        let d = load("input { keyboard {} }");
        assert!(!d.config.input.keyboard_layout_configured);
        assert!(!d.config.input.repeat_delay_configured);
        assert!(!d.config.input.repeat_rate_configured);
        assert!(!d.config.input.numlock_configured);
        let d = load("input { keyboard { xkb { layout \"us\"; }; repeat-delay 500; repeat-rate 0; numlock false; } }");
        assert!(!d.has_errors(), "{:?}", d.issues);
        assert!(d.config.input.keyboard_layout_configured);
        assert!(d.config.input.repeat_delay_configured);
        assert!(d.config.input.repeat_rate_configured);
        assert!(d.config.input.numlock_configured);
        assert_eq!(d.config.input.keyboard_layout, "us");
        assert_eq!(d.config.input.repeat_rate, 0);
        assert!(!d.config.input.numlock);
    }

    #[test]
    fn environment_strings_empty_and_null_are_distinct() {
        let before = std::env::var_os("PATH");
        let d = load("environment { Path null; EMPTY \"\"; UNICODE \"hello 🦀\"; }");
        assert!(!d.has_errors(), "{:?}", d.issues);
        assert_eq!(d.config.environment["Path"], None);
        assert_eq!(d.config.environment["EMPTY"].as_deref(), Some(""));
        assert_eq!(d.config.environment["UNICODE"].as_deref(), Some("hello 🦀"));
        assert_eq!(std::env::var_os("PATH"), before);
        let d = load("environment { Path \"first\"; PATH null; }");
        assert_eq!(d.error_count(), 1, "{:?}", d.issues);
        assert_eq!(d.first_error().unwrap().context.related.len(), 1);
        for invalid in [
            "environment { A 42; }",
            "environment { \"A=B\" \"x\"; }",
            "environment { \"\" null; }",
        ] {
            assert!(load(invalid).has_errors(), "{invalid}");
        }
    }

    #[test]
    fn environment_included_parts_merge_case_insensitively() {
        let dir = Path::new(".scratch").join(format!("c2-env-parts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("env.kdl"),
            "environment { PATH \"second\"; REMOVE null; }",
        )
        .unwrap();
        let d = decode_text(
            "environment { Path \"first\"; KEEP \"yes\"; }\ninclude \"env.kdl\"",
            "root.kdl",
            Some(&dir),
        );
        assert!(!d.has_errors(), "{:?}", d.issues);
        assert_eq!(d.config.environment.len(), 3);
        assert_eq!(d.config.environment["PATH"].as_deref(), Some("second"));
        assert_eq!(d.config.environment["REMOVE"], None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn switches_support_only_one_spawn_and_preserve_all_arguments() {
        use crate::hooks::switch_events::SwitchEvent;
        let d = load(
            r##"switch-events { lid-close { spawn r#"C:\a b\p.exe"# "a b" ""; }; lid-open { spawn "cmd.exe"; }; tablet-mode-on { spawn "tablet.exe"; }; tablet-mode-off { spawn "laptop.exe"; } }"##,
        );
        assert!(!d.has_errors(), "{:?}", d.issues);
        let switches = &d.config.switch_events;
        assert_eq!(
            switches.command(SwitchEvent::LidClose).unwrap(),
            ["C:\\a b\\p.exe", "a b", ""]
        );
        assert_eq!(switches.command(SwitchEvent::LidOpen).unwrap(), ["cmd.exe"]);
        assert!(switches.subscriptions().lid && switches.subscriptions().tablet_mode);
        for invalid in [
            "switch-events { lid-close { quit; } }",
            "switch-events { lid-close {} }",
            "switch-events { lid-close { spawn \"x\"; spawn \"y\"; } }",
            "switch-events { lid-close { spawn; } }",
            "switch-events { tablet-mood-on { spawn \"x\"; } }",
        ] {
            assert!(load(invalid).has_errors(), "{invalid}");
        }
    }

    #[test]
    fn workspace_layout_partial_and_output_priority_decode() {
        let d = load("layout { gaps 8; always-center-single-column; }\nworkspace \"chat\" { open-on-output \"first\"; open-on-output \"second\"; layout { gaps 16; always-center-single-column false; } }");
        assert!(!d.has_errors(), "{:?}", d.issues);
        let ws = &d.config.workspace[0];
        assert_eq!(ws.open_on_outputs, ["first", "second"]);
        let layout = ws
            .layout_override
            .as_ref()
            .unwrap()
            .apply_to(&d.config.layout);
        assert_eq!(layout.inner_gaps, 16.0);
        assert!(!layout.always_center_single_column);
        for option in ["empty-workspace-above-first", "insert-hint {}"] {
            let d = load(&format!("workspace \"a\" {{ layout {{ {option}; }} }}"));
            assert!(d.has_errors(), "{option}");
        }
    }

    #[test]
    fn includes_are_resolved_against_the_including_file() {
        let dir = std::env::temp_dir().join(format!("wiri_decode_inc_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(
            dir.join("binds.kdl"),
            "binds {\n    Mod+Z { spawn \"notepad.exe\"; }\n    Mod+T { quit; }\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("sub").join("deep.kdl"), "layout { gaps 3 }\n").unwrap();
        std::fs::write(
            dir.join("sub").join("mid.kdl"),
            "include \"deep.kdl\"\ninput { repeat-delay 123 }\n",
        )
        .unwrap();
        let main = "binds {\n    Mod+T { close-window; }\n}\ninclude \"binds.kdl\"\ninclude \"sub/mid.kdl\"\ninclude \"missing.kdl\" optional=true\n";
        let d = decode_text(main, "main.kdl", Some(&dir));
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        assert_eq!(
            warnings(&d).len(),
            1,
            "only the optional include warns: {:?}",
            warnings(&d)
        );
        assert_eq!(d.config.input.repeat_delay, 123);
        assert_eq!(d.config.layout.inner_gaps, 3.0);
        // A later bind on the same chord replaces the earlier one.
        let t: Vec<&HotkeyBinding> = d
            .config
            .binds
            .hotkeys
            .iter()
            .filter(|b| b.key == "T")
            .collect();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].command, "quit");
        assert!(d.config.binds.hotkeys.iter().any(|b| b.key == "Z"));
        assert_eq!(
            d.includes.len(),
            4,
            "every include is recorded for the watcher: {:?}",
            d.includes
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn include_errors_name_the_included_file() {
        let dir = std::env::temp_dir().join(format!("wiri_decode_incerr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.kdl"), "layout {\n    gaps 3\n    gapz 4\n}\n").unwrap();
        let d = decode_text("include \"bad.kdl\"\n", "main.kdl", Some(&dir));
        let first = d.first_error().expect("an error");
        assert_eq!((first.line, first.column), (3, 5));
        assert!(first.file.ends_with("bad.kdl"), "{}", first.file);
        assert!(
            first.located("main.kdl").contains("bad.kdl:3:5: error"),
            "{}",
            first.located("main.kdl")
        );
        // Missing files, cycles and the depth limit.
        let d = decode_text("include \"nope.kdl\"\n", "", Some(&dir));
        assert!(errors(&d)[0].contains("file not found"), "{:?}", errors(&d));
        std::fs::write(dir.join("a.kdl"), "include \"b.kdl\"\n").unwrap();
        std::fs::write(dir.join("b.kdl"), "include \"a.kdl\"\n").unwrap();
        let d = decode_text("include \"a.kdl\"\n", "", Some(&dir));
        assert!(
            errors(&d)
                .iter()
                .any(|m| m.contains("cyclic include") || m.contains("depth exceeds")),
            "{:?}",
            errors(&d)
        );
        for i in 0..(MAX_INCLUDE_DEPTH as usize + 2) {
            let body = if i + 1 < MAX_INCLUDE_DEPTH as usize + 2 {
                format!("include \"f{}.kdl\"\n", i + 1)
            } else {
                "input { repeat-delay 42 }\n".to_string()
            };
            std::fs::write(dir.join(format!("f{}.kdl", i)), body).unwrap();
        }
        let d = decode_text("include \"f0.kdl\"\n", "", Some(&dir));
        assert!(
            errors(&d).iter().any(|m| m.contains("depth exceeds")),
            "{:?}",
            errors(&d)
        );
        // Without a directory an include cannot be followed.
        let d = decode_text("include \"x.kdl\"\n", "", None);
        assert!(errors(&d).is_empty());
        assert_eq!(warnings(&d).len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn include_paths_expand_environment_variables_and_the_home_directory() {
        std::env::set_var("WIRI_TEST_INCLUDE_DIR", "C:\\Users\\someone\\conf");
        assert_eq!(
            expand_user_path("%WIRI_TEST_INCLUDE_DIR%\\x.kdl"),
            "C:\\Users\\someone\\conf\\x.kdl"
        );
        // An unknown variable is left as it is; a lone percent sign is not a variable.
        assert_eq!(
            expand_user_path("%WIRI_NO_SUCH_VARIABLE%\\x"),
            "%WIRI_NO_SUCH_VARIABLE%\\x"
        );
        assert_eq!(expand_user_path("100%"), "100%");
        assert_eq!(expand_user_path("plain/path.kdl"), "plain/path.kdl");
        if let Ok(home) = std::env::var("USERPROFILE") {
            assert_eq!(expand_user_path("~/wiri.kdl"), format!("{}/wiri.kdl", home));
            assert_eq!(expand_user_path("~"), home);
        }
        // End to end: an include that names its directory through a variable.
        let dir = std::env::temp_dir().join(format!("wiri_decode_env_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("env.kdl"), "layout { gaps 5 }\n").unwrap();
        std::env::set_var("WIRI_TEST_INCLUDE_DIR2", dir.to_str().unwrap());
        let d = decode_text(
            "include \"%WIRI_TEST_INCLUDE_DIR2%\\\\env.kdl\"\n",
            "",
            Some(Path::new("C:\\nowhere")),
        );
        assert!(errors(&d).is_empty(), "{:?}", errors(&d));
        assert_eq!(d.config.layout.inner_gaps, 5.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn include_is_a_top_level_node_only() {
        let d = load("layout {\n    include \"x.kdl\"\n}\n");
        assert!(
            errors(&d)[0].contains("unknown option 'include'"),
            "{:?}",
            errors(&d)
        );
    }

    #[test]
    fn config_bytes_in_the_encodings_windows_tools_write() {
        assert_eq!(decode_config_bytes(b"layout { }").unwrap(), "layout { }");
        assert_eq!(
            decode_config_bytes(b"\xEF\xBB\xBFlayout { }").unwrap(),
            "layout { }"
        );
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "layout { gaps 8 }".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_config_bytes(&utf16).unwrap(), "layout { gaps 8 }");
        let mut be = vec![0xFE, 0xFF];
        for u in "ab".encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_config_bytes(&be).unwrap(), "ab");
        let err = decode_config_bytes(b"layout \xFF").unwrap_err();
        assert!(err.contains("UTF-8"), "{}", err);
        // A BOM at the start of a config does not hide its first section.
        let cfg = decode_text("\u{FEFF}input { repeat-delay 321 }", "", None).config;
        assert_eq!(cfg.input.repeat_delay, 321);
    }

    #[test]
    fn colors_are_normalised() {
        assert_eq!(normalize_color("#FFF").as_deref(), Some("#ffffff"));
        assert_eq!(normalize_color("#f008").as_deref(), Some("#ff000088"));
        assert_eq!(normalize_color("#AbCdEf").as_deref(), Some("#abcdef"));
        assert_eq!(normalize_color("accent").as_deref(), Some("accent"));
        assert_eq!(normalize_color("").as_deref(), Some(""));
        assert_eq!(normalize_color("red").as_deref(), Some("#ff0000"));
        assert_eq!(normalize_color("#12345"), None);
        assert_eq!(normalize_color("#ggg"), None);
    }

    #[test]
    fn issues_print_like_compiler_output() {
        let d = load("layout {\n    nope 1\n}\n");
        let i = &d.issues[0];
        assert_eq!(
            i.to_string(),
            "2:5: error: unknown option 'nope' in 'layout'"
        );
        assert_eq!(
            i.located("C:\\cfg.kdl"),
            "C:\\cfg.kdl:2:5: error: unknown option 'nope' in 'layout'"
        );
        assert_eq!(d.error_count(), 1);
        assert_eq!(d.warning_count(), 0);
    }

    #[test]
    fn edit_distance_and_suggestions() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(
            suggestion("gapz", &["gaps", "struts"]).as_deref(),
            Some("gaps")
        );
        assert_eq!(suggestion("zzzzzz", &["gaps", "struts"]), None);
    }
}
