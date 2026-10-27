//! Loaded keyboard layouts and foreground layout requests. Notifications use the
//! existing hotkey window (`keyboard_notify`) and foreground hook: no thread or window.
use serde_json::{json, Value};
use std::sync::LazyLock;

use super::messages::KeyboardLayoutsIpc;
use crate::input::system_actions::LayoutSwitchTarget;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
};

#[derive(Default)]
struct NamesCache {
    layouts: Vec<u32>,
    names: Vec<String>,
}
static NAMES: LazyLock<parking_lot::Mutex<NamesCache>> =
    LazyLock::new(|| parking_lot::Mutex::new(NamesCache::default()));

const MAX_LAYOUTS: usize = 256;
const LIST_ATTEMPTS: usize = 3;
const MAX_REGISTRY_LAYOUTS: u32 = 4096;
const STRING_CHARS: usize = 2048;
const LAYOUTS_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts";

/// Describe loaded layouts in OS enumeration order, using the foreground
/// window's thread (never this IPC thread) to select `current_idx`.
///
/// If enumeration races a switch, the foreground HKL is appended if necessary.
/// With no foreground window, index 0 is the numeric-schema fallback; an empty
/// enumeration is represented by `{"names":[],"current_idx":0}`.
pub fn keyboard_layouts() -> Value {
    json!(snapshot())
}

/// Queue a layout change for the foreground window using snapshot ordering.
/// Indices are zero-based. Success only means the request was posted; the
/// foreground application may still reject it.
pub(crate) fn switch_layout(target: LayoutSwitchTarget) -> Result<(), String> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return Err("cannot switch keyboard layout: no foreground window".to_owned());
    }
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, None) };
    // Zero would query this IPC thread, not the foreground window's thread.
    if thread_id == 0 {
        return Err("cannot switch keyboard layout: no foreground window thread".to_owned());
    }
    let layouts = loaded_layouts();
    let current = unsafe { GetKeyboardLayout(thread_id) };
    if current.0.is_null() {
        return Err("cannot switch keyboard layout: no foreground keyboard layout".to_owned());
    }
    let selected = select_layout(&layouts, current, target)?;
    post_layout_request(hwnd, selected)
}

// Shared Lane-H poster: never activates a layout on the calling thread.
// Success means queued, not accepted by the foreground application.
fn post_layout_request(
    hwnd: windows::Win32::Foundation::HWND,
    selected: HKL,
) -> Result<(), String> {
    unsafe {
        PostMessageW(
            hwnd,
            WM_INPUTLANGCHANGEREQUEST,
            WPARAM(0),
            LPARAM(selected.0 as isize),
        )
    }
    .map_err(|error| format!("failed to post keyboard layout switch request: {error}"))
}

/// Foreground/ownership-guarded configured request via the Lane-H poster.
/// Callers MUST select the dry-run backend before reaching this function.
/// HWND/thread/layout checks cannot be atomic with an asynchronous post.
pub(crate) fn request_configured_layout(
    plan: &ConfiguredLayoutPlan,
    restore: bool,
) -> Result<bool, String> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() || hwnd.0 as isize != plan.window {
        return Ok(false);
    }
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, None) };
    if thread_id == 0 || thread_id != plan.thread_id {
        return Ok(false);
    }
    let current = unsafe { GetKeyboardLayout(thread_id) }.0 as isize;
    let Some(target) =
        configured_request_target(plan, hwnd.0 as isize, thread_id, current, restore)
    else {
        return Ok(false);
    };
    post_layout_request(hwnd, HKL(target as *mut _))?;
    Ok(true)
}

fn configured_request_target(
    plan: &ConfiguredLayoutPlan,
    window: isize,
    thread_id: u32,
    current: isize,
    restore: bool,
) -> Option<isize> {
    if window == 0
        || window != plan.window
        || thread_id == 0
        || thread_id != plan.thread_id
        || current == 0
        || plan.original == 0
        || plan.selected == 0
    {
        return None;
    }
    if restore {
        // The baseline can mean apply is pending. Queue baseline after it too;
        // never overwrite a third-party HKL or another foreground thread.
        (current == plan.selected || current == plan.original).then_some(plan.original)
    } else {
        (current == plan.original).then_some(plan.selected)
    }
}

