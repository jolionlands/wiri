//! Output matching and conservative Windows display configuration.
//!
//! A snapshot is read once, then a pure planner decides whether anything needs
//! changing. In particular, defaults, layout scale, startup focus, and an explicit
//! current mode never reach `SetDisplayConfig` (not even its validation operation).
//! Windows owns effective DPI and automatic output placement; neither is reset.

use crate::config::{Config, OutputConfig};
use super::MonitorInfo;
use tracing::{info, warn};
use windows::core::{w, PCWSTR};
use windows::Win32::Devices::Display as ccd;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY};

/// Windows' GDI name is the connector. EDID's three-letter PNP ID is the make.
/// `model` normally comes from EDID, with the CCD friendly name as a fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputIdentity {
    pub connector: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub friendly_name: String,
    pub device_path: String,
}

impl OutputIdentity {
    /// Exact, case-insensitive matching, never a prefix or enumeration-index match.
    pub fn matches(&self, name: &str) -> bool {
        if same_name(name, &self.connector) || same_name(name, &self.friendly_name) {
            return true;
        }
        let mut makes = vec![self.make.as_str()];
        // These are aliases, not guesses used to identify a particular monitor.
        // The PNP spelling always remains available, including for unknown vendors.
        makes.extend_from_slice(match self.make.to_ascii_uppercase().as_str() {
            "DEL" => &["Dell", "Dell Inc."],
            "SAM" => &["Samsung", "Samsung Electric Company"],
            "GSM" => &["LG Electronics"],
            "ACI" | "AUS" => &["ASUSTek COMPUTER INC", "ASUS"],
            "ACR" => &["Acer", "Acer Technologies"],
            "HWP" | "HPN" => &["HP", "HP Inc.", "Hewlett Packard"],
            "LEN" => &["Lenovo", "Lenovo Group Limited"],
            _ => &[],
        });
        for make in makes {
            for model in [&self.model, &self.friendly_name] {
                if model.is_empty() || make.is_empty() {
                    continue;
                }
                if same_name(name, &format!("{make} {model}"))
                    || (!self.serial.is_empty()
                        && same_name(name, &format!("{make} {model} {}", self.serial)))
                {
                    return true;
                }
            }
        }
        false
    }
}

fn normalized_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn same_name(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && !b.trim().is_empty() && normalized_name(a) == normalized_name(b)
}

/// Reuse this for a whole monitor-registration/reload pass instead of querying
/// CCD and the registry once per monitor. Matching itself is pure.
#[derive(Debug, Clone, Default)]
pub struct OutputMatcher {
    pub identities: Vec<OutputIdentity>,
}

impl OutputMatcher {
    /// Read-only. Connector matching still works when CCD/EDID is unavailable
    /// (for example in a remote desktop session).
    pub fn snapshot() -> Self {
        match Topology::query() {
            Ok(topology) => Self { identities: topology.identities },
            Err(error) => {
                warn!(%error, "Could not read output identities; using connector names only");
                Self::default()
            }
        }
    }

    pub fn matching_config<'a>(
        &self,
        config: &'a Config,
        monitor: &MonitorInfo,
    ) -> Option<&'a OutputConfig> {
        // Explicit connector blocks take precedence over model/friendly-name
        // blocks. An unmatched named block must never fall back by index.
        config.output.iter().find(|out| same_name(&out.name, &monitor.name))
            .or_else(|| config.output.iter().find(|out| {
                !out.name.trim().is_empty() && self.identities.iter().any(|identity| {
                    same_name(&identity.connector, &monitor.name) && identity.matches(&out.name)
                })
            }))
            .or_else(|| config.output.iter().find(|out| out.name.trim().is_empty()))
    }
}

/// Convenience for a single monitor; prefer `OutputMatcher::snapshot()` for a pass.
pub fn matching_config<'a>(config: &'a Config, monitor: &MonitorInfo) -> Option<&'a OutputConfig> {
    // A connector match needs no OS queries.
    if let Some(out) = config.output.iter().find(|out| same_name(&out.name, &monitor.name)) {
        return Some(out);
    }
    OutputMatcher::snapshot().matching_config(config, monitor)
}

fn matching_identity<'a>(config: &'a Config, identity: &OutputIdentity) -> Option<&'a OutputConfig> {
    config.output.iter().find(|out| same_name(&out.name, &identity.connector))
        .or_else(|| config.output.iter().find(|out| identity.matches(&out.name)))
        .or_else(|| config.output.iter().find(|out| out.name.trim().is_empty()))
}

/// Effective scale for wiri's layout only. Zero/unset never overrides Windows DPI.
pub fn effective_scale(output: Option<&OutputConfig>, monitor: &MonitorInfo) -> f64 {
    output.map(|out| out.scale).filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(monitor.scale_factor)
}

/// Counter-clockwise, matching niri's output transform rather than Win32's
/// clockwise DISPLAYCONFIG_ROTATION convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
}

impl Rotation {
    fn swaps_axes(self) -> bool {
        matches!(self, Self::Rotate90 | Self::Rotate270)
    }

    fn from_windows(rotation: ccd::DISPLAYCONFIG_ROTATION) -> Option<Self> {
        match rotation {
            ccd::DISPLAYCONFIG_ROTATION_IDENTITY => Some(Self::Normal),
            ccd::DISPLAYCONFIG_ROTATION_ROTATE90 => Some(Self::Rotate270),
            ccd::DISPLAYCONFIG_ROTATION_ROTATE180 => Some(Self::Rotate180),
            ccd::DISPLAYCONFIG_ROTATION_ROTATE270 => Some(Self::Rotate90),
            _ => None,
        }
    }

    fn as_windows(self) -> ccd::DISPLAYCONFIG_ROTATION {
        match self {
            Self::Normal => ccd::DISPLAYCONFIG_ROTATION_IDENTITY,
            Self::Rotate90 => ccd::DISPLAYCONFIG_ROTATION_ROTATE270,
            Self::Rotate180 => ccd::DISPLAYCONFIG_ROTATION_ROTATE180,
            Self::Rotate270 => ccd::DISPLAYCONFIG_ROTATION_ROTATE90,
        }
    }
}

/// Dimensions are unrotated source pixels, refresh is millihertz, not rounded Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputMode {
    pub width: u32,
    pub height: u32,
    pub refresh_millihz: Option<u32>,
}

