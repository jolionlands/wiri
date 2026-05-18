use crate::config::error::Result;
use crate::config::types::*;

/// Parse a KDL configuration string into a Config struct.
/// Supports both KDL property syntax (`key "value"`) and equals syntax (`key=value`).
pub fn parse_kdl_config(input: &str) -> Result<Config> {
    let mut config = Config::default();
    // Start with an empty output list; default will be replaced by parsed entries.
    config.output.clear();

    // A section stack entry: (name, optional_quoted_arg)
    // e.g. ("output", Some("DP-1")), ("input", None), ("window-rule", None)
    let mut section_stack: Vec<(String, Option<String>)> = Vec::new();

    // Accumulate in-progress complex objects while inside their blocks.
    let mut current_output: Option<OutputConfig> = None;
    let mut current_workspace: Option<WorkspaceConfig> = None;
    let mut current_window_rule: Option<WindowRule> = None;

    for line in input.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with("//") {
            continue;
        }

        // Remove inline comments (but be careful not to strip # inside strings)
        let line = strip_inline_comment(line);

        // Handle section opening: "section {" or `section "name" {`
        if let Some((section_name, section_arg)) = try_parse_section(line) {
            // Before pushing, start accumulating if appropriate.
            let depth = section_stack.len();
            let parent = section_stack.last().map(|(n, _)| n.as_str()).unwrap_or("");

            if section_name == "output" && depth == 0 {
                current_output = Some(OutputConfig {
                    name: section_arg.clone().unwrap_or_default(),
                    position: Position::default(),
                    width: 0,
                    height: 0,
                    scale: 1.0,
                    mode: String::new(),
                    vrr: false,
                    primary: false,
                    transform: "normal".to_string(),
                    enable: true,
                });
            } else if section_name == "workspace" && depth == 0 {
                current_workspace = Some(WorkspaceConfig {
                    name: section_arg.clone().unwrap_or_default(),
                    layout: "tile".to_string(),
                    monitor: String::new(),
                    follow_on_focus: true,
                });
            } else if section_name == "window-rule" && depth == 0 {
                current_window_rule = Some(WindowRule::default());
            }

            let _ = parent; // suppress unused warning
            section_stack.push((section_name, section_arg));
            continue;
        }

        // Handle closing brace
        if line.starts_with('}') || line == ")" {
            if let Some((section_name, _)) = section_stack.pop() {
                // Finalise block objects when their block closes.
                let depth = section_stack.len();
                if section_name == "output" && depth == 0 {
                    if let Some(out) = current_output.take() {
                        config.output.push(out);
                    }
                } else if section_name == "workspace" && depth == 0 {
                    if let Some(ws) = current_workspace.take() {
                        config.workspace.push(ws);
                    }
                } else if section_name == "window-rule" && depth == 0 {
                    if let Some(wr) = current_window_rule.take() {
                        config.window_rules.push(wr);
                    }
                }
            }
            continue;
        }

        // Current section path (join only the names, not the args).
        let section = section_stack
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(".");

        // Binds section: each line is a binding (not key=value)
        if section.as_str() == "binds" {
            if let Some(binding) = parse_bind_line(line) {
                config.binds.hotkeys.push(binding);
            }
            continue;
        }

        // Lines inside `window-rule.match { }` populate wr.matchers.
        // Flag matchers (is-active, is-floating, is-urgent, at-startup) have no
        // value; they must be handled before the key-value parse path.
        if section.as_str() == "window-rule.match" {
            if let Some(ref mut wr) = current_window_rule {
                if let Some((key, value)) = parse_property(line) {
                    apply_window_rule_matcher_kv(&key, &value, wr);
                } else {
                    // No value — may be a bare flag keyword.
                    apply_window_rule_matcher_flag(line.trim(), wr);
                }
            }
            continue;
        }

        // Top-level `spawn-at-startup` directive: collect all quoted/unquoted tokens
        // on the line into a Vec<String> entry.  Must be handled before the
        // single-value parse_property path because it can have 2+ token arguments.
        if section.is_empty() && (line.starts_with("spawn-at-startup") || line.starts_with("spawn_at_startup")) {
            let rest = if let Some(r) = line.splitn(2, |c: char| c.is_whitespace()).nth(1) {
                r.trim()
            } else {
                ""
            };
            let tokens = tokenize_shell_line(rest);
            if !tokens.is_empty() {
                config.spawn_at_startup.push(tokens);
            }
            continue;
        }

        // Parse key-value pairs
        if let Some((key, value)) = parse_property(line) {
            match section.as_str() {
                "input" | "input.keyboard" => apply_input_keyboard_property(&key, &value, &mut config),
                // Bug fix #5: input.mouse now parsed.
                "input.mouse" => apply_input_mouse_property(&key, &value, &mut config),
                // Nested input.focus block: route follows-mouse/etc. to flat InputConfig fields.
                "input.focus" => apply_input_focus_property(&key, &value, &mut config),
                // Nested input.touch block — currently no flat fields; values logged for future wiring.
                "input.touch" => apply_input_touch_property(&key, &value, &mut config),
                "layout" => apply_layout_property(&key, &value, &mut config),
                "layout.gaps" => apply_layout_property(&key, &value, &mut config),
                "layout.borders" => apply_layout_property(&key, &value, &mut config),
                "layout.focus-ring" => apply_layout_property(&key, &value, &mut config),
                "layout.shadows" => apply_layout_property(&key, &value, &mut config),
                "animations" => apply_animations_property(&key, &value, &mut config),
                // Bug fix #6: output block properties.
                "output" => {
                    if let Some(ref mut out) = current_output {
                        apply_output_property(&key, &value, out);
                    }
                }
                // Bug fix #7: workspace block properties.
                "workspace" => {
                    if let Some(ref mut ws) = current_workspace {
                        apply_workspace_property(&key, &value, ws);
                    }
                }
                // Bug fix #8: window-rule block properties.
                "window-rule" => {
                    if let Some(ref mut wr) = current_window_rule {
                        apply_window_rule_property(&key, &value, wr);
                    }
                }
                _ => {}
            }
        }
    }

    // If no output blocks were parsed, restore the single default entry.
    if config.output.is_empty() {
        config.output.push(OutputConfig {
            name: String::new(),
            position: Position::default(),
            width: 0,
            height: 0,
            scale: 1.0,
            mode: String::new(),
            vrr: false,
            primary: false,
            transform: "normal".to_string(),
            enable: true,
        });
    }

    Ok(config)
}

