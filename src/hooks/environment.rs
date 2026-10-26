//! Child-only environment overrides. Never modify wiri's process environment:
//! that would race other threads and would change config/include resolution.
//!
//! All launchers take one snapshot per launch, including executable/shell lookup.
//! Windows variable names are case-insensitive; inherited UTF-16 (including lone
//! surrogates and drive-current-directory entries) is preserved without a UTF-8
//! round trip. Configured entries are authoritative over per-call additions.

use anyhow::{bail, Result};
use parking_lot::RwLock;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;

/// `None` removes an inherited variable; `Some("")` sets an empty value.
pub type Environment = BTreeMap<String, Option<String>>;

static CHILD_ENV: RwLock<Environment> = RwLock::new(BTreeMap::new());

// Use Windows' ordinal, locale-independent comparison, not Unicode lowercasing
// (which can expand a character and does not match Windows variable lookup).
#[link(name = "kernel32")]
extern "system" {
    fn CompareStringOrdinal(
        left: *const u16,
        left_len: i32,
        right: *const u16,
        right_len: i32,
        ignore_case: i32,
    ) -> i32;
}

fn compare_names(left: &OsStr, right: &OsStr) -> Ordering {
    let left: Vec<_> = left.encode_wide().collect();
    let right: Vec<_> = right.encode_wide().collect();
    // Environment names cannot approach i32::MAX UTF-16 code units.
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        )
    };
    debug_assert!((1..=3).contains(&result));
    result.cmp(&2)
}

pub fn validate_entry(name: &str, value: Option<&str>) -> Result<()> {
    if name.is_empty() || name.contains(['=', '\0']) {
        bail!("environment variable name must be nonempty and contain neither '=' nor NUL");
    }
    if value.is_some_and(|value| value.contains('\0')) {
        bail!("environment variable {name:?} contains NUL");
    }
    Ok(())
}

pub fn validate(environment: &Environment) -> Result<()> {
    let mut names: Vec<&str> = Vec::new();
    for (name, value) in environment {
        validate_entry(name, value.as_deref())?;
        if names
            .iter()
            .any(|other| compare_names(OsStr::new(other), OsStr::new(name)) == Ordering::Equal)
        {
            bail!("duplicate Windows environment variable {name:?} (names are case-insensitive)");
        }
        names.push(name);
    }
    Ok(())
}

/// Publish only after a successful config load, before startup commands and on
/// successful reloads. Validation failure leaves the previous map untouched.
pub fn configure(environment: &Environment) -> Result<()> {
    validate(environment)?;
    *CHILD_ENV.write() = environment.clone();
    Ok(())
}

/// ShellExecute's UAC broker cannot accept a custom environment. Its launcher
/// must refuse configured overrides rather than silently bypass this contract.
pub(super) fn has_overrides() -> bool {
    !CHILD_ENV.read().is_empty()
}

#[derive(Debug, Clone)]
pub(super) struct ChildEnvironment {
    entries: Vec<(OsString, OsString)>,
}

impl ChildEnvironment {
    pub(super) fn capture() -> Self {
        Self::capture_with(&[]).expect("an empty per-launch environment is valid")
    }

    pub(super) fn capture_with(extra: &[(String, String)]) -> Result<Self> {
        for (name, value) in extra {
            validate_entry(name, Some(value))?;
        }
        let configured = CHILD_ENV.read().clone();
        Ok(Self::from_entries(std::env::vars_os(), extra, &configured))
    }

    pub(super) fn from_entries(
        inherited: impl IntoIterator<Item = (OsString, OsString)>,
        extra: &[(String, String)],
        configured: &Environment,
    ) -> Self {
        let mut result = Self {
            entries: Vec::new(),
        };
        for (name, value) in inherited {
            result.set(name, Some(value));
        }
        for (name, value) in extra {
            result.set(name.into(), Some(value.into()));
        }
        for (name, value) in configured {
            result.set(name.into(), value.as_ref().map(OsString::from));
        }
        result.entries.sort_by(|(a, _), (b, _)| compare_names(a, b));
        result
    }