/// Parse `1920x1080`, `1920x1080@60`, or `1920x1080@59.940` without float rounding.
/// Extra fractional precision is accepted only when it is zero beyond millihertz.
pub fn parse_mode(value: &str) -> Result<OutputMode, String> {
    let value = value.trim();
    let (size, refresh) = match value.split_once('@') {
        Some((size, refresh)) => (size, Some(parse_refresh(refresh)?)),
        None => (value, None),
    };
    let (width, height) = size.split_once(['x', 'X'])
        .ok_or_else(|| format!("invalid output mode '{value}'; expected WIDTHxHEIGHT[@HZ]"))?;
    let positive = |text: &str| -> Option<u32> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse::<u32>().ok().filter(|n| *n > 0 && *n <= 100_000)
    };
    Ok(OutputMode {
        width: positive(width).ok_or_else(|| format!("invalid mode width '{width}'"))?,
        height: positive(height).ok_or_else(|| format!("invalid mode height '{height}'"))?,
        refresh_millihz: refresh,
    })
}

fn parse_refresh(value: &str) -> Result<u32, String> {
    let bad = || format!("invalid refresh rate '{value}'; use positive Hz with millihertz precision");
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (value.contains('.') && fraction.is_empty())
        || fraction.bytes().skip(3).any(|b| b != b'0')
    {
        return Err(bad());
    }
    let whole: u32 = whole.parse().map_err(|_| bad())?;
    let mut fractional = 0u32;
    for index in 0..3 {
        fractional = fractional * 10 + fraction.as_bytes().get(index).map_or(0, |b| (b - b'0') as u32);
    }
    whole.checked_mul(1000).and_then(|n| n.checked_add(fractional))
        .filter(|n| *n > 0).ok_or_else(bad)
}

/// A pure snapshot used by the planner, independently of the Win32 executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputState {
    pub mode: OutputMode,
    pub position: (i32, i32),
    pub rotation: Rotation,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputPlan {
    pub disable: bool,
    /// Present only when an explicitly requested mode differs from the current one.
    pub mode: Option<OutputMode>,
    pub position: Option<(i32, i32)>,
    pub rotation: Option<Rotation>,
    pub layout_scale: Option<f64>,
    pub focus_at_startup: bool,
    pub warnings: Vec<String>,
}

impl OutputPlan {
    pub fn changes_windows(&self) -> bool {
        self.disable || self.mode.is_some() || self.position.is_some() || self.rotation.is_some()
    }

    fn descriptions(&self) -> Vec<String> {
        let mut changes = Vec::new();
        if self.disable { changes.push("off (detach from the Windows desktop)".to_string()); }
        if let Some(mode) = self.mode {
            changes.push(match mode.refresh_millihz {
                Some(refresh) => format!("mode {}x{}@{}.{:03}", mode.width, mode.height, refresh / 1000, refresh % 1000),
                None => format!("mode {}x{} (preserve refresh)", mode.width, mode.height),
            });
        }
        if let Some((x, y)) = self.position { changes.push(format!("position {x},{y}")); }
        if let Some(rotation) = self.rotation { changes.push(format!("transform {rotation:?}")); }
        if let Some(scale) = self.layout_scale { changes.push(format!("layout scale {scale} (Windows DPI unchanged)")); }
        if self.focus_at_startup { changes.push("initial wiri focus (Windows primary unchanged)".to_string()); }
        changes
    }
}

/// Pure: constructing a plan neither reads nor changes the machine's display state.
/// Invalid/unsupported transforms fail closed instead of partially applying them.
pub fn plan_output(config: &OutputConfig, current: &OutputState) -> Result<OutputPlan, String> {
    if !config.enable {
        return Ok(OutputPlan { disable: true, ..OutputPlan::default() });
    }
    if !config.scale.is_finite() || config.scale < 0.0 {
        return Err("output scale must be finite and non-negative (0 means unset)".to_string());
    }
    let mut plan = OutputPlan {
        layout_scale: (config.scale > 0.0).then_some(config.scale),
        focus_at_startup: config.focus_at_startup || config.primary,
        ..OutputPlan::default()
    };
    if config.vrr {
        plan.warnings.push("per-output VRR is controlled by Windows/the GPU driver; ignored".to_string());
    }
    let rotation = match config.transform.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "normal" => Some(Rotation::Normal),
        "90" => Some(Rotation::Rotate90),
        "180" => Some(Rotation::Rotate180),
        "270" => Some(Rotation::Rotate270),
        transform if transform.starts_with("flipped") => {
            return Err(format!("transform '{transform}' is unsupported on Windows; no physical changes made"));
        }
        transform => return Err(format!("unknown output transform '{transform}'")),
    };
    plan.rotation = rotation.filter(|rotation| *rotation != current.rotation);
    if config.position_set && (config.position.x, config.position.y) != current.position {
        plan.position = Some((config.position.x, config.position.y));
    }
    let requested = if !config.mode.trim().is_empty() {
        let mode = parse_mode(&config.mode)?;
        if (config.width != 0 && config.width != mode.width)
            || (config.height != 0 && config.height != mode.height)
        {
            return Err("output width/height conflict with the explicit mode".to_string());
        }
        Some(mode)
    } else if config.width != 0 || config.height != 0 {
        if config.width == 0 || config.height == 0 || config.width > 100_000 || config.height > 100_000 {
            return Err("an explicit output resolution needs both positive width and height".to_string());
        }
        Some(OutputMode { width: config.width, height: config.height, refresh_millihz: None })
    } else {
        None
    };
    if let Some(mode) = requested {
        // Unknown refresh is not permission to reset a display to a guessed rate.
        if mode.refresh_millihz.is_some() && current.mode.refresh_millihz.is_none() {
            return Err("cannot compare the requested refresh with the current Windows timing".to_string());
        }
        if mode.width != current.mode.width || mode.height != current.mode.height
            || mode.refresh_millihz.is_some_and(|refresh| Some(refresh) != current.mode.refresh_millihz)
        {
            plan.mode = Some(mode);
        }
    }
    Ok(plan)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStatus {
    Unchanged,
    Applied,
    Unmatched,
    Failed,
}

