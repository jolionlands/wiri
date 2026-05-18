//! Save / restore tile layouts to JSON snapshots on disk.
//!
//! A snapshot captures every monitor's workspaces, the columns inside each
//! workspace, and the tile HWNDs (plus per-column width hint and tabbed-mode
//! flag) so a user can re-create their dev environment after a reboot or
//! window-manager restart.
//!
//! Storage path: `%APPDATA%\wiri\snapshots\<name>.json`.  Falls back to
//! `%USERPROFILE%\.wiri\snapshots\<name>.json` when APPDATA is unavailable.
//!
//! Restore behaviour: HWNDs that no longer exist (window closed since the
//! snapshot was taken) are silently skipped.  HWND identity is the only
//! key — there is no class/title fuzzy matching today.  Stale snapshots
//! still load cleanly; their tiles just don't materialise.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::utils::{OutputId, WindowId};
use super::workspace::{Column, ColumnDisplay, Tile, Workspace};

/// One tile inside a snapshot.  We persist only the bits the engine
/// actually needs to rebuild the layout — bounds and visual state are
/// recomputed by the layout pass after restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotTile {
    pub hwnd: isize,
    /// Niri-style relative weight (1.0 = equal share).
    #[serde(default = "default_height_weight")]
    pub height_weight: f32,
}

fn default_height_weight() -> f32 { 1.0 }

/// One column inside a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotColumn {
    pub tiles: Vec<SnapshotTile>,
    pub width: Option<u32>,
    /// When `Some(idx)` the column is in Tabbed mode with the given active tab.
    #[serde(default)]
    pub tabbed_active: Option<usize>,
    #[serde(default)]
    pub maximized: bool,
}

/// One workspace inside a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotWorkspace {
    pub id: i32,
    pub columns: Vec<SnapshotColumn>,
    #[serde(default)]
    pub scroll_x: i32,
    #[serde(default)]
    pub scroll_y: i32,
}

/// One monitor inside a snapshot.  Monitors are matched at restore time by
/// the deterministic `OutputId::from_name(...)` u64 so multi-monitor
/// snapshots survive cable swaps as long as the device name is the same.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMonitor {
    pub output_id: u64,
    pub active_workspace: i32,
    pub workspaces: Vec<SnapshotWorkspace>,
}

/// Top-level snapshot document.  Versioned so future format additions
/// can be loaded by older binaries (with a warn-on-mismatch fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Format version.  `1` is the initial release.
    #[serde(default = "default_version")]
    pub version: u32,
    pub monitors: Vec<SnapshotMonitor>,
}

fn default_version() -> u32 { 1 }

const SNAPSHOT_VERSION: u32 = 1;

impl Snapshot {
    /// Build a snapshot from a `MonitorSet` (read directly off the engine).
    pub fn from_monitors(
        monitors: &super::MonitorSet<OutputId, super::Monitor>,
    ) -> Self {
        let mut out_monitors = Vec::with_capacity(monitors.len());
        for (oid, m) in monitors.iter() {
            let mut ws_list: Vec<(i32, &Workspace)> = m
                .workspaces
                .iter()
                .map(|(k, v)| (*k, v))
                .collect();
            ws_list.sort_by_key(|(k, _)| *k);
            let workspaces = ws_list
                .into_iter()
                .map(|(ws_id, ws)| SnapshotWorkspace {
                    id: ws_id,
                    columns: ws.columns.iter().map(snapshot_column).collect(),
                    scroll_x: ws.scroll_offset.x,
                    scroll_y: ws.scroll_offset.y,
                })
                .collect();
            out_monitors.push(SnapshotMonitor {
                output_id: oid.as_u64(),
                active_workspace: m.active_workspace,
                workspaces,
            });
        }
        Self {
            version: SNAPSHOT_VERSION,
            monitors: out_monitors,
        }
    }
}

fn snapshot_column(col: &Column) -> SnapshotColumn {
    SnapshotColumn {
        tiles: col
            .tiles
            .iter()
            .map(|t| SnapshotTile {
                hwnd: t.window_id.as_isize(),
                height_weight: t.height_weight,
            })
            .collect(),
        width: col.width,
        tabbed_active: match col.display {
            ColumnDisplay::Tabbed { active_tab } => Some(active_tab),
            ColumnDisplay::Stacked => None,
        },
        maximized: col.maximized,
    }
}

