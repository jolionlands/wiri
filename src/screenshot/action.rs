//! Validated niri screen/window action options, shared by config, CLI and IPC.
use anyhow::Result;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CaptureAction {
    pub window: bool,
    pub id: Option<isize>,
    pub write_to_disk: bool,
    pub show_pointer: bool,
    pub path: Option<String>,
}
impl CaptureAction {
    pub fn defaults(window: bool) -> Self {
        Self {
            window,
            id: None,
            write_to_disk: true,
            show_pointer: !window,
            path: None,
        }
    }
    pub fn parse(window: bool, words: &[String]) -> Option<Self> {
        let mut out = Self::defaults(window);
        let mut pointer = false;
        let mut disk = false;
        let mut i = 0;
        while i < words.len() {
            let word = &words[i];
            let (key, inline) = word
                .split_once('=')
                .map_or((word.as_str(), None), |(k, v)| (k, Some(v)));
            let mut value = || {
                if let Some(value) = inline {
                    Some(value)
                } else {
                    i += 1;
                    words.get(i).map(String::as_str)
                }
            };
            match key {
                "--show-pointer" | "show-pointer" | "-p" if !pointer => {
                    pointer = true;
                    out.show_pointer = match value()? {
                        "true" => true,
                        "false" => false,
                        _ => return None,
                    };
                }
                "--write-to-disk" | "write-to-disk" | "-d" if !disk => {
                    disk = true;
                    out.write_to_disk = match value()? {
                        "true" => true,
                        "false" => false,
                        _ => return None,
                    };
                }
                "--path" | "path" if out.path.is_none() => out.path = Some(value()?.to_owned()),
                "--id" if window && out.id.is_none() => {
                    out.id = Some(value()?.parse::<isize>().ok().filter(|n| *n > 0)?)
                }
                _ if !word.starts_with('-') && out.path.is_none() => out.path = Some(word.clone()),
                _ => return None,
            }
            i += 1;
        }
        if out
            .path
            .as_ref()
            .is_some_and(|path| !Path::new(path).is_absolute() || path.contains('\0'))
        {
            return None;
        }
        Some(out)
    }
    pub fn run(&self) -> Result<Option<String>> {
        let window = if self.window {
            Some(self.id.unwrap_or_else(|| unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize
            }))
        } else {
            None
        };
        super::capture_with_options(
            window,
            self.path.as_deref().map(Path::new),
            self.write_to_disk,
            self.show_pointer,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn capture_defaults_match_niri() {
        assert!(CaptureAction::defaults(false).show_pointer);
        assert!(!CaptureAction::defaults(true).show_pointer);
        assert!(CaptureAction::parse(false, &[]).unwrap().write_to_disk);
    }
    #[test]
    fn every_cli_and_bind_option_reaches_the_capture() {
        let got = CaptureAction::parse(
            true,
            &words(&[
                "--id=42",
                "--show-pointer",
                "true",
                "write-to-disk=false",
                "--path",
                r"C:\shots\x.png",
            ]),
        )
        .unwrap();
        assert_eq!(
            got,
            CaptureAction {
                window: true,
                id: Some(42),
                write_to_disk: false,
                show_pointer: true,
                path: Some(r"C:\shots\x.png".into())
            }
        );
    }
    #[test]
    fn absolute_positional_destination_can_contain_equals() {
        let action = CaptureAction::parse(false, &words(&[r"C:\shots\a=b.png"])).unwrap();
        assert_eq!(action.path.as_deref(), Some(r"C:\shots\a=b.png"));
    }
    #[test]
    fn invalid_or_duplicate_options_are_not_silent_noops() {
        for bad in [
            &["--id", "0"][..],
            &["--id", "-1"],
            &["--show-pointer", "yes"],
            &["--write-to-disk"],
            &["relative.png"],
            &["--path", r"C:\a.png", "--path", r"C:\b.png"],
            &["--show-pointer=true", "--show-pointer=false"],
        ] {
            assert!(CaptureAction::parse(true, &words(bad)).is_none(), "{bad:?}");
        }
        assert!(CaptureAction::parse(false, &words(&["--id", "42"])).is_none());
    }
    #[test]
    fn niri_bind_properties_act_instead_of_being_stored_only() {
        let config = crate::config::parse_kdl_config(
            r#"binds { Mod+P { screenshot-screen write-to-disk=false show-pointer=false; } }"#,
        )
        .unwrap();
        let action = config.binds.hotkeys[0].parse_action().unwrap();
        assert!(
            matches!(action,crate::input::Action::Capture(options) if !options.write_to_disk && !options.show_pointer)
        );
    }
    #[test]
    fn shared_action_parser_preserves_defaults_and_options() {
        assert_eq!(
            crate::input::parse_action_name("screenshot-screen", &[]),
            Some(crate::input::Action::ScreenshotScreen)
        );
        let action =
            crate::input::parse_action_name("screenshot-window", &words(&["-d", "false"])).unwrap();
        assert!(matches!(action,crate::input::Action::Capture(options) if !options.write_to_disk));
    }
}