/// Try to detect a section header like `input {`, `keyboard {`, or `output "DP-1" {`.
/// Returns `Some((section_name, optional_quoted_arg))` on a match.
fn try_parse_section(line: &str) -> Option<(String, Option<String>)> {
    let line = line.trim();
    if !line.ends_with('{') {
        return None;
    }
    let inner = line[..line.len() - 1].trim();
    if inner.is_empty() || inner.contains('(') {
        return None;
    }

    // Does the name part contain a quoted argument?  e.g. `output "DP-1"`
    // Split on first whitespace.
    let parts: Vec<&str> = inner.splitn(2, |c: char| c.is_whitespace()).collect();
    let name = parts[0].to_string();
    let arg = if parts.len() == 2 {
        let rest = parts[1].trim().trim_matches('"').to_string();
        if rest.is_empty() { None } else { Some(rest) }
    } else {
        None
    };

    Some((name, arg))
}

/// Strip inline comments (//) but not inside quoted strings
fn strip_inline_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
        }
        if !in_string && ch == '/' && i + 1 < line.len() && line.as_bytes()[i + 1] == b'/' {
            return &line[..i].trim_end();
        }
    }
    line
}

/// Parse a property line into (key, value).
/// Supports: key=value, key "value", key 123, key=true
fn parse_property(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    // Try key=value format
    if let Some(pos) = line.find('=') {
        let key = line[..pos].trim().to_string();
        let value = line[pos + 1..].trim().trim_matches('"').to_string();
        if !key.is_empty() {
            return Some((key, value));
        }
    }

    // Try KDL property format: key "value" or key 123 or key true
    let parts: Vec<&str> = line.splitn(2, |c: char| c.is_whitespace()).collect();
    if parts.len() == 2 {
        let key = parts[0].trim().to_string();
        let value = parts[1].trim().trim_matches('"').to_string();
        if !key.is_empty() {
            return Some((key, value));
        }
    }

    None
}

/// Keyboard-specific input properties (under `input` or `input.keyboard`).
fn apply_input_keyboard_property(key: &str, value: &str, config: &mut Config) {
    match key {
        "layout" | "keyboard_layout" => config.input.keyboard_layout = value.to_string(),
        "repeat_delay" | "repeat-delay" => {
            if let Ok(v) = value.parse() {
                config.input.repeat_delay = v;
            }
        }
        "repeat_rate" | "repeat-rate" => {
            if let Ok(v) = value.parse() {
                config.input.repeat_rate = v;
            }
        }
        "focus-follows-mouse" | "focus_follows_mouse" => {
            config.input.focus_follows_mouse = value == "true" || value == "1";
        }
        "focus-delay-ms" | "focus_delay_ms" => {
            if let Ok(v) = value.parse() {
                config.input.focus_delay_ms = v;
            }
        }
        "warp-on-focus" | "warp_on_focus" => {
            config.input.warp_on_focus = value == "true" || value == "1";
        }
        "numlock" => {
            config.input.numlock = value == "true" || value == "1";
        }
        "mod-key" | "mod_key" => {
            // Normalize: strip optional surrounding quotes, lowercase, replace + with -
            let v = value.trim().trim_matches('"').to_lowercase().replace('+', "-");
            config.input.mod_key = v;
        }
        _ => {}
    }
}