#[derive(Debug, Clone)]
pub struct OutputReport {
    pub config_name: String,
    pub connector: Option<String>,
    pub status: OutputStatus,
    /// Includes layout-only requests, explicitly labelled as such.
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

/// Apply explicit physical differences as one validated CCD transaction.
///
/// Unconfigured paths/timings are retained verbatim. No ALLOW_CHANGES, automatic
/// repositioning, DPI setter, primary-display setter, or integer-Hz fallback is
/// used. If the driver cannot honour the supplied configuration, it is reported
/// instead of retrying with a different mode. Detaching the last display is refused.
/// Inactive outputs are not automatically enabled: their position/timing cannot
/// be reconstructed from an active snapshot without guessing. Use Windows Display
/// Settings to reconnect one, after which its block will be applied normally.
/// Startup-focus and scale are reported here and consumed by the layout caller.
pub fn apply(config: &Config) -> Vec<OutputReport> {
    let topology = match Topology::query() {
        Ok(topology) => topology,
        Err(error) => {
            warn!(%error, "Output configuration left unchanged");
            return config.output.iter().map(|out| OutputReport {
                config_name: out.name.clone(), connector: None, status: OutputStatus::Failed,
                changes: Vec::new(), warnings: Vec::new(), error: Some(error.clone()),
            }).collect();
        }
    };
    let mut reports = Vec::new();
    let mut planned = Vec::new();
    for (index, identity) in topology.identities.iter().enumerate() {
        let Some(out) = matching_identity(config, identity) else { continue; };
        let mut report = OutputReport {
            config_name: out.name.clone(), connector: Some(identity.connector.clone()),
            status: OutputStatus::Unchanged, changes: Vec::new(), warnings: Vec::new(), error: None,
        };
        match topology.state(index).and_then(|current| plan_output(out, &current)) {
            Ok(plan) => {
                report.changes = plan.descriptions();
                report.warnings = plan.warnings.clone();
                if plan.changes_windows() {
                    planned.push((index, plan, reports.len()));
                }
            }
            Err(error) => { report.status = OutputStatus::Failed; report.error = Some(error); }
        }
        reports.push(report);
    }
    for out in &config.output {
        if !out.name.trim().is_empty() && !topology.identities.iter().any(|identity| identity.matches(&out.name)) {
            reports.push(OutputReport {
                config_name: out.name.clone(), connector: None, status: OutputStatus::Unmatched,
                changes: Vec::new(), warnings: Vec::new(),
                error: Some("no active Windows output matched (inactive outputs are not enabled automatically)".to_string()),
            });
        }
    }
    // This is deliberately outside the executor: no-op requests do not even call
    // SetDisplayConfig(SDC_VALIDATE). It also prevents WM_DISPLAYCHANGE loops.
    if !planned.is_empty() {
        let result = build_update(&topology.paths, &topology.modes, &planned)
            .and_then(|(paths, modes)| execute_update(&paths, &modes));
        for (_, _, report_index) in planned {
            match &result {
                Ok(()) => reports[report_index].status = OutputStatus::Applied,
                Err(error) => {
                    reports[report_index].status = OutputStatus::Failed;
                    reports[report_index].error = Some(error.clone());
                }
            }
        }
    }
    for report in &reports {
        if let Some(error) = &report.error {
            warn!(output = %report.config_name, connector = ?report.connector, %error, "Output configuration not applied");
        } else {
            info!(output = %report.config_name, connector = ?report.connector, status = ?report.status,
                changes = ?report.changes, "Output configuration");
        }
        for warning in &report.warnings {
            warn!(output = %report.config_name, %warning, "Output configuration warning");
        }
    }
    reports
}

struct Topology {
    paths: Vec<ccd::DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<ccd::DISPLAYCONFIG_MODE_INFO>,
    identities: Vec<OutputIdentity>,
}

impl Topology {
    fn query() -> Result<Self, String> {
        // No VIRTUAL_MODE_AWARE flag: the ordinary modeInfoIdx union members
        // below are valid. Bounded retry handles a hotplug during enumeration.
        for _ in 0..3 {
            let mut path_count = 0;
            let mut mode_count = 0;
            let error = unsafe { ccd::GetDisplayConfigBufferSizes(ccd::QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) };
            if error.0 != 0 { return Err(format!("GetDisplayConfigBufferSizes failed: {}", error.0)); }
            if path_count > 4096 || mode_count > 16384 {
                return Err("Windows returned unreasonable display configuration buffer sizes".to_string());
            }
            let mut paths = vec![ccd::DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![ccd::DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            let error = unsafe {
                ccd::QueryDisplayConfig(ccd::QDC_ONLY_ACTIVE_PATHS, &mut path_count, paths.as_mut_ptr(),
                    &mut mode_count, modes.as_mut_ptr(), None)
            };
            if error.0 == 122 { continue; } // ERROR_INSUFFICIENT_BUFFER
            if error.0 != 0 { return Err(format!("QueryDisplayConfig failed: {}", error.0)); }
            if path_count as usize > paths.len() || mode_count as usize > modes.len() {
                return Err("Windows returned inconsistent display configuration counts".to_string());
            }
            paths.truncate(path_count as usize);
            modes.truncate(mode_count as usize);
            let identities = paths.iter().map(read_identity).collect();
            return Ok(Self { paths, modes, identities });
        }
        Err("display topology kept changing while it was being queried".to_string())
    }

    fn state(&self, index: usize) -> Result<OutputState, String> {
        let path = self.paths.get(index).ok_or("output path disappeared")?;
        let (_, source) = source_mode(path, &self.modes)?;
        let rotation = Rotation::from_windows(path.targetInfo.rotation)
            .ok_or("Windows returned an unknown output rotation")?;
        let (width, height) = if rotation.swaps_axes() { (source.height, source.width) } else { (source.width, source.height) };
        Ok(OutputState {
            mode: OutputMode { width, height, refresh_millihz: current_refresh(path, &self.modes) },
            position: (source.position.x, source.position.y), rotation,
        })
    }
}

fn source_mode(path: &ccd::DISPLAYCONFIG_PATH_INFO, modes: &[ccd::DISPLAYCONFIG_MODE_INFO])
    -> Result<(usize, ccd::DISPLAYCONFIG_SOURCE_MODE), String>
{
    // SAFETY: queries/updates never use virtual-mode-aware index bitfields.
    let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
    let mode = modes.get(index).ok_or("output has no current source mode; left unchanged")?;
    if mode.infoType != ccd::DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE || mode.id != path.sourceInfo.id
        || mode.adapterId != path.sourceInfo.adapterId
    {
        return Err("Windows source mode does not belong to the output path".to_string());
    }
    // SAFETY: infoType was checked before reading this union member.
    Ok((index, unsafe { mode.Anonymous.sourceMode }))
}

fn millihertz(rate: ccd::DISPLAYCONFIG_RATIONAL) -> Option<u32> {
    if rate.Numerator == 0 || rate.Denominator == 0 { return None; }
    let value = (u64::from(rate.Numerator) * 1000 + u64::from(rate.Denominator) / 2) / u64::from(rate.Denominator);
    u32::try_from(value).ok().filter(|n| *n > 0)
}

fn target_timing(path: &ccd::DISPLAYCONFIG_PATH_INFO, modes: &[ccd::DISPLAYCONFIG_MODE_INFO])
    -> Option<ccd::DISPLAYCONFIG_VIDEO_SIGNAL_INFO>
{
    let index = unsafe { path.targetInfo.Anonymous.modeInfoIdx } as usize;
    let mode = modes.get(index).filter(|mode| {
        mode.infoType == ccd::DISPLAYCONFIG_MODE_INFO_TYPE_TARGET && mode.id == path.targetInfo.id
            && mode.adapterId == path.targetInfo.adapterId
    })?;
    // SAFETY: infoType and the owning target were checked above.
    Some(unsafe { mode.Anonymous.targetMode.targetVideoSignalInfo })
}

fn current_refresh_rate(path: &ccd::DISPLAYCONFIG_PATH_INFO, modes: &[ccd::DISPLAYCONFIG_MODE_INFO])
    -> Option<ccd::DISPLAYCONFIG_RATIONAL>
{
    // Supplied target timing overrides path.refreshRate, which may even be 0/0.
    // Keep its exact rational when discarding a target timing for a mode change.
    target_timing(path, modes).map(|timing| timing.vSyncFreq)
        .filter(|rate| millihertz(*rate).is_some())
        .or_else(|| millihertz(path.targetInfo.refreshRate).map(|_| path.targetInfo.refreshRate))
}

fn current_refresh(path: &ccd::DISPLAYCONFIG_PATH_INFO, modes: &[ccd::DISPLAYCONFIG_MODE_INFO]) -> Option<u32> {
    current_refresh_rate(path, modes).and_then(millihertz)
}

/// Pure preparation: all other paths, source modes and target timings survive.
fn build_update(
    old_paths: &[ccd::DISPLAYCONFIG_PATH_INFO],
    old_modes: &[ccd::DISPLAYCONFIG_MODE_INFO],
    planned: &[(usize, OutputPlan, usize)],
) -> Result<(Vec<ccd::DISPLAYCONFIG_PATH_INFO>, Vec<ccd::DISPLAYCONFIG_MODE_INFO>), String> {
    let mut paths = old_paths.to_vec();
    let mut modes = old_modes.to_vec();
    let mut disabled = vec![false; paths.len()];
    for (index, plan, _) in planned {
        let path = paths.get_mut(*index).ok_or("output path disappeared before preparation")?;
        if plan.disable { disabled[*index] = true; continue; }
        if !plan.changes_windows() { continue; }
        // A cloned source is shared desktop state. Changing it could silently
        // alter a monitor without a block, so fail closed for that transaction.
        if old_paths.iter().filter(|other| {
            other.sourceInfo.id == path.sourceInfo.id && other.sourceInfo.adapterId == path.sourceInfo.adapterId
        }).count() > 1 {
            return Err("mode/position/rotation changes on cloned outputs are unsupported; Windows left unchanged".to_string());
        }
        let (mode_index, mut source) = source_mode(path, &modes)?;
        let old_rotation = Rotation::from_windows(path.targetInfo.rotation).ok_or("unknown current output rotation")?;
        let rotation = plan.rotation.unwrap_or(old_rotation);
        let (mut width, mut height) = if old_rotation.swaps_axes() { (source.height, source.width) } else { (source.width, source.height) };
        if let Some(mode) = plan.mode {
            width = mode.width;
            height = mode.height;
            let old_rate = current_refresh_rate(path, old_modes)
                .ok_or("cannot preserve the current refresh while changing output dimensions")?;
            path.targetInfo.refreshRate = match mode.refresh_millihz {
                Some(refresh) if Some(refresh) != millihertz(old_rate) => {
                    ccd::DISPLAYCONFIG_RATIONAL { Numerator: refresh, Denominator: 1000 }
                }
                _ => old_rate,
            };
            if let Some(timing) = target_timing(path, old_modes) {
                path.targetInfo.scanLineOrdering = timing.scanLineOrdering;
            }
            // Do not fabricate pixel clocks/blanking. Windows selects a compatible
            // target timing for this source and exact path refresh, or rejects it.
            // In particular never submit 0/0 (Windows' "choose optimal" request).
            path.targetInfo.Anonymous.modeInfoIdx = u32::MAX;
        }
        (source.width, source.height) = if rotation.swaps_axes() { (height, width) } else { (width, height) };
        if let Some((x, y)) = plan.position { source.position.x = x; source.position.y = y; }
        if plan.rotation.is_some() { path.targetInfo.rotation = rotation.as_windows(); }
        modes[mode_index].Anonymous.sourceMode = source;
    }
    let paths: Vec<_> = paths.into_iter().enumerate().filter_map(|(index, path)| (!disabled[index]).then_some(path)).collect();
    if paths.is_empty() { return Err("refusing to detach every active display; Windows left unchanged".to_string()); }
    Ok((paths, modes))
}

// A test binary cannot execute a mutating display API, even accidentally. All unit
// tests below exercise parsing/matching/planning/preparation using synthetic data.
#[cfg(not(test))]
fn execute_update(paths: &[ccd::DISPLAYCONFIG_PATH_INFO], modes: &[ccd::DISPLAYCONFIG_MODE_INFO]) -> Result<(), String> {
    let flags = ccd::SDC_USE_SUPPLIED_DISPLAY_CONFIG;
    let error = unsafe { ccd::SetDisplayConfig(Some(paths), Some(modes), flags | ccd::SDC_VALIDATE) };
    if error != 0 { return Err(format!("Windows rejected the requested output configuration (validation error {error}); nothing applied")); }
    let error = unsafe { ccd::SetDisplayConfig(Some(paths), Some(modes), flags | ccd::SDC_APPLY | ccd::SDC_SAVE_TO_DATABASE) };
    if error != 0 { return Err(format!("SetDisplayConfig failed with error {error}")); }
    Ok(())
}

#[cfg(test)]
fn execute_update(_: &[ccd::DISPLAYCONFIG_PATH_INFO], _: &[ccd::DISPLAYCONFIG_MODE_INFO]) -> Result<(), String> {
    Err("physical output changes are disabled in unit tests".to_string())
}

fn wide_string(units: &[u16]) -> String {
    String::from_utf16_lossy(&units[..units.iter().position(|c| *c == 0).unwrap_or(units.len())]).trim().to_string()
}

fn read_identity(path: &ccd::DISPLAYCONFIG_PATH_INFO) -> OutputIdentity {
    let mut identity = OutputIdentity::default();
    let mut source = ccd::DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    source.header.r#type = ccd::DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    source.header.size = std::mem::size_of_val(&source) as u32;
    source.header.adapterId = path.sourceInfo.adapterId;
    source.header.id = path.sourceInfo.id;
    let error = unsafe { ccd::DisplayConfigGetDeviceInfo(&mut source.header) };
    if error == 0 { identity.connector = wide_string(&source.viewGdiDeviceName); }
    else { warn!(error, "Could not read output connector name"); }
    let mut target = ccd::DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    target.header.r#type = ccd::DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
    target.header.size = std::mem::size_of_val(&target) as u32;
    target.header.adapterId = path.targetInfo.adapterId;
    target.header.id = path.targetInfo.id;
    let error = unsafe { ccd::DisplayConfigGetDeviceInfo(&mut target.header) };
    if error != 0 {
        warn!(connector = %identity.connector, error, "Could not read output monitor identity");
        return identity;
    }
    identity.friendly_name = wide_string(&target.monitorFriendlyDeviceName);
    identity.device_path = wide_string(&target.monitorDevicePath);
    let edid = read_edid(&identity.device_path).and_then(|bytes| parse_edid(&bytes));
    if let Some(edid) = edid {
        identity.make = edid.make;
        identity.model = edid.model;
        identity.serial = edid.serial;
    }
    if identity.make.is_empty() {
        // The hardware ID in the CCD device path has an unambiguous PNP spelling.
        identity.make = pnp_from_device_path(&identity.device_path)
            .unwrap_or_else(|| pnp_manufacturer(target.edidManufactureId).unwrap_or_default());
    }
    if identity.model.is_empty() {
        identity.model = if identity.friendly_name.is_empty() { format!("{:04X}", target.edidProductCodeId) } else { identity.friendly_name.clone() };
    }
    info!(connector = %identity.connector, make = %identity.make, model = %identity.model,
        serial = %identity.serial, friendly_name = %identity.friendly_name, "Output identity");
    identity
}

fn pnp_from_device_path(path: &str) -> Option<String> {
    let hardware = path.split('#').nth(1)?;
    let prefix = hardware.get(..3)?;
    prefix.bytes().all(|b| b.is_ascii_alphabetic()).then(|| prefix.to_ascii_uppercase())
}

/// Map only a CCD monitor interface path, not an arbitrary registry path.
fn edid_registry_key(path: &str) -> Option<String> {
    let mut parts = path.split('#');
    if !parts.next()?.eq_ignore_ascii_case(r"\\?\DISPLAY") { return None; }
    let hardware = parts.next()?;
    let instance = parts.next()?;
    let valid = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'&' | b'_' | b'-' | b'.'))
        && part != "." && part != "..";
    if !valid(hardware) || !valid(instance) { return None; }
    Some(format!(r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{hardware}\{instance}\Device Parameters"))
}

fn read_edid(device_path: &str) -> Option<Vec<u8>> {
    let key = edid_registry_key(device_path)?;
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    // A single bounded, read-only registry query. Serial lives in the base block;
    // extensions need not be parsed or trusted for output matching.
    let mut bytes = vec![0u8; 4096];
    let mut size = bytes.len() as u32;
    let error = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), w!("EDID"), RRF_RT_REG_BINARY,
            None, Some(bytes.as_mut_ptr().cast()), Some(&mut size))
    };
    if error.0 != 0 || size as usize > bytes.len() { return None; }
    bytes.truncate(size as usize);
    Some(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EdidIdentity { make: String, model: String, serial: String }

fn pnp_manufacturer(value: u16) -> Option<String> {
    let letters = [(value >> 10) & 31, (value >> 5) & 31, value & 31];
    if letters.iter().any(|letter| !(1..=26).contains(letter)) { return None; }
    Some(letters.into_iter().map(|letter| char::from(b'A' + letter as u8 - 1)).collect())
}

fn parse_edid(bytes: &[u8]) -> Option<EdidIdentity> {
    let base = bytes.get(..128)?;
    if base[..8] != [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]
        || base.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0
    { return None; }
    let make = pnp_manufacturer(u16::from_be_bytes([base[8], base[9]]))?;
    let mut model = String::new();
    let mut serial = String::new();
    for descriptor in base[54..126].chunks_exact(18) {
        if descriptor[..3] != [0, 0, 0] || descriptor[4] != 0 { continue; }
        let text = &descriptor[5..18];
        let end = text.iter().position(|byte| matches!(*byte, 0 | b'\n' | b'\r')).unwrap_or(text.len());
        let text = &text[..end];
        if !text.iter().all(|byte| (0x20..=0x7e).contains(byte)) { continue; }
        let text = std::str::from_utf8(text).ok()?.trim().to_string();
        match descriptor[3] { 0xfc => model = text, 0xff => serial = text, _ => {} }
    }
    if serial.is_empty() {
        let number = u32::from_le_bytes(base[12..16].try_into().ok()?);
        if number != 0 && number != u32::MAX { serial = number.to_string(); }
    }
    Some(EdidIdentity { make, model, serial })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::{OutputId, Rect};

    fn output() -> OutputConfig { Config::default().output.remove(0) }

    fn current() -> OutputState {
        OutputState {
            mode: OutputMode { width: 1920, height: 1080, refresh_millihz: Some(59_940) },
            position: (1920, -40), rotation: Rotation::Normal,
        }
    }

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: OutputId::from_name(r"\\.\DISPLAY1"), name: r"\\.\DISPLAY1".to_string(),
            bounds: Rect::new(1920, -40, 1920, 1080), work_area: Rect::new(1920, -40, 1920, 1040),
            is_primary: false, scale_factor: 1.5,
        }
    }

