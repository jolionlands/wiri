//! Windows key names and canonical bind identities.
//!
//! XKB/niri, Win32 `VK_*`, and legacy wiri names are case-insensitive. This is
//! a virtual-key table, not an XKB keysym implementation: shifted punctuation
//! names share the underlying US OEM key, and do not add an implicit Shift.
//! Layout-dependent Unicode/dead keys and Linux-only hardware keys are not
//! guessed. In particular, no distinct Windows level-5 modifier exists.

use super::{ModifierError, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};

/// Internal hook-only modifier, never a `RegisterHotKey` flag. Windows AltGr is
/// the observed Ctrl+right-Alt pair; ordinary Ctrl+left-Alt remains distinct.
pub const MOD_ALTGR: u32 = 0x0010;
const WINDOWS_MODIFIERS: u32 = MOD_ALT | MOD_CTRL | MOD_SHIFT | MOD_WIN;

fn canonical_name(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Parse the Windows-applicable keyboard names. `VK_` prefixes are optional.
/// Mouse buttons and wheel events deliberately use `pointer::parse_trigger`.
pub fn parse_key_name(name: &str) -> Option<u32> {
    if name == " " {
        return Some(0x20);
    }
    let name = name.trim();
    if name.chars().count() == 1 {
        let ch = name.chars().next()?;
        if ch.is_ascii_alphabetic() {
            return Some(ch.to_ascii_uppercase() as u32);
        }
        if ch.is_ascii_digit() {
            return Some(ch as u32);
        }
        // US physical-key aliases, preserving wiri's historic Plus == Equal.
        let vk = match ch {
            '!' => 0x31,
            '@' => 0x32,
            '#' => 0x33,
            '$' => 0x34,
            '%' => 0x35,
            '^' => 0x36,
            '&' => 0x37,
            '*' => 0x38,
            '(' => 0x39,
            ')' => 0x30,
            ';' | ':' => 0xBA,
            '=' | '+' => 0xBB,
            ',' | '<' => 0xBC,
            '-' | '_' => 0xBD,
            '.' | '>' => 0xBE,
            '/' | '?' => 0xBF,
            '`' | '~' => 0xC0,
            '[' | '{' => 0xDB,
            '\\' | '|' => 0xDC,
            ']' | '}' => 0xDD,
            '\'' | '"' => 0xDE,
            _ => return None,
        };
        return Some(vk);
    }

    let canon = canonical_name(name);
    let canon = if let Some(vk_name) = canon.strip_prefix("vk") {
        // These Win32 constants conflict with XKB/legacy names. Bare Menu is
        // the context-menu key; bare Print is the physical PrintScreen key.
        match vk_name {
            "menu" => return Some(0x12),
            "print" => return Some(0x2A),
            _ => vk_name,
        }
    } else {
        &canon
    };
    // Decimal function-key names only: reject F01 and out-of-range F25.
    if let Some(n) = canon.strip_prefix('f') {
        if let Ok(n) = n.parse::<u32>() {
            if (1..=24).contains(&n) && canon == format!("f{n}") {
                return Some(0x6F + n);
            }
        }
    }
    if canon.len() == 1 {
        let c = canon.as_bytes()[0];
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u32);
        }
    }
    KEY_NAMES
        .iter()
        .find_map(|(vk, names)| names.contains(&canon).then_some(*vk))
}