/// Resolve the directory that holds snapshot files.
///
/// Order: `WIRI_SNAPSHOT_DIR` env var, then `%APPDATA%\wiri\snapshots`,
/// then `%USERPROFILE%\.wiri\snapshots`.  Returns the resolved path even
/// if the directory does not yet exist — callers are responsible for
/// `create_dir_all` when writing.
pub fn snapshot_dir() -> PathBuf {
    if let Ok(p) = std::env::var("WIRI_SNAPSHOT_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let mut p = PathBuf::from(appdata);
        p.push("wiri");
        p.push("snapshots");
        return p;
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let mut p = PathBuf::from(home);
        p.push(".wiri");
        p.push("snapshots");
        return p;
    }
    PathBuf::from(".").join("snapshots")
}

/// Resolve the full path for a snapshot file by name.  Strips path
/// separators from `name` to keep callers honest (a snapshot named
/// `..\..\evil` shouldn't escape the dir).
pub fn snapshot_path(name: &str) -> PathBuf {
    let sanitised = sanitise_name(name);
    snapshot_dir().join(format!("{}.json", sanitised))
}

fn sanitise_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect::<String>()
        .trim_matches('.')
        .to_string()
}

/// Write the snapshot to disk as pretty-printed JSON.  Creates parent
/// directories as needed.
pub fn save_to(name: &str, snapshot: &Snapshot) -> std::io::Result<PathBuf> {
    let path = snapshot_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let serialised = serde_json::to_string_pretty(snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, serialised)?;
    Ok(path)
}

