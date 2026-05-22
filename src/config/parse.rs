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
                    layout_override: None,
                    mod_key: None,
                });
            } else if section_name == "output.layout" && depth == 0 {
                // Defensive: top-level `output.layout` cannot happen because
                // try_parse_section returns a single name; we get there via
                // the section_stack push of `layout` under `output` instead.
                // No-op kept so the table is exhaustive.
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

        // Binds section: each line is a binding (not key=value), with the
        // exception of `extend-defaults <bool>` which toggles whether the
        // user's binds extend (merge with) or replace the built-in default
        // hotkey table.  Recognised before the per-line bind parser so the
        // "key combo + action" parser doesn't misinterpret the directive.
        if section.as_str() == "binds" {
            let trimmed = line.trim();
            let mut tokens = trimmed.split_whitespace();
            if let Some(head) = tokens.next() {
                let head_norm = head.to_lowercase();
                if head_norm == "extend-defaults" || head_norm == "extend_defaults" {
                    // Accept the niri spelling (bare bool) AND the KDL
                    // `key=value` flavour. Default to true when no value
                    // follows the directive (mirrors niri's behaviour).
                    let raw_val = trimmed
                        .splitn(2, |c: char| c.is_whitespace() || c == '=')
                        .nth(1)
                        .map(|s| s.trim_matches(|c: char| c.is_whitespace() || c == '=' || c == '"').to_lowercase())
                        .unwrap_or_else(|| "true".to_string());
                    let want_extend = matches!(raw_val.as_str(),
                        "true" | "1" | "yes" | "on" | "");
                    config.binds.extend_defaults = want_extend;
                    continue;
                }
            }
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
                // Per-output layout override block (niri parity).  Routes the
                // key=value pairs into `OutputConfig.layout_override`.
                "output.layout" => {
                    if let Some(ref mut out) = current_output {
                        let lo = out.layout_override.get_or_insert_with(LayoutConfigPartial::default);
                        apply_output_layout_property(&key, &value, lo);
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
            layout_override: None,
            mod_key: None,
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
            // Applied by `TilingEngine::apply_shadow_for_window` during the
            // initial tile pass for each window.
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
        "snap_on_drag" | "snap-on-drag" => {
            config.layout.snap_on_drag = value == "true" || value == "1";
        }
        "snap_threshold_px" | "snap-threshold-px" => {
            if let Ok(v) = value.parse() {
                config.layout.snap_threshold_px = v;
            }
        }
        // niri-parity `border-smart` / `smart-borders` flat-form aliases,
        // plus the bare `smart` key inside `borders { smart true }`
        // (handled by the same routing arm because `layout.borders` keys
        // are folded into the same parser).
        "smart_borders" | "smart-borders" | "border_smart" | "border-smart" | "smart" => {
            config.layout.smart_borders = value == "true" || value == "1";
        }
        "status_bar" | "status-bar" => {
            config.layout.status_bar = value == "true" || value == "1";
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
            if let Ok(v) = value.parse::<f64>() {
                if !(0.5..=4.0).contains(&v) {
                    tracing::warn!(
                        "output scale {} is outside the supported range [0.5, 4.0] — \
                         keeping value but Windows DPI APIs may behave unexpectedly",
                        v,
                    );
                }
                out.scale = v;
            } else {
                tracing::warn!("output scale {:?} is not a valid number; ignoring", value);
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
        "mod-key" | "mod_key" => {
            // Per-monitor modifier override (niri parity).  Stored as-is for
            // future monitor-scoped IPC use.  Global hotkeys are not affected —
            // Windows registers them per-thread, not per-display.
            out.mod_key = Some(value.trim().trim_matches('"').to_string());
        }
        _ => {}
    }
}

/// Parse properties inside `output "name" { layout { … } }`.  Routes the
/// key=value pair onto the matching `LayoutConfigPartial` field as `Some(_)`;
/// fields the user doesn't set remain `None` and fall through to the global
/// layout config (see `LayoutConfigPartial::apply_to`).
fn apply_output_layout_property(key: &str, value: &str, lo: &mut LayoutConfigPartial) {
    match key {
        "inner_gaps" | "inner-gaps" | "inner" => {
            if let Ok(v) = value.parse() { lo.inner_gaps = Some(v); }
        }
        "outer_gaps" | "outer-gaps" | "outer" => {
            if let Ok(v) = value.parse() { lo.outer_gaps = Some(v); }
        }
        "border_width" | "border-width" => {
            if let Ok(v) = value.parse() { lo.border_width = Some(v); }
        }
        "border_color" | "border-color" => { lo.border_color = Some(value.to_string()); }
        "border_color_focused" | "border-color-focused" => { lo.border_color_focused = Some(value.to_string()); }
        "focus_ring_width" | "focus-ring-width" => {
            if let Ok(v) = value.parse() { lo.focus_ring_width = Some(v); }
        }
        "dim_unfocused" | "dim-unfocused" => {
            if let Ok(v) = value.parse::<f32>() { lo.dim_unfocused = Some(v.clamp(0.1, 1.0)); }
        }
        "focus_ring_color" | "focus-ring-color" => { lo.focus_ring_color = Some(value.to_string()); }
        "shadow_enable" | "shadow-enable" | "shadows-enable" => {
            lo.shadow_enable = Some(value == "true" || value == "1");
        }
        "shadow_opacity" | "shadow-opacity" => {
            if let Ok(v) = value.parse() { lo.shadow_opacity = Some(v); }
        }
        "shadow_offset_x" | "shadow-offset-x" => {
            if let Ok(v) = value.parse() { lo.shadow_offset_x = Some(v); }
        }
        "shadow_offset_y" | "shadow-offset-y" => {
            if let Ok(v) = value.parse() { lo.shadow_offset_y = Some(v); }
        }
        "shadow_blur" | "shadow-blur" => {
            if let Ok(v) = value.parse() { lo.shadow_blur = Some(v); }
        }
        "shadow_color" | "shadow-color" => { lo.shadow_color = Some(value.to_string()); }
        "scroll_step" | "scroll-step" => {
            if let Ok(v) = value.parse() { lo.scroll_step = Some(v); }
        }
        "column_width" | "column-width" => {
            if let Ok(v) = value.parse() { lo.column_width = Some(v); }
        }
        "column_width_mode" | "column-width-mode" => {
            lo.column_width_mode = Some(value.to_string());
        }
        "split_ratio" | "split-ratio" => {
            if let Ok(v) = value.parse() { lo.split_ratio = Some(v); }
        }
        "auto_balance" | "auto-balance" => {
            lo.auto_balance = Some(value == "true" || value == "1");
        }
        "strip_frame" | "strip-frame" => {
            lo.strip_frame = Some(value == "true" || value == "1");
        }
        "border_color_urgent" | "border-color-urgent" => {
            lo.border_color_urgent = Some(value.to_string());
        }
        "border_radius" | "border-radius" => {
            if let Ok(v) = value.parse() { lo.border_radius = Some(v); }
        }
        "border_padding" | "border-padding" => {
            if let Ok(v) = value.parse() { lo.border_padding = Some(v); }
        }
        "focus_ring_gap" | "focus-ring-gap" => {
            if let Ok(v) = value.parse() { lo.focus_ring_gap = Some(v); }
        }
        "focus_ring_inactive_color" | "focus-ring-inactive-color" => {
            lo.focus_ring_inactive_color = Some(value.to_string());
        }
        "shadow_spread" | "shadow-spread" => {
            if let Ok(v) = value.parse() { lo.shadow_spread = Some(v); }
        }
        // Per-output `smart-borders` override; mirrors the global flat field
        // plus the `border { smart true }` nested-block alias.
        "smart_borders" | "smart-borders" | "border_smart" | "border-smart" | "smart" => {
            lo.smart_borders = Some(value == "true" || value == "1");
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
        // ── niri-parity `open-*` placement properties ────────────────────────
        "open-on-output" | "open_on_output" => {
            wr.open_on_output = Some(value.to_string());
        }
        "open-in-workspace" | "open_in_workspace" => {
            if let Ok(v) = value.parse() {
                wr.open_in_workspace = Some(v);
            }
        }
        "open-fullscreen" | "open_fullscreen" => {
            wr.open_fullscreen = Some(value == "true" || value == "1");
        }
        "open-floating" | "open_floating" => {
            wr.open_floating = Some(value == "true" || value == "1");
        }
        "open-max-bounds" | "open_max_bounds" => {
            // Accept "WxH", "W H", or "W,H".  Each axis with 0 means "no cap".
            let parts: Vec<&str> = value
                .split(|c: char| c == 'x' || c == 'X' || c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                    wr.open_max_bounds = Some((w, h));
                } else {
                    tracing::warn!(
                        "window-rule open-max-bounds {:?} could not be parsed as `<w> <h>`",
                        value,
                    );
                }
            } else {
                tracing::warn!(
                    "window-rule open-max-bounds {:?} must have two dimensions (got {})",
                    value, parts.len(),
                );
            }
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

// ---------------------------------------------------------------------------
// Config validation (used by `wiri-ctl validate-config`)
// ---------------------------------------------------------------------------

/// Severity of a config validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationSeverity {
    /// A fatal issue — the daemon would still load (parser is lenient) but the
    /// indicated configuration cannot take effect.  Examples: invalid colour
    /// literal, unparseable integer, unknown section name.
    Error,
    /// A non-fatal issue worth surfacing — typically a recoverable typo or a
    /// rule that will be silently ignored.  Examples: unrecognised key inside
    /// a known section, unbalanced trailing brace, or an unmatched bind line.
    Warning,
}

impl std::fmt::Display for ValidationSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationSeverity::Error => write!(f, "error"),
            ValidationSeverity::Warning => write!(f, "warning"),
        }
    }
}

/// A single validation finding produced by [`validate_kdl_config`].
///
/// `line` and `column` are 1-indexed positions into the input string.
/// `column` is best-effort and currently always points to the start of the
/// offending token (column 1 when no specific token is known).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub severity: ValidationSeverity,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}",
            self.line, self.column, self.severity, self.message
        )
    }
}

