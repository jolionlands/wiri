//! Opt-in, event-driven clipping of a foreign HWND to four circular CSS corners.
//!
//! Integration: add `mod native_clip;` and a default-initialized
//! `native_clips: native_clip::ClipState` to the engine. Call `sync` after placement,
//! on observed resize/DPI/rule changes, and for actual resize-animation frames (not
//! just their final targets). Pass **physical visible-frame bounds**, not the tile,
//! client rectangle, or `GetWindowRect` including its invisible sizing borders.
//! `place_window_frame` can be asynchronous: use the observed visible rectangle;
//! a pending target of a different size is rejected, not installed optimistically.
//! Use the layout/backend's per-monitor-DPI-aware thread context for these reads.
//! Call `remove` before unmanaging and `restore_all` before shutdown/style teardown.
//! Keep the existing DWM-corner preference separate: do not reduce these four
//! radii through `dwm_corner_for`. Use no DWM rounding while a native clip is
//! active, then resume the existing configured preference after its removal.
//!
//! This is a binary, pixel-centre-sampled HWND region, not an antialiased compositor
//! mask. Regions can suppress DWM's own rounding/shadow behavior. The mask clips
//! non-client content too, not separate owned popups. A foreign app
//! can replace its region afterwards; resizing or a changed clip reapplies it,
//! but there is deliberately no polling/region-allocation loop. Permission failures,
//! unsupported geometry and failed restoration return false and warn. RTL window
//! layouts are conservatively rejected: GetWindowRgn and SetWindowRgn document
//! different origins for them, so guessing would corrupt asymmetric/original regions.
//!
//! Original regions are intersected with our mask and saved *once*, including the
//! distinction between no region and an explicitly empty region. They are stored as
//! RGNDATA, not handles: ClipState is Send + Sync without unsafe implementations.
//! All temporary GDI handles are local RAII owners, never selected into a DC. Only
//! a successful SetWindowRgn transfers ownership to Windows; restoration recreates
//! a fresh handle and never consumes the saved snapshot. GetWindowRgn reports
//! ERROR for both absence and failure; a cleared, still-zero last-error on a
//! same-owner live HWND is treated as documented absence. Windows offers no
//! atomic snapshot/set or HWND-generation token: destroy/unmanage hooks remain
//! necessary even with PID/TID checks (same-thread HWND reuse is indistinguishable).

use std::collections::HashMap;
use std::mem::size_of;

use anyhow::{anyhow, bail, ensure, Result};
use tracing::warn;
use windows::Win32::Foundation::{GetLastError, SetLastError, BOOL, HWND, RECT, WIN32_ERROR};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, DeleteObject, ExtCreateRegion, GetRegionData, GetWindowRgn,
    SetWindowRgn, HGDIOBJ, HRGN, RGNDATA, RGN_AND,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindow, GWL_EXSTYLE,
    WS_EX_LAYOUTRTL,
};

use crate::utils::{Rect, WindowId};

// GDI region coordinates are signed 27-bit integers. Also bound work/allocation for
// hostile or nonsensical window dimensions without imposing a normal monitor limit.
const GDI_MIN: i64 = -(1 << 26);
const GDI_MAX: i64 = (1 << 26) - 1;
const MAX_SCAN_ROWS: u32 = 131_072;
const MAX_REGION_BYTES: usize = 16 * 1024 * 1024;
const HEADER_WORDS: usize = 8; // RGNDATAHEADER: four DWORDs followed by a RECT.

#[derive(Default)]
pub(super) struct ClipState {
    book: ClipBook<Win32Regions>,
}

impl ClipState {
    /// `corners` are TL/TR/BR/BL in physical pixels, already DPI-scaled by the
    /// caller. None removes the request; Some([0.; 4]) is a rectangular clip.
    /// True means applied/already applied, or successfully restored/no request.
    /// False is never cached as a successful application or restoration.
    pub(super) fn sync(
        &mut self,
        window: WindowId,
        visible: Rect,
        corners: Option<[f32; 4]>,
    ) -> bool {
        self.book.sync(window, visible, corners)
    }

    /// Restore the pre-request region. Keep its snapshot on failure for retry.
    /// A destroyed HWND needs no restoration; a reused HWND is never modified.
    pub(super) fn remove(&mut self, window: WindowId) -> bool {
        self.book.remove(window)
    }

