//! Read-only IPC snapshots and target-only, non-persistent output changes.
//! Physical updates reuse the conservative CCD planner/executor in the parent
//! module. No display settings file, Windows DPI, or primary display is changed.

use std::collections::HashMap;

use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput, DXGI_ENUM_MODES, DXGI_ERROR_MORE_DATA,
    DXGI_ERROR_NOT_FOUND,
};

use super::{
    build_update, ccd, execute_update, millihertz, parse_edid, plan_output, read_edid, wide_string,
    OutputConfig, OutputIdentity, Rotation, Topology,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputModeSnapshot {
    pub width: u32,
    pub height: u32,
    /// Millihertz, including fractional rates such as 59_940.
    pub refresh_rate: u32,
    pub is_preferred: bool,
}

#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    pub identity: OutputIdentity,
    pub modes: Vec<OutputModeSnapshot>,
    pub current_mode: Option<usize>,
    /// Unrotated physical dimensions in millimetres, only from validated EDID.
    pub physical_size: Option<(u32, u32)>,
    pub rotation: Rotation,
}

// Width, height, millihertz. Never use EnumDisplaySettings' rounded integer Hz.
type ModeKey = (u32, u32, u32);
const MAX_MODES: u32 = 16_384;

fn connector_key(name: &str) -> String {
    name.trim().trim_start_matches(r"\\.\").to_ascii_lowercase()
}

/// Connector matches take priority; duplicate/ambiguous identities fail closed.
fn target_index(identities: &[OutputIdentity], name: &str) -> Result<usize, String> {
    let key = connector_key(name);
    if key.is_empty() {
        return Err("an explicit output name is required".to_string());
    }
    let mut matches: Vec<usize> = identities
        .iter()
        .enumerate()
        .filter_map(|(index, identity)| {
            (connector_key(&identity.connector) == key).then_some(index)
        })
        .collect();
    if matches.is_empty() {
        matches = identities
            .iter()
            .enumerate()
            .filter_map(|(index, identity)| identity.matches(name).then_some(index))
            .collect();
    }
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(format!(
            "output '{name}' is not active; enabling an inactive output is unsupported"
        )),
        _ => Err(format!(
            "output '{name}' is ambiguous; use a unique connector name"
        )),
    }
}

/// Apply exactly one target. The caller's config name cannot select other
/// outputs or act as a default block. No-op/layout-only plans never call CCD's
/// mutating API, not even its validation operation.
pub fn apply_one(name: &str, config: &OutputConfig) -> Result<(), String> {
    let topology = Topology::query()?;
    let index = target_index(&topology.identities, name)?;
    let plan = plan_output(config, &topology.state(index)?)?;
    if !plan.warnings.is_empty() {
        return Err(plan.warnings.join("; "));
    }
    if !plan.changes_windows() {
        return Ok(());
    }
    let (paths, modes) = build_update(&topology.paths, &topology.modes, &[(index, plan, 0)])?;
    // The shared executor does not SAVE_TO_DATABASE or ALLOW_CHANGES, refuses
    // detaching every display, and is replaced by a non-mutating stub in tests.
    execute_update(&paths, &modes)
}