pub(crate) fn snapshot() -> KeyboardLayoutsIpc {
    let (layouts, current_idx) = order_and_index(
        loaded_layouts().into_iter().map(hkl_bits).collect(),
        foreground_layout(),
    );
    KeyboardLayoutsIpc {
        names: names_for_layouts(&layouts),
        current_idx,
    }
}

fn names_for_layouts(layouts: &[u32]) -> Vec<String> {
    let mut cache = NAMES.lock();
    if cache.layouts != layouts {
        // Registry/resource lookups only when the loaded list changes, never
        // on each foreground/layout event (or animation state notification).
        let registry = RegistryKey::open();
        let special_ids = match registry.as_ref() {
            Some(key) if layouts.iter().any(|hkl| is_special(*hkl)) => special_layout_ids(key),
            _ => Vec::new(),
        };
        cache.names = layouts
            .iter()
            .map(|hkl| layout_name(*hkl, registry.as_ref(), &special_ids))
            .collect();
        cache.layouts = layouts.to_vec();
    }
    cache.names.clone()
}

/// Read-only plan for an explicit configured layout. Handles retain the full
/// OS-provided bits; low 32-bit snapshot identities must not become LPARAMs.
/// No layout is loaded, installed, activated, or requested by this helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfiguredLayoutPlan {
    pub(crate) window: isize,
    pub(crate) thread_id: u32,
    pub(crate) original: isize,
    pub(crate) selected: isize,
    pub(crate) name: String,
}

/// Resolve one already-loaded layout by its exact Lane-H IPC name. The niri
/// `us` alias denotes only the standard US physical layout, not every layout
/// with LANGID 0409. Multiple matching HKLs are an error, never a best guess.
///
/// Dry-run may call this planner: it only reads Windows state.
/// This returns a foreground-thread plan only; it never posts a switch request
/// or changes the caller's thread layout or the installed/system layout list.
pub(crate) fn plan_configured_layout(requested: &str) -> Result<ConfiguredLayoutPlan, String> {
    if requested.trim().is_empty() {
        return Err("configured keyboard layout name is empty".to_owned());
    }
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return Err("cannot configure keyboard layout: no foreground window".to_owned());
    }
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, None) };
    if thread_id == 0 {
        return Err("cannot configure keyboard layout: no foreground window thread".to_owned());
    }
    let current = unsafe { GetKeyboardLayout(thread_id) };
    if current.0.is_null() {
        return Err("cannot configure keyboard layout: no foreground keyboard layout".to_owned());
    }
    let loaded = loaded_layouts();
    let (ordered, _) = order_and_index(
        loaded.iter().copied().map(hkl_bits).collect(),
        Some(hkl_bits(current)),
    );
    let names = names_for_layouts(&ordered);
    // Preserve full handles from the enumeration, including nonstandard high
    // bits, and use the foreground handle only for its appended snapshot entry.
    let handles: Vec<HKL> = ordered
        .iter()
        .map(|bits| {
            loaded
                .iter()
                .copied()
                .find(|hkl| hkl_bits(*hkl) == *bits)
                .unwrap_or(current)
        })
        .collect();
    let selected = select_named_layout(&handles, &names, requested)?;
    Ok(ConfiguredLayoutPlan {
        window: hwnd.0 as isize,
        thread_id,
        original: current.0 as isize,
        selected: selected.0 as isize,
        name: names[handles
            .iter()
            .position(|hkl| *hkl == selected)
            .expect("selected loaded handle")]
        .clone(),
    })
}

fn select_named_layout(layouts: &[HKL], names: &[String], requested: &str) -> Result<HKL, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("configured keyboard layout name is empty".to_owned());
    }
    if layouts.len() != names.len() {
        return Err(
            "keyboard layout names changed during selection; retry config reload".to_owned(),
        );
    }
    let mut matches = Vec::new();
    for (&hkl, name) in layouts.iter().zip(names) {
        let bits = hkl_bits(hkl);
        let matches_name = if requested == "us" {
            // Fxxx variants require a Layout Id lookup and must never be
            // guessed to be US solely because their LANGID happens to be 0409.
            registry_klid(bits, &[]) == Some(0x0000_0409)
        } else {
            requested == name
        };
        if bits != 0 && matches_name && !matches.iter().any(|old| hkl_bits(*old) == bits) {
            matches.push(hkl);
        }
    }
    match matches.as_slice() {
        [selected] => Ok(*selected),
        [] => Err(format!("keyboard layout {requested:?} is not loaded by that name (use keyboard-layouts IPC names or the us alias)")),
        _ => Err(format!("keyboard layout {requested:?} is ambiguous across {} loaded HKLs; no layout selected", matches.len())),
    }
}