/// Bug fix #5: Mouse-specific input properties (under `input.mouse`).
fn apply_input_mouse_property(key: &str, value: &str, config: &mut Config) {
    match key {
        "scroll-speed" | "mouse_speed" => {
            if let Ok(v) = value.parse() {
                config.input.mouse_speed = v;
            }
        }
        "acceleration" | "mouse_acceleration" => {
            if let Ok(v) = value.parse() {
                config.input.mouse_acceleration = v;
            }
        }
        "natural-scroll" | "natural_scroll" => {
            config.input.natural_scroll = value == "true" || value == "1";
        }
        "sensitivity" | "mouse_sensitivity" => {
            if let Ok(v) = value.parse() {
                config.input.mouse_sensitivity = v;
            }
        }
        "tap-to-click" | "tap_to_click" => {
            config.input.tap_to_click = value == "true" || value == "1";
        }
        "focus-follows-mouse" | "focus_follows_mouse" => {
            config.input.focus_follows_mouse = value == "true" || value == "1";
        }
        _ => {}
    }
}

/// Properties inside an `input.focus { }` nested KDL block.
/// Routes the spec-shaped fields back to the existing flat InputConfig.
fn apply_input_focus_property(key: &str, value: &str, config: &mut Config) {
    match key {
        "follows-mouse" | "follows_mouse" | "focus-follows-mouse" => {
            config.input.focus_follows_mouse = value == "true" || value == "1";
        }
        "focus-delay-ms" | "focus_delay_ms" => {
            if let Ok(v) = value.parse() {
                config.input.focus_delay_ms = v;
            }
        }
        "warp-on-focus" | "warp_on_focus" => {
            config.input.warp_on_focus = value == "true" || value == "1";
        }
        _ => {}
    }
}

/// Properties inside an `input.touch { }` nested KDL block.
fn apply_input_touch_property(key: &str, value: &str, config: &mut Config) {
    match key {
        "enable" | "enabled" => {
            config.input.touch_enabled = value == "true" || value == "1";
        }
        "tap-to-click" | "tap_to_click" => {
            config.input.touch_tap_to_click = value == "true" || value == "1";
        }
        "scroll-speed" | "scroll_speed" => {
            if let Ok(v) = value.parse() {
                config.input.touch_scroll_speed = v;
            }
        }
        _ => {}
    }
}

fn apply_layout_property(key: &str, value: &str, config: &mut Config) {
    match key {
        "inner_gaps" | "inner-gaps" | "inner" => {
            if let Ok(v) = value.parse() {
                config.layout.inner_gaps = v;
            }
        }
        "outer_gaps" | "outer-gaps" | "outer" => {
            if let Ok(v) = value.parse() {
                config.layout.outer_gaps = v;
            }
        }
        "border_width" | "border-width" | "width" => {
            if let Ok(v) = value.parse() {
                config.layout.border_width = v;
            }
        }
        "border_color" | "border-color" | "color" => {
            config.layout.border_color = value.to_string();
        }
        "border_color_focused" | "border-color-focused" | "color-focused" => {
            config.layout.border_color_focused = value.to_string();
        }
        "focus_ring_width" | "focus-ring-width" => {
            if let Ok(v) = value.parse() {
                config.layout.focus_ring_width = v;
            }
        }
        "dim_unfocused" | "dim-unfocused" => {
            if let Ok(v) = value.parse::<f32>() {
                config.layout.dim_unfocused = v.clamp(0.1, 1.0);
            }
        }
        "focus_ring_color" | "focus-ring-color" => {
            config.layout.focus_ring_color = value.to_string();
        }
        "shadow_enable" | "shadow-enable" | "shadows-enable" | "shadow" | "enable" => {
            config.layout.shadow_enable = value == "true" || value == "1";
            // TODO(audit): apply config.layout.shadow_enable in engine.rs strip_frame_for_tiling
            // — DwmExtendFrameIntoClientArea(0,0,0,0) restores native shadow
        }
        "shadow_opacity" | "shadow-opacity" | "opacity" => {
            if let Ok(v) = value.parse() {
                config.layout.shadow_opacity = v;
            }
        }
        "shadow_offset_x" | "shadow-offset-x" => {
            if let Ok(v) = value.parse() {
                config.layout.shadow_offset_x = v;
            }
        }
        "shadow_offset_y" | "shadow-offset-y" => {
            if let Ok(v) = value.parse() {
                config.layout.shadow_offset_y = v;
            }
        }
        "shadow_blur" | "shadow-blur" => {
            if let Ok(v) = value.parse() {
                config.layout.shadow_blur = v;
            }
        }
        "shadow_color" | "shadow-color" => {
            config.layout.shadow_color = value.to_string();
        }
        "scroll_step" | "scroll-step" => {
            if let Ok(v) = value.parse() {
                config.layout.scroll_step = v;
            }
        }
        "column_width" | "column-width" => {
            if let Ok(v) = value.parse() {
                config.layout.column_width = v;
            }
        }
        "column_width_mode" | "column-width-mode" => {
            config.layout.column_width_mode = value.to_string();
        }
        "split_ratio" | "split-ratio" => {
            if let Ok(v) = value.parse() {
                config.layout.split_ratio = v;
            }
        }
        "strip_frame" | "strip-frame" => {
            config.layout.strip_frame = value == "true" || value == "1";
        }
        "border_color_urgent" | "border-color-urgent" | "color-urgent" => {
            config.layout.border_color_urgent = value.to_string();
        }
        "border_radius" | "border-radius" | "radius" => {
            if let Ok(v) = value.parse() {
                config.layout.border_radius = v;
            }
        }
        "border_padding" | "border-padding" | "padding" => {
            if let Ok(v) = value.parse() {
                config.layout.border_padding = v;
            }
        }
        "focus_ring_gap" | "focus-ring-gap" | "gap" => {
            if let Ok(v) = value.parse() {
                config.layout.focus_ring_gap = v;
            }
        }
        "focus_ring_inactive_color" | "focus-ring-inactive-color" | "inactive-color" => {
            config.layout.focus_ring_inactive_color = value.to_string();
        }
        "shadow_spread" | "shadow-spread" | "spread" => {
            if let Ok(v) = value.parse() {
                config.layout.shadow_spread = v;
            }
        }
        _ => {}
    }
}