/// Active CCD paths supply identity/rotation and the exact current timing.
/// DXGI supplies available rational-rate modes. Missing DXGI/EDID information
/// stays missing; it is never replaced by a guessed resolution, rate, or size.
pub fn snapshots() -> Result<Vec<OutputSnapshot>, String> {
    let topology = Topology::query()?;
    let available = modes_by_connector().unwrap_or_else(|error| {
        tracing::debug!(%error, "Available output modes unavailable");
        HashMap::new()
    });
    let mut result = Vec::with_capacity(topology.paths.len());
    for (index, path) in topology.paths.iter().enumerate() {
        let identity = topology.identities[index].clone();
        let rotation = Rotation::from_windows(path.targetInfo.rotation)
            .ok_or("Windows returned an unknown output rotation")?;
        let current = topology.state(index).ok().and_then(|state| {
            let refresh = state.mode.refresh_millihz?;
            (state.mode.width > 0 && state.mode.height > 0 && refresh > 0).then_some((
                state.mode.width,
                state.mode.height,
                refresh,
            ))
        });
        let preferred = preferred_mode(path);
        let (modes, current_mode) = assemble_modes(
            available
                .get(&connector_key(&identity.connector))
                .cloned()
                .unwrap_or_default(),
            current,
            preferred,
        );
        let physical_size = read_edid(&identity.device_path)
            .as_deref()
            .and_then(edid_physical_size);
        result.push(OutputSnapshot {
            identity,
            modes,
            current_mode,
            physical_size,
            rotation,
        });
    }
    Ok(result)
}

fn mode_key(width: u32, height: u32, numerator: u32, denominator: u32) -> Option<ModeKey> {
    if width == 0 || height == 0 || width > 100_000 || height > 100_000 {
        return None;
    }
    let refresh = millihertz(ccd::DISPLAYCONFIG_RATIONAL {
        Numerator: numerator,
        Denominator: denominator,
    })?;
    (refresh > 0).then_some((width, height, refresh))
}

/// Include exact CCD current/preferred timings even when DXGI omits a custom
/// mode. Deduplication uses the protocol's millihertz precision, not integer Hz.
fn assemble_modes(
    mut keys: Vec<ModeKey>,
    current: Option<ModeKey>,
    preferred: Option<ModeKey>,
) -> (Vec<OutputModeSnapshot>, Option<usize>) {
    keys.extend(current);
    keys.extend(preferred);
    keys.sort_unstable();
    keys.dedup();
    let current_mode = current.and_then(|current| keys.iter().position(|key| *key == current));
    let modes = keys
        .into_iter()
        .map(|key| OutputModeSnapshot {
            width: key.0,
            height: key.1,
            refresh_rate: key.2,
            is_preferred: Some(key) == preferred,
        })
        .collect();
    (modes, current_mode)
}

fn preferred_mode(path: &ccd::DISPLAYCONFIG_PATH_INFO) -> Option<ModeKey> {
    let mut preferred = ccd::DISPLAYCONFIG_TARGET_PREFERRED_MODE::default();
    preferred.header.r#type = ccd::DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE;
    preferred.header.size = std::mem::size_of_val(&preferred) as u32;
    preferred.header.adapterId = path.targetInfo.adapterId;
    preferred.header.id = path.targetInfo.id;
    if unsafe { ccd::DisplayConfigGetDeviceInfo(&mut preferred.header) } != 0 {
        return None;
    }
    let rate = preferred.targetMode.targetVideoSignalInfo.vSyncFreq;
    mode_key(
        preferred.width,
        preferred.height,
        rate.Numerator,
        rate.Denominator,
    )
}

fn modes_by_connector() -> Result<HashMap<String, Vec<ModeKey>>, String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|error| format!("CreateDXGIFactory1 failed: {error}"))?;
    let mut result = HashMap::new();
    // Bounded enumeration even if a driver keeps returning new objects.
    for adapter_index in 0..128 {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(format!("DXGI adapter enumeration failed: {error}")),
        };
        for output_index in 0..128 {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(format!("DXGI output enumeration failed: {error}")),
            };
            let Ok(desc) = (unsafe { output.GetDesc() }) else {
                continue;
            };
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let key = connector_key(&wide_string(&desc.DeviceName));
            if key.is_empty() {
                continue;
            }
            let modes = display_modes(&output).unwrap_or_default();
            // A duplicated connector must not acquire another output's modes.
            match result.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(modes);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().clear();
                }
            }
        }
    }
    Ok(result)
}

