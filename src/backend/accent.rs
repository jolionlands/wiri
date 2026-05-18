//! Wallpaper-aware accent colour reader.
//!
//! Reads the user's Windows accent colour from
//! `HKCU\Software\Microsoft\Windows\DWM\AccentColor`, where the value is
//! stored as a 32-bit DWORD packed as `0xAABBGGRR` (ABGR).  Returns the
//! unpacked `[r, g, b, a]` tuple so callers can route it straight into
//! `DWMWA_BORDER_COLOR` (which expects `0x00BBGGRR`).
//!
//! Accent changes are rare (the user has to crack open
//! `Settings → Personalization → Colors`), so we cache the lookup for
//! [`ACCENT_CACHE_TTL`] (30 s by default).  Any error path — missing key,
//! wrong value type, regdb closed, registry permission denied — collapses
//! to `None`, which lets callers fall back to their configured fixed
//! colour.
//!
//! The [`AccentReader`] trait is the seam for unit tests: the real
//! `RegistryAccentReader` hits the live regdb, while tests can plug in a
//! deterministic mock implementation that returns a fixed value or
//! `None`.

use parking_lot::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long an [`AccentReader::read_now`] result is cached before the
/// reader re-queries the registry.  Windows users change their accent
/// colour very rarely, so a 30-second window is a comfortable balance
/// between freshness and registry traffic.
pub const ACCENT_CACHE_TTL: Duration = Duration::from_secs(30);

/// Abstract reader over the AccentColor registry value.
///
/// `read_now` returns the raw `[r, g, b, a]` colour or `None` if the
/// registry lookup failed for any reason.  The default
/// [`RegistryAccentReader`] hits HKCU; mock impls in unit tests return
/// canned values.
pub trait AccentReader: Send + Sync {
    /// Read the current Windows accent colour.  Returns the unpacked
    /// `[r, g, b, a]` channels (8 bits each) on success, or `None` if the
    /// registry value is missing / unreadable.
    fn read_now(&self) -> Option<[u8; 4]>;
}

/// Live registry reader — queries
/// `HKCU\Software\Microsoft\Windows\DWM` for the `AccentColor` value
/// (REG_DWORD, packed as `0xAABBGGRR`).
pub struct RegistryAccentReader;

impl AccentReader for RegistryAccentReader {
    fn read_now(&self) -> Option<[u8; 4]> {
        read_accent_from_registry()
    }
}

/// Cached accent fetch.  Re-reads via `reader.read_now()` when the cache
/// is empty or older than [`ACCENT_CACHE_TTL`]; otherwise returns the
/// previously-cached value.  `None` results are also cached so a missing
/// registry key doesn't get re-queried on every layout pass.
struct AccentCache {
    cached: Mutex<Option<(Instant, Option<[u8; 4]>)>>,
}

impl AccentCache {
    const fn new() -> Self {
        Self { cached: Mutex::new(None) }
    }

    /// Returns the cached accent colour, refreshing through `reader` when
    /// the entry is missing or stale.  Holds the lock only across the
    /// (possibly expensive) registry call when a refresh is needed —
    /// every other call is a fast lock-and-clone.
    fn get(&self, reader: &dyn AccentReader) -> Option<[u8; 4]> {
        let mut guard = self.cached.lock();
        let now = Instant::now();
        let stale = match &*guard {
            Some((t, _)) => now.duration_since(*t) >= ACCENT_CACHE_TTL,
            None => true,
        };
        if stale {
            let fresh = reader.read_now();
            *guard = Some((now, fresh));
        }
        guard.as_ref().and_then(|(_, v)| *v)
    }

    /// Force the cache to expire — used by callers that know an accent
    /// change just happened (e.g. WM_SETTINGCHANGE handler if we ever
    /// wire one).  Currently exposed for tests.
    #[cfg(test)]
    fn invalidate(&self) {
        *self.cached.lock() = None;
    }
}

static CACHE: OnceLock<AccentCache> = OnceLock::new();
fn cache() -> &'static AccentCache {
    CACHE.get_or_init(AccentCache::new)
}