// All documented non-mouse, non-gamepad Windows keyboard VK values with useful
// names, including media/browser/IME/OEM keys. Reserved and unassigned values
// are intentionally absent. Numeric VK passthrough would hide spelling errors.
const KEY_NAMES: &[(u32, &[&str])] = &[
    (0x03, &["cancel", "ctrlbreak"]),
    (0x08, &["back", "backspace"]),
    (0x09, &["tab", "kptab", "isolefttab"]),
    (0x0C, &["clear", "kpbegin", "begin"]),
    (0x0D, &["return", "enter", "kpenter", "numpadenter"]),
    (0x10, &["shift"]),
    (0x11, &["control", "ctrl"]),
    (0x12, &["alt"]),
    (0x13, &["pause", "break"]),
    (0x14, &["capital", "capslock"]),
    (0x15, &["kana", "hangul", "hangeul", "kanalock"]),
    (0x16, &["imeon"]),
    (0x17, &["junja"]),
    (0x18, &["final"]),
    (0x19, &["hanja", "kanji"]),
    (0x1A, &["imeoff"]),
    (0x1B, &["escape", "esc"]),
    (0x1C, &["convert", "henkan", "henkanmode"]),
    (0x1D, &["nonconvert", "muhenkan"]),
    (0x1E, &["accept"]),
    (0x1F, &["modechange"]),
    (0x20, &["space", "kpspace"]),
    (0x21, &["prior", "pageup", "kppageup", "kpprior"]),
    (0x22, &["next", "pagedown", "kppagedown", "kpnext"]),
    (0x23, &["end", "kpend"]),
    (0x24, &["home", "kphome"]),
    (0x25, &["left", "arrowleft", "kpleft"]),
    (0x26, &["up", "arrowup", "kpup"]),
    (0x27, &["right", "arrowright", "kpright"]),
    (0x28, &["down", "arrowdown", "kpdown"]),
    (0x29, &["select"]),
    // XKB Print and wiri PrintScreen refer to VK_SNAPSHOT, not the obsolete VK_PRINT.
    (0x2A, &["vkprint"]),
    (0x2B, &["execute"]),
    (
        0x2C,
        &["snapshot", "print", "printscreen", "sysreq", "sysrq"],
    ),
    (0x2D, &["insert", "ins", "kpinsert"]),
    (0x2E, &["delete", "del", "kpdelete"]),
    (0x2F, &["help"]),
    (0x5B, &["lwin", "superl", "metal", "winl"]),
    (0x5C, &["rwin", "superr", "metar", "winr"]),
    (0x5D, &["apps", "application", "contextmenu", "menu"]),
    (0x5F, &["sleep", "xf86sleep", "xf86standby"]),
    (0x60, &["numpad0", "kp0"]),
    (0x61, &["numpad1", "kp1"]),
    (0x62, &["numpad2", "kp2"]),
    (0x63, &["numpad3", "kp3"]),
    (0x64, &["numpad4", "kp4"]),
    (0x65, &["numpad5", "kp5"]),
    (0x66, &["numpad6", "kp6"]),
    (0x67, &["numpad7", "kp7"]),
    (0x68, &["numpad8", "kp8"]),
    (0x69, &["numpad9", "kp9"]),
    (0x6A, &["multiply", "kpmultiply", "numpadmultiply"]),
    (0x6B, &["add", "kpadd", "numpadadd"]),
    (0x6C, &["separator", "kpseparator", "numpadseparator"]),
    (0x6D, &["subtract", "kpsubtract", "numpadsubtract"]),
    (0x6E, &["decimal", "kpdecimal", "numpaddecimal"]),
    (0x6F, &["divide", "kpdivide", "numpaddivide"]),
    (0x90, &["numlock"]),
    (0x91, &["scroll", "scrolllock"]),
    (0x92, &["oemnecequal", "oemfjjisho", "kpequal"]),
    (0x93, &["oemfjmasshou"]),
    (0x94, &["oemfjtouroku"]),
    (0x95, &["oemfjloya"]),
    (0x96, &["oemfjroya"]),
    (0xA0, &["lshift", "shiftl"]),
    (0xA1, &["rshift", "shiftr"]),
    (0xA2, &["lcontrol", "lctrl", "controll", "ctrll"]),
    (0xA3, &["rcontrol", "rctrl", "controlr", "ctrlr"]),
    (0xA4, &["lmenu", "lalt", "altl"]),
    (0xA5, &["rmenu", "ralt", "altr", "altgr", "isolevel3shift"]),
    (0xA6, &["browserback", "xf86back"]),
    (0xA7, &["browserforward", "xf86forward"]),
    (0xA8, &["browserrefresh", "xf86refresh", "xf86reload"]),
    (0xA9, &["browserstop", "xf86stop"]),
    (0xAA, &["browsersearch", "xf86search"]),
    (0xAB, &["browserfavorites", "xf86favorites"]),
    (0xAC, &["browserhome", "xf86homepage", "xf86www"]),
    (0xAD, &["volumemute", "xf86audiomute"]),
    (0xAE, &["volumedown", "xf86audiolowervolume"]),
    (0xAF, &["volumeup", "xf86audioraisevolume"]),
    (0xB0, &["medianexttrack", "medianext", "xf86audionext"]),
    (0xB1, &["mediaprevtrack", "mediaprevious", "xf86audioprev"]),
    (0xB2, &["mediastop", "xf86audiostop"]),
    (0xB3, &["mediaplaypause", "xf86audioplay", "xf86audiopause"]),
    (0xB4, &["launchmail", "xf86mail"]),
    (
        0xB5,
        &[
            "launchmediaselect",
            "mediaselect",
            "xf86audio",
            "xf86audiomedia",
        ],
    ),
    (
        0xB6,
        &[
            "launchapp1",
            "xf86launch0",
            "xf86mycomputer",
            "xf86explorer",
        ],
    ),
    (0xB7, &["launchapp2", "xf86launch1", "xf86calculator"]),
    (0xBA, &["oem1", "semicolon", "colon"]),
    (0xBB, &["oemplus", "equal", "plus"]),
    (0xBC, &["oemcomma", "comma", "less"]),
    (0xBD, &["oemminus", "minus", "underscore"]),
    (0xBE, &["oemperiod", "period", "dot", "greater"]),
    (0xBF, &["oem2", "slash", "question"]),
    (
        0xC0,
        &["oem3", "grave", "backtick", "quoteleft", "asciitilde"],
    ),
    (0xDB, &["oem4", "bracketleft", "braceleft"]),
    (0xDC, &["oem5", "backslash", "bar"]),
    (0xDD, &["oem6", "bracketright", "braceright"]),
    (
        0xDE,
        &["oem7", "apostrophe", "quote", "quoteright", "quotedbl"],
    ),
    (0xDF, &["oem8"]),
    (0xE1, &["oemax"]),
    (0xE2, &["oem102", "lessgreater"]),
    (0xE3, &["icohelp"]),
    (0xE4, &["ico00"]),
    (0xE5, &["processkey"]),
    (0xE6, &["icoclear"]),
    (0xE7, &["packet"]),
    (0xE9, &["oemreset"]),
    (0xEA, &["oemjump"]),
    (0xEB, &["oempa1"]),
    (0xEC, &["oempa2"]),
    (0xED, &["oempa3"]),
    (0xEE, &["oemwsctrl"]),
    (0xEF, &["oemcusel"]),
    (0xF0, &["oemattn"]),
    (0xF1, &["oemfinish"]),
    (0xF2, &["oemcopy"]),
    (0xF3, &["oemauto"]),
    (0xF4, &["oemenlw"]),
    (0xF5, &["oembacktab"]),
    (0xF6, &["attn"]),
    (0xF7, &["crsel"]),
    (0xF8, &["exsel"]),
    (0xF9, &["ereof"]),
    (0xFA, &["play"]),
    (0xFB, &["zoom", "xf86zoom"]),
    (0xFC, &["noname"]),
    (0xFD, &["pa1"]),
    (0xFE, &["oemclear"]),
    // Shifted US number-row keysyms, like the shifted OEM aliases above.
    (0x30, &["parenright"]),
    (0x31, &["exclam"]),
    (0x32, &["at"]),
    (0x33, &["numbersign"]),
    (0x34, &["dollar"]),
    (0x35, &["percent"]),
    (0x36, &["asciicircum"]),
    (0x37, &["ampersand"]),
    (0x38, &["asterisk"]),
    (0x39, &["parenleft"]),
];