    /// Attempt every restoration even if one fails. Explicitly call before the
    /// engine restores native styles/positions; Drop is only a best-effort backup.
    pub(super) fn restore_all(&mut self) -> bool {
        self.book.restore_all()
    }

    /// App-side native resize processing may replace the region. Retain the original.
    pub(super) fn invalidate(&mut self, window: WindowId) {
        if let Some(entry) = self.book.entries.get_mut(&window) {
            entry.applied = None;
        }
    }
}

impl Drop for ClipState {
    fn drop(&mut self) {
        self.restore_all();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowOwner {
    process: u32,
    thread: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Bounds {
    fn checked(left: i64, top: i64, right: i64, bottom: i64) -> Result<Self> {
        ensure!(left < right && top < bottom, "empty clip bounds");
        ensure!(
            [left, top, right, bottom]
                .iter()
                .all(|v| (GDI_MIN..=GDI_MAX).contains(v)),
            "clip bounds exceed GDI's signed 27-bit coordinate range"
        );
        Ok(Self {
            left: left as i32,
            top: top as i32,
            right: right as i32,
            bottom: bottom as i32,
        })
    }

    fn width(self) -> u32 {
        (self.right - self.left) as u32
    }

    fn height(self) -> u32 {
        (self.bottom - self.top) as u32
    }
}

/// Position-independent cache key: visible-frame insets, size, and fitted radii.
/// Absolute screen position is deliberately absent, so moving/scrolling a window
/// of unchanged size does not create any GDI objects or call SetWindowRgn again.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ClipKey {
    bounds: Bounds,
    corners: [f64; 4],
}

impl ClipKey {
    fn new(bounds: Bounds, corners: [f32; 4]) -> Result<Self> {
        Ok(Self {
            bounds,
            corners: fit_corners(bounds.width(), bounds.height(), corners)?,
        })
    }
}

/// CSS overlap reduction uses ONE common factor for all four radii. Clamping
/// each radius to half the short edge would incorrectly shrink lone corners.
fn fit_corners(width: u32, height: u32, corners: [f32; 4]) -> Result<[f64; 4]> {
    ensure!(width > 0 && height > 0, "empty visible frame");
    ensure!(
        corners.iter().all(|r| r.is_finite() && *r >= 0.0),
        "corner radii must be finite and nonnegative"
    );
    let radii = corners.map(f64::from);
    let [tl, tr, br, bl] = radii;
    let mut factor = 1.0_f64;
    for (edge, sum) in [
        (width as f64, tl + tr),
        (height as f64, tr + br),
        (width as f64, br + bl),
        (height as f64, bl + tl),
    ] {
        if sum > 0.0 {
            factor = factor.min(edge / sum);
        }
    }
    Ok(radii.map(|r| r * factor))
}

/// Compute the relative region origin from *live* native/DWM rectangles. The
/// supplied visible position is not subtracted from a potentially older HWND
/// position; that would turn asynchronous movement into an enormous inset.
fn local_bounds(visible: Rect, native: RECT, frame: RECT) -> Result<Bounds> {
    let width = i64::from(frame.right) - i64::from(frame.left);
    let height = i64::from(frame.bottom) - i64::from(frame.top);
    ensure!(
        native.right > native.left && native.bottom > native.top,
        "empty native window rectangle"
    );
    ensure!(width > 0 && height > 0, "empty DWM visible frame");
    ensure!(
        width == i64::from(visible.size.w) && height == i64::from(visible.size.h),
        "visible frame size changed or asynchronous placement is still pending"
    );
    Bounds::checked(
        i64::from(frame.left) - i64::from(native.left),
        i64::from(frame.top) - i64::from(native.top),
        i64::from(frame.right) - i64::from(native.left),
        i64::from(frame.bottom) - i64::from(native.top),
    )
}

/// The horizontal inset of a circular corner at a pixel-centre's distance from
/// the adjacent horizontal edge. The rationalized expression avoids cancellation
/// for tiny insets / very large radii.
fn circle_inset(radius: f64, edge_distance: f64) -> f64 {
    if radius == 0.0 || edge_distance >= radius {
        return 0.0;
    }
    let dy = radius - edge_distance;
    let dx = (edge_distance * (2.0 * radius - edge_distance))
        .max(0.0)
        .sqrt();
    dy * dy / (radius + dx)
}

/// Pure rasterization of the continuous four-corner shape at pixel centres.
/// Coalesce identical adjacent scanlines into rectangles. ExtCreateRegion can
/// install the entire result in one GDI call, rather than O(height) CombineRgns.
fn scan_rectangles(key: ClipKey) -> Result<Vec<Bounds>> {
    let bounds = key.bounds;
    let width = bounds.width();
    let height = bounds.height();
    let [tl, tr, br, bl] = key.corners;
    if key.corners.iter().all(|r| *r == 0.0) {
        return Ok(vec![bounds]);
    }
    ensure!(height <= MAX_SCAN_ROWS, "clip exceeds scanline work limit");
    let mut rectangles: Vec<Bounds> = Vec::new();
    for row in 0..height {
        let y = f64::from(row) + 0.5;
        let from_bottom = f64::from(height) - y;
        let left = circle_inset(tl, y).max(circle_inset(bl, from_bottom));
        let right = circle_inset(tr, y).max(circle_inset(br, from_bottom));
        // Pixel x belongs iff its centre x + .5 is inside both circle edges.
        let first = (left - 0.5).ceil().max(0.0).min(f64::from(width)) as i32;
        let end = width as i32 - (right - 0.5).ceil().max(0.0).min(f64::from(width)) as i32;
        if first >= end {
            continue;
        }
        let span = Bounds {
            left: bounds.left + first,
            top: bounds.top + row as i32,
            right: bounds.left + end,
            bottom: bounds.top + row as i32 + 1,
        };
        if let Some(previous) = rectangles.last_mut() {
            if previous.left == span.left
                && previous.right == span.right
                && previous.bottom == span.top
            {
                previous.bottom = span.bottom;
                continue;
            }
        }
        rectangles.push(span);
    }
    Ok(rectangles)
}

// A small injectable boundary keeps all lifecycle tests desktop-free.
trait RegionApi {
    type Snapshot;
    fn owner(&mut self, window: WindowId) -> Result<Option<WindowOwner>>;
    fn measure(&mut self, window: WindowId, visible: Rect, corners: [f32; 4]) -> Result<ClipKey>;
    fn snapshot(&mut self, window: WindowId, owner: WindowOwner) -> Result<Self::Snapshot>;
    fn apply(
        &mut self,
        window: WindowId,
        owner: WindowOwner,
        key: ClipKey,
        original: &Self::Snapshot,
    ) -> Result<()>;
    fn restore(
        &mut self,
        window: WindowId,
        owner: WindowOwner,
        original: &Self::Snapshot,
    ) -> Result<()>;
}

struct ClipEntry<S> {
    owner: WindowOwner,
    original: S,
    applied: Option<ClipKey>,
}

struct ClipBook<A: RegionApi> {
    api: A,
    entries: HashMap<WindowId, ClipEntry<A::Snapshot>>,
}

impl<A: RegionApi + Default> Default for ClipBook<A> {
    fn default() -> Self {
        Self {
            api: A::default(),
            entries: HashMap::new(),
        }
    }
}

impl<A: RegionApi> ClipBook<A> {
    fn sync(&mut self, window: WindowId, visible: Rect, corners: Option<[f32; 4]>) -> bool {
        let Some(corners) = corners else {
            return self.remove(window);
        };
        let result = (|| -> Result<()> {
            let Some(owner) = self.api.owner(window)? else {
                self.entries.remove(&window);
                bail!("window no longer exists");
            };
            if self
                .entries
                .get(&window)
                .is_some_and(|entry| entry.owner != owner)
            {
                // Never restore a snapshot onto a different process/thread's HWND.
                self.entries.remove(&window);
                warn!(%window, "native clip discarded a stale HWND snapshot");
            }
            let key = self.api.measure(window, visible, corners)?;
            if let Some(entry) = self.entries.get_mut(&window) {
                if entry.applied != Some(key) {
                    self.api.apply(window, owner, key, &entry.original)?;
                    entry.applied = Some(key);
                }
            } else {
                let original = self.api.snapshot(window, owner)?;
                self.api.apply(window, owner, key, &original)?;
                // In particular, a failed first SetWindowRgn must not leave a
                // stale snapshot or claim we are managing a region we never set.
                self.entries.insert(
                    window,
                    ClipEntry {
                        owner,
                        original,
                        applied: Some(key),
                    },
                );
            }
            Ok(())
        })();
        if let Err(error) = result {
            warn!(%window, %error, "native clip not applied");
            return false;
        }
        true
    }

    fn remove(&mut self, window: WindowId) -> bool {
        let Some(entry) = self.entries.get(&window) else {
            return true;
        };
        let result = (|| -> Result<()> {
            match self.api.owner(window)? {
                None => Ok(()), // Destruction already freed the OS-owned region.
                Some(owner) if owner == entry.owner => {
                    self.api.restore(window, owner, &entry.original)
                }
                Some(_) => bail!("HWND was reused; refusing to restore another window's region"),
            }
        })();
        match result {
            Ok(()) => {
                self.entries.remove(&window);
                true
            }
            Err(error) => {
                warn!(%window, %error, "native clip original region not restored");
                // Retain the snapshot even on identity-query/restore failure.
                false
            }
        }
    }

    fn restore_all(&mut self) -> bool {
        let windows: Vec<_> = self.entries.keys().copied().collect();
        let mut restored = true;
        for window in windows {
            // Do not short-circuit: a protected window must not strand others.
            restored = self.remove(window) && restored;
        }
        restored
    }
}

/// Aligned, process-independent RGNDATA. Keeping bytes rather than HRGNs in the
/// engine avoids any Send/Sync assertion about Windows GDI handle wrappers.
struct RegionData {
    words: Vec<u32>,
}

impl RegionData {
    fn rectangles(rectangles: &[Bounds]) -> Result<Self> {
        let size = HEADER_WORDS * 4 + rectangles.len() * size_of::<RECT>();
        ensure!(size <= MAX_REGION_BYTES, "region exceeds memory limit");
        let mut bound = [0_i32; 4];
        if let Some(first) = rectangles.first() {
            bound = [first.left, first.top, first.right, first.bottom];
            for rect in rectangles.iter().skip(1) {
                bound[0] = bound[0].min(rect.left);
                bound[1] = bound[1].min(rect.top);
                bound[2] = bound[2].max(rect.right);
                bound[3] = bound[3].max(rect.bottom);
            }
        }
        let mut words = Vec::with_capacity(size / 4);
        words.extend_from_slice(&[
            (HEADER_WORDS * 4) as u32,
            1, // RDH_RECTANGLES
            rectangles.len() as u32,
            (rectangles.len() * size_of::<RECT>()) as u32,
            bound[0] as u32,
            bound[1] as u32,
            bound[2] as u32,
            bound[3] as u32,
        ]);
        for rect in rectangles {
            words.extend_from_slice(&[
                rect.left as u32,
                rect.top as u32,
                rect.right as u32,
                rect.bottom as u32,
            ]);
        }
        Ok(Self { words })
    }

    fn snapshot(region: &OwnedRegion) -> Result<Self> {
        let size = unsafe { GetRegionData(region.0, 0, None) } as usize;
        ensure!(
            (HEADER_WORDS * 4..=MAX_REGION_BYTES).contains(&size) && size % 4 == 0,
            "GetRegionData returned invalid/oversized length {size}"
        );
        let mut data = Self {
            words: vec![0; size / 4],
        };
        let copied = unsafe {
            GetRegionData(
                region.0,
                size as u32,
                Some(data.words.as_mut_ptr().cast::<RGNDATA>()),
            )
        };
        ensure!(copied as usize == size, "GetRegionData snapshot failed");
        Ok(data)
    }

    fn create_region(&self) -> Result<OwnedRegion> {
        if self.words[2] == 0 {
            // Preserve an explicit NULLREGION; it is not a request to remove it.
            return OwnedRegion::new(unsafe { CreateRectRgn(0, 0, 0, 0) }, "CreateRectRgn");
        }
        // Vec<u32> supplies RGNDATA's DWORD alignment; all initialized words are
        // valid to read. ExtCreateRegion copies them only during this call.
        OwnedRegion::new(
            unsafe {
                ExtCreateRegion(
                    None,
                    (self.words.len() * 4) as u32,
                    self.words.as_ptr().cast::<RGNDATA>(),
                )
            },
            "ExtCreateRegion",
        )
    }
}

struct OwnedRegion(HRGN);

impl OwnedRegion {
    fn new(region: HRGN, operation: &'static str) -> Result<Self> {
        ensure!(!region.0.is_null(), "{operation} failed");
        Ok(Self(region))
    }

    fn intersect(&mut self, other: &Self) -> Result<()> {
        let kind = unsafe { CombineRgn(self.0, self.0, other.0, RGN_AND) };
        ensure!(kind.0 != 0, "CombineRgn(RGN_AND) failed");
        Ok(())
    }
}

impl Drop for OwnedRegion {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.0 .0));
            }
        }
    }
}