fn display_modes(output: &IDXGIOutput) -> Result<Vec<ModeKey>, String> {
    for _ in 0..3 {
        let mut count = 0;
        unsafe {
            output.GetDisplayModeList(
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_ENUM_MODES(0),
                &mut count,
                None,
            )
        }
        .map_err(|error| format!("DXGI mode count failed: {error}"))?;
        if count > MAX_MODES {
            return Err("DXGI returned an unreasonable mode count".to_string());
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut modes = vec![DXGI_MODE_DESC::default(); count as usize];
        let status = unsafe {
            output.GetDisplayModeList(
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_ENUM_MODES(0),
                &mut count,
                Some(modes.as_mut_ptr()),
            )
        };
        match status {
            Err(error) if error.code() == DXGI_ERROR_MORE_DATA => continue,
            Err(error) => return Err(format!("DXGI mode enumeration failed: {error}")),
            Ok(()) => {}
        }
        if count as usize > modes.len() {
            return Err("DXGI returned an inconsistent mode count".to_string());
        }
        return Ok(modes[..count as usize]
            .iter()
            .filter_map(|mode| {
                mode_key(
                    mode.Width,
                    mode.Height,
                    mode.RefreshRate.Numerator,
                    mode.RefreshRate.Denominator,
                )
            })
            .collect());
    }
    Err("output modes kept changing during enumeration".to_string())
}

fn edid_physical_size(bytes: &[u8]) -> Option<(u32, u32)> {
    // Reuse the identity parser's base-block header/checksum validation.
    parse_edid(bytes)?;
    let base = bytes.get(..128)?;
    for descriptor in base[54..126].chunks_exact(18) {
        if descriptor[..2] == [0, 0] {
            continue;
        }
        let width = u32::from(descriptor[12]) | (u32::from(descriptor[14] >> 4) << 8);
        let height = u32::from(descriptor[13]) | (u32::from(descriptor[14] & 15) << 8);
        if width > 0 && height > 0 {
            return Some((width, height));
        }
    }
    // A zero in either base-size byte means unknown or an aspect ratio in
    // EDID 1.4, not a physical dimension.
    (base[21] > 0 && base[22] > 0).then_some((u32::from(base[21]) * 10, u32::from(base[22]) * 10))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(connector: &str) -> OutputIdentity {
        OutputIdentity {
            connector: connector.to_string(),
            make: "DEL".to_string(),
            model: "U2720Q".to_string(),
            friendly_name: "Shared monitor".to_string(),
            ..OutputIdentity::default()
        }
    }

    #[test]
    fn target_selection_is_exact_connector_first_and_ambiguity_safe() {
        let identities = [identity(r"\\.\DISPLAY1"), identity(r"\\.\DISPLAY10")];
        assert_eq!(target_index(&identities, "display1"), Ok(0));
        assert_eq!(target_index(&identities, r"\\.\display10"), Ok(1));
        assert!(target_index(&identities, "DISPLAY").is_err());
        assert!(target_index(&identities, "DEL U2720Q").is_err());
        assert!(target_index(&identities, "").is_err());
        assert!(target_index(&identities, "DISPLAY2").is_err());
        let mut other = identity(r"\\.\DISPLAY2");
        other.make.clear();
        other.model = "DISPLAY1".to_string();
        assert_eq!(
            target_index(&[identities[0].clone(), other], "display1"),
            Ok(0)
        );
        assert!(target_index(&[identities[0].clone(), identities[0].clone()], "DISPLAY1").is_err());
        assert_eq!(target_index(&[identities[0].clone()], "DEL U2720Q"), Ok(0));
    }

    #[test]
    fn rational_modes_keep_fractional_rates_and_reject_unknown_values() {
        assert_eq!(
            mode_key(1920, 1080, 60_000, 1001),
            Some((1920, 1080, 59_940))
        );
        assert_eq!(mode_key(1920, 1080, 60, 1), Some((1920, 1080, 60_000)));
        assert!(mode_key(1920, 1080, 0, 1).is_none());
        assert!(mode_key(1920, 1080, 60, 0).is_none());
        assert!(mode_key(0, 1080, 60, 1).is_none());
        assert!(mode_key(100_001, 1080, 60, 1).is_none());
    }

    #[test]
    fn mode_indices_are_deduplicated_and_preference_is_exact_not_nearest() {
        let current = (1920, 1080, 59_940);
        let preferred = (1920, 1080, 60_000);
        let (modes, index) = assemble_modes(vec![current, current], Some(current), Some(preferred));
        assert_eq!(modes.len(), 2);
        let current_mode = &modes[index.unwrap()];
        assert_eq!(current_mode.refresh_rate, 59_940);
        assert!(!current_mode.is_preferred);
        assert_eq!(modes.iter().filter(|mode| mode.is_preferred).count(), 1);
        assert!(modes
            .iter()
            .any(|mode| mode.refresh_rate == 60_000 && mode.is_preferred));
        let (modes, index) = assemble_modes(vec![current], None, None);
        assert!(index.is_none());
        assert!(!modes[0].is_preferred);
        let (modes, index) = assemble_modes(Vec::new(), None, None);
        assert!(modes.is_empty());
        assert!(index.is_none());
    }

    fn checksum(bytes: &mut [u8; 128]) {
        bytes[127] = 0u8.wrapping_sub(
            bytes[..127]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        );
    }

    fn edid() -> [u8; 128] {
        let mut bytes = [0; 128];
        bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        bytes[8..10].copy_from_slice(&[0x10, 0xac]); // DEL
        bytes[21] = 60;
        bytes[22] = 34;
        checksum(&mut bytes);
        bytes
    }

    #[test]
    fn physical_dimensions_require_valid_edid_and_do_not_guess_aspect_ratios() {
        let mut bytes = edid();
        assert_eq!(edid_physical_size(&bytes), Some((600, 340)));
        bytes[22] = 0;
        checksum(&mut bytes);
        assert_eq!(edid_physical_size(&bytes), None);
        bytes[54] = 1; // Detailed timing, not a textual descriptor.
        bytes[66] = 597u32 as u8;
        bytes[67] = 336u32 as u8;
        bytes[68] = 0x21;
        checksum(&mut bytes);
        assert_eq!(edid_physical_size(&bytes), Some((597, 336)));
        bytes[100] ^= 1;
        assert_eq!(edid_physical_size(&bytes), None);
        assert_eq!(edid_physical_size(&bytes[..127]), None);
        bytes[0] = 1;
        checksum(&mut bytes);
        assert_eq!(edid_physical_size(&bytes), None);
    }

    #[test]
    fn target_only_preparation_preserves_other_paths_and_last_display_guard() {
        use super::super::{OutputMode, OutputState};
        use crate::config::Config;
        let mut paths = [ccd::DISPLAYCONFIG_PATH_INFO::default(); 2];
        paths[0].sourceInfo.id = 3;
        paths[0].targetInfo.id = 30;
        paths[1].sourceInfo.id = 4;
        paths[1].targetInfo.id = 40;
        let current = OutputState {
            mode: OutputMode {
                width: 1920,
                height: 1080,
                refresh_millihz: Some(59_940),
            },
            position: (0, 0),
            rotation: Rotation::Normal,
        };
        let mut config = Config::default().output.remove(0);
        config.enable = false;
        let plan = plan_output(&config, &current).unwrap();
        let (remaining, modes) = build_update(&paths, &[], &[(1, plan, 0)]).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].sourceInfo.id, paths[0].sourceInfo.id);
        assert_eq!(remaining[0].targetInfo.id, paths[0].targetInfo.id);
        assert!(modes.is_empty());
        let plan = plan_output(&config, &current).unwrap();
        assert!(build_update(&paths[..1], &[], &[(0, plan, 0)]).is_err());
    }
}