/// Resolve one modifier without ever dropping unsupported level-5 modifiers.
/// Repeated modifiers are harmless, as in niri.
pub fn resolve_modifiers(names: &[String], mod_key: u32) -> Result<u32, ModifierError> {
    let mut flags = 0;
    for raw in names {
        let name = raw.trim();
        flags |= match canonical_name(name).as_str() {
            "mod" => mod_key,
            "ctrl" | "control" => MOD_CTRL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "super" | "win" | "meta" => MOD_WIN,
            "altgr" | "mod5" | "isolevel3shift" => MOD_ALTGR,
            "mod3" | "isolevel5shift" => {
                return Err(ModifierError::Unsupported(name.to_string()));
            }
            _ => return Err(ModifierError::Unknown(name.to_string())),
        };
    }
    // Ctrl and Alt in the AltGr pair cannot be distinguished from extra Ctrl
    // or Alt presses by a generic modifier mask. Canonicalize redundant names.
    Ok(canonical_modifiers(flags))
}

fn canonical_modifiers(flags: u32) -> u32 {
    if flags & MOD_ALTGR != 0 {
        flags & !(MOD_CTRL | MOD_ALT)
    } else {
        flags
    }
}

/// Normalize only an actual Ctrl+RAlt observation, not Ctrl+LAlt or RAlt alone.
/// No layout API is needed: this is Windows' Ctrl+RAlt AltGr approximation.
pub fn normalize_observed_modifiers(flags: u32, right_alt_down: bool) -> u32 {
    if right_alt_down && flags & (MOD_CTRL | MOD_ALT) == MOD_CTRL | MOD_ALT {
        canonical_modifiers(flags | MOD_ALTGR)
    } else {
        flags
    }
}

