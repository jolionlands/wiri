//! Event-driven bridge from physical outputs/config to wiri's monitor model.
//! Kept outside main/backend/engine so the shared-file hooks remain small.
use super::{BackendHandle, MonitorInfo};
use crate::config::Config;
use crate::layout::TilingEngine;
use crate::utils::OutputId;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

/// Refresh geometry and real DPI after a display-change event or an explicit config reload.
/// A failed enumeration leaves the last complete snapshot intact.
pub fn refresh(handle: &BackendHandle) {
    unsafe extern "system" fn collect(
        hmonitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        if let Some(info) = MonitorInfo::from_hmonitor(hmonitor) {
            let result = &mut *(data.0 as *mut Vec<MonitorInfo>);
            result.push(info);
        }
        BOOL(1)
    }
    let mut monitors = Vec::<MonitorInfo>::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(collect),
            LPARAM(&mut monitors as *mut _ as isize),
        )
    };
    if ok.as_bool() {
        *handle.monitors.write() = monitors.into_iter().map(|info| (info.id, info)).collect();
    } else {
        tracing::warn!("Output enumeration failed; keeping the last monitor snapshot");
    }
}

/// Update geometry/scale/off without replacing surviving monitors and their workspaces.
pub fn sync(config: &Config, handle: &BackendHandle, engine: &mut TilingEngine) {
    let monitors = handle.get_monitors();
    let matcher = super::outputs::OutputMatcher::snapshot();
    let live: Vec<OutputId> = monitors
        .iter()
        .filter_map(|monitor| {
            let setting = matcher.matching_config(config, monitor);
            if setting.is_some_and(|out| !out.enable) {
                return None;
            }
            let scale = super::outputs::effective_scale(setting, monitor);
            engine.sync_monitor_with_scale(monitor.id, monitor.bounds, monitor.work_area, scale);
            engine.set_output_name(monitor.id, &monitor.name);
            Some(monitor.id)
        })
        .collect();
    let gone: Vec<OutputId> = engine
        .monitors()
        .keys()
        .filter(|id| !live.contains(id))
        .copied()
        .collect();
    for id in gone {
        engine.unregister_monitor(&id);
    }
}

/// Initial focus is chosen once. Reloading config must not steal the user's focus.
pub fn focus_at_startup(config: &Config, handle: &BackendHandle, engine: &mut TilingEngine) {
    let matcher = super::outputs::OutputMatcher::snapshot();
    let mut monitors = handle.get_monitors();
    monitors.sort_by_key(|monitor| {
        (
            monitor.bounds.loc.x,
            monitor.bounds.loc.y,
            monitor.name.clone(),
        )
    });
    if let Some(id) = startup_output(config, &monitors, &matcher) {
        engine.set_focused_output(id);
        if let Some(window) = engine.monitors().get(&id).and_then(|out| out.focus_window) {
            let _ = handle.activate_window(window.as_isize());
        }
    }
}

fn startup_output(
    config: &Config,
    monitors: &[MonitorInfo],
    matcher: &super::outputs::OutputMatcher,
) -> Option<OutputId> {
    // niri chooses the first eligible block in configuration order, not monitor order.
    config
        .output
        .iter()
        .filter(|out| out.enable && (out.focus_at_startup || out.primary))
        .find_map(|out| {
            monitors
                .iter()
                .find(|monitor| {
                    matcher
                        .matching_config(config, monitor)
                        .is_some_and(|selected| std::ptr::eq(selected, out))
                })
                .map(|monitor| monitor.id)
        })
}

#[cfg(test)]
mod tests {
    use super::super::outputs::{OutputIdentity, OutputMatcher};
    use super::*;
    use crate::utils::Rect;
    fn monitor(name: &str) -> MonitorInfo {
        MonitorInfo {
            id: OutputId::from_name(name),
            name: name.into(),
            bounds: Rect::new(0, 0, 100, 100),
            work_area: Rect::new(0, 0, 100, 100),
            is_primary: false,
            scale_factor: 1.5,
        }
    }
    #[test]
    fn startup_focus_follows_config_order_not_enumeration_and_skips_disabled() {
        let monitors = [monitor("A"), monitor("B")];
        let mut config = Config::default();
        let mut b = config.output[0].clone();
        b.name = "B".into();
        b.focus_at_startup = true;
        let mut a = b.clone();
        a.name = "A".into();
        config.output = vec![b, a];
        let matcher = OutputMatcher::default();
        assert_eq!(
            startup_output(&config, &monitors, &matcher),
            Some(monitors[1].id)
        );
        config.output[0].enable = false;
        assert_eq!(
            startup_output(&config, &monitors, &matcher),
            Some(monitors[0].id)
        );
        config.output[1].focus_at_startup = false;
        assert_eq!(startup_output(&config, &monitors, &matcher), None);
    }
    #[test]
    fn startup_focus_uses_identity_matching_without_stealing_a_connector_override() {
        let monitors = [monitor("A")];
        let mut config = Config::default();
        config.output[0].name = "DEL Model Serial".into();
        config.output[0].focus_at_startup = true;
        let matcher = OutputMatcher {
            identities: vec![OutputIdentity {
                connector: "A".into(),
                make: "DEL".into(),
                model: "Model".into(),
                serial: "Serial".into(),
                ..OutputIdentity::default()
            }],
        };
        assert_eq!(
            startup_output(&config, &monitors, &matcher),
            Some(monitors[0].id)
        );
        let mut connector = config.output[0].clone();
        connector.name = "a".into();
        connector.focus_at_startup = false;
        config.output.push(connector);
        assert_eq!(startup_output(&config, &monitors, &matcher), None);
    }
}
