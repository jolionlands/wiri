//! Opt-in Precision Touchpad Raw Input on the existing hotkey pump. No idle clock.
//! HID descriptors, not a vendor-specific packed layout, identify each contact.
use crate::backend::BackendHandle;
use crate::layout::TilingEngine;
use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;
use windows::core::w;
use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub id: u32,
    pub x: f64,
    pub y: f64,
}
#[derive(Debug, PartialEq)]
pub enum Swipe {
    Begin,
    Delta(f64, f64),
    End(bool),
}
#[derive(Default)]
pub struct Recognizer {
    last: BTreeMap<u32, (f64, f64)>,
    active: bool,
    blocked: bool,
}
impl Recognizer {
    pub fn frame(&mut self, contacts: &[Contact]) -> Vec<Swipe> {
        if contacts.is_empty() {
            self.blocked = false;
        }
        let points: BTreeMap<_, _> = contacts.iter().map(|c| (c.id, (c.x, c.y))).collect();
        let valid = points.len() == 3
            && points.values().all(|(x, y)| x.is_finite() && y.is_finite())
            && !self.blocked;
        let mut out = vec![];
        if contacts.len() > 3 {
            self.blocked = true;
        }
        if !valid {
            if self.active {
                out.push(Swipe::End(contacts.len() > 3));
            }
            self.active = false;
            self.last.clear();
            return out;
        }
        if !self.active {
            self.active = true;
            out.push(Swipe::Begin);
        } else if self.last.keys().eq(points.keys()) {
            let (dx, dy) = points.iter().fold((0.0, 0.0), |(dx, dy), (id, (x, y))| {
                let (px, py) = self.last[id];
                (dx + x - px, dy + y - py)
            });
            out.push(Swipe::Delta(dx / 3.0, dy / 3.0));
        }
        self.last = points;
        out
    }
}
/// Serial/hybrid reports with Contact Count only in the first report of a frame.
#[derive(Default)]
struct Assembly {
    scan: Option<u32>,
    expected: usize,
    contacts: Vec<Contact>,
}
impl Assembly {
    fn push(&mut self, scan: u32, count: usize, contacts: Vec<Contact>) -> Option<Vec<Contact>> {
        if self.scan != Some(scan) || count > 0 {
            self.scan = Some(scan);
            self.expected = count;
            self.contacts.clear();
        }
        self.contacts.extend(contacts);
        if self.contacts.len() >= self.expected {
            self.scan = None;
            Some(std::mem::take(&mut self.contacts))
        } else {
            None
        }
    }
}
struct Device {
    data: Vec<usize>,
    ids: Vec<HIDP_VALUE_CAPS>,
    assembly: Assembly,
}
impl Device {
    unsafe fn load(handle: HANDLE) -> Option<Self> {
        let mut len = 0;
        if GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, None, &mut len) == u32::MAX
            || len == 0
            || len > 65536
        {
            return None;
        }
        let mut data = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetRawInputDeviceInfoW(
            handle,
            RIDI_PREPARSEDDATA,
            Some(data.as_mut_ptr().cast()),
            &mut len,
        ) == u32::MAX
        {
            return None;
        }
        let ppd = PHIDP_PREPARSED_DATA(data.as_mut_ptr() as isize);
        let mut caps = HIDP_CAPS::default();
        if HidP_GetCaps(ppd, &mut caps) != HIDP_STATUS_SUCCESS
            || caps.UsagePage != 0x0d
            || caps.Usage != 0x05
        {
            return None;
        }
        let mut ids = vec![HIDP_VALUE_CAPS::default(); caps.NumberInputValueCaps as usize];
        let mut count = ids.len() as u16;
        if count == 0
            || HidP_GetValueCaps(HidP_Input, ids.as_mut_ptr(), &mut count, ppd)
                != HIDP_STATUS_SUCCESS
        {
            return None;
        }
        ids.truncate(count as usize);
        ids.retain(|c| {
            c.UsagePage == 0x0d && !c.IsRange.as_bool() && c.Anonymous.NotRange.Usage == 0x51
        });
        Some(Self {
            data,
            ids,
            assembly: Assembly::default(),
        })
    }
    unsafe fn value(&self, page: u16, link: u16, usage: u16, report: &[u8]) -> Option<u32> {
        let mut value = 0;
        (HidP_GetUsageValue(
            HidP_Input,
            page,
            link,
            usage,
            &mut value,
            PHIDP_PREPARSED_DATA(self.data.as_ptr() as isize),
            report,
        ) == HIDP_STATUS_SUCCESS)
            .then_some(value)
    }
    unsafe fn coordinate(&self, link: u16, usage: u16, report: &[u8]) -> Option<f64> {
        let mut cap = HIDP_VALUE_CAPS::default();
        let mut n = 1;
        let ppd = PHIDP_PREPARSED_DATA(self.data.as_ptr() as isize);
        if HidP_GetSpecificValueCaps(HidP_Input, 1, link, usage, &mut cap, &mut n, ppd)
            != HIDP_STATUS_SUCCESS
            || cap.LogicalMax <= cap.LogicalMin
        {
            return None;
        }
        let raw = self.value(1, link, usage, report)? as i64;
        Some(
            (raw - cap.LogicalMin as i64) as f64 * 300.0
                / (cap.LogicalMax as i64 - cap.LogicalMin as i64) as f64,
        )
    }
    unsafe fn report(&mut self, report: &[u8]) -> Option<Vec<Contact>> {
        let count = self.value(0x0d, 0, 0x54, report)? as usize;
        if count > 32 {
            return None;
        }
        let scan = self.value(0x0d, 0, 0x56, report).unwrap_or(0);
        let mut contacts = vec![];
        // Keep inactive entries until frame assembly is complete, then filter them.
        for cap in &self.ids {
            if cap.ReportID != 0 && report.first() != Some(&cap.ReportID) {
                continue;
            }
            let link = cap.LinkCollection;
            let Some(id) = self.value(0x0d, link, 0x51, report) else {
                continue;
            };
            let mut usages = [USAGE_AND_PAGE::default(); 16];
            let mut n = usages.len() as u32;
            let ppd = PHIDP_PREPARSED_DATA(self.data.as_ptr() as isize);
            if HidP_GetUsagesEx(HidP_Input, link, usages.as_mut_ptr(), &mut n, ppd, report)
                != HIDP_STATUS_SUCCESS
            {
                continue;
            }
            let has = |usage| {
                usages[..(n as usize).min(usages.len())]
                    .iter()
                    .any(|u| u.UsagePage == 0x0d && u.Usage == usage)
            };
            let mut confidence_cap = HIDP_BUTTON_CAPS::default();
            let mut confidence_count = 1;
            let has_confidence = HidP_GetSpecificButtonCaps(
                HidP_Input,
                0x0d,
                link,
                0x47,
                &mut confidence_cap,
                &mut confidence_count,
                ppd,
            ) == HIDP_STATUS_SUCCESS;
            let active = has(0x42) && (!has_confidence || has(0x47));
            let x = self.coordinate(link, 0x30, report)?;
            let y = self.coordinate(link, 0x31, report)?;
            contacts.push(Contact {
                id,
                x: if active { x } else { f64::NAN },
                y,
            });
        }
        self.assembly.push(scan, count, contacts).map(|mut c| {
            c.retain(|c| c.x.is_finite());
            c
        })
    }
}
struct Service {
    hwnd: HWND,
    engine: Arc<RwLock<TilingEngine>>,
    backend: BackendHandle,
    devices: HashMap<isize, Option<Device>>,
    recognizer: Recognizer,
    owner: Option<isize>,
    clock: Instant,
}
thread_local! { static SERVICE: RefCell<Option<Service>> = const { RefCell::new(None) }; }