    fn set(&mut self, name: OsString, value: Option<OsString>) {
        self.entries
            .retain(|(other, _)| compare_names(other, &name) != Ordering::Equal);
        if let Some(value) = value {
            self.entries.push((name, value));
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<&OsStr> {
        self.entries
            .iter()
            .find(|(other, _)| compare_names(other, OsStr::new(name)) == Ordering::Equal)
            .map(|(_, value)| value.as_os_str())
    }

    /// Sorted, case-insensitive UTF-16 `name=value\0...\0\0` block for
    /// CREATE_UNICODE_ENVIRONMENT. Even an empty block has two terminators.
    pub(super) fn to_block(&self) -> Vec<u16> {
        let mut result = Vec::new();
        for (name, value) in &self.entries {
            result.extend(name.encode_wide());
            result.push(b'=' as u16);
            result.extend(value.encode_wide());
            result.push(0);
        }
        if result.is_empty() {
            result.push(0);
        }
        result.push(0);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt;

    #[test]
    fn overrides_remove_and_empty_are_case_insensitive() {
        let inherited = [("Path", "old"), ("REMOVE", "old"), ("kept", "base")]
            .map(|(key, value)| (key.into(), value.into()));
        let configured = BTreeMap::from([
            ("PATH".into(), Some("new".into())),
            ("remove".into(), None),
            ("EMPTY".into(), Some(String::new())),
        ]);
        let snapshot = ChildEnvironment::from_entries(
            inherited,
            &[("path".into(), "per-call".into())],
            &configured,
        );
        assert_eq!(snapshot.get("pAtH"), Some(OsStr::new("new")));
        assert_eq!(snapshot.get("remove"), None);
        assert_eq!(snapshot.get("empty"), Some(OsStr::new("")));
        assert_eq!(snapshot.get("kept"), Some(OsStr::new("base")));
        let block = String::from_utf16(&snapshot.to_block()).unwrap();
        assert_eq!(block, "EMPTY=\0kept=base\0PATH=new\0\0");
    }

    #[test]
    fn preserves_utf16_unicode_and_drive_entries() {
        let raw = OsString::from_wide(&[0xd800]);
        let snapshot = ChildEnvironment::from_entries(
            [
                ("=C:".into(), "C:\\work".into()),
                ("raw".into(), raw.clone()),
            ],
            &[],
            &BTreeMap::from([("日本語".into(), Some("hello 🦀".into()))]),
        );
        assert_eq!(snapshot.get("raw"), Some(raw.as_os_str()));
        assert_eq!(snapshot.get("日本語"), Some(OsStr::new("hello 🦀")));
        assert!(snapshot.to_block().contains(&0xd800));
        assert!(snapshot
            .to_block()
            .starts_with(&"=C:=C:\\work\0".encode_utf16().collect::<Vec<_>>()));
    }

    #[test]
    fn empty_block_is_double_terminated() {
        let snapshot = ChildEnvironment::from_entries([], &[], &Environment::new());
        assert_eq!(snapshot.to_block(), [0, 0]);
    }

    #[test]
    fn validates_entries_and_windows_duplicates() {
        for name in ["", "A=B", "A\0B"] {
            assert!(validate_entry(name, Some("value")).is_err());
        }
        assert!(validate_entry("NAME", Some("a\0b")).is_err());
        assert!(validate_entry("NAME", None).is_ok());
        assert!(validate(&BTreeMap::from([
            ("Path".into(), None),
            ("PATH".into(), Some("new".into()))
        ]))
        .is_err());
    }

    #[test]
    fn snapshots_do_not_mutate_the_parent() {
        let before = std::env::var_os("PATH");
        let configured = BTreeMap::from([("PATH".into(), None)]);
        let snapshot = ChildEnvironment::from_entries(std::env::vars_os(), &[], &configured);
        assert!(snapshot.get("PATH").is_none());
        assert_eq!(std::env::var_os("PATH"), before);
    }
}
