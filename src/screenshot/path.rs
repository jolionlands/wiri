//! niri's strftime screenshot-path, with the redirected Windows Pictures known folder.
use anyhow::{bail, ensure, Context, Result};
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ScreenshotPath {
    #[default]
    Pictures,
    Template(String),
    ClipboardOnly,
}

pub const DEFAULT_FILENAME: &str = "Screenshot from %Y-%m-%d %H-%M-%S.png";

// The UCRT struct tm has nine int fields (not Unix's extra gmtoff/zone fields).
#[repr(C)]
#[derive(Default)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
}
extern "C" {
    fn _localtime64_s(tm: *mut Tm, time: *const i64) -> i32;
    fn wcsftime(dst: *mut u16, len: usize, format: *const u16, tm: *const Tm) -> usize;
}

// Validate before calling UCRT: an unknown conversion invokes its invalid-parameter handler.
// Translate POSIX aliases which the Windows CRT does not consistently implement.
fn local_time(epoch: i64) -> Result<Tm> {
    let mut tm = Tm::default();
    ensure!(
        unsafe { _localtime64_s(&mut tm, &epoch) } == 0,
        "invalid screenshot date"
    );
    Ok(tm)
}

fn leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
fn iso_week(tm: &Tm) -> (i32, i32) {
    let year = tm.year + 1900;
    let weekday = (tm.wday + 6) % 7 + 1;
    let week = (tm.yday + 1 - weekday + 10) / 7;
    let jan1 = (tm.wday - tm.yday).rem_euclid(7);
    let weeks = |y: i32, jan: i32| {
        if jan == 4 || (jan == 3 && leap(y)) {
            53
        } else {
            52
        }
    };
    if week == 0 {
        let prev_jan = (jan1 - 365 - i32::from(leap(year - 1))).rem_euclid(7);
        (year - 1, weeks(year - 1, prev_jan))
    } else if week > weeks(year, jan1) {
        (year + 1, 1)
    } else {
        (year, week)
    }
}

fn crt_format(template: &str, epoch: i64) -> Result<String> {
    let tm = local_time(epoch)?;
    let mut out = String::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut code = chars.next().context("unfinished '%' in screenshot-path")?;
        let unpadded = matches!(code, '-' | '#');
        if unpadded {
            code = chars.next().context("unfinished date conversion")?;
        }
        match code {
            'F' => out.push_str("%Y-%m-%d"),
            'T' => out.push_str("%H:%M:%S"),
            'R' => out.push_str("%H:%M"),
            'D' => out.push_str("%m/%d/%y"),
            'h' => out.push_str("%b"),
            'r' => out.push_str("%I:%M:%S %p"),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            's' => out.push_str(&epoch.to_string()),
            'e' => out.push_str(&format!("{:2}", tm.mday)),
            'C' => out.push_str(&format!("{:02}", (tm.year + 1900) / 100)),
            'u' => out.push_str(&((tm.wday + 6) % 7 + 1).to_string()),
            'G' => out.push_str(&iso_week(&tm).0.to_string()),
            'g' => out.push_str(&format!("{:02}", iso_week(&tm).0 % 100)),
            'V' => out.push_str(&format!("{:02}", iso_week(&tm).1)),
            c if "aAbBcdHIjmMpSUwWxXyYzZ%".contains(c) => {
                out.push('%');
                if unpadded {
                    out.push('#');
                }
                out.push(c);
            }
            _ => bail!("unsupported screenshot-path date conversion '%{}'", code),
        }
    }
    ensure!(!out.contains('\0'), "screenshot-path contains a NUL");
    Ok(out)
}

pub fn format_at(template: &str, epoch: i64) -> Result<String> {
    let format = crt_format(template, epoch)?;
    let wide: Vec<u16> = format.encode_utf16().chain(Some(0)).collect();
    let tm = local_time(epoch)?;
    let mut result = vec![0; 32768];
    let len = unsafe { wcsftime(result.as_mut_ptr(), result.len(), wide.as_ptr(), &tm) };
    ensure!(len > 0, "screenshot-path is empty or too long");
    Ok(String::from_utf16(&result[..len])?)
}

fn pictures() -> Result<PathBuf> {
    unsafe {
        let ptr = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None)?;
        let result = ptr.to_string();
        CoTaskMemFree(Some(ptr.0.cast()));
        Ok(PathBuf::from(result?))
    }
}

pub fn expand_home(template: &str, home: &Path) -> PathBuf {
    if template == "~" {
        return home.to_path_buf();
    }
    if let Some(tail) = template
        .strip_prefix("~/")
        .or_else(|| template.strip_prefix("~\\"))
    {
        home.join(tail)
    } else {
        PathBuf::from(template)
    }
}

pub fn resolve(setting: &ScreenshotPath) -> Result<Option<PathBuf>> {
    let template = match setting {
        ScreenshotPath::ClipboardOnly => return Ok(None),
        ScreenshotPath::Pictures => pictures()?
            .join("Screenshots")
            .join(DEFAULT_FILENAME)
            .to_string_lossy()
            .into_owned(),
        ScreenshotPath::Template(s) => s.clone(),
    };
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let formatted = format_at(&template, epoch)?;
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    Ok(Some(expand_home(&formatted, &home)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_path_is_png_and_has_no_windows_illegal_time_colons() {
        let date = format_at(DEFAULT_FILENAME, 1_730_000_000).unwrap();
        assert!(date.starts_with("Screenshot from 2024-"));
        assert!(date.ends_with(".png"));
        assert!(!date.contains(':'));
    }
    #[test]
    fn percent_and_unicode_are_preserved() {
        assert_eq!(
            format_at("écran %% %Y.png", 1_730_000_000).unwrap(),
            "écran % 2024.png"
        );
    }
    #[test]
    fn posix_aliases_are_translated() {
        assert_eq!(
            crt_format("%F_%R_%T_%h_%s", 42).unwrap(),
            "%Y-%m-%d_%H:%M_%H:%M:%S_%b_42"
        );
    }
    #[test]
    fn invalid_conversions_do_not_reach_crt() {
        for bad in ["%", "%Q", "%f", "%Y\0", "%-"] {
            assert!(format_at(bad, 0).is_err());
        }
    }
    #[test]
    fn only_leading_home_is_expanded() {
        let home = Path::new(r"C:\Users\test");
        assert_eq!(
            expand_home("~/Pictures/a.png", home),
            home.join("Pictures/a.png")
        );
        assert_eq!(expand_home("x~/a.png", home), PathBuf::from("x~/a.png"));
    }
    #[test]
    fn null_never_resolves_a_file() {
        assert_eq!(resolve(&ScreenshotPath::ClipboardOnly).unwrap(), None);
    }
    #[test]
    fn iso_week_year_handles_both_new_year_boundaries() {
        assert_eq!(
            iso_week(&Tm {
                year: 121,
                yday: 0,
                wday: 5,
                ..Tm::default()
            }),
            (2020, 53)
        );
        assert_eq!(
            iso_week(&Tm {
                year: 118,
                yday: 364,
                wday: 1,
                ..Tm::default()
            }),
            (2019, 1)
        );
        assert_eq!(
            iso_week(&Tm {
                year: 120,
                yday: 365,
                wday: 4,
                ..Tm::default()
            }),
            (2020, 53)
        );
    }
}
