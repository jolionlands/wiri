//! Entry points of the config parser.
//!
//! The text is read by [`super::kdl`] and mapped onto [`Config`] by [`super::decode`],
//! which also collects every problem it finds with `line:column`.  This module keeps the
//! names the rest of the code (and `wiri-ctl validate-config`) uses.

use std::path::Path;

use crate::config::decode::{self, Decoded};
use crate::config::error::{ConfigError, Result};
use crate::config::types::Config;

pub use crate::config::decode::{ValidationIssue, ValidationSeverity};

/// Parse a KDL configuration string into a [`Config`].  `include` lines are not followed
/// (there is no directory to resolve them against; use [`Config::load_from_str_with_base`]).
/// Fails with the first error when the text has any; warnings are dropped here.
pub fn parse_kdl_config(input: &str) -> Result<Config> {
    into_result(decode::decode_text(input, "", None), "")
}

/// Every error and warning in a configuration string, without loading it.
pub fn validate_kdl_config(input: &str) -> Vec<ValidationIssue> {
    decode::decode_text(input, "", None).issues
}

/// Every error and warning in a config file and the files it includes.
pub fn validate_config_file(path: &Path) -> Vec<ValidationIssue> {
    decode::decode_file(path).issues
}

/// The config of a decoded text, or the first error (with a count of the others) in
/// `file:line:column: error: message` form.
pub fn into_result(decoded: Decoded, default_file: &str) -> Result<Config> {
    if !decoded.has_errors() {
        return Ok(decoded.config);
    }
    let errors: Vec<_> = decoded
        .issues
        .iter()
        .filter(|i| i.severity == ValidationSeverity::Error)
        .collect();
    let mut message = errors
        .iter()
        .map(|i| super::diagnostics::render(i, default_file))
        .collect::<Vec<_>>()
        .join("\n\n");
    let more = errors.len().saturating_sub(1);
    if more > 0 {
        message.push_str(&format!(
            " (and {} more error{})",
            more,
            if more == 1 { "" } else { "s" }
        ));
    }
    Err(ConfigError::Invalid(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::*;

    #[test]
    fn test_parse_empty() {
        let config = parse_kdl_config("").unwrap();
        assert_eq!(config.input.keyboard_layout, "us");
    }

    #[test]
    fn a_config_with_errors_fails_with_the_first_one_and_a_count() {
        let err =
            parse_kdl_config("layout {\n    gapz 8\n    border-colour \"x\"\n}\n").unwrap_err();
        let text = err.to_string();
        assert!(
            text.starts_with("2:5: error: unknown option 'gapz'"),
            "{}",
            text
        );
        assert!(text.ends_with("(and 1 more error)"), "{}", text);
        // Warnings alone do not fail the load.
        assert!(parse_kdl_config("prefer-no-csd\n").is_ok());
    }

    #[test]
    fn a_malformed_config_is_an_error_not_a_panic() {
        let err = parse_kdl_config("input { layout \"us\"").unwrap_err();
        assert!(err.to_string().contains("never closed"), "{}", err);
    }

    #[test]
    fn validate_reports_errors_and_warnings_with_positions() {
        let issues =
            validate_kdl_config("telemetry {\n}\nbinds {\n    Hyper+F9 { close-window; }\n}\n");
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Error)
            .collect();
        let warnings: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == ValidationSeverity::Warning)
            .collect();
        assert_eq!(errors.len(), 2, "{:?}", issues);
        assert_eq!(errors[0].line, 1);
        assert_eq!(errors[1].line, 4);
        assert!(errors[1].message.contains("Hyper"), "{:?}", errors[1]);
        assert!(warnings.is_empty(), "{:?}", issues);
        assert!(validate_kdl_config("").is_empty());
    }

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

    /// Every bind of a shipped config must resolve to a real key and a real action, and the
    /// file must load without a single error or warning: the shipped files are what new users
    /// copy.
    fn assert_ships_clean(label: &str, text: &str, expect_binds: bool) {
        let decoded = decode::decode_text(text, label, None);
        assert!(
            decoded.issues.is_empty(),
            "{}: {:#?}",
            label,
            decoded.issues
        );
        let cfg = decoded.config;
        if expect_binds {
            assert!(!cfg.binds.hotkeys.is_empty(), "{}: no binds parsed", label);
        }
        for bind in &cfg.binds.hotkeys {
            assert!(
                bind.vk_code().is_some(),
                "{}: unknown key '{}' in bind '{} {:?}'",
                label,
                bind.key,
                bind.command,
                bind.args
            );
            assert!(
                bind.parse_action().is_some(),
                "{}: unknown action '{}' {:?} (bound to '{}')",
                label,
                bind.command,
                bind.args,
                bind.key
            );
        }
    }

    #[test]
    fn the_default_config_ships_clean() {
        assert_ships_clean(
            "resources/default_config.kdl",
            include_str!("../../resources/default_config.kdl"),
            true,
        );
    }

    #[test]
    fn the_example_configs_ship_clean() {
        assert_ships_clean(
            "examples/cheatsheet.kdl",
            include_str!("../../examples/cheatsheet.kdl"),
            true,
        );
        assert_ships_clean(
            "examples/minimal.kdl",
            include_str!("../../examples/minimal.kdl"),
            false,
        );
    }

    /// `width` / `color` are valid in `border`, `focus-ring` and `shadow`; they must stay in
    /// their own blocks (the focus ring's used to overwrite the border's).
    #[test]
    fn the_default_config_layout_values_stay_in_their_own_blocks() {
        let cfg = parse_kdl_config(include_str!("../../resources/default_config.kdl")).unwrap();
        assert_eq!(cfg.layout.border_width, 4);
        assert_eq!(cfg.layout.border_color, "#4d4d4d");
        assert_eq!(cfg.layout.border_color_focused, "#3381d9");
        assert_eq!(cfg.layout.focus_ring_width, 3);
        assert_eq!(cfg.layout.focus_ring_color, "#3381d9");
        assert_eq!(cfg.layout.inner_gaps, 16);
        assert_eq!(cfg.layout.outer_gaps, 8);
        assert!(!cfg.layout.shadow_enable);
    }

    #[test]
    fn the_default_config_binds_what_the_old_one_did() {
        let cfg = parse_kdl_config(include_str!("../../resources/default_config.kdl")).unwrap();
        let binds = &cfg.binds.hotkeys;
        let find = |mods: &[&str], key: &str| {
            binds.iter().find(|b| {
                b.key.eq_ignore_ascii_case(key)
                    && b.modifiers.len() == mods.len()
                    && mods
                        .iter()
                        .all(|m| b.modifiers.iter().any(|x| x.eq_ignore_ascii_case(m)))
            })
        };
        let left = find(&["Mod"], "Left").expect("Mod+Left");
        assert_eq!(
            left.parse_action(),
            Some(crate::input::Action::FocusColumnLeft)
        );
        let q = find(&["Mod"], "Q").expect("Mod+Q");
        assert_eq!(q.parse_action(), Some(crate::input::Action::CloseWindow));
        let esc = find(&[], "Escape").expect("Escape");
        assert_eq!(
            esc.parse_action(),
            Some(crate::input::Action::OverviewToggle)
        );
        let ws3 = find(&["Mod"], "3").expect("Mod+3");
        assert_eq!(
            ws3.parse_action(),
            Some(crate::input::Action::FocusWorkspace(3))
        );
        let quit = find(&["Mod", "Shift"], "Q").expect("Mod+Shift+Q");
        assert_eq!(quit.parse_action(), Some(crate::input::Action::Quit));
        let term = find(&["Mod"], "Return").expect("Mod+Return");
        assert_eq!(
            term.parse_action(),
            Some(crate::input::Action::SpawnCmd(
                "start wt.exe || start powershell.exe".to_string()
            ))
        );
        assert!(binds.len() >= 50, "{} binds", binds.len());
    }

    #[test]
    fn test_hide_method_parses_and_defaults_to_hide() {
        let cfg = parse_kdl_config("layout {\n    gaps 8\n}\n").unwrap();
        assert_eq!(cfg.layout.hide_method, "hide");
        let cfg = parse_kdl_config("layout {\n    hide-method \"cloak\"\n}\n").unwrap();
        assert_eq!(cfg.layout.hide_method, "cloak");
        assert_eq!(
            crate::backend::cloak::HideMethod::parse(&cfg.layout.hide_method),
            Some(crate::backend::cloak::HideMethod::Cloak)
        );
        let cfg = parse_kdl_config("layout {\n    hide-method hide\n}\n").unwrap();
        assert_eq!(cfg.layout.hide_method, "hide");
    }

    #[test]
    fn test_max_visible_columns_parses_and_defaults_to_three() {
        let cfg = parse_kdl_config("").unwrap();
        assert_eq!(cfg.layout.max_visible_columns, 3);
        let cfg = parse_kdl_config("layout {\n    max-visible-columns 4\n}\n").unwrap();
        assert_eq!(cfg.layout.max_visible_columns, 4);
        let cfg = parse_kdl_config("layout {\n    max-visible-columns 0\n}\n").unwrap();
        assert_eq!(cfg.layout.max_visible_columns, 0);
    }

    #[test]
    fn test_unknown_modifiers_are_fatal_but_mod5_is_a_supported_altgr_alias() {
        // An unknown modifier must never become a bare global F9 or a partial load.
        let input =
            "binds {\n  Hyper+F9 close-window\n  Ctrl+Alt+Q close-window\n  Mod5+T quit\n}\n";
        let error = parse_kdl_config(input).unwrap_err();
        assert!(error.to_string().contains("unknown modifier 'Hyper'"));
        let config = parse_kdl_config("binds { Mod5+T quit; }").unwrap();
        assert_eq!(config.binds.hotkeys.len(), 1);
        assert_eq!(config.binds.hotkeys[0].key, "T");
        assert_eq!(
            config.binds.hotkeys[0].resolve_modifiers(crate::input::DEFAULT_MOD_KEY),
            Ok(crate::input::keynames::MOD_ALTGR)
        );
    }

    #[test]
    fn test_mod_token_is_a_valid_modifier_and_follows_mod_key() {
        let input = "input {\n  mod-key \"super\"\n}\nbinds {\n  Mod+T toggle-floating\n  Mod+Shift+Q quit\n}\n";
        let config = parse_kdl_config(input).unwrap();
        assert_eq!(config.binds.hotkeys.len(), 2);
        let mod_key = crate::input::mod_key_flags(&config.input.mod_key).unwrap();
        assert_eq!(mod_key, crate::input::MOD_WIN);
        assert_eq!(
            config.binds.hotkeys[0].resolve_modifiers(mod_key),
            Ok(crate::input::MOD_WIN)
        );
        assert_eq!(
            config.binds.hotkeys[1].resolve_modifiers(mod_key),
            Ok(crate::input::MOD_WIN | crate::input::MOD_SHIFT)
        );
        // Without a mod-key the prefix is wiri's long-standing Ctrl+Alt.
        let plain = parse_kdl_config("binds {\n  Mod+T toggle-floating\n}\n").unwrap();
        assert_eq!(
            plain.binds.hotkeys[0]
                .resolve_modifiers(crate::input::mod_key_flags(&plain.input.mod_key).unwrap()),
            Ok(crate::input::MOD_CTRL | crate::input::MOD_ALT)
        );
    }

    #[test]
    fn test_parse_binds_vk_code() {
        let config = parse_kdl_config("binds {\n    Ctrl+Alt+Left focus-left\n}").unwrap();
        let b = &config.binds.hotkeys[0];
        assert_eq!(b.vk_code(), Some(0x25)); // VK_LEFT
        assert_eq!(b.mod_flags(), 0x0002 | 0x0001); // MOD_CTRL | MOD_ALT
        assert_eq!(
            b.parse_action(),
            Some(crate::input::Action::FocusColumnLeft)
        );
    }

    #[test]
    fn test_parse_binds_spawn_cmd() {
        let cfg = parse_kdl_config("binds {\n    Ctrl+Alt+S spawn-cmd \"wt.exe\"\n}").unwrap();
        let b = &cfg.binds.hotkeys[0];
        assert_eq!(b.modifiers, vec!["Ctrl", "Alt"]);
        assert_eq!(b.key, "S");
        assert_eq!(b.command, "spawn-cmd");
        assert_eq!(b.args, vec!["wt.exe".to_string()]);
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

    #[test]
    fn test_parse_window_rule_open_max_bounds_variants() {
        for v in ["1600x900", "1600 900", "1600,900"] {
            let input = format!(
                "window-rule {{\n  class-name \"X\"\n  open-max-bounds \"{}\"\n}}",
                v
            );
            let cfg = parse_kdl_config(&input).unwrap_or_else(|e| panic!("parse {}: {}", v, e));
            assert_eq!(
                cfg.window_rules[0].open_max_bounds,
                Some((1600, 900)),
                "open-max-bounds {:?} must parse to (1600, 900)",
                v
            );
        }
        let err =
            parse_kdl_config("window-rule {\n  class-name \"X\"\n  open-max-bounds \"1600\"\n}");
        assert!(err.is_err(), "a single dimension is an error now");
    }
}