fn apply_animations_property(key: &str, value: &str, config: &mut Config) {
    match key {
        "enabled" | "enable" => {
            config.animations.enabled = value == "true" || value == "1";
        }
        "duration" => {
            if let Ok(v) = value.parse() {
                config.animations.duration = v;
            }
        }
        "easing" | "curve" => {
            config.animations.easing = value.to_string();
        }
        _ => {}
    }
}

/// Bug fix #6: parse properties inside an `output "name" { … }` block.
fn apply_output_property(key: &str, value: &str, out: &mut OutputConfig) {
    match key {
        "x" => {
            if let Ok(v) = value.parse() {
                out.position.x = v;
            }
        }
        "y" => {
            if let Ok(v) = value.parse() {
                out.position.y = v;
            }
        }
        "width" => {
            if let Ok(v) = value.parse() {
                out.width = v;
            }
        }
        "height" => {
            if let Ok(v) = value.parse() {
                out.height = v;
            }
        }
        "scale" => {
            if let Ok(v) = value.parse() {
                out.scale = v;
            }
        }
        "enable" | "enabled" => {
            out.enable = value == "true" || value == "1";
        }
        "mode" => {
            out.mode = value.to_string();
        }
        "transform" => {
            out.transform = value.to_string();
        }
        "vrr" => {
            out.vrr = value == "true" || value == "1";
        }
        "primary" => {
            out.primary = value == "true" || value == "1";
        }
        _ => {}
    }
}

/// Bug fix #7: parse properties inside a `workspace "name" { … }` block.
fn apply_workspace_property(key: &str, value: &str, ws: &mut WorkspaceConfig) {
    match key {
        "layout" => ws.layout = value.to_string(),
        "monitor" => ws.monitor = value.to_string(),
        "follow-on-focus" | "follow_on_focus" => {
            ws.follow_on_focus = value == "true" || value == "1";
        }
        _ => {}
    }
}

/// Bug fix #8: parse properties inside a `window-rule { … }` block.
fn apply_window_rule_property(key: &str, value: &str, wr: &mut WindowRule) {
    match key {
        // matchers (exact-string; TODO(audit): regex deferred)
        "class-name" | "class" => wr.class = Some(value.to_string()),
        "title" => wr.title = Some(value.to_string()),
        "instance" => wr.instance = Some(value.to_string()),
        "process-name" | "process_name" => wr.process_name = Some(value.to_string()),
        // properties
        "float" | "floating" => {
            wr.floating = value == "true" || value == "1";
        }
        "default-width" | "default_width" => {
            if let Ok(v) = value.parse() {
                wr.default_width = Some(v);
            }
        }
        "default-height" | "default_height" => {
            if let Ok(v) = value.parse() {
                wr.default_height = Some(v);
            }
        }
        "opacity" => {
            if let Ok(v) = value.parse::<f64>() {
                // Validate range: clamp values outside [0.0, 1.0] and warn
                // (rather than reject silently) so the user sees the typo.
                let clamped = if !(0.0..=1.0).contains(&v) {
                    let c = v.clamp(0.0, 1.0);
                    tracing::warn!(
                        "window-rule opacity {} is outside [0.0, 1.0]; clamped to {}",
                        v, c,
                    );
                    c
                } else {
                    v
                };
                wr.opacity = Some(clamped);
            } else {
                tracing::warn!(
                    "window-rule opacity {:?} is not a valid number; ignoring",
                    value,
                );
            }
        }
        "workspace" => {
            wr.workspace = Some(value.to_string());
        }
        "blur" => {
            wr.blur = value == "true" || value == "1";
        }
        "sticky" => {
            wr.sticky = value == "true" || value == "1";
        }
        _ => {}
    }
}