    fn identity() -> OutputIdentity {
        OutputIdentity {
            connector: r"\\.\DISPLAY1".to_string(), make: "DEL".to_string(),
            model: "U2720Q".to_string(), serial: "ABC123".to_string(),
            friendly_name: "DELL U2720Q".to_string(), device_path: String::new(),
        }
    }

    #[test]
    fn matching_is_case_insensitive_and_exact() {
        let id = identity();
        for name in [r"\\.\display1", "del u2720q abc123", "Dell Inc. U2720Q ABC123", "dell u2720q", " DEL  U2720Q  ABC123 "] {
            assert!(id.matches(name), "{name}");
        }
        for name in ["", "Primary", "DISPLAY1", r"\\.\DISPLAY10", "DEL U2720Q ABC12", "DEL U2720Q OTHER"] {
            assert!(!id.matches(name), "{name}");
        }
    }

    #[test]
    fn connector_precedes_identity_and_named_mismatch_has_no_index_fallback() {
        let mut config = Config::default();
        let mut by_model = output();
        by_model.name = "del u2720q abc123".to_string();
        by_model.scale = 2.0;
        let mut by_connector = output();
        by_connector.name = r"\\.\display1".to_string();
        by_connector.scale = 1.25;
        config.output = vec![by_model, by_connector];
        let matcher = OutputMatcher { identities: vec![identity()] };
        assert_eq!(matcher.matching_config(&config, &monitor()).unwrap().scale, 1.25);
        config.output.pop();
        assert_eq!(matcher.matching_config(&config, &monitor()).unwrap().scale, 2.0);
        config.output[0].name = r"\\.\DISPLAY2".to_string();
        assert!(matcher.matching_config(&config, &monitor()).is_none());
        config.output.push(output());
        assert!(matcher.matching_config(&config, &monitor()).unwrap().name.is_empty());
    }