pub fn configure(enabled: bool, engine: Arc<RwLock<TilingEngine>>, backend: BackendHandle) {
    shutdown();
    if !enabled {
        return;
    }
    unsafe {
        let class = w!("WiriTouchpadRawInput");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = match CreateWindowExW(
            Default::default(),
            class,
            class,
            Default::default(),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            None,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("Precision touchpad HWND failed: {e}");
                return;
            }
        };
        let devices = [RAWINPUTDEVICE {
            usUsagePage: 0x0d,
            usUsage: 0x05,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: hwnd,
        }];
        if let Err(e) =
            RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32)
        {
            let _ = DestroyWindow(hwnd);
            tracing::warn!("Precision touchpad registration failed: {e}");
            return;
        }
        SERVICE.with(|s| {
            *s.borrow_mut() = Some(Service {
                hwnd,
                engine,
                backend,
                devices: HashMap::new(),
                recognizer: Recognizer::default(),
                owner: None,
                clock: Instant::now(),
            })
        });
        tracing::info!("Precision touchpad Raw Input registered");
    }
}
pub fn shutdown() {
    if let Some(s) = SERVICE.with(|s| s.borrow_mut().take()) {
        super::gestures::external_end(&s.engine, &s.backend, s.clock.elapsed(), true);
        unsafe {
            let devices = [RAWINPUTDEVICE {
                usUsagePage: 0x0d,
                usUsage: 0x05,
                dwFlags: RIDEV_REMOVE,
                hwndTarget: HWND::default(),
            }];
            if let Err(e) =
                RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32)
            {
                tracing::warn!("Precision touchpad unregister failed: {e}");
            }
            let _ = DestroyWindow(s.hwnd);
        }
        tracing::info!("Precision touchpad Raw Input removed");
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if message == WM_INPUT {
        receive(HRAWINPUT(lp.0 as *mut _));
    } else if message == WM_INPUT_DEVICE_CHANGE {
        SERVICE.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut() {
                s.devices.remove(&lp.0);
                if s.owner == Some(lp.0) {
                    super::gestures::external_end(&s.engine, &s.backend, s.clock.elapsed(), true);
                    s.recognizer = Recognizer::default();
                    s.owner = None;
                }
            }
        });
    }
    // Required for RIM_INPUT cleanup, also harmless for background RIM_INPUTSINK.
    DefWindowProcW(hwnd, message, wp, lp)
}
unsafe fn receive(raw: HRAWINPUT) {
    let header_size = std::mem::size_of::<RAWINPUTHEADER>();
    let mut len = 0;
    if GetRawInputData(raw, RID_INPUT, None, &mut len, header_size as u32) == u32::MAX
        || len < (header_size + 8) as u32
        || len > 1_048_576
    {
        return;
    }
    let mut words = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
    let got = GetRawInputData(
        raw,
        RID_INPUT,
        Some(words.as_mut_ptr().cast()),
        &mut len,
        header_size as u32,
    );
    if got == u32::MAX || got < (header_size + 8) as u32 || got > len {
        return;
    }
    let bytes = std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), got as usize);
    let header = std::ptr::read_unaligned(bytes.as_ptr().cast::<RAWINPUTHEADER>());
    if header.dwType != RIM_TYPEHID.0 {
        return;
    }
    let size = u32::from_ne_bytes(bytes[header_size..header_size + 4].try_into().unwrap()) as usize;
    let count =
        u32::from_ne_bytes(bytes[header_size + 4..header_size + 8].try_into().unwrap()) as usize;
    let Some(end) = size
        .checked_mul(count)
        .and_then(|n| n.checked_add(header_size + 8))
    else {
        return;
    };
    if size == 0 || end > bytes.len() {
        return;
    }
    SERVICE.with(|slot| {
        if let Some(s) = slot.borrow_mut().as_mut() {
            let key = header.hDevice.0 as isize;
            if s.owner.is_some_and(|o| o != key) {
                return;
            }
            if !s.devices.contains_key(&key) {
                // Bounded descriptor cache, including unsupported devices.
                if s.devices.len() >= 16 {
                    return;
                }
                s.devices.insert(key, Device::load(header.hDevice));
            }
            for report in bytes[header_size + 8..end].chunks_exact(size) {
                let frame = s
                    .devices
                    .get_mut(&key)
                    .and_then(|d| d.as_mut())
                    .and_then(|d| d.report(report));
                if let Some(frame) = frame {
                    for event in s.recognizer.frame(&frame) {
                        let engine = s.engine.clone();
                        let backend = s.backend.clone();
                        let time = s.clock.elapsed();
                        match event {
                            Swipe::Begin => s.owner = Some(key),
                            Swipe::End(_) => s.owner = None,
                            _ => {}
                        }
                        super::dispatch::send_work(Box::new(move || match event {
                            Swipe::Begin => super::gestures::external_begin(&engine, &backend),
                            Swipe::Delta(dx, dy) => {
                                super::gestures::external_update(&engine, &backend, dx, dy, time)
                            }
                            Swipe::End(cancel) => {
                                super::gestures::external_end(&engine, &backend, time, cancel)
                            }
                        }));
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(x: f64, y: f64, n: u32) -> Vec<Contact> {
        (0..n)
            .map(|id| Contact {
                id,
                x: x + id as f64,
                y,
            })
            .collect()
    }
    #[test]
    fn recorded_three_finger_frames_follow_centroid_and_end_once() {
        let mut r = Recognizer::default();
        assert!(r.frame(&frame(0.0, 0.0, 2)).is_empty());
        assert_eq!(r.frame(&frame(0.0, 0.0, 3)), vec![Swipe::Begin]);
        assert_eq!(
            r.frame(&frame(12.0, -5.0, 3)),
            vec![Swipe::Delta(12.0, -5.0)]
        );
        let mut changed = frame(50.0, 50.0, 3);
        changed[0].id = 8;
        assert!(
            r.frame(&changed).is_empty(),
            "contact replacement rebases, never jumps"
        );
        assert_eq!(r.frame(&[]), vec![Swipe::End(false)]);
        assert!(r.frame(&[]).is_empty());
    }
    #[test]
    fn recorded_swipe_hands_distance_velocity_and_pause_to_kinetics() {
        use crate::input::gestures::{Axis, SpatialGesture};
        use std::time::Duration;
        let mut r = Recognizer::default();
        let mut g = SpatialGesture::default();
        for (ms, x) in [(0, 0.0), (20, 12.0), (40, 30.0), (60, 54.0)] {
            for event in r.frame(&frame(x, 0.0, 3)) {
                if let Swipe::Delta(dx, dy) = event {
                    g.update(dx, dy, Duration::from_millis(ms));
                }
            }
        }
        assert_eq!(g.axis, Some(Axis::Horizontal));
        assert_eq!(g.x.position(), 54.0);
        assert!(g.x.velocity() > 0.0 && g.x.projected() > 54.0);
        g.release(Duration::from_millis(300));
        assert_eq!(g.x.velocity(), 0.0);
    }
    #[test]
    fn raw_contacts_are_opt_in_and_niri_off_removes_opt_in() {
        let decode = |text| crate::config::decode::decode_text(text, "", None);
        assert!(!decode("").config.input.touchpad_enabled);
        let enabled = decode("input { touchpad { } }");
        assert!(!enabled.has_errors());
        assert!(enabled.config.input.touchpad_enabled);
        let disabled = decode("input { touchpad { off; } }");
        assert!(!disabled.has_errors());
        assert!(!disabled.config.input.touchpad_enabled);
    }
    #[test]
    fn fourth_finger_cancels_and_requires_all_up() {
        let mut r = Recognizer::default();
        r.frame(&frame(0.0, 0.0, 3));
        assert_eq!(r.frame(&frame(5.0, 0.0, 4)), vec![Swipe::End(true)]);
        assert!(r.frame(&frame(5.0, 0.0, 3)).is_empty());
        r.frame(&[]);
        assert_eq!(r.frame(&frame(5.0, 0.0, 3)), vec![Swipe::Begin]);
    }
    #[test]
    fn serial_reports_assemble_before_recognition_and_abandon_incomplete_frame() {
        let mut a = Assembly::default();
        assert!(a.push(1, 3, frame(0.0, 0.0, 1)).is_none());
        assert_eq!(a.push(1, 0, frame(0.0, 0.0, 2)).unwrap().len(), 3);
        assert!(a.push(2, 3, frame(0.0, 0.0, 1)).is_none());
        assert_eq!(a.push(3, 1, frame(0.0, 0.0, 1)).unwrap().len(), 1);
        assert!(a.push(4, 0, vec![]).unwrap().is_empty());
    }
}