/// A pure adapter for the keyboard engine's existing pressed-key array.
/// Modifier transitions must update the array before calling this helper.
pub fn modifiers_from_pressed(down: &[bool; 256]) -> u32 {
    let groups: &[(&[usize], u32)] = &[
        (&[0x12, 0xA4, 0xA5], MOD_ALT),
        (&[0x11, 0xA2, 0xA3], MOD_CTRL),
        (&[0x10, 0xA0, 0xA1], MOD_SHIFT),
        (&[0x5B, 0x5C], MOD_WIN),
    ];
    let flags = groups.iter().fold(0, |flags, (keys, bit)| {
        if keys.iter().any(|key| down[*key]) {
            flags | bit
        } else {
            flags
        }
    });
    normalize_observed_modifiers(flags, down[0xA5])
}

/// Hook-only flags must not be passed to Win32 `RegisterHotKey`. A rejected
/// result means use the primary keyboard hook, not a weaker Ctrl+Alt fallback.
pub fn register_hotkey_modifiers(flags: u32) -> Option<u32> {
    (flags & !WINDOWS_MODIFIERS == 0).then_some(flags)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordError {
    MissingKey,
    EmptyModifier,
    Modifier(ModifierError),
    UnknownKey(String),
}
impl std::fmt::Display for ChordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingKey => write!(f, "bind has no key (for + write Plus)"),
            Self::EmptyModifier => write!(f, "bind has an empty modifier between '+' separators"),
            Self::Modifier(error) => error.fmt(f),
            Self::UnknownKey(name) => write!(f, "unknown or unsupported Windows key '{name}'"),
        }
    }
}
impl std::error::Error for ChordError {}

/// Split niri's `modifier+...+key` grammar. A literal '+' must be named `Plus`.
/// Modifier validity and key lookup are intentionally separate from splitting.
pub fn split_chord(chord: &str) -> Result<(Vec<String>, String), ChordError> {
    let mut parts: Vec<_> = chord.split('+').map(str::trim).collect();
    let key = parts.pop().unwrap_or_default();
    if key.is_empty() {
        return Err(ChordError::MissingKey);
    }
    if parts.iter().any(|part| part.is_empty()) {
        return Err(ChordError::EmptyModifier);
    }
    Ok((
        parts.into_iter().map(str::to_string).collect(),
        key.to_string(),
    ))
}