fn hkl_bits(hkl: HKL) -> u32 {
    hkl.0 as usize as u32
}

fn loaded_layouts() -> Vec<HKL> {
    let mut layouts = Vec::new();
    // Both the allocation and retries are bounded even if layouts keep changing.
    for _ in 0..LIST_ATTEMPTS {
        let needed = unsafe { GetKeyboardLayoutList(None) };
        if needed <= 0 {
            break;
        }
        let capacity = (needed as usize).min(MAX_LAYOUTS);
        let mut buffer = vec![HKL::default(); capacity];
        let copied = unsafe { GetKeyboardLayoutList(Some(&mut buffer)) };
        if copied <= 0 {
            continue;
        }
        buffer.truncate((copied as usize).min(capacity));
        layouts = buffer;
        let latest_count = unsafe { GetKeyboardLayoutList(None) };
        if latest_count <= copied || capacity == MAX_LAYOUTS {
            break;
        }
    }
    layouts
}

fn foreground_layout() -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, None) };
    // Passing zero would silently ask for the caller's layout instead.
    if thread_id == 0 {
        return None;
    }
    let hkl = hkl_bits(unsafe { GetKeyboardLayout(thread_id) });
    (hkl != 0).then_some(hkl)
}

fn order_and_index(layouts: Vec<u32>, current: Option<u32>) -> (Vec<u32>, usize) {
    let mut ordered = Vec::with_capacity(layouts.len() + 1);
    for hkl in layouts {
        if hkl != 0 && !ordered.contains(&hkl) {
            ordered.push(hkl);
        }
    }
    let index = match current.filter(|hkl| *hkl != 0) {
        Some(current) => match ordered.iter().position(|hkl| *hkl == current) {
            Some(index) => index,
            None => {
                let index = ordered.len();
                ordered.push(current);
                index
            }
        },
        None => 0,
    };
    (ordered, index)
}

fn select_layout(layouts: &[HKL], current: HKL, target: LayoutSwitchTarget) -> Result<HKL, String> {
    let (ordered, current_idx) = order_and_index(
        layouts.iter().copied().map(hkl_bits).collect(),
        Some(hkl_bits(current)),
    );
    if ordered.is_empty() {
        return Err("no keyboard layouts are available".to_owned());
    }
    let index = match target {
        LayoutSwitchTarget::Next => (current_idx + 1) % ordered.len(),
        LayoutSwitchTarget::Prev => (current_idx + ordered.len() - 1) % ordered.len(),
        LayoutSwitchTarget::Index(index) => usize::from(index),
    };
    let selected = *ordered.get(index).ok_or_else(|| {
        format!(
            "keyboard layout index {index} is out of range ({} layouts available)",
            ordered.len()
        )
    })?;
    // Snapshot identities are u32, but LPARAM needs the full OS-provided HKL.
    // In particular, zero-extending an Fxxx/E-prefixed ID on 64-bit Windows
    // loses sign extension. Preserve the actual loaded handle; the only entry
    // order_and_index can append is the actual foreground handle.
    Ok(layouts
        .iter()
        .copied()
        .find(|hkl| hkl_bits(*hkl) == selected)
        .unwrap_or(current))
}

struct RegistryKey(HKEY);

impl RegistryKey {
    fn open() -> Option<Self> {
        let path = wide(LAYOUTS_KEY);
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(path.as_ptr()),
                0,
                KEY_READ,
                &mut key,
            )
        };
        if status.is_ok() {
            Some(Self(key))
        } else {
            None
        }
    }
}

impl Drop for RegistryKey {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_text(buffer: &[u16]) -> Option<String> {
    let end = buffer
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..end]).ok()
}