    #[test]
    fn defaults_and_unset_fields_never_plan_windows_changes() {
        let config = output();
        let plan = plan_output(&config, &current()).unwrap();
        assert!(!plan.changes_windows());
        assert_eq!(plan.position, None);
        assert_eq!(plan.rotation, None);
        assert_eq!(plan.mode, None);
        assert_eq!(effective_scale(Some(&config), &monitor()), 1.5);
        assert_eq!(effective_scale(None, &monitor()), 1.5);
    }

    #[test]
    fn layout_scale_and_startup_focus_do_not_change_windows() {
        let mut config = output();
        config.scale = 1.75;
        config.focus_at_startup = true;
        let plan = plan_output(&config, &current()).unwrap();
        assert!(!plan.changes_windows());
        assert_eq!(plan.layout_scale, Some(1.75));
        assert!(plan.focus_at_startup);
        assert_eq!(effective_scale(Some(&config), &monitor()), 1.75);
    }

    #[test]
    fn explicit_current_mode_position_and_rotation_are_a_noop() {
        let mut config = output();
        config.mode = "1920x1080@59.940".to_string();
        config.transform = "normal".to_string();
        config.position_set = true;
        config.position.x = 1920;
        config.position.y = -40;
        assert!(!plan_output(&config, &current()).unwrap().changes_windows());
        config.mode = "1920x1080".to_string();
        assert!(!plan_output(&config, &current()).unwrap().changes_windows());
        config.mode.clear();
        config.width = 1920;
        config.height = 1080;
        assert!(!plan_output(&config, &current()).unwrap().changes_windows());
    }