#[derive(Default)]
struct Win32Regions;

fn hwnd(window: WindowId) -> HWND {
    HWND(window.as_isize() as *mut std::ffi::c_void)
}

fn last_error(operation: &'static str) -> anyhow::Error {
    anyhow!("{operation} failed (Win32 {})", unsafe { GetLastError().0 })
}

impl Win32Regions {
    fn check_owner(&mut self, window: WindowId, expected: WindowOwner) -> Result<()> {
        ensure!(
            self.owner(window)? == Some(expected),
            "window identity changed"
        );
        Ok(())
    }

    fn check_layout(window: WindowId) -> Result<()> {
        unsafe {
            SetLastError(WIN32_ERROR(0));
        }
        let style = unsafe { GetWindowLongPtrW(hwnd(window), GWL_EXSTYLE) };
        if style == 0 && unsafe { GetLastError().0 } != 0 {
            return Err(last_error("GetWindowLongPtrW(GWL_EXSTYLE)"));
        }
        ensure!(
            style & WS_EX_LAYOUTRTL.0 as isize == 0,
            "RTL window regions are unsupported"
        );
        Ok(())
    }

    fn bounds(window: WindowId, visible: Rect) -> Result<Bounds> {
        // Same per-monitor-DPI-aware context as backend placement is required:
        // GetWindowRect is otherwise virtualized, while DWM bounds are physical.
        let mut native = RECT::default();
        let mut frame = RECT::default();
        let mut after = RECT::default();
        unsafe {
            GetWindowRect(hwnd(window), &mut native)?;
            DwmGetWindowAttribute(
                hwnd(window),
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut frame as *mut RECT).cast(),
                size_of::<RECT>() as u32,
            )?;
            GetWindowRect(hwnd(window), &mut after)?;
        }
        ensure!(
            native.left == after.left
                && native.top == after.top
                && native.right == after.right
                && native.bottom == after.bottom,
            "window moved during native/DWM frame measurement"
        );
        local_bounds(visible, native, frame)
    }

    fn install(window: WindowId, mut region: Option<OwnedRegion>) -> Result<()> {
        unsafe {
            SetLastError(WIN32_ERROR(0));
        }
        let raw = region.as_ref().map_or(HRGN::default(), |r| r.0);
        if unsafe { SetWindowRgn(hwnd(window), raw, BOOL(1)) } == 0 {
            return Err(last_error("SetWindowRgn")); // Still ours: RAII deletes it.
        }
        if let Some(region) = &mut region {
            region.0 = HRGN::default(); // Ownership transferred, only on success.
        }
        Ok(())
    }
}