/// Parse a key-value line inside a `match { }` block and push the corresponding
/// `Matcher` variant onto `wr.matchers`.
///
/// Exact-match keys: `class-name`, `title`, `instance`, `process-name`.
/// Regex-match keys: `class-name-regex`, `title-regex`, `instance-regex`,
///   `process-name-regex` — the value is stored as a pattern string and compiled
///   lazily in `Matcher::matches`.
/// Flag variants (no value) are handled by `apply_window_rule_matcher_flag`.
fn apply_window_rule_matcher_kv(key: &str, value: &str, wr: &mut WindowRule) {
    use crate::config::types::Matcher;
    match key {
        // ── exact-string matchers ────────────────────────────────────────────
        "class-name" | "class" => wr.matchers.push(Matcher::ClassName(value.to_string())),
        "title" => wr.matchers.push(Matcher::Title(value.to_string())),
        "instance" => wr.matchers.push(Matcher::Instance(value.to_string())),
        "process-name" | "process_name" => wr.matchers.push(Matcher::ProcessName(value.to_string())),
        // ── regex matchers ───────────────────────────────────────────────────
        "class-name-regex" | "class-regex" => wr.matchers.push(Matcher::ClassNameRegex(value.to_string())),
        "title-regex" => wr.matchers.push(Matcher::TitleRegex(value.to_string())),
        "instance-regex" => wr.matchers.push(Matcher::InstanceRegex(value.to_string())),
        "process-name-regex" | "process-regex" => wr.matchers.push(Matcher::ProcessNameRegex(value.to_string())),
        // ── flag variants with an explicit value (e.g. `is-active true`) ────
        // Bare flags go through `apply_window_rule_matcher_flag`.
        "is-active" | "is_active" => {
            if value == "true" || value == "1" {
                wr.matchers.push(Matcher::IsActive);
            }
        }
        "is-floating" | "is_floating" => {
            if value == "true" || value == "1" {
                wr.matchers.push(Matcher::IsFloating);
            }
        }
        "is-urgent" | "is_urgent" => {
            if value == "true" || value == "1" {
                wr.matchers.push(Matcher::IsUrgent);
            }
        }
        "at-startup" | "at_startup" => {
            if value == "true" || value == "1" {
                wr.matchers.push(Matcher::AtStartup);
            }
        }
        _ => {}
    }
}

/// Parse a bare flag keyword inside a `match { }` block (e.g. `is-active`) and
/// push the corresponding `Matcher` variant.  These lines have no value token.
fn apply_window_rule_matcher_flag(token: &str, wr: &mut WindowRule) {
    use crate::config::types::Matcher;
    match token {
        "is-active" | "is_active" => wr.matchers.push(Matcher::IsActive),
        "is-floating" | "is_floating" => wr.matchers.push(Matcher::IsFloating),
        "is-urgent" | "is_urgent" => wr.matchers.push(Matcher::IsUrgent),
        "at-startup" | "at_startup" => wr.matchers.push(Matcher::AtStartup),
        _ => {}
    }
}