    #[test]
    fn mode_refresh_comparison_keeps_millihertz_precision() {
        let mut config = output();
        config.mode = "1920x1080@60".to_string();
        assert_eq!(plan_output(&config, &current()).unwrap().mode.unwrap().refresh_millihz, Some(60_000));
        config.mode = "1920x1080@59.941".to_string();
        assert!(plan_output(&config, &current()).unwrap().changes_windows());
        assert_eq!(millihertz(ccd::DISPLAYCONFIG_RATIONAL { Numerator: 60_000, Denominator: 1001 }), Some(59_940));
        assert_eq!(millihertz(ccd::DISPLAYCONFIG_RATIONAL { Numerator: 60, Denominator: 0 }), None);
    }

    #[test]
    fn mode_parser_is_strict_and_does_not_guess() {
        assert_eq!(parse_mode(" 2560X1440@59.9400 ").unwrap(), OutputMode { width: 2560, height: 1440, refresh_millihz: Some(59_940) });
        assert_eq!(parse_mode("1920x1080@60.5").unwrap().refresh_millihz, Some(60_500));
        assert_eq!(parse_mode("1920x1080@0.001").unwrap().refresh_millihz, Some(1));
        for bad in ["", "auto", "1920", "0x1080", "1920x0", "-1x1080", "1920x1080@", "1920x1080@0", "1920x1080@NaN", "1920x1080@60.0001", "1920x1080@60@60", "1920x1080@60.", "1920x1080@1e2", "1920x1080@4294967296", "100001x1080"] {
            assert!(parse_mode(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_explicit_position_can_move_an_output() {
        let mut config = output();
        config.position.x = -2560;
        config.position.y = 90;
        assert_eq!(plan_output(&config, &current()).unwrap().position, None);
        config.position_set = true;
        assert_eq!(plan_output(&config, &current()).unwrap().position, Some((-2560, 90)));
    }

    #[test]
    fn rotations_compare_without_resetting_unset_and_flips_fail_closed() {
        let mut config = output();
        let mut state = current();
        state.rotation = Rotation::Rotate90;
        assert!(!plan_output(&config, &state).unwrap().changes_windows());
        config.transform = "90".to_string();
        assert!(!plan_output(&config, &state).unwrap().changes_windows());
        config.transform = "normal".to_string();
        assert_eq!(plan_output(&config, &state).unwrap().rotation, Some(Rotation::Normal));
        for bad in ["flipped", "flipped-90", "flipped-180", "flipped-270", "diagonal"] {
            config.transform = bad.to_string();
            assert!(plan_output(&config, &state).is_err());
        }
    }

    #[test]
    fn niri_counter_clockwise_rotation_maps_to_win32_clockwise_rotation() {
        assert_eq!(Rotation::Rotate90.as_windows(), ccd::DISPLAYCONFIG_ROTATION_ROTATE270);
        assert_eq!(Rotation::Rotate270.as_windows(), ccd::DISPLAYCONFIG_ROTATION_ROTATE90);
        for rotation in [Rotation::Normal, Rotation::Rotate90, Rotation::Rotate180, Rotation::Rotate270] {
            assert_eq!(Rotation::from_windows(rotation.as_windows()), Some(rotation));
        }
    }

    #[test]
    fn off_is_physical_but_does_not_validate_irrelevant_mode_or_scale() {
        let mut config = output();
        config.enable = false;
        config.mode = "invalid".to_string();
        config.scale = f64::NAN;
        config.focus_at_startup = true;
        let plan = plan_output(&config, &current()).unwrap();
        assert!(plan.disable && plan.changes_windows());
        assert!(!plan.focus_at_startup);
        assert_eq!(plan.mode, None);
    }

    #[test]
    fn invalid_requests_do_not_become_guessed_physical_changes() {
        let mut config = output();
        config.width = 1920;
        assert!(plan_output(&config, &current()).is_err());
        config.height = 1080;
        config.mode = "2560x1440@60".to_string();
        assert!(plan_output(&config, &current()).is_err());
        config.width = 0;
        config.height = 0;
        let mut unknown_rate = current();
        unknown_rate.mode.refresh_millihz = None;
        assert!(plan_output(&config, &unknown_rate).is_err());
        config.mode.clear();
        for scale in [f64::NAN, f64::INFINITY, -1.0] {
            config.scale = scale;
            assert!(plan_output(&config, &current()).is_err());
            assert_eq!(effective_scale(Some(&config), &monitor()), 1.5);
        }
    }

    #[test]
    fn unsupported_vrr_is_reported_without_mutation() {
        let mut config = output();
        config.vrr = true;
        let plan = plan_output(&config, &current()).unwrap();
        assert!(!plan.changes_windows());
        assert_eq!(plan.warnings.len(), 1);
    }

    fn synthetic_topology() -> Topology {
        let mut paths = Vec::new();
        let mut modes = Vec::new();
        for index in 0..2u32 {
            let mut path = ccd::DISPLAYCONFIG_PATH_INFO::default();
            path.flags = 1; // DISPLAYCONFIG_PATH_ACTIVE
            path.sourceInfo.id = index;
            path.sourceInfo.Anonymous.modeInfoIdx = index * 2;
            path.targetInfo.id = index + 10;
            path.targetInfo.Anonymous.modeInfoIdx = index * 2 + 1;
            path.targetInfo.rotation = ccd::DISPLAYCONFIG_ROTATION_IDENTITY;
            path.targetInfo.refreshRate = ccd::DISPLAYCONFIG_RATIONAL { Numerator: 60_000, Denominator: 1001 };
            let mut source = ccd::DISPLAYCONFIG_MODE_INFO::default();
            source.id = index;
            source.infoType = ccd::DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
            source.Anonymous.sourceMode = ccd::DISPLAYCONFIG_SOURCE_MODE {
                width: 1920, height: 1080, pixelFormat: ccd::DISPLAYCONFIG_PIXELFORMAT_32BPP,
                position: windows::Win32::Foundation::POINTL { x: index as i32 * 1920, y: 0 },
            };
            let mut target = ccd::DISPLAYCONFIG_MODE_INFO::default();
            target.id = index + 10;
            target.infoType = ccd::DISPLAYCONFIG_MODE_INFO_TYPE_TARGET;
            let mut timing = ccd::DISPLAYCONFIG_TARGET_MODE::default();
            timing.targetVideoSignalInfo.vSyncFreq = path.targetInfo.refreshRate;
            timing.targetVideoSignalInfo.activeSize = ccd::DISPLAYCONFIG_2DREGION { cx: 1920, cy: 1080 };
            target.Anonymous.targetMode = timing;
            modes.push(source);
            modes.push(target);
            paths.push(path);
        }
        Topology { paths, modes, identities: Vec::new() }
    }

    #[test]
    fn pure_preparation_preserves_unconfigured_output_and_fractional_timing() {
        let topology = synthetic_topology();
        let mut config = output();
        config.mode = "2560x1440".to_string();
        let plan = plan_output(&config, &topology.state(1).unwrap()).unwrap();
        let (paths, modes) = build_update(&topology.paths, &topology.modes, &[(1, plan, 0)]).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].targetInfo.refreshRate, topology.paths[0].targetInfo.refreshRate);
        assert_eq!(unsafe { paths[0].targetInfo.Anonymous.modeInfoIdx }, 1);
        assert_eq!(unsafe { modes[0].Anonymous.sourceMode }, unsafe { topology.modes[0].Anonymous.sourceMode });
        assert_eq!(paths[1].targetInfo.refreshRate, topology.paths[1].targetInfo.refreshRate);
        assert_eq!(unsafe { paths[1].targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
        let source = unsafe { modes[2].Anonymous.sourceMode };
        assert_eq!((source.width, source.height, source.position.x, source.position.y), (2560, 1440, 1920, 0));
        assert_eq!(unsafe { modes[1].Anonymous.targetMode.targetVideoSignalInfo.vSyncFreq }, topology.paths[0].targetInfo.refreshRate);
    }

    #[test]
    fn replacing_target_timing_preserves_its_effective_rational_not_stale_path_rate() {
        let mut topology = synthetic_topology();
        let effective = topology.paths[1].targetInfo.refreshRate;
        topology.paths[1].targetInfo.refreshRate = ccd::DISPLAYCONFIG_RATIONAL { Numerator: 60, Denominator: 1 };
        let mut timing = unsafe { topology.modes[3].Anonymous.targetMode };
        timing.targetVideoSignalInfo.scanLineOrdering = ccd::DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
        topology.modes[3].Anonymous.targetMode = timing;
        for requested in ["2560x1440", "2560x1440@59.940"] {
            let mut config = output();
            config.mode = requested.to_string();
            let plan = plan_output(&config, &topology.state(1).unwrap()).unwrap();
            let (paths, _) = build_update(&topology.paths, &topology.modes, &[(1, plan, 0)]).unwrap();
            assert_eq!(paths[1].targetInfo.refreshRate, effective);
            assert_eq!(paths[1].targetInfo.scanLineOrdering, ccd::DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE);
            assert_eq!(unsafe { paths[1].targetInfo.Anonymous.modeInfoIdx }, u32::MAX);
        }
        topology.paths[1].targetInfo.refreshRate = ccd::DISPLAYCONFIG_RATIONAL::default();
        let mut config = output();
        config.mode = "2560x1440".to_string();
        let plan = plan_output(&config, &topology.state(1).unwrap()).unwrap();
        let (paths, _) = build_update(&topology.paths, &topology.modes, &[(1, plan, 0)]).unwrap();
        assert_eq!(paths[1].targetInfo.refreshRate, effective);
    }

    #[test]
    fn no_unknown_refresh_can_be_replaced_with_an_optimal_refresh_request() {
        let mut topology = synthetic_topology();
        topology.paths[1].targetInfo.refreshRate = ccd::DISPLAYCONFIG_RATIONAL::default();
        let mut timing = unsafe { topology.modes[3].Anonymous.targetMode };
        timing.targetVideoSignalInfo.vSyncFreq = ccd::DISPLAYCONFIG_RATIONAL::default();
        topology.modes[3].Anonymous.targetMode = timing;
        let mut config = output();
        config.mode = "2560x1440".to_string();
        let plan = plan_output(&config, &topology.state(1).unwrap()).unwrap();
        assert!(build_update(&topology.paths, &topology.modes, &[(1, plan, 0)]).is_err());
    }

    #[test]
    fn rotation_swaps_source_dimensions_once_and_preserves_target_timing() {
        let topology = synthetic_topology();
        let mut config = output();
        config.transform = "90".to_string();
        let plan = plan_output(&config, &topology.state(1).unwrap()).unwrap();
        let (paths, modes) = build_update(&topology.paths, &topology.modes, &[(1, plan, 0)]).unwrap();
        let rotated = Topology { paths, modes, identities: Vec::new() };
        assert_eq!(unsafe { rotated.modes[2].Anonymous.sourceMode.width }, 1080);
        assert_eq!(unsafe { rotated.modes[2].Anonymous.sourceMode.height }, 1920);
        assert_eq!(rotated.state(1).unwrap().mode, topology.state(1).unwrap().mode);
        assert!(!plan_output(&config, &rotated.state(1).unwrap()).unwrap().changes_windows());
        assert_eq!(unsafe { rotated.paths[1].targetInfo.Anonymous.modeInfoIdx }, 3);
    }

    #[test]
    fn removing_one_path_is_allowed_but_removing_every_path_is_refused() {
        let topology = synthetic_topology();
        let off = OutputPlan { disable: true, ..OutputPlan::default() };
        let (paths, _) = build_update(&topology.paths, &topology.modes, &[(1, off.clone(), 0)]).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].targetInfo.id, 10);
        assert!(build_update(&topology.paths, &topology.modes, &[(0, off.clone(), 0), (1, off, 1)]).is_err());
    }

    #[test]
    fn cloned_source_changes_are_refused_before_the_executor() {
        let mut topology = synthetic_topology();
        topology.paths[1].sourceInfo = topology.paths[0].sourceInfo;
        let plan = OutputPlan { position: Some((10, 20)), ..OutputPlan::default() };
        assert!(build_update(&topology.paths, &topology.modes, &[(1, plan, 0)]).is_err());
    }

    fn edid() -> Vec<u8> {
        let mut bytes = vec![0u8; 128];
        bytes[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        bytes[8..10].copy_from_slice(&0x10acu16.to_be_bytes()); // DEL
        bytes[12..16].copy_from_slice(&123456u32.to_le_bytes());
        for (start, tag, text) in [(54, 0xfc, "U2720Q"), (72, 0xff, "ABC123")] {
            bytes[start + 3] = tag;
            bytes[start + 5..start + 18].fill(b' ');
            bytes[start + 5..start + 5 + text.len()].copy_from_slice(text.as_bytes());
            bytes[start + 5 + text.len()] = b'\n';
        }
        checksum(&mut bytes);
        bytes
    }

    fn checksum(bytes: &mut [u8]) {
        bytes[127] = 0;
        bytes[127] = 0u8.wrapping_sub(bytes[..127].iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    }

    #[test]
    fn edid_text_serial_wins_and_numeric_serial_is_a_fallback() {
        let mut bytes = edid();
        assert_eq!(parse_edid(&bytes), Some(EdidIdentity { make: "DEL".to_string(), model: "U2720Q".to_string(), serial: "ABC123".to_string() }));
        bytes[72..90].fill(0);
        checksum(&mut bytes);
        assert_eq!(parse_edid(&bytes).unwrap().serial, "123456");
        bytes[12..16].fill(0);
        checksum(&mut bytes);
        assert!(parse_edid(&bytes).unwrap().serial.is_empty());
    }

    #[test]
    fn malformed_edid_is_not_used_as_an_identity() {
        assert!(parse_edid(&[]).is_none());
        assert!(parse_edid(&[0u8; 128]).is_none());
        let mut bytes = edid();
        bytes[20] ^= 1;
        assert!(parse_edid(&bytes).is_none());
        bytes[20] ^= 1;
        bytes[8] = 0;
        bytes[9] = 0;
        checksum(&mut bytes);
        assert!(parse_edid(&bytes).is_none());
    }

    #[test]
    fn ccd_device_path_maps_only_to_a_monitor_edid_key() {
        let path = r"\\?\DISPLAY#DEL40A9#5&123&0&UID4357#{monitor-guid}";
        assert_eq!(edid_registry_key(path).unwrap(), r"SYSTEM\CurrentControlSet\Enum\DISPLAY\DEL40A9\5&123&0&UID4357\Device Parameters");
        assert_eq!(pnp_from_device_path(path).unwrap(), "DEL");
        for bad in [r"C:\DISPLAY#DEL40A9#instance", r"\\?\USB#DEL40A9#instance", r"\\?\DISPLAY#..#instance", r"\\?\DISPLAY#DEL40A9#..\secret", r"\\?\DISPLAY##instance"] {
            assert!(edid_registry_key(bad).is_none(), "{bad}");
        }
    }
}