/// Runtime identities, not textual key spellings. Keypad Enter and Return,
/// media aliases, and touchpad/wheel aliases intentionally collide on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChordTrigger {
    Key(u32),
    Pointer(super::pointer::Trigger),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChordIdentity {
    pub modifiers: u32,
    pub trigger: ChordTrigger,
}
impl ChordIdentity {
    /// `mod_key` must be resolved from the final merged input configuration,
    /// even if `input` appears after `binds` in the source. Keep duplicate maps
    /// local to each binds block: an included file may override an earlier key.
    pub fn new(modifiers: &[String], key: &str, mod_key: u32) -> Result<Self, ChordError> {
        if key.trim().is_empty() {
            return Err(ChordError::MissingKey);
        }
        if modifiers.iter().any(|name| name.trim().is_empty()) {
            return Err(ChordError::EmptyModifier);
        }
        let modifiers = resolve_modifiers(modifiers, mod_key).map_err(ChordError::Modifier)?;
        let trigger = if let Some(vk) = parse_key_name(key) {
            ChordTrigger::Key(vk)
        } else if let Some(trigger) = super::pointer::parse_trigger(key.trim()) {
            ChordTrigger::Pointer(trigger)
        } else {
            return Err(ChordError::UnknownKey(key.to_string()));
        };
        Ok(Self { modifiers, trigger })
    }