fn registry_string(key: &RegistryKey, subkey: &str, value: &str) -> Option<String> {
    let subkey = wide(subkey);
    let value = wide(value);
    let mut buffer = [0u16; STRING_CHARS];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            key.0,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    let chars = bytes as usize / 2;
    if status.is_err() || bytes % 2 != 0 || chars > buffer.len() {
        return None;
    }
    wide_text(&buffer[..chars])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutId {
    klid: u32,
    id: u16,
}

fn parse_klid(text: &str) -> Option<u32> {
    if text.len() != 8 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

fn special_layout_ids(key: &RegistryKey) -> Vec<LayoutId> {
    let mut entries = Vec::new();
    for index in 0..MAX_REGISTRY_LAYOUTS {
        let mut buffer = [0u16; 32];
        let mut len = buffer.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
                None,
                PWSTR::null(),
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status == ERROR_MORE_DATA {
            continue;
        }
        if status.is_err() || len as usize > buffer.len() {
            break;
        }
        let Some(name) = wide_text(&buffer[..len as usize]) else {
            continue;
        };
        let Some(klid) = parse_klid(&name) else {
            continue;
        };
        let Some(id) = registry_string(key, &name, "Layout Id")
            .and_then(|id| u16::from_str_radix(id.trim(), 16).ok())
        else {
            continue;
        };
        entries.push(LayoutId { klid, id });
    }
    entries
}

fn is_special(hkl: u32) -> bool {
    hkl & 0xf000_0000 == 0xf000_0000
}

fn registry_klid(hkl: u32, special_ids: &[LayoutId]) -> Option<u32> {
    let high = hkl >> 16;
    if is_special(hkl) {
        // Fxxx HKLs encode a registry "Layout Id", NOT a KLID or LANGID.
        let candidates: Vec<u32> = special_ids
            .iter()
            .filter(|entry| u32::from(entry.id) == high & 0x0fff)
            .map(|entry| entry.klid)
            .collect();
        let same_language: Vec<u32> = candidates
            .iter()
            .copied()
            .filter(|klid| klid & 0xffff == hkl & 0xffff)
            .collect();
        return match same_language.as_slice() {
            [klid] => Some(*klid),
            [] => match candidates.as_slice() {
                [klid] => Some(*klid),
                _ => None,
            },
            _ => None, // Ambiguous: do not mislabel a variant as the base layout.
        };
    }
    if high & 0xf000 == 0xe000 {
        // Legacy IME KLIDs retain their E-prefixed high word.
        Some(hkl)
    } else if high != 0 {
        // The high word identifies the physical layout; the low word can be a
        // different input language (e.g. US keys with UK English).
        Some(high)
    } else {
        (hkl != 0).then_some(hkl)
    }
}

fn fallback_name(hkl: u32) -> String {
    format!("Keyboard layout (HKL {hkl:08X})")
}

fn readable_name(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('@')).then_some(text)
}

fn select_name(
    hkl: u32,
    display: Option<&str>,
    text: Option<&str>,
    resolve: impl FnOnce(&str) -> Option<String>,
) -> String {
    if let Some(display) = display.map(str::trim).filter(|name| !name.is_empty()) {
        if let Some(name) = readable_name(display) {
            return name.to_owned();
        }
        if let Some(resolved) = resolve(display) {
            if let Some(name) = readable_name(&resolved) {
                return name.to_owned();
            }
        }
    }
    text.and_then(readable_name)
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_name(hkl))
}

fn indirect_name(source: &str) -> Option<String> {
    let source = wide(source);
    let mut buffer = [0u16; STRING_CHARS];
    unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut buffer, None) }.ok()?;
    wide_text(&buffer)
}