/// Split a line of whitespace-separated tokens, respecting double-quoted strings.
/// Quotes are stripped from each token.  No shell escaping is performed.
/// Examples:
///   `"alacritty" "-e" "tmux"` → ["alacritty", "-e", "tmux"]
///   `firefox`                  → ["firefox"]
fn tokenize_shell_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                // Don't push the quote character itself
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            c => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parse a bind line inside a binds {} block.
/// Format: Mod+Key action-name [args]
/// Example: Ctrl+Alt+Left focus-column-left
///          Ctrl+Alt+1 switch-workspace 1
fn parse_bind_line(line: &str) -> Option<crate::config::HotkeyBinding> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("//") { return None; }

    // Split: first token is key combo, rest is action + args
    let parts: Vec<&str> = line.splitn(2, |c: char| c.is_whitespace()).collect();
    if parts.len() < 2 { return None; }

    let key_combo = parts[0];
    let action_rest = parts[1].trim();

    // Parse modifiers from the combo (e.g. "Ctrl+Alt+Left" -> ["Ctrl", "Alt"], "Left")
    let tokens: Vec<&str> = key_combo.split('+').collect();
    let (modifiers, key) = if tokens.len() == 1 {
        (Vec::new(), tokens[0].to_string())
    } else {
        let mods: Vec<String> = tokens[..tokens.len()-1]
            .iter().map(|t| t.to_string()).collect();
        let key = tokens[tokens.len()-1].to_string();
        (mods, key)
    };

    // Parse action: could be just "action-name" or "action-name arg1 arg2"
    let action_parts: Vec<&str> = action_rest.split_whitespace().collect();
    if action_parts.is_empty() { return None; }

    let command = action_parts[0].trim_matches('"').to_string();
    let args: Vec<String> = action_parts[1..]
        .iter()
        .map(|s| s.trim_matches('"').to_string())
        .collect();

    Some(crate::config::HotkeyBinding {
        modifiers,
        key,
        command,
        args,
        repeat: false,
        release: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let config = parse_kdl_config("").unwrap();
        assert_eq!(config.input.keyboard_layout, "us");
    }

    #[test]
    fn test_parse_comments() {
        let input = r#"
            // This is a comment
            input {
                // Another comment
                layout "de"
            }
        "#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.input.keyboard_layout, "de");
    }

    #[test]
    fn test_parse_input() {
        let input = r#"
            input {
                layout "us"
                repeat-delay 300
                repeat-rate 25
            }
        "#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.input.keyboard_layout, "us");
        assert_eq!(config.input.repeat_delay, 300);
        assert_eq!(config.input.repeat_rate, 25);
    }

    #[test]
    fn test_parse_layout() {
        let input = r##"
            layout {
                inner-gaps 12
                border-width 3
                border-color "#333333"
                border-color-focused "#0066cc"
                focus-ring-width 4
                focus-ring-color "#00aaff"
                shadow-enable true
            }
        "##;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.layout.inner_gaps, 12);
        assert_eq!(config.layout.border_width, 3);
        assert!(config.layout.shadow_enable);
        assert_eq!(config.layout.border_color, "#333333");
    }

    #[test]
    fn test_parse_animations() {
        let input = r#"
            animations {
                enable true
                duration 250
                easing "ease-out-cubic"
            }
        "#;
        let config = parse_kdl_config(input).unwrap();
        assert!(config.animations.enabled);
        assert_eq!(config.animations.duration, 250);
        assert_eq!(config.animations.easing, "ease-out-cubic");
    }

    #[test]
    fn test_parse_kebab_case() {
        let input = r#"
            input {
                repeat-delay 500
            }
            input {
                mouse {
                    natural-scroll true
                }
            }
        "#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.input.repeat_delay, 500);
        assert!(config.input.natural_scroll);
    }

    #[test]
    fn test_parse_binds() {
        let input = r#"
binds {
    Ctrl+Alt+Left focus-column-left
    Ctrl+Alt+Q close-window
    Ctrl+Alt+1 switch-workspace 1
    Ctrl+Alt+Shift+Q quit
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.binds.hotkeys.len(), 4);
        // First bind
        let b = &config.binds.hotkeys[0];
        assert_eq!(b.modifiers, vec!["Ctrl", "Alt"]);
        assert_eq!(b.key, "Left");
        assert_eq!(b.command, "focus-column-left");
        // Second bind
        let b = &config.binds.hotkeys[1];
        assert_eq!(b.key, "Q");
        assert_eq!(b.command, "close-window");
        // Third bind (with arg)
        let b = &config.binds.hotkeys[2];
        assert_eq!(b.command, "switch-workspace");
        assert_eq!(b.args, vec!["1"]);
    }

    #[test]
    fn test_parse_binds_vk_code() {
        let input = "binds {
    Ctrl+Alt+Left focus-left
}";
        let config = parse_kdl_config(input).unwrap();
        let b = &config.binds.hotkeys[0];
        assert_eq!(b.vk_code(), Some(0x25)); // VK_LEFT
        assert_eq!(b.mod_flags(), 0x0002 | 0x0001); // MOD_CTRL | MOD_ALT
        assert_eq!(b.parse_action(), Some(crate::input::Action::FocusColumnLeft));
    }

    #[test]
    fn test_parse_equals_syntax() {
        let input = r#"
            input {
                repeat_delay=600
            }
        "#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.input.repeat_delay, 600);
    }

    /// Bug fix #5 — input.mouse block is now parsed.
    #[test]
    fn test_parse_input_mouse() {
        let input = r#"
input {
    mouse {
        natural-scroll false
        scroll-speed 2.0
        acceleration 0.5
        sensitivity 1.5
        tap-to-click true
    }
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert!(!config.input.natural_scroll);
        assert!((config.input.mouse_speed - 2.0).abs() < 0.001);
        assert!((config.input.mouse_acceleration - 0.5).abs() < 0.001);
        assert!((config.input.mouse_sensitivity - 1.5).abs() < 0.001);
        assert!(config.input.tap_to_click);
    }

    /// Bug fix #12 — focus-follows-mouse field parsed under input.
    #[test]
    fn test_parse_focus_follows_mouse() {
        let input = r#"
input {
    focus-follows-mouse true
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert!(config.input.focus_follows_mouse);
    }

    /// Bug fix #6 + #13 — output block is now fully parsed.
    #[test]
    fn test_parse_output_section() {
        let input = r#"output "DP-1" {
    x 0
    y 0
    width 2560
    height 1440
    scale 1.5
    enable true
}"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.output.len(), 1);
        let out = &config.output[0];
        assert_eq!(out.name, "DP-1");
        assert_eq!(out.position.x, 0);
        assert_eq!(out.position.y, 0);
        assert_eq!(out.width, 2560);
        assert_eq!(out.height, 1440);
        assert!((out.scale - 1.5).abs() < 0.001);
        assert!(out.enable);
    }

    /// Two output blocks produce two entries.
    #[test]
    fn test_parse_multiple_outputs() {
        let input = r#"
output "DP-1" {
    x 0
    y 0
    width 2560
    height 1440
}
output "HDMI-1" {
    x 2560
    y 0
    width 1920
    height 1080
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.output.len(), 2);
        assert_eq!(config.output[0].name, "DP-1");
        assert_eq!(config.output[1].name, "HDMI-1");
        assert_eq!(config.output[1].position.x, 2560);
    }

    /// No output block → single default entry (len == 1).
    #[test]
    fn test_parse_no_output_gives_default() {
        let config = parse_kdl_config("").unwrap();
        assert_eq!(config.output.len(), 1);
        assert_eq!(config.output[0].name, "");
    }

    /// Bug fix #7 — workspace block is now parsed.
    #[test]
    fn test_parse_workspace_section() {
        let input = r#"workspace "Main" {
    layout "tile"
    monitor "DP-1"
    follow-on-focus true
}"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.workspace.len(), 1);
        let ws = &config.workspace[0];
        assert_eq!(ws.name, "Main");
        assert_eq!(ws.layout, "tile");
        assert_eq!(ws.monitor, "DP-1");
        assert!(ws.follow_on_focus);
    }

    /// Bug fix #8 — window-rule block is now parsed.
    #[test]
    fn test_parse_window_rules() {
        let input = r#"window-rule {
    class-name "Firefox"
    floating true
    opacity 0.9
    default-width 1200
    default-height 800
    workspace "Main"
}"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.window_rules.len(), 1);
        let wr = &config.window_rules[0];
        assert_eq!(wr.class.as_deref(), Some("Firefox"));
        assert!(wr.floating);
        let op = wr.opacity.expect("opacity should be Some(0.9) after parse");
        assert!((op - 0.9).abs() < 0.001);
        assert_eq!(wr.default_width, Some(1200));
        assert_eq!(wr.default_height, Some(800));
        assert_eq!(wr.workspace.as_deref(), Some("Main"));
    }

    /// Item 1: `opacity 0` parses to `Some(0.0)`, not None / not dropped.
    #[test]
    fn test_parse_window_rule_opacity_zero() {
        let input = r#"window-rule {
    class-name "Ghost"
    opacity 0.0
}"#;
        let config = parse_kdl_config(input).unwrap();
        let wr = &config.window_rules[0];
        assert_eq!(wr.opacity, Some(0.0));
    }

    /// Item 1: opacity outside [0,1] is clamped (and a warning is logged).
    #[test]
    fn test_parse_window_rule_opacity_clamped() {
        let input = r#"window-rule {
    class-name "Bright"
    opacity 2.5
}"#;
        let config = parse_kdl_config(input).unwrap();
        let wr = &config.window_rules[0];
        assert_eq!(wr.opacity, Some(1.0));
    }

    /// Item 1: no opacity line → `None` on the rule (not the previous default 0.0).
    #[test]
    fn test_parse_window_rule_no_opacity_is_none() {
        let input = r#"window-rule {
    class-name "Plain"
}"#;
        let config = parse_kdl_config(input).unwrap();
        let wr = &config.window_rules[0];
        assert_eq!(wr.opacity, None);
    }

    #[test]
    fn test_parse_window_rule_process_name() {
        let input = r#"window-rule {
    process-name "notepad"
    float true
}"#;
        let config = parse_kdl_config(input).unwrap();
        let wr = &config.window_rules[0];
        assert_eq!(wr.process_name.as_deref(), Some("notepad"));
        assert!(wr.floating);
    }

    #[test]
    fn test_parse_multiple_binds_with_args() {
        let input = "
binds {
    Ctrl+Alt+1 switch-workspace 1
    Ctrl+Alt+2 switch-workspace 2
    Ctrl+Alt+Return exec cmd.exe
}";
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.binds.hotkeys.len(), 3);
        // Check the exec bind has correct args
        let exec_bind = &config.binds.hotkeys[2];
        assert_eq!(exec_bind.command, "exec");
        assert_eq!(exec_bind.args, vec!["cmd.exe"]);
    }

    #[test]
    fn test_parse_malformed_config() {
        // Unmatched braces should still parse gracefully
        let input = r#"input { layout "us""#;
        // Should not panic — may return error or partial config
        let result = parse_kdl_config(input);
        // Either it parses or returns an error — just don't panic
        let _ = result;
    }

    #[test]
    fn test_parse_empty_binds() {
        let input = "binds { }";
        let config = parse_kdl_config(input).unwrap();
        assert!(config.binds.hotkeys.is_empty());
    }

    #[test]
    fn test_parse_shadow_section() {
        let input = r#"
layout {
    shadow-enable true
    shadow-opacity 0.75
    shadow-offset-x 5
    shadow-offset-y 5
    shadow-blur 15
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert!(config.layout.shadow_enable);
        assert!((config.layout.shadow_opacity - 0.75).abs() < 0.01);
        assert_eq!(config.layout.shadow_offset_x, 5);
        assert_eq!(config.layout.shadow_offset_y, 5);
        assert_eq!(config.layout.shadow_blur, 15);
    }

    /// `match { }` block: string matchers are populated into `wr.matchers`.
    #[test]
    fn test_parse_window_rule_match_block_string_matchers() {
        use crate::config::types::Matcher;
        let input = r#"window-rule {
    match {
        class-name "Firefox"
        process-name "firefox.exe"
    }
    float true
}"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.window_rules.len(), 1);
        let wr = &config.window_rules[0];
        assert_eq!(wr.matchers.len(), 2);
        assert!(matches!(&wr.matchers[0], Matcher::ClassName(s) if s == "Firefox"));
        assert!(matches!(&wr.matchers[1], Matcher::ProcessName(s) if s == "firefox.exe"));
        assert!(wr.floating);
        // Flat fields must remain untouched when using match block.
        assert!(wr.class.is_none());
    }

    /// `match { }` block: bare flag matchers with no value.
    #[test]
    fn test_parse_window_rule_match_block_flag_matchers() {
        use crate::config::types::Matcher;
        let input = r#"window-rule {
    match {
        is-active
        at-startup
    }
    float true
}"#;
        let config = parse_kdl_config(input).unwrap();
        let wr = &config.window_rules[0];
        assert_eq!(wr.matchers.len(), 2);
        assert!(matches!(&wr.matchers[0], Matcher::IsActive));
        assert!(matches!(&wr.matchers[1], Matcher::AtStartup));
    }

    /// Flat `class-name` directly in `window-rule` still works (backwards compat).
    #[test]
    fn test_parse_window_rule_flat_still_works() {
        let input = r#"window-rule {
    class-name "Notepad"
    float true
}"#;
        let config = parse_kdl_config(input).unwrap();
        let wr = &config.window_rules[0];
        assert!(wr.matchers.is_empty());
        assert_eq!(wr.class.as_deref(), Some("Notepad"));
        assert!(wr.floating);
    }

    /// `match { }` block with `process-name-regex` produces a `ProcessNameRegex` matcher.
    #[test]
    fn test_parse_window_rule_match_block_process_name_regex() {
        use crate::config::types::Matcher;
        let input = r#"window-rule {
    match {
        process-name-regex "^firefox.exe$"
    }
    float true
}"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.window_rules.len(), 1);
        let wr = &config.window_rules[0];
        assert_eq!(wr.matchers.len(), 1);
        assert!(
            matches!(&wr.matchers[0], Matcher::ProcessNameRegex(s) if s == "^firefox.exe$"),
            "expected ProcessNameRegex(\"^firefox.exe$\"), got {:?}",
            &wr.matchers[0]
        );
    }

    /// New flat input fields round-trip through the KDL parser.
    #[test]
    fn test_parse_input_extras() {
        let input = r#"
input {
    focus-delay-ms 250
    warp-on-focus true
    numlock true
    focus {
        focus-delay-ms 999
        warp-on-focus false
    }
    touch {
        enable true
        tap-to-click false
        scroll-speed 1.75
    }
}
"#;
        let config = parse_kdl_config(input).unwrap();
        // `input.focus { }` block runs after top-level keys; nested values win.
        assert_eq!(config.input.focus_delay_ms, 999);
        assert!(!config.input.warp_on_focus);
        assert!(config.input.numlock);
        assert!(config.input.touch_enabled);
        assert!(!config.input.touch_tap_to_click);
        assert!((config.input.touch_scroll_speed - 1.75).abs() < 0.001);
    }

    /// New flat layout fields round-trip through the KDL parser.
    #[test]
    fn test_parse_layout_extras() {
        // Use `r##"..."##` so the embedded `#` characters in CSS-hex colours
        // don't confuse the raw string terminator.
        let input = r##"
layout {
    border-color-urgent "#ff0000"
    border-radius 12
    border-padding 4
    focus-ring-gap 6
    focus-ring-inactive-color "#404040"
    shadow-spread 5
}
"##;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.layout.border_color_urgent, "#ff0000");
        assert_eq!(config.layout.border_radius, 12);
        assert_eq!(config.layout.border_padding, 4);
        assert_eq!(config.layout.focus_ring_gap, 6);
        assert_eq!(config.layout.focus_ring_inactive_color, "#404040");
        assert_eq!(config.layout.shadow_spread, 5);
    }

    /// `match { }` block with multiple regex keys.
    #[test]
    fn test_parse_window_rule_match_block_regex_variants() {
        use crate::config::types::Matcher;
        let input = r#"window-rule {
    match {
        class-name-regex "^Term.*"
        title-regex ".*Editor.*"
        instance-regex "^main$"
        process-name-regex "(?i)firefox"
    }
}"#;
        let config = parse_kdl_config(input).unwrap();
        let wr = &config.window_rules[0];
        assert_eq!(wr.matchers.len(), 4);
        assert!(matches!(&wr.matchers[0], Matcher::ClassNameRegex(s) if s == "^Term.*"));
        assert!(matches!(&wr.matchers[1], Matcher::TitleRegex(s) if s == ".*Editor.*"));
        assert!(matches!(&wr.matchers[2], Matcher::InstanceRegex(s) if s == "^main$"));
        assert!(matches!(&wr.matchers[3], Matcher::ProcessNameRegex(s) if s == "(?i)firefox"));
    }
}