    pub fn parse(chord: &str, mod_key: u32) -> Result<Self, ChordError> {
        let (modifiers, key) = split_chord(chord)?;
        Self::new(&modifiers, &key, mod_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn every_table_alias_is_case_insensitive_and_unambiguous() {
        let mut seen = HashMap::new();
        for (vk, aliases) in KEY_NAMES {
            for alias in *aliases {
                assert_eq!(seen.insert(*alias, *vk), None, "duplicate alias {alias}");
                assert_eq!(parse_key_name(alias), Some(*vk), "{alias}");
                assert_eq!(parse_key_name(&alias.to_ascii_uppercase()), Some(*vk));
            }
        }
    }

    #[test]
    fn letters_digits_and_function_keys() {
        for c in b'a'..=b'z' {
            assert_eq!(
                parse_key_name(&(c as char).to_string()),
                Some((c - 32) as u32)
            );
        }
        for c in b'0'..=b'9' {
            assert_eq!(parse_key_name(&(c as char).to_string()), Some(c as u32));
        }
        for f in 1..=24 {
            assert_eq!(parse_key_name(&format!("f{f}")), Some(0x6F + f));
            assert_eq!(parse_key_name(&format!("VK_F{f}")), Some(0x6F + f));
        }
        assert_eq!(parse_key_name("F-1"), Some(0x70));
        for bad in [
            "F0",
            "F25",
            "F01",
            "Retrun",
            "0xAD",
            "é",
            "λ",
            "🦀",
            "",
            "dead_acute",
        ] {
            assert_eq!(parse_key_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn common_xkb_win32_and_legacy_names() {
        for (key, vk) in [
            ("Page_Up", 0x21),
            ("pAgE-dOwN", 0x22),
            ("arrow-left", 0x25),
            ("KP_Separator", 0x6C),
            ("KP_Home", 0x24),
            ("KP_Enter", 0x0D),
            ("Bracket_Left", 0xDB),
            ("VK_OEM_102", 0xE2),
            ("Control_R", 0xA3),
            ("ISO_Level3_Shift", 0xA5),
            ("XF86AudioMute", 0xAD),
            ("xf86audioraisevolume", 0xAF),
            ("XF86AudioLowerVolume", 0xAE),
            ("VK_MEDIA_PLAY_PAUSE", 0xB3),
            ("XF86AudioNext", 0xB0),
            ("XF86AudioPrev", 0xB1),
            ("XF86HomePage", 0xAC),
            ("VK_BROWSER_BACK", 0xA6),
            ("XF86Sleep", 0x5F),
            ("VK_LAUNCH_MAIL", 0xB4),
            ("XF86Calculator", 0xB7),
            ("Menu", 0x5D),
            ("VK_MENU", 0x12),
            ("Print", 0x2C),
            ("VK_PRINT", 0x2A),
        ] {
            assert_eq!(parse_key_name(key), Some(vk), "{key}");
        }
        for key in [
            "XF86MonBrightnessUp",
            "XF86AudioMicMute",
            "ISO_Level5_Shift",
            "MouseLeft",
        ] {
            assert_eq!(parse_key_name(key), None, "must not guess a VK for {key}");
        }
    }

    #[test]
    fn punctuation_is_a_physical_us_key_without_implicit_shift() {
        for (symbol, name, vk) in [
            ("+", "Plus", 0xBB),
            ("-", "Minus", 0xBD),
            ("_", "underscore", 0xBD),
            (";", "semicolon", 0xBA),
            ("[", "bracketleft", 0xDB),
            ("!", "exclam", 0x31),
            ("|", "bar", 0xDC),
            ("'", "apostrophe", 0xDE),
            ("\"", "quotedbl", 0xDE),
            (" ", "Space", 0x20),
        ] {
            assert_eq!(parse_key_name(symbol), Some(vk), "{symbol:?}");
            assert_eq!(parse_key_name(name), Some(vk), "{name}");
        }
        assert_eq!(parse_key_name("Plus"), parse_key_name("Equal"));
    }

    #[test]
    fn chord_syntax_refuses_missing_keys_and_empty_modifiers() {
        for chord in ["", " ", "+", "Ctrl+", "Ctrl++", "Ctrl+Alt+"] {
            assert_eq!(split_chord(chord), Err(ChordError::MissingKey), "{chord}");
        }
        for chord in ["+A", "Ctrl++A", "Ctrl+ +A"] {
            assert_eq!(
                split_chord(chord),
                Err(ChordError::EmptyModifier),
                "{chord}"
            );
        }
        assert_eq!(
            split_chord(" cTrL + Alt + Plus "),
            Ok((names(&["cTrL", "Alt"]), "Plus".into()))
        );
        assert!(ChordIdentity::parse("Ctrl+Plus", 0).is_ok());
        assert!(ChordIdentity::parse("Hyper+A", 0).is_err());
        assert!(ChordIdentity::parse("Ctrl+Retrun", 0).is_err());
    }

    #[test]
    fn altgr_aliases_resolve_but_level5_is_an_explicit_error() {
        for name in ["AltGr", "Mod5", "ISO_Level3_Shift", "iso-level3-shift"] {
            assert_eq!(resolve_modifiers(&names(&[name]), MOD_WIN), Ok(MOD_ALTGR));
        }
        for name in ["Mod3", "ISO_Level5_Shift", "iso-level5-shift"] {
            let error = resolve_modifiers(&names(&[name]), MOD_WIN).unwrap_err();
            assert!(matches!(error, ModifierError::Unsupported(_)));
            assert!(error.to_string().contains("Windows"));
        }
        assert_eq!(
            resolve_modifiers(&names(&["Ctrl", "AltGr", "Shift"]), MOD_WIN),
            Ok(MOD_ALTGR | MOD_SHIFT)
        );
        assert_eq!(
            resolve_modifiers(&names(&["Mod", "Win"]), MOD_ALTGR),
            Ok(MOD_ALTGR | MOD_WIN)
        );
    }

    #[test]
    fn observed_ctrl_right_alt_is_not_ordinary_ctrl_alt() {
        let mut down = [false; 256];
        down[0xA2] = true;
        down[0xA4] = true;
        assert_eq!(modifiers_from_pressed(&down), MOD_CTRL | MOD_ALT);
        down[0xA4] = false;
        down[0xA5] = true;
        assert_eq!(modifiers_from_pressed(&down), MOD_ALTGR);
        down[0xA0] = true;
        down[0x5B] = true;
        assert_eq!(
            modifiers_from_pressed(&down),
            MOD_ALTGR | MOD_SHIFT | MOD_WIN
        );
        down[0xA2] = false;
        assert_eq!(modifiers_from_pressed(&down), MOD_ALT | MOD_SHIFT | MOD_WIN);
        down[0xA5] = false;
        assert_eq!(modifiers_from_pressed(&down), MOD_SHIFT | MOD_WIN);
        assert_eq!(
            normalize_observed_modifiers(MOD_CTRL | MOD_ALT, false),
            MOD_CTRL | MOD_ALT
        );
        assert_eq!(normalize_observed_modifiers(MOD_ALT, true), MOD_ALT);
    }

    #[test]
    fn generic_and_right_control_work_and_releases_clear_the_pair() {
        for control in [0x11, 0xA2, 0xA3] {
            let mut down = [false; 256];
            down[control] = true;
            down[0xA5] = true;
            assert_eq!(modifiers_from_pressed(&down), MOD_ALTGR);
            down[0xA5] = false;
            assert_eq!(modifiers_from_pressed(&down), MOD_CTRL);
            down[control] = false;
            assert_eq!(modifiers_from_pressed(&down), 0);
        }
    }

    #[test]
    fn register_hotkey_never_receives_hook_only_altgr() {
        assert_eq!(
            register_hotkey_modifiers(MOD_CTRL | MOD_ALT),
            Some(MOD_CTRL | MOD_ALT)
        );
        assert_eq!(register_hotkey_modifiers(MOD_ALTGR), None);
        assert_eq!(register_hotkey_modifiers(MOD_ALTGR | MOD_SHIFT), None);
        assert_eq!(register_hotkey_modifiers(0x100), None);
    }

    #[test]
    fn duplicate_identities_resolve_mod_and_aliases_not_modifier_order() {
        let expected = ChordIdentity::parse("Mod+Return", MOD_WIN).unwrap();
        for alias in [
            "Win+Enter",
            "Super+KP_Enter",
            "meta+return",
            "Win+Win+ENTER",
        ] {
            assert_eq!(
                ChordIdentity::parse(alias, MOD_WIN).unwrap(),
                expected,
                "{alias}"
            );
        }
        assert_eq!(
            ChordIdentity::parse("Control+Alt+Page_Up", 0),
            ChordIdentity::parse("alt+ctrl+Prior", 0)
        );
        assert_ne!(ChordIdentity::parse("Mod+Return", MOD_ALT), Ok(expected));
        assert_eq!(
            ChordIdentity::parse("Mod5+X", 0),
            ChordIdentity::parse("ISO_Level3_Shift+x", 0)
        );
        assert_ne!(
            ChordIdentity::parse("Ctrl+Alt+X", 0),
            ChordIdentity::parse("AltGr+X", 0)
        );
        assert_eq!(
            ChordIdentity::parse("XF86AudioPlay", 0),
            ChordIdentity::parse("VK_MEDIA_PLAY_PAUSE", 0)
        );
    }

    #[test]
    fn pointer_identity_uses_the_runtime_aliases_and_direction() {
        assert_eq!(
            ChordIdentity::parse("Mod+TouchpadScrollUp", MOD_WIN),
            ChordIdentity::parse("Super+WheelScrollUp", MOD_WIN)
        );
        assert_ne!(
            ChordIdentity::parse("Mod+MouseLeft", MOD_WIN),
            ChordIdentity::parse("Mod+MouseRight", MOD_WIN)
        );
        assert_ne!(
            ChordIdentity::parse("Mod+WheelScrollUp", MOD_WIN),
            ChordIdentity::parse("Mod+WheelScrollDown", MOD_WIN)
        );
    }

    #[test]
    fn duplicate_tracking_is_local_to_a_binds_block_not_includes() {
        let mut first_block = HashMap::new();
        let a = ChordIdentity::parse("Mod+Return", MOD_WIN).unwrap();
        let b = ChordIdentity::parse("Super+Enter", MOD_WIN).unwrap();
        assert_eq!(first_block.insert(a, 1), None);
        assert_eq!(first_block.insert(b, 2), Some(1));
        let mut included_block = HashMap::new();
        assert_eq!(included_block.insert(b, 3), None);
    }
}