/// Top-level sections accepted by the KDL parser.
const KNOWN_TOP_LEVEL_SECTIONS: &[&str] = &[
    "input",
    "output",
    "layout",
    "workspace",
    "window-rule",
    "binds",
    "animations",
];

/// Sections that may appear nested inside another section. Keyed by parent.
fn known_nested_sections(parent: &str) -> &'static [&'static str] {
    match parent {
        "input" => &["keyboard", "mouse", "focus", "touch"],
        "layout" => &["gaps", "borders", "focus-ring", "shadows"],
        "window-rule" => &["match"],
        // niri parity: `output "DP-1" { layout { … } }` for per-output
        // layout overrides.  See `OutputConfig::layout_override`.
        "output" => &["layout"],
        _ => &[],
    }
}

/// Validate a KDL configuration string and return any structural / semantic
/// issues without halting on the first error.
///
/// The check is intentionally limited to issues a user can fix from the
/// surface syntax: brace balance, unknown sections, unparseable values, and
/// invalid colour literals.  Deep semantic validation (e.g. workspace name
/// references) is left to runtime.
pub fn validate_kdl_config(input: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut section_stack: Vec<(String, usize)> = Vec::new();

    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();

        // Skip empty / comment lines.
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let line = strip_inline_comment(line);
        if line.is_empty() {
            continue;
        }

        // Section open?
        if let Some((name, _arg)) = try_parse_section(line) {
            let depth = section_stack.len();
            if depth == 0 {
                if !KNOWN_TOP_LEVEL_SECTIONS.contains(&name.as_str()) {
                    issues.push(ValidationIssue {
                        severity: ValidationSeverity::Error,
                        line: line_no,
                        column: 1,
                        message: format!(
                            "unknown top-level section '{}' (known: {})",
                            name,
                            KNOWN_TOP_LEVEL_SECTIONS.join(", ")
                        ),
                    });
                }
            } else {
                let parent = section_stack.last().map(|(n, _)| n.as_str()).unwrap_or("");
                let allowed = known_nested_sections(parent);
                if !allowed.is_empty() && !allowed.contains(&name.as_str()) {
                    issues.push(ValidationIssue {
                        severity: ValidationSeverity::Warning,
                        line: line_no,
                        column: 1,
                        message: format!(
                            "unknown nested section '{}' under '{}' (known: {})",
                            name,
                            parent,
                            allowed.join(", ")
                        ),
                    });
                }
            }
            section_stack.push((name, line_no));
            continue;
        }

        // Section close?
        if line.starts_with('}') {
            if section_stack.pop().is_none() {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Error,
                    line: line_no,
                    column: 1,
                    message: "unmatched closing brace".to_string(),
                });
            }
            continue;
        }

        let section_path = section_stack
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(".");

        // binds {} entries are validated separately — they're not key=value.
        if section_path.as_str() == "binds" {
            if parse_bind_line(line).is_none() {
                issues.push(ValidationIssue {
                    severity: ValidationSeverity::Warning,
                    line: line_no,
                    column: 1,
                    message: "could not parse as a `Mod+Key action` bind line".to_string(),
                });
            }
            continue;
        }

        // Inside window-rule.match a bare flag keyword is OK (is-active /
        // is-floating / is-urgent / at-startup); otherwise we want key=value.
        if section_path.as_str() == "window-rule.match" {
            if parse_property(line).is_some() {
                continue;
            }
            let trimmed = line.trim();
            if matches!(trimmed, "is-active" | "is-floating" | "is-urgent" | "at-startup") {
                continue;
            }
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Warning,
                line: line_no,
                column: 1,
                message: format!(
                    "unrecognised matcher '{}' (expected key=value or one of: is-active, is-floating, is-urgent, at-startup)",
                    trimmed
                ),
            });
            continue;
        }

        // spawn-at-startup is a multi-arg top-level directive; just accept it.
        if section_path.is_empty()
            && (line.starts_with("spawn-at-startup") || line.starts_with("spawn_at_startup"))
        {
            continue;
        }

        // Standard key=value path. Pull (key, value) and run semantic checks
        // (colour validity, integer parse) where applicable.
        let Some((key, value)) = parse_property(line) else {
            // Could not parse at all — flag as warning, don't choke the rest.
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Warning,
                line: line_no,
                column: 1,
                message: format!("could not parse '{}' as a key=value property", line),
            });
            continue;
        };

        // Colour-typed fields. We don't reject empty strings (they're how
        // users disable a field) but anything non-empty that doesn't parse is
        // an error.
        let is_color_key = key.ends_with("_color")
            || key.ends_with("-color")
            || key == "shadow-color"
            || key == "shadow_color";
        if is_color_key && !value.is_empty() && crate::config::parse_color(&value).is_none() {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                line: line_no,
                column: 1,
                message: format!(
                    "invalid colour '{}' for '{}' (expected #rgb hex of length 6 or 8)",
                    value, key
                ),
            });
        }

        // Numeric-typed fields. Look at well-known integer keys (gaps,
        // borders, widths, etc.) and surface a parse error early.
        let is_int_key = matches!(
            key.as_str(),
            "inner_gaps" | "inner-gaps"
                | "outer_gaps" | "outer-gaps"
                | "border_width" | "border-width"
                | "border_radius" | "border-radius"
                | "border_padding" | "border-padding"
                | "focus_ring_width" | "focus-ring-width"
                | "focus_ring_gap" | "focus-ring-gap"
                | "shadow_blur" | "shadow-blur"
                | "shadow_spread" | "shadow-spread"
                | "column_width" | "column-width"
                | "scroll_step" | "scroll-step"
                | "repeat_delay" | "repeat-delay"
                | "repeat_rate" | "repeat-rate"
                | "duration"
        );
        if is_int_key && !value.is_empty() && value.parse::<i64>().is_err() {
            issues.push(ValidationIssue {
                severity: ValidationSeverity::Error,
                line: line_no,
                column: 1,
                message: format!("'{}' expects an integer, got '{}'", key, value),
            });
        }
    }

    // Anything left on the stack is an unclosed section.
    for (name, line_no) in section_stack {
        issues.push(ValidationIssue {
            severity: ValidationSeverity::Error,
            line: line_no,
            column: 1,
            message: format!("unclosed section '{}' — missing closing brace", name),
        });
    }

    issues
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
    fn test_parse_border_color_focused_accepts_hex_and_accent() {
        // Hex literal — the parser stores the raw string verbatim; the
        // engine's `LayoutConfig::from_config` is what interprets it.
        let cfg = parse_kdl_config(r##"
            layout {
                border-color-focused "#abcdef"
            }
        "##)
            .expect("hex parse");
        assert_eq!(cfg.layout.border_color_focused, "#abcdef");

        // "accent" sentinel — also accepted verbatim; engine flips the
        // mode to WindowsAccent on `from_config`.
        let cfg = parse_kdl_config(r#"
            layout {
                border-color-focused "accent"
            }
        "#)
            .expect("accent parse");
        assert_eq!(cfg.layout.border_color_focused, "accent");

        // "windows-accent" alias — same code path; the engine recognises
        // it as a sentinel too.
        let cfg = parse_kdl_config(r#"
            layout {
                border-color-focused "windows-accent"
            }
        "#)
            .expect("windows-accent parse");
        assert_eq!(cfg.layout.border_color_focused, "windows-accent");
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

    /// niri-parity `spawn-cmd "<command>"` bind line round-trips through the
    /// binds parser and is recognised by `HotkeyBinding::is_spawn_cmd`.
    #[test]
    fn test_parse_binds_spawn_cmd() {
        let input = r#"binds {
    Ctrl+Alt+S spawn-cmd "wt.exe"
}"#;
        let cfg = parse_kdl_config(input).unwrap();
        assert_eq!(cfg.binds.hotkeys.len(), 1);
        let b = &cfg.binds.hotkeys[0];
        assert_eq!(b.modifiers, vec!["Ctrl", "Alt"]);
        assert_eq!(b.key, "S");
        assert_eq!(b.command, "spawn-cmd");
        assert_eq!(b.args, vec!["wt.exe".to_string()]);
        assert!(b.is_spawn_cmd());
        assert_eq!(
            b.shell_command_argv(),
            Some(vec!["cmd.exe".to_string(), "/C".to_string(), "wt.exe".to_string()]),
        );
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

    #[test]
    fn test_parse_snap_on_drag_enabled() {
        let input = r#"
layout {
    snap-on-drag true
    snap-threshold-px 32
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert!(config.layout.snap_on_drag);
        assert_eq!(config.layout.snap_threshold_px, 32);
    }

    #[test]
    fn test_parse_snap_on_drag_disabled() {
        let input = r#"
layout {
    snap-on-drag false
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert!(!config.layout.snap_on_drag);
    }

    #[test]
    fn test_parse_snap_defaults_when_omitted() {
        // Without the keys, the layout defaults (snap_on_drag=true,
        // snap_threshold_px=20) should hold.
        let input = r#"
layout {
    inner-gaps 8
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert!(config.layout.snap_on_drag);
        assert_eq!(config.layout.snap_threshold_px, 20);
    }

    #[test]
    fn test_parse_snap_threshold_underscore_alias() {
        // Both spellings (`snap-threshold-px` and `snap_threshold_px`) work.
        let input = r#"
layout {
    snap_threshold_px 5
    snap_on_drag true
}
"#;
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.layout.snap_threshold_px, 5);
        assert!(config.layout.snap_on_drag);
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

    // -----------------------------------------------------------------------
    // validate_kdl_config tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_empty_input_has_no_issues() {
        assert!(validate_kdl_config("").is_empty());
    }

    #[test]
    fn test_validate_clean_config_has_no_issues() {
        let input = r##"
            input {
                keyboard_layout "us"
            }
            layout {
                inner_gaps 8
                border_color "#333333"
            }
        "##;
        let issues = validate_kdl_config(input);
        assert!(issues.is_empty(), "expected no issues, got: {:?}", issues);
    }

    #[test]
    fn test_validate_unknown_top_level_section() {
        let input = "telemetry {\n  enabled true\n}\n";
        let issues = validate_kdl_config(input);
        assert!(
            issues.iter().any(|i| i.severity == ValidationSeverity::Error
                && i.message.contains("unknown top-level section")
                && i.line == 1),
            "expected error on line 1, got: {:?}", issues
        );
    }

    #[test]
    fn test_validate_unclosed_section_is_error() {
        let input = "layout {\n  inner_gaps 8\n";
        let issues = validate_kdl_config(input);
        assert!(
            issues.iter().any(|i| i.severity == ValidationSeverity::Error
                && i.message.contains("unclosed section 'layout'")),
            "expected unclosed-section error, got: {:?}", issues
        );
    }

    #[test]
    fn test_validate_unmatched_close_brace() {
        let input = "}\n";
        let issues = validate_kdl_config(input);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, ValidationSeverity::Error);
        assert!(issues[0].message.contains("unmatched closing brace"));
    }

    #[test]
    fn test_validate_invalid_color_is_error() {
        let input = "layout {\n  border_color \"red\"\n}\n";
        let issues = validate_kdl_config(input);
        assert!(
            issues.iter().any(|i| i.severity == ValidationSeverity::Error
                && i.message.contains("invalid colour 'red'")),
            "expected colour error, got: {:?}", issues
        );
    }

    #[test]
    fn test_validate_empty_color_value_is_accepted() {
        // Users sometimes set a colour field to "" to disable it; this should
        // not produce an error.
        let input = "layout {\n  focus_ring_inactive_color \"\"\n}\n";
        let issues = validate_kdl_config(input);
        assert!(issues.is_empty(), "empty colour string must not be flagged");
    }

    #[test]
    fn test_validate_non_integer_for_int_field_is_error() {
        let input = "layout {\n  inner_gaps \"eight\"\n}\n";
        let issues = validate_kdl_config(input);
        assert!(
            issues.iter().any(|i| i.severity == ValidationSeverity::Error
                && i.message.contains("'inner_gaps' expects an integer")),
            "expected integer parse error, got: {:?}", issues
        );
    }

    #[test]
    fn test_validate_bind_warning() {
        let input = "binds {\n  ____\n}\n";
        let issues = validate_kdl_config(input);
        assert!(
            issues.iter().any(|i| i.severity == ValidationSeverity::Warning
                && i.message.contains("bind line")),
            "expected bind warning, got: {:?}", issues
        );
    }

    #[test]
    fn test_validate_window_rule_match_flag_keywords_accepted() {
        let input = "window-rule {\n  match {\n    is-active\n    is-floating\n    is-urgent\n    at-startup\n  }\n}\n";
        let issues = validate_kdl_config(input);
        assert!(issues.is_empty(), "flag keywords must not produce issues, got: {:?}", issues);
    }

    #[test]
    fn test_validate_unknown_nested_section_is_warning() {
        let input = "input {\n  joystick {\n    enable true\n  }\n}\n";
        let issues = validate_kdl_config(input);
        assert!(
            issues.iter().any(|i| i.severity == ValidationSeverity::Warning
                && i.message.contains("unknown nested section 'joystick'")),
            "expected nested-section warning, got: {:?}", issues
        );
    }

    // -----------------------------------------------------------------------
    // Output-relative layout overrides
    // -----------------------------------------------------------------------

    /// `output "DP-1" { layout { column-width 800 } }` populates
    /// `OutputConfig.layout_override` with only the explicitly-set field.
    #[test]
    fn test_parse_output_layout_override_basic() {
        let input = r##"output "DP-1" {
    x 0
    y 0
    width 2560
    height 1440
    layout {
        column-width 800
        inner-gaps 20
        border-color "#abcdef"
    }
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        let out = &cfg.output[0];
        assert_eq!(out.name, "DP-1");
        let lo = out.layout_override.as_ref().expect("layout override populated");
        assert_eq!(lo.column_width, Some(800));
        assert_eq!(lo.inner_gaps, Some(20));
        assert_eq!(lo.border_color.as_deref(), Some("#abcdef"));
        // Fields not set in KDL must remain None.
        assert!(lo.scroll_step.is_none());
        assert!(lo.border_color_focused.is_none());
    }

    /// `LayoutConfigPartial::apply_to` overlays Some(_) fields on the base.
    #[test]
    fn test_layout_partial_apply_to_overlays_only_set_fields() {
        let mut base = LayoutConfig::default_config();
        base.column_width = 500;
        base.inner_gaps = 16;
        base.border_color = "#111111".to_string();

        let partial = LayoutConfigPartial {
            column_width: Some(900),
            border_color: Some("#222222".to_string()),
            ..LayoutConfigPartial::default()
        };

        let effective = partial.apply_to(&base);
        assert_eq!(effective.column_width, 900, "Some(900) wins");
        assert_eq!(effective.border_color, "#222222", "Some color wins");
        assert_eq!(effective.inner_gaps, 16, "None field falls through to base");
    }

    /// Output without a nested layout block has `layout_override = None`.
    #[test]
    fn test_parse_output_without_layout_override() {
        let input = r##"output "Primary" {
    x 0
    y 0
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        assert!(cfg.output[0].layout_override.is_none());
    }

    /// Multi-output configs keep per-output layout overrides separated.
    #[test]
    fn test_parse_multiple_outputs_with_distinct_layout_overrides() {
        let input = r##"output "DP-1" {
    layout {
        column-width 800
    }
}
output "HDMI-1" {
    layout {
        column-width 400
    }
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        assert_eq!(cfg.output.len(), 2);
        assert_eq!(cfg.output[0].layout_override.as_ref().and_then(|p| p.column_width), Some(800));
        assert_eq!(cfg.output[1].layout_override.as_ref().and_then(|p| p.column_width), Some(400));
    }

    /// Validator recognises `output > layout` as a known nested section.
    #[test]
    fn test_validate_output_layout_nested_section_is_known() {
        let input = "output \"DP-1\" {\n  layout {\n    column-width 800\n  }\n}\n";
        let issues = validate_kdl_config(input);
        let unknown_nested = issues.iter().filter(|i|
            i.message.contains("unknown nested section")
        ).count();
        assert_eq!(unknown_nested, 0, "output > layout must be known, got: {:?}", issues);
    }

    // -----------------------------------------------------------------------
    // Scale validation
    // -----------------------------------------------------------------------

    /// In-range scale values are accepted silently.
    #[test]
    fn test_parse_output_scale_in_range_is_accepted() {
        let input = r##"output "DP-1" {
    scale 1.5
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        assert!((cfg.output[0].scale - 1.5).abs() < 1e-9);

        let input2 = r##"output "DP-1" {
    scale 4.0
}"##;
        let cfg2 = parse_kdl_config(input2).unwrap();
        assert!((cfg2.output[0].scale - 4.0).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // niri-parity `open-*` window-rule properties
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_window_rule_open_on_output() {
        let input = r#"window-rule {
    class-name "Firefox"
    open-on-output "HDMI-1"
}"#;
        let cfg = parse_kdl_config(input).unwrap();
        let wr = &cfg.window_rules[0];
        assert_eq!(wr.open_on_output.as_deref(), Some("HDMI-1"));
    }

    #[test]
    fn test_parse_window_rule_open_in_workspace_and_fullscreen() {
        let input = r#"window-rule {
    class-name "Steam"
    open-in-workspace 4
    open-fullscreen true
}"#;
        let cfg = parse_kdl_config(input).unwrap();
        let wr = &cfg.window_rules[0];
        assert_eq!(wr.open_in_workspace, Some(4));
        assert_eq!(wr.open_fullscreen, Some(true));
    }

    #[test]
    fn test_parse_window_rule_open_floating() {
        let input = r#"window-rule {
    class-name "Calculator"
    open-floating true
}"#;
        let cfg = parse_kdl_config(input).unwrap();
        let wr = &cfg.window_rules[0];
        assert_eq!(wr.open_floating, Some(true));
    }

    /// `open-max-bounds` accepts `<w>x<h>`, `<w> <h>`, and `<w>,<h>` syntaxes.
    #[test]
    fn test_parse_window_rule_open_max_bounds_variants() {
        for v in ["1600x900", "1600 900", "1600,900"] {
            let input = format!(
                "window-rule {{\n  class-name \"X\"\n  open-max-bounds \"{}\"\n}}",
                v
            );
            let cfg = parse_kdl_config(&input).unwrap_or_else(|e| panic!("parse {}: {}", v, e));
            let wr = &cfg.window_rules[0];
            assert_eq!(
                wr.open_max_bounds,
                Some((1600, 900)),
                "open-max-bounds {:?} must parse to (1600, 900)",
                v
            );
        }
    }

    /// `open-max-bounds` with a single dimension is ignored (warn logged).
    #[test]
    fn test_parse_window_rule_open_max_bounds_invalid_is_ignored() {
        let input = r#"window-rule {
    class-name "X"
    open-max-bounds "1600"
}"#;
        let cfg = parse_kdl_config(input).unwrap();
        let wr = &cfg.window_rules[0];
        assert_eq!(wr.open_max_bounds, None);
    }

    /// Out-of-range scale values still parse (warn is logged) but the value is
    /// stored verbatim so the user can observe the effect.
    #[test]
    fn test_parse_output_scale_out_of_range_still_parses() {
        let input = r##"output "DP-1" {
    scale 8.0
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        // Value preserved; downstream Win32 APIs may reject it, but parser
        // does not silently clamp (matches niri: warn-and-keep).
        assert!((cfg.output[0].scale - 8.0).abs() < 1e-9);

        let input_low = r##"output "DP-1" {
    scale 0.25
}"##;
        let cfg_low = parse_kdl_config(input_low).unwrap();
        assert!((cfg_low.output[0].scale - 0.25).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // Per-monitor mod-key parsing (Item 2 — niri parity)
    // -----------------------------------------------------------------------

    /// `output "DP-1" { mod-key "super" }` populates `OutputConfig.mod_key`.
    #[test]
    fn test_output_config_mod_key_parsing() {
        let input = r##"output "DP-1" {
    mod-key "super"
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        assert_eq!(cfg.output.len(), 1);
        assert_eq!(
            cfg.output[0].mod_key.as_deref(),
            Some("super"),
            "mod-key should be stored verbatim (without quotes)",
        );
    }

    /// The underscore alias `mod_key` also parses correctly.
    #[test]
    fn test_output_config_mod_key_underscore_alias() {
        let input = r##"output "HDMI-1" {
    mod_key "ctrl-alt"
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        assert_eq!(cfg.output[0].mod_key.as_deref(), Some("ctrl-alt"));
    }

    /// An output block without `mod-key` leaves the field as `None`.
    #[test]
    fn test_output_config_mod_key_absent_stays_none() {
        let input = r##"output "DP-2" {
    x 1920
    y 0
}"##;
        let cfg = parse_kdl_config(input).unwrap();
        assert!(
            cfg.output[0].mod_key.is_none(),
            "mod_key should be None when not specified in output block",
        );
    }
}