fn layout_name(hkl: u32, registry: Option<&RegistryKey>, special_ids: &[LayoutId]) -> String {
    let Some((key, klid)) =
        registry.and_then(|key| registry_klid(hkl, special_ids).map(|id| (key, id)))
    else {
        return fallback_name(hkl);
    };
    let subkey = format!("{klid:08X}");
    let display = registry_string(key, &subkey, "Layout Display Name");
    let text = registry_string(key, &subkey, "Layout Text");
    select_name(hkl, display.as_deref(), text.as_deref(), indirect_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::system_actions::LayoutSwitchTarget::{Index, Next, Prev};

    fn test_hkl(bits: u32) -> HKL {
        HKL((bits as i32 as isize) as *mut _)
    }

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn configured_request_guard_preserves_full_handles_and_rejects_lost_ownership() {
        let original = test_hkl(0xf002_0409).0 as isize;
        let selected = test_hkl(0xe001_0411).0 as isize;
        let plan = ConfiguredLayoutPlan {
            window: 10,
            thread_id: 20,
            original,
            selected,
            name: "Synthetic".into(),
        };
        assert_eq!(
            configured_request_target(&plan, 10, 20, original, false),
            Some(selected)
        );
        assert_eq!(
            configured_request_target(&plan, 10, 20, selected, true),
            Some(original)
        );
        assert_eq!(
            configured_request_target(&plan, 10, 20, original, true),
            Some(original)
        );
        for (window, thread, current, restore) in [
            (0, 20, original, false),
            (11, 20, original, false),
            (10, 0, original, false),
            (10, 21, selected, true),
            (10, 20, 0, true),
            (10, 20, 30, true),
            (10, 20, selected, false),
        ] {
            assert_eq!(
                configured_request_target(&plan, window, thread, current, restore),
                None
            );
        }
    }

    #[test]
    fn configured_layout_uses_exact_lane_h_names_including_localized_variants() {
        let layouts = [
            test_hkl(0x0409_0409),
            test_hkl(0xf002_0409),
            test_hkl(0x0419_0419),
        ];
        let names = names(&["US", "United States-Dvorak", "Русская"]);
        for (name, expected) in names.iter().zip(layouts) {
            assert_eq!(select_named_layout(&layouts, &names, name), Ok(expected));
        }
        assert_eq!(
            select_named_layout(&layouts, &names, "  Русская  "),
            Ok(layouts[2])
        );
        for unknown in [
            "United States",
            "Dvorak",
            "united states-dvorak",
            "ru",
            "us,ru",
            "00000409",
        ] {
            assert!(
                select_named_layout(&layouts, &names, unknown).is_err(),
                "{unknown}"
            );
        }
    }

    #[test]
    fn configured_us_alias_is_physical_us_not_a_langid_match_or_variant_guess() {
        let base = test_hkl(0x0409_0409);
        let variant = test_hkl(0xf002_0409);
        let layouts = [variant, base];
        // The alias must work independently of localized Lane-H display text.
        let names = names(&["US-Dvorak", "Localized standard US"]);
        assert_eq!(select_named_layout(&layouts, &names, "us"), Ok(base));
        assert!(select_named_layout(&[variant], &names[..1], "us").is_err());
        assert!(select_named_layout(&[test_hkl(0xe001_0409)], &names[..1], "us").is_err());
        assert_eq!(
            select_named_layout(&layouts, &names, "US-Dvorak"),
            Ok(variant)
        );
        let cross_language = test_hkl(0x0409_0809);
        assert_eq!(
            select_named_layout(&[cross_language], &names[..1], "us"),
            Ok(cross_language)
        );
    }

    #[test]
    fn configured_layout_rejects_ambiguous_names_and_us_aliases() {
        let layouts = [test_hkl(0xf002_0409), test_hkl(0xf001_0409)];
        let same_names = names(&["Same", "Same"]);
        assert!(select_named_layout(&layouts, &same_names, "Same")
            .unwrap_err()
            .contains("ambiguous"));
        let two_us_handles = [test_hkl(0x0409_0409), test_hkl(0x0409_0809)];
        assert!(select_named_layout(
            &two_us_handles,
            &names(&["US", "US with UK language"]),
            "us"
        )
        .unwrap_err()
        .contains("ambiguous"));
    }

    #[test]
    fn configured_layout_deduplicates_identical_hkls_and_ignores_null_handles() {
        let actual = test_hkl(0xf002_0409);
        assert_eq!(
            select_named_layout(&[actual, actual], &names(&["Dvorak", "Dvorak"]), "Dvorak"),
            Ok(actual)
        );
        assert!(select_named_layout(&[HKL::default()], &names(&["Null"]), "Null").is_err());
        assert!(select_named_layout(&[], &[], "us").is_err());
    }

    #[test]
    fn configured_layout_rejects_empty_names_and_mismatched_snapshots() {
        for empty in ["", " ", "\t\n"] {
            assert!(select_named_layout(&[], &[], empty).is_err());
        }
        assert!(select_named_layout(&[test_hkl(0x0409_0409)], &[], "us").is_err());
    }

    #[test]
    fn configured_selection_keeps_actual_native_handle_bits_not_reconstructed_ids() {
        for actual in [
            test_hkl(0xf002_0409),
            HKL(0xf002_0409usize as *mut _),
            test_hkl(0xe001_0411),
        ] {
            let selected =
                select_named_layout(&[actual], &names(&["Exact variant"]), "Exact variant")
                    .unwrap();
            assert_eq!(selected.0 as isize, actual.0 as isize);
        }
        let fallback = fallback_name(0xf003_0409);
        let unknown = test_hkl(0xf003_0409);
        assert_eq!(
            select_named_layout(&[unknown], std::slice::from_ref(&fallback), &fallback),
            Ok(unknown)
        );
    }

    #[test]
    fn switch_next_and_prev_wrap_using_exact_hkls_in_os_order() {
        // Same language, distinct layouts, deliberately not numerically sorted.
        let layouts = [
            test_hkl(0xf002_0409),
            test_hkl(0x0409_0409),
            test_hkl(0xf001_0409),
        ];
        for (current, next, prev) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
            assert_eq!(
                select_layout(&layouts, layouts[current], Next),
                Ok(layouts[next])
            );
            assert_eq!(
                select_layout(&layouts, layouts[current], Prev),
                Ok(layouts[prev])
            );
        }
        for target in [Next, Prev, Index(0)] {
            assert_eq!(
                select_layout(&layouts[..1], layouts[0], target),
                Ok(layouts[0])
            );
        }
    }

    #[test]
    fn switch_indexes_are_zero_based_and_bounded() {
        let layouts = [test_hkl(0x0409_0409), test_hkl(0xf002_0409)];
        for (index, expected) in layouts.iter().copied().enumerate() {
            assert_eq!(
                select_layout(&layouts, layouts[1], Index(index as u8)),
                Ok(expected)
            );
        }
        for index in [2, u8::MAX] {
            assert!(select_layout(&layouts, layouts[0], Index(index)).is_err());
        }
    }

    #[test]
    fn switch_selection_rejects_empty_layouts() {
        for target in [Next, Prev, Index(0), Index(u8::MAX)] {
            assert!(select_layout(&[], HKL::default(), target).is_err());
        }
    }

    #[test]
    fn switch_selection_matches_snapshot_deduplication_and_foreground_append() {
        let base = test_hkl(0x0409_0409);
        let variant = test_hkl(0xf002_0409);
        let current = test_hkl(0xf001_0409);
        let layouts = [HKL::default(), base, base, variant];
        assert_eq!(select_layout(&layouts, current, Index(0)), Ok(base));
        assert_eq!(select_layout(&layouts, current, Index(1)), Ok(variant));
        assert_eq!(select_layout(&layouts, current, Index(2)), Ok(current));
        assert!(select_layout(&layouts, current, Index(3)).is_err());
        assert_eq!(select_layout(&layouts, current, Next), Ok(base));
        assert_eq!(select_layout(&layouts, current, Prev), Ok(variant));
        assert_eq!(select_layout(&[], current, Next), Ok(current));
    }

    #[test]
    fn switch_selection_preserves_sign_extended_hkls_for_lparam() {
        let base = test_hkl(0x0409_0409);
        for bits in [0xf002_0409, 0xe001_0411] {
            let full_hkl = test_hkl(bits);
            let loaded = select_layout(&[base, full_hkl], base, Index(1)).unwrap();
            let appended = select_layout(&[base], full_hkl, Index(1)).unwrap();
            for selected in [loaded, appended] {
                let lparam = LPARAM(selected.0 as isize);
                assert_eq!(lparam.0, bits as i32 as isize);
                assert!(lparam.0 < 0);
            }
        }
    }

    #[test]
    fn switch_selection_uses_actual_loaded_handles_instead_of_reconstructing_them() {
        // Preserve even a zero-extended OS handle rather than assuming its high
        // bits from the snapshot ID or substituting the foreground's handle.
        let actual = HKL(0xf002_0409usize as *mut _);
        let current = test_hkl(0xf002_0409);
        let selected = select_layout(&[actual], current, Index(0)).unwrap();
        assert_eq!(LPARAM(selected.0 as isize).0, actual.0 as isize);
    }

    #[test]
    fn localized_display_name_precedes_layout_text() {
        let name = select_name(0x0409_0409, Some("@input.dll,-1"), Some("US"), |source| {
            assert_eq!(source, "@input.dll,-1");
            Some("  Localized US  ".to_owned())
        });
        assert_eq!(name, "Localized US");
        assert_eq!(
            select_name(1, Some("Literal display"), Some("Text"), |_| panic!(
                "not indirect"
            )),
            "Literal display"
        );
    }

    #[test]
    fn unresolved_or_empty_display_uses_text_then_an_honest_fallback() {
        for resolved in [None, Some(String::new()), Some("@unresolved,-1".to_owned())] {
            assert_eq!(
                select_name(0xf002_0409, Some("@missing,-1"), Some(" Dvorak "), |_| {
                    resolved
                }),
                "Dvorak"
            );
        }
        assert_eq!(select_name(1, Some("  "), Some("Text"), |_| None), "Text");
        assert_eq!(
            select_name(0xf002_0409, None, Some("@not-a-name"), |_| None),
            "Keyboard layout (HKL F0020409)"
        );
    }

    #[test]
    fn selection_uses_exact_hkl_not_just_the_language() {
        let layouts = vec![0x0409_0409, 0xf002_0409, 0xf001_0409];
        assert_eq!(
            order_and_index(layouts.clone(), Some(0xf001_0409)),
            (layouts, 2)
        );
        assert_eq!(order_and_index(vec![1, 2], Some(3)), (vec![1, 2, 3], 2));
        assert_eq!(order_and_index(vec![0, 2, 2, 1], None), (vec![2, 1], 0));
        assert_eq!(order_and_index(vec![], Some(0)), (vec![], 0));
    }

    #[test]
    fn special_layout_ids_resolve_variants_without_guessing_base_layouts() {
        let ids = [
            LayoutId {
                klid: 0x0001_0409,
                id: 2,
            },
            LayoutId {
                klid: 0x0002_0409,
                id: 1,
            },
            LayoutId {
                klid: 0x0001_0405,
                id: 1,
            },
        ];
        assert_eq!(registry_klid(0xf002_0409, &ids), Some(0x0001_0409));
        assert_eq!(registry_klid(0xf001_0409, &ids), Some(0x0002_0409));
        assert_eq!(registry_klid(0xf001_0405, &ids), Some(0x0001_0405));
        assert_eq!(registry_klid(0xf001_0419, &ids), None); // Ambiguous.
        assert_eq!(registry_klid(0xf003_0409, &ids), None); // Unknown, not US.
        assert_eq!(registry_klid(0xf002_0809, &ids), Some(0x0001_0409));
    }

    #[test]
    fn normal_cross_language_and_ime_handles_have_distinct_registry_rules() {
        assert_eq!(registry_klid(0x0409_0409, &[]), Some(0x0409));
        assert_eq!(registry_klid(0x0409_0809, &[]), Some(0x0409));
        assert_eq!(registry_klid(0x0000_0409, &[]), Some(0x0409));
        assert_eq!(registry_klid(0xe001_0411, &[]), Some(0xe001_0411));
        assert_eq!(registry_klid(0, &[]), None);
    }

    #[test]
    fn registry_key_parsing_and_utf16_are_bounded_and_strict() {
        assert_eq!(parse_klid("00010409"), Some(0x0001_0409));
        assert_eq!(parse_klid("e0010411"), Some(0xe001_0411));
        for invalid in ["409", "000004090", "00000Z09", "-0000409", " 0000409"] {
            assert_eq!(parse_klid(invalid), None);
        }
        assert_eq!(wide_text(&[65, 0, 66]), Some("A".to_owned()));
        assert_eq!(wide_text(&[65, 66]), Some("AB".to_owned()));
        assert_eq!(wide_text(&[0xd800]), None);
    }
}