/// Read and deserialise a snapshot from disk.
pub fn load_from(name: &str) -> std::io::Result<Snapshot> {
    let path = snapshot_path(name);
    let raw = std::fs::read_to_string(&path)?;
    serde_json::from_str(&raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Enumerate every snapshot in `snapshot_dir()`.  Returns just the base
/// names without the `.json` extension.
pub fn list_names() -> std::io::Result<Vec<String>> {
    let dir = snapshot_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Remove a snapshot file by name.
pub fn delete(name: &str) -> std::io::Result<PathBuf> {
    let path = snapshot_path(name);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(path)
}

/// Apply a snapshot back onto a fresh workspace map for one monitor.
/// HWNDs in `surviving_hwnds` are kept; others are dropped silently.
///
/// Returned map is keyed by workspace id.  Callers should swap the
/// monitor's existing `workspaces` map with this one and reset the
/// `active_workspace` cursor to `snap_monitor.active_workspace`.
pub fn rebuild_workspaces(
    snap_monitor: &SnapshotMonitor,
    surviving_hwnds: &std::collections::HashSet<isize>,
) -> std::collections::HashMap<i32, Workspace> {
    let mut out: std::collections::HashMap<i32, Workspace> = std::collections::HashMap::new();
    for snap_ws in &snap_monitor.workspaces {
        let mut ws = Workspace::new();
        ws.scroll_offset.x = snap_ws.scroll_x;
        ws.scroll_offset.y = snap_ws.scroll_y;
        for snap_col in &snap_ws.columns {
            let mut col = Column::new();
            col.width = snap_col.width;
            col.maximized = snap_col.maximized;
            for snap_tile in &snap_col.tiles {
                if !surviving_hwnds.contains(&snap_tile.hwnd) {
                    continue;
                }
                let mut tile = Tile::new(WindowId::new(snap_tile.hwnd));
                tile.height_weight = snap_tile.height_weight;
                col.tiles.push(tile);
            }
            // Skip empty columns — they would have been pruned by the
            // engine anyway, and an empty column makes no visual sense.
            if col.tiles.is_empty() {
                continue;
            }
            if let Some(active) = snap_col.tabbed_active {
                col.display = ColumnDisplay::Tabbed {
                    active_tab: active.min(col.tiles.len().saturating_sub(1)),
                };
            }
            ws.columns.push(col);
        }
        // Always materialise the workspace, even when empty — preserves the
        // workspace stack so workspace 5 doesn't silently collapse to 2.
        out.insert(snap_ws.id, ws);
    }
    // Ensure workspace 0 always exists (Monitor::new default).
    out.entry(0).or_insert_with(Workspace::new);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::{Point, Rect};
    use crate::layout::{Monitor, MonitorSet};

    fn temp_snapshot_dir() -> PathBuf {
        // Each test gets a unique temp subdir so concurrent tests don't
        // stomp on each other.
        let n: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let mut p = std::env::temp_dir();
        p.push(format!("wiri-snapshot-test-{}", n));
        p
    }

    /// Saving a snapshot then loading it round-trips every monitor /
    /// workspace / column / tile through serde.
    #[test]
    fn test_snapshot_round_trips_through_serde() {
        let tmp = temp_snapshot_dir();
        std::env::set_var("WIRI_SNAPSHOT_DIR", tmp.as_os_str());

        let mut ms: MonitorSet<OutputId, Monitor> = MonitorSet::new();
        let oid = OutputId::from_name("RT-MON-1");
        let mut m = Monitor::new(oid, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080));
        m.add_window(WindowId::new(0x1001));
        m.add_window(WindowId::new(0x1002));
        if let Some(ws) = m.workspace_mut() {
            ws.scroll_offset = Point::new(33, 0);
            // Stack a tile in column 0 for variety.
            ws.add_window_to_column(0, WindowId::new(0x1003));
        }
        ms.insert(oid, m);

        let snap = Snapshot::from_monitors(&ms);
        let saved = save_to("rt-test", &snap).expect("save ok");
        assert!(saved.exists(), "snapshot file was created on disk");

        let loaded = load_from("rt-test").expect("load ok");
        assert_eq!(loaded.version, snap.version);
        assert_eq!(loaded.monitors.len(), 1);
        let lm = &loaded.monitors[0];
        assert_eq!(lm.output_id, oid.as_u64());
        assert_eq!(lm.workspaces.len(), 1);
        let lws = &lm.workspaces[0];
        assert_eq!(lws.id, 0);
        assert_eq!(lws.scroll_x, 33);
        assert_eq!(lws.columns.len(), 2);
        // Column 0 has 2 tiles (0x1001 + the stacked 0x1003).
        let hwnds: Vec<isize> = lws.columns[0].tiles.iter().map(|t| t.hwnd).collect();
        assert!(hwnds.contains(&0x1001));
        assert!(hwnds.contains(&0x1003));
        assert_eq!(lws.columns[1].tiles[0].hwnd, 0x1002);

        let _ = delete("rt-test");
        std::env::remove_var("WIRI_SNAPSHOT_DIR");
    }

    /// Restoring a snapshot whose tiles point at HWNDs that no longer
    /// exist drops those tiles silently and keeps the rest.
    #[test]
    fn test_load_skips_missing_hwnds_silently() {
        let snap_mon = SnapshotMonitor {
            output_id: 0xDEAD_BEEF,
            active_workspace: 0,
            workspaces: vec![SnapshotWorkspace {
                id: 0,
                columns: vec![SnapshotColumn {
                    tiles: vec![
                        SnapshotTile { hwnd: 100, height_weight: 1.0 },
                        SnapshotTile { hwnd: 200, height_weight: 1.0 },
                        SnapshotTile { hwnd: 300, height_weight: 1.0 },
                    ],
                    width: Some(500),
                    tabbed_active: None,
                    maximized: false,
                }],
                scroll_x: 0,
                scroll_y: 0,
            }],
        };
        // Only HWND 200 still exists.
        let surviving: std::collections::HashSet<isize> =
            std::iter::once(200isize).collect();
        let rebuilt = rebuild_workspaces(&snap_mon, &surviving);
        let ws = rebuilt.get(&0).expect("workspace 0 materialised");
        assert_eq!(ws.columns.len(), 1, "column kept because it has survivors");
        assert_eq!(ws.columns[0].tiles.len(), 1);
        assert_eq!(
            ws.columns[0].tiles[0].window_id,
            WindowId::new(200),
            "only the surviving HWND remains",
        );
    }

    /// Sanitise-name rejects path-escape attempts.
    #[test]
    fn test_sanitise_name_strips_separators() {
        let original = "../../evil/payload";
        let cleaned = sanitise_name(original);
        // No '..', no '/', no '\\' allowed.
        assert!(!cleaned.contains('/'));
        assert!(!cleaned.contains('\\'));
        assert!(!cleaned.contains(".."));
    }
}