/// Process-wide convenience: return the current Windows accent colour,
/// caching the registry lookup for [`ACCENT_CACHE_TTL`].
///
/// The first call (and any call after the cache expires) reads
/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor`.  Subsequent calls
/// return the cached value.  Returns `None` if the registry value is
/// missing or unreadable.
pub fn current_windows_accent_rgba() -> Option<[u8; 4]> {
    cache().get(&RegistryAccentReader)
}

/// Test-only seam: read through a custom [`AccentReader`] implementation
/// rather than the live registry.  Each call still goes through the
/// process-wide cache, so behaviour matches the production code path.
#[cfg(test)]
pub fn current_windows_accent_rgba_with(reader: &dyn AccentReader) -> Option<[u8; 4]> {
    cache().get(reader)
}

/// Test-only seam: clear the process-wide cache so the next
/// `current_windows_accent_rgba*` call performs a fresh read.
#[cfg(test)]
pub fn invalidate_accent_cache_for_test() {
    cache().invalidate();
}

// ---------------------------------------------------------------------------
// Win32 registry plumbing
// ---------------------------------------------------------------------------

fn read_accent_from_registry() -> Option<[u8; 4]> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    };

    unsafe {
        let key_path: Vec<u16> = OsStr::new(r"Software\Microsoft\Windows\DWM")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut hkey: HKEY = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(key_path.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        );
        if open.is_err() {
            return None;
        }

        let value_name: Vec<u16> = OsStr::new("AccentColor")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut data: u32 = 0;
        let mut data_size: u32 = std::mem::size_of::<u32>() as u32;
        let q = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(value_name.as_ptr()),
            None,
            None,
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut data_size),
        );
        // Best-effort close; we already have the value (or an error) by now.
        let _ = RegCloseKey(hkey);
        if q.is_err() || data_size as usize != std::mem::size_of::<u32>() {
            return None;
        }
        Some(unpack_abgr(data))
    }
}

/// Unpack a Windows AccentColor DWORD (`0xAABBGGRR`) into `[r, g, b, a]`.
///
/// Windows stores the accent as little-endian ABGR — alpha in the high
/// byte, then blue, green, red.  This helper is `pub(crate)` so the
/// engine's border-color codepath can reuse it when an in-process caller
/// produces its own DWORD (e.g. a test wiring a mock reader).
pub fn unpack_abgr(dword: u32) -> [u8; 4] {
    let r = (dword & 0xFF) as u8;
    let g = ((dword >> 8) & 0xFF) as u8;
    let b = ((dword >> 16) & 0xFF) as u8;
    let a = ((dword >> 24) & 0xFF) as u8;
    [r, g, b, a]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Serialises tests that touch the process-wide cache.  Without this,
    // parallel test runs can interleave `invalidate_accent_cache_for_test`
    // calls and observe surprising values.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    struct MockReader {
        value: Option<[u8; 4]>,
        calls: Mutex<u32>,
    }
    impl MockReader {
        fn new(value: Option<[u8; 4]>) -> Self {
            Self { value, calls: Mutex::new(0) }
        }
        fn call_count(&self) -> u32 {
            *self.calls.lock()
        }
    }
    impl AccentReader for MockReader {
        fn read_now(&self) -> Option<[u8; 4]> {
            *self.calls.lock() += 1;
            self.value
        }
    }

    #[test]
    fn test_unpack_abgr_decomposes_dword() {
        // 0xAABBGGRR = 0xFF112233 → r=0x33 g=0x22 b=0x11 a=0xFF
        let c = unpack_abgr(0xFF11_2233);
        assert_eq!(c, [0x33, 0x22, 0x11, 0xFF]);
    }

    #[test]
    fn test_cache_hits_reader_only_once_within_ttl() {
        let _g = TEST_LOCK.lock().unwrap();
        invalidate_accent_cache_for_test();
        let reader = MockReader::new(Some([10, 20, 30, 255]));

        let a = current_windows_accent_rgba_with(&reader);
        let b = current_windows_accent_rgba_with(&reader);
        assert_eq!(a, Some([10, 20, 30, 255]));
        assert_eq!(b, Some([10, 20, 30, 255]));
        assert_eq!(
            reader.call_count(),
            1,
            "second call within TTL must come from cache"
        );
    }

    #[test]
    fn test_cache_returns_none_when_reader_returns_none() {
        let _g = TEST_LOCK.lock().unwrap();
        invalidate_accent_cache_for_test();
        let reader = MockReader::new(None);
        assert_eq!(current_windows_accent_rgba_with(&reader), None);
        // The None result is also cached so we don't thrash the registry
        // when the key is missing.
        assert_eq!(current_windows_accent_rgba_with(&reader), None);
        assert_eq!(reader.call_count(), 1);
    }

    #[test]
    fn test_invalidate_forces_refresh() {
        let _g = TEST_LOCK.lock().unwrap();
        invalidate_accent_cache_for_test();
        let r1 = MockReader::new(Some([1, 2, 3, 255]));
        assert_eq!(current_windows_accent_rgba_with(&r1), Some([1, 2, 3, 255]));
        // New reader returns a different value; without invalidation the
        // cache would shadow it.
        let r2 = MockReader::new(Some([9, 9, 9, 255]));
        assert_eq!(current_windows_accent_rgba_with(&r2), Some([1, 2, 3, 255]));
        invalidate_accent_cache_for_test();
        assert_eq!(current_windows_accent_rgba_with(&r2), Some([9, 9, 9, 255]));
    }
}