impl RegionApi for Win32Regions {
    type Snapshot = Option<RegionData>;

    fn owner(&mut self, window: WindowId) -> Result<Option<WindowOwner>> {
        if !unsafe { IsWindow(hwnd(window)) }.as_bool() {
            return Ok(None);
        }
        let mut process = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd(window), Some(&mut process)) };
        ensure!(
            thread != 0 && process != 0,
            "GetWindowThreadProcessId failed"
        );
        Ok(Some(WindowOwner { process, thread }))
    }

    fn measure(&mut self, window: WindowId, visible: Rect, corners: [f32; 4]) -> Result<ClipKey> {
        Self::check_layout(window)?;
        ClipKey::new(Self::bounds(window, visible)?, corners)
    }

    fn snapshot(&mut self, window: WindowId, owner: WindowOwner) -> Result<Self::Snapshot> {
        self.check_owner(window, owner)?;
        Self::check_layout(window)?;
        let region = OwnedRegion::new(unsafe { CreateRectRgn(0, 0, 0, 0) }, "CreateRectRgn")?;
        // ERROR is also the documented result for *no associated region*. Clear
        // last-error first: only accept it as absent on a still-valid same-owner
        // HWND with no reported error. Never mistake a reported failure for None.
        unsafe {
            SetLastError(WIN32_ERROR(0));
        }
        let kind = unsafe { GetWindowRgn(hwnd(window), region.0) }.0;
        let error = unsafe { GetLastError().0 };
        self.check_owner(window, owner)?;
        match kind {
            0 if error == 0 => Ok(None),
            0 => bail!("GetWindowRgn failed (Win32 {error})"),
            1..=3 => Ok(Some(RegionData::snapshot(&region)?)),
            _ => bail!("GetWindowRgn returned unexpected region type {kind}"),
        }
    }

    fn apply(
        &mut self,
        window: WindowId,
        owner: WindowOwner,
        key: ClipKey,
        original: &Self::Snapshot,
    ) -> Result<()> {
        let mut region = RegionData::rectangles(&scan_rectangles(key)?)?.create_region()?;
        if let Some(original) = original {
            // Narrow the application's shape; never expose pixels it excluded.
            region.intersect(&original.create_region()?)?;
        }
        self.check_owner(window, owner)?;
        Self::check_layout(window)?;
        // Geometry construction can take time. Do not knowingly install a stale
        // mask if an asynchronous/native resize completed in the meantime.
        let size = Rect::new(0, 0, key.bounds.width(), key.bounds.height());
        ensure!(
            Self::bounds(window, size)? == key.bounds,
            "window frame changed before SetWindowRgn"
        );
        Self::install(window, Some(region))
    }

    fn restore(
        &mut self,
        window: WindowId,
        owner: WindowOwner,
        original: &Self::Snapshot,
    ) -> Result<()> {
        let region = original
            .as_ref()
            .map(RegionData::create_region)
            .transpose()?;
        self.check_owner(window, owner)?;
        Self::check_layout(window)?;
        Self::install(window, region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(width: u32, height: u32, corners: [f32; 4]) -> ClipKey {
        ClipKey::new(
            Bounds::checked(0, 0, width as i64, height as i64).unwrap(),
            corners,
        )
        .unwrap()
    }

    fn contains(rectangles: &[Bounds], x: i32, y: i32) -> bool {
        rectangles
            .iter()
            .any(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
    }

    #[test]
    fn state_is_send_and_sync_without_unsafe_handle_assertions() {
        fn check<T: Send + Sync>() {}
        check::<ClipState>();
    }

    #[test]
    fn css_fitting_scales_all_four_corners_by_one_factor() {
        let fitted = fit_corners(100, 60, [80.0, 40.0, 20.0, 20.0]).unwrap();
        for (actual, expected) in fitted.into_iter().zip([48.0, 24.0, 12.0, 12.0]) {
            assert!((actual - expected).abs() < 1e-10);
        }
        // A lone corner is allowed to exceed half either edge.
        assert_eq!(
            fit_corners(100, 100, [80.0, 0.0, 0.0, 0.0]).unwrap()[0],
            80.0
        );
        for radius in fit_corners(100, 50, [f32::MAX; 4]).unwrap() {
            assert!((radius - 25.0).abs() < 1e-10);
        }
    }

    #[test]
    fn physical_scaling_preserves_fractional_independent_radii() {
        let logical = [10.5_f32, 6.25, 3.0, 0.0];
        let physical = logical.map(|r| r * 1.5);
        assert_eq!(
            fit_corners(100, 80, physical).unwrap(),
            [15.75, 9.375, 4.5, 0.0]
        );
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
            assert!(fit_corners(100, 80, [invalid, 0.0, 0.0, 0.0]).is_err());
        }
        assert!(fit_corners(0, 80, [0.0; 4]).is_err());
    }

    #[test]
    fn visible_frame_insets_not_screen_or_native_frame_size_drive_region() {
        let native = RECT {
            left: -1008,
            top: 192,
            right: -392,
            bottom: 608,
        };
        let frame = RECT {
            left: -1000,
            top: 200,
            right: -400,
            bottom: 600,
        };
        let visible = Rect::new(-1000, 200, 600, 400);
        assert_eq!(
            local_bounds(visible, native, frame).unwrap(),
            Bounds {
                left: 8,
                top: 8,
                right: 608,
                bottom: 408,
            }
        );
        // Moving the pending target does not shift a region within the HWND.
        assert_eq!(
            local_bounds(visible, native, frame).unwrap(),
            local_bounds(Rect::new(1200, -500, 600, 400), native, frame).unwrap()
        );
        assert!(local_bounds(Rect::new(0, 0, 700, 400), native, frame).is_err());
        assert!(Bounds::checked(GDI_MIN - 1, 0, 10, 10).is_err());
    }

    #[test]
    fn scanlines_are_exact_pixel_centre_samples_of_four_quarter_circles() {
        let key = key(29, 19, [14.0, 3.25, 7.0, 0.0]);
        let rects = scan_rectangles(key).unwrap();
        let [tl, tr, br, bl] = key.corners;
        for y in 0..19 {
            for x in 0..29 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let mut expected = true;
                for (r, dx, dy) in [
                    (tl, tl - px, tl - py),
                    (tr, px - (29.0 - tr), tr - py),
                    (br, px - (29.0 - br), py - (19.0 - br)),
                    (bl, bl - px, py - (19.0 - bl)),
                ] {
                    if dx > 0.0 && dy > 0.0 && dx * dx + dy * dy > r * r {
                        expected = false;
                    }
                }
                assert_eq!(contains(&rects, x, y), expected, "pixel ({x}, {y})");
            }
        }
        assert!(!contains(&rects, 0, 0));
        assert!(contains(&rects, 0, 18)); // BL is square, not TL's radius.
    }

    #[test]
    fn rectangles_coalesce_and_do_not_include_right_or_bottom_edges() {
        let rects = scan_rectangles(key(200, 100, [0.0; 4])).unwrap();
        assert_eq!(
            rects,
            vec![Bounds {
                left: 0,
                top: 0,
                right: 200,
                bottom: 100
            }]
        );
        assert!(contains(&rects, 199, 99));
        assert!(!contains(&rects, 200, 99));
        assert!(!contains(&rects, 199, 100));
        let rounded = scan_rectangles(key(200, 100, [8.0; 4])).unwrap();
        assert!(rounded.len() < 20);
        let data = RegionData::rectangles(&rects).unwrap();
        assert_eq!(&data.words[..8], &[32, 1, 1, 16, 0, 0, 200, 100]);
        assert_eq!(
            RegionData::rectangles(&[]).unwrap().words,
            vec![32, 1, 0, 0, 0, 0, 0, 0]
        );
    }

    #[derive(Default)]
    struct FakeRegions {
        dead: bool,
        generation: u32,
        original: Option<Vec<u8>>,
        snapshots: usize,
        applications: usize,
        restorations: Vec<(WindowId, Option<Vec<u8>>)>,
        fail_snapshot: bool,
        fail_apply: bool,
        fail_restore: bool,
    }

    impl RegionApi for FakeRegions {
        type Snapshot = Option<Vec<u8>>;
        fn owner(&mut self, _: WindowId) -> Result<Option<WindowOwner>> {
            Ok((!self.dead).then_some(WindowOwner {
                process: 42 + self.generation,
                thread: 7,
            }))
        }
        fn measure(&mut self, _: WindowId, visible: Rect, corners: [f32; 4]) -> Result<ClipKey> {
            ClipKey::new(
                Bounds::checked(8, 0, 8 + visible.size.w as i64, visible.size.h as i64)?,
                corners,
            )
        }
        fn snapshot(&mut self, _: WindowId, _: WindowOwner) -> Result<Self::Snapshot> {
            self.snapshots += 1;
            ensure!(!self.fail_snapshot, "snapshot denied");
            Ok(self.original.clone())
        }
        fn apply(
            &mut self,
            _: WindowId,
            _: WindowOwner,
            _: ClipKey,
            _: &Self::Snapshot,
        ) -> Result<()> {
            self.applications += 1;
            ensure!(!self.fail_apply, "SetWindowRgn denied");
            Ok(())
        }
        fn restore(
            &mut self,
            window: WindowId,
            _: WindowOwner,
            original: &Self::Snapshot,
        ) -> Result<()> {
            self.restorations.push((window, original.clone()));
            ensure!(!self.fail_restore, "restore denied");
            Ok(())
        }
    }

    #[test]
    fn cache_skips_unchanged_gdi_work_but_not_resize_or_asymmetric_corner_change() {
        let mut state = ClipBook::<FakeRegions>::default();
        let id = WindowId::new(1);
        let visible = Rect::new(20, 30, 100, 80);
        assert!(state.sync(id, visible, Some([8.0, 4.0, 2.0, 0.0])));
        assert!(state.sync(
            id,
            Rect::new(-900, 200, 100, 80),
            Some([8.0, 4.0, 2.0, 0.0])
        ));
        assert_eq!(state.api.applications, 1);
        assert!(state.sync(id, Rect::new(20, 30, 101, 80), Some([8.0, 4.0, 2.0, 0.0])));
        assert!(state.sync(id, visible, Some([8.0, 4.0, 0.0, 2.0])));
        assert_eq!(state.api.applications, 3);
        // The explicit invalidation used after an app-side region change must
        // reapply even the same shape without replacing the original snapshot.
        state.entries.get_mut(&id).unwrap().applied = None;
        assert!(state.sync(id, visible, Some([8.0, 4.0, 0.0, 2.0])));
        assert_eq!(state.api.applications, 4);
        assert_eq!(state.api.snapshots, 1);
    }

    #[test]
    fn removal_restores_original_once_and_distinguishes_absent_empty_and_shaped() {
        for original in [None, Some(vec![]), Some(vec![1, 9, 3])] {
            let mut state = ClipBook::<FakeRegions>::default();
            state.api.original = original.clone();
            let id = WindowId::new(1);
            let visible = Rect::new(0, 0, 100, 80);
            assert!(state.sync(id, visible, Some([0.0; 4])));
            // A later update must not snapshot our own region or a newer value.
            state.api.original = Some(vec![99]);
            assert!(state.sync(id, visible, Some([12.0; 4])));
            assert!(state.sync(id, visible, None));
            assert!(state.remove(id));
            assert_eq!(state.api.restorations, vec![(id, original)]);
            assert!(state.entries.is_empty());
        }
    }

    #[test]
    fn failed_snapshot_or_first_apply_is_not_cached_as_success() {
        let id = WindowId::new(1);
        let visible = Rect::new(0, 0, 100, 80);
        let mut state = ClipBook::<FakeRegions>::default();
        state.api.fail_snapshot = true;
        assert!(!state.sync(id, visible, Some([8.0; 4])));
        assert_eq!(state.api.applications, 0);
        assert!(state.entries.is_empty());
        state.api.fail_snapshot = false;
        state.api.fail_apply = true;
        assert!(!state.sync(id, visible, Some([8.0; 4])));
        assert!(state.entries.is_empty());
        state.api.fail_apply = false;
        assert!(state.sync(id, visible, Some([8.0; 4])));
        let old_key = state.entries[&id].applied;
        state.api.fail_apply = true;
        assert!(!state.sync(id, visible, Some([20.0; 4])));
        assert_eq!(state.entries[&id].applied, old_key);
    }

    #[test]
    fn restore_all_attempts_every_window_and_retains_failed_snapshots_for_retry() {
        let mut state = ClipBook::<FakeRegions>::default();
        for id in [WindowId::new(1), WindowId::new(2)] {
            assert!(state.sync(id, Rect::new(0, 0, 100, 80), Some([8.0; 4])));
        }
        state.api.fail_restore = true;
        assert!(!state.restore_all());
        assert_eq!(state.api.restorations.len(), 2);
        assert_eq!(state.entries.len(), 2);
        state.api.fail_restore = false;
        assert!(state.restore_all());
        assert!(state.entries.is_empty());
    }

    #[test]
    fn destroyed_or_reused_windows_are_not_restored_with_somebody_elses_region() {
        let id = WindowId::new(1);
        let visible = Rect::new(0, 0, 100, 80);
        let mut state = ClipBook::<FakeRegions>::default();
        assert!(state.sync(id, visible, Some([8.0; 4])));
        state.api.generation += 1;
        assert!(!state.remove(id));
        assert!(state.api.restorations.is_empty());
        // A new explicit request starts a fresh ownership epoch and snapshot.
        assert!(state.sync(id, visible, Some([8.0; 4])));
        assert_eq!(state.api.snapshots, 2);
        state.api.dead = true;
        assert!(state.remove(id));
        assert!(state.api.restorations.is_empty());
        assert!(state.entries.is_empty());
    }
}
