use crate::config::decode::{self, Decoded, ValidationIssue, ValidationSeverity};
use crate::config::error::{ConfigError, Result};
use crate::config::parse::into_result;
use crate::config::types::Config;
use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

pub use crate::config::decode::MAX_INCLUDE_DEPTH;

/// How long the config (and the files it includes) must stay quiet before it is read.  An
/// editor saves a file in several steps (truncate, write, rename); the config is read once
/// after the last of them, so half a file is never applied.
pub const RELOAD_QUIET_MS: u64 = 200;

/// What a file-watcher reload (or an explicit one) came to.
#[derive(Debug, Clone)]
pub enum ReloadOutcome {
    /// The file parsed: apply `config`.  `warnings` are the things in it that wiri ignores.
    Applied {
        config: Box<Config>,
        warnings: Vec<ValidationIssue>,
    },
    /// The file has errors: the last good config stays.  `issues` has them all (and the
    /// warnings), `summary` is the first with its place, ready for a balloon.
    Failed {
        issues: Vec<ValidationIssue>,
        summary: String,
    },
}

/// `file:line:column: error: message` of the first error in `issues`, with how many more
/// there are, for a balloon or a log line.
pub fn failure_summary(config_path: &Path, issues: &[ValidationIssue]) -> String {
    let errors: Vec<&ValidationIssue> = issues
        .iter()
        .filter(|i| i.severity == ValidationSeverity::Error)
        .collect();
    match errors.first() {
        Some(first) => {
            let mut s = first.located(&config_path.display().to_string());
            if errors.len() > 1 {
                s.push_str(&format!(" (+{} more)", errors.len() - 1));
            }
            s
        }
        None => "unknown error".to_string(),
    }
}

/// Read and decode `path`, writing every issue to the log.
pub fn load_logged(path: &Path) -> Decoded {
    let decoded = load_quiet(path);
    log_issues(path, &decoded.issues);
    decoded
}

/// Read and decode `path` (a missing file is the default config).
pub fn load_quiet(path: &Path) -> Decoded {
    if path.exists() {
        decode::decode_file(path)
    } else {
        Decoded {
            config: Config::default_config(),
            issues: Vec::new(),
            includes: Vec::new(),
        }
    }
}

fn log_issues(path: &Path, issues: &[ValidationIssue]) {
    let label = path.display().to_string();
    for issue in issues {
        match issue.severity {
            ValidationSeverity::Error => tracing::error!("config {}", issue.located(&label)),
            ValidationSeverity::Warning => tracing::warn!("config {}", issue.located(&label)),
        }
    }
}

/// Shared with the watcher thread: the names of the files whose changes matter.
#[derive(Default)]
struct WatchState {
    /// Lower-case file names of the config and of every file it includes.
    names: HashSet<String>,
    /// Directories that are being watched.
    dirs: HashSet<PathBuf>,
}

fn name_key(path: &Path) -> Option<String> {
    path.file_name()
        .map(|f| f.to_string_lossy().to_ascii_lowercase())
}

pub struct ConfigLoader {
    config: Arc<RwLock<Config>>,
    path: PathBuf,
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
    /// Sender half of the reload broadcast channel.
    reload_tx: broadcast::Sender<ReloadOutcome>,
    quiet: Duration,
    state: Arc<Mutex<WatchState>>,
}

impl ConfigLoader {
    pub fn new(path: PathBuf) -> Self {
        let decoded = load_quiet(&path);
        let mut state = WatchState::default();
        if let Some(name) = name_key(&path) {
            state.names.insert(name);
        }
        for inc in &decoded.includes {
            if let Some(name) = name_key(inc) {
                state.names.insert(name);
            }
        }
        let config = if decoded.has_errors() {
            Config::default_config()
        } else {
            decoded.config
        };
        let (reload_tx, _) = broadcast::channel(16);

        Self {
            config: Arc::new(RwLock::new(config)),
            path,
            watcher: Arc::new(Mutex::new(None)),
            reload_tx,
            quiet: Duration::from_millis(RELOAD_QUIET_MS),
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// How long the files must stay quiet before they are read again.
    pub fn with_quiet_period(mut self, duration: Duration) -> Self {
        self.quiet = duration;
        self
    }

    /// Load the file now.  Fails with the first error when the file has any; the config
    /// the loader holds is unchanged then.
    pub fn load(&mut self) -> Result<()> {
        let decoded = load_quiet(&self.path);
        let new_config = into_result(decoded, &self.path.display().to_string())?;
        *self.config.write() = new_config;
        Ok(())
    }

    /// Re-read the file now (an explicit reload: no waiting for quiet).
    pub fn reload(&self) -> Result<Config> {
        let decoded = load_quiet(&self.path);
        let new_config = into_result(decoded, &self.path.display().to_string())?;
        *self.config.write() = new_config.clone();
        Ok(new_config)
    }

    /// Watch the config file and its includes.  A change is acted on once the files have
    /// been quiet for [`RELOAD_QUIET_MS`] (a trailing-edge debounce); the result of the
    /// reload goes out on the channel of [`ConfigLoader::subscribe`].  Event driven: the
    /// thread sleeps in `recv` until the watcher says a file changed.
    pub fn start_watching(&mut self) -> Result<()> {
        let (tx, rx) = channel::<()>();
        let state = Arc::clone(&self.state);

        let watcher_result = RecommendedWatcher::new(
            move |res: std::result::Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    if matches!(event.kind, EventKind::Access(_)) {
                        return;
                    }
                    let hit = {
                        let st = state.lock().unwrap();
                        event
                            .paths
                            .iter()
                            .any(|p| name_key(p).map_or(false, |name| st.names.contains(&name)))
                    };
                    if hit {
                        let _ = tx.send(());
                    }
                }
            },
            NotifyConfig::default(),
        );

        match watcher_result {
            Ok(mut watcher) => {
                if let Some(parent) = self.path.parent() {
                    let dir = if parent.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        parent.to_path_buf()
                    };
                    watcher
                        .watch(&dir, RecursiveMode::NonRecursive)
                        .map_err(|e| ConfigError::WatchError(e.to_string()))?;
                    self.state.lock().unwrap().dirs.insert(dir);
                }
                *self.watcher.lock().unwrap() = Some(watcher);
                // The includes of the config as it stands are watched from the start.
                let initial = load_quiet(&self.path);
                watch_includes(&self.watcher, &self.state, &initial.includes);
                self.spawn_reload_handler(rx);
                Ok(())
            }
            Err(e) => Err(ConfigError::WatchError(e.to_string())),
        }
    }

    fn spawn_reload_handler(&self, rx: Receiver<()>) {
        let config = Arc::clone(&self.config);
        let tx = self.reload_tx.clone();
        let path = self.path.clone();
        let quiet = self.quiet;
        let watcher = Arc::clone(&self.watcher);
        let state = Arc::clone(&self.state);

        let spawned = std::thread::Builder::new()
            .name("wiri-config-reload".to_string())
            .spawn(move || {
                while rx.recv().is_ok() {
                    if !settle(&rx, quiet) {
                        return;
                    }
                    let mut decoded = load_quiet(&path);
                    if decoded.has_errors() {
                        // A writer that is still going (a slow save, a sync client) leaves a
                        // half file: look once more after another quiet period before
                        // calling it an error.
                        match rx.recv_timeout(quiet * 2) {
                            Ok(()) => {
                                if !settle(&rx, quiet) {
                                    return;
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                        decoded = load_quiet(&path);
                    }
                    log_issues(&path, &decoded.issues);
                    watch_includes(&watcher, &state, &decoded.includes);
                    if decoded.has_errors() {
                        let summary = failure_summary(&path, &decoded.issues);
                        let _ = tx.send(ReloadOutcome::Failed {
                            issues: decoded.issues,
                            summary,
                        });
                    } else {
                        *config.write() = decoded.config.clone();
                        let _ = tx.send(ReloadOutcome::Applied {
                            config: Box::new(decoded.config),
                            warnings: decoded.issues,
                        });
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start the config reload thread: {}", e);
        }
    }

    pub fn get_config(&self) -> Config {
        self.config.read().clone()
    }

    pub fn config(&self) -> Arc<RwLock<Config>> {
        Arc::clone(&self.config)
    }

    /// Each call produces an independent subscription (broadcast semantics).
    pub fn subscribe(&self) -> broadcast::Receiver<ReloadOutcome> {
        self.reload_tx.subscribe()
    }

    pub fn stop_watching(&mut self) {
        *self.watcher.lock().unwrap() = None;
    }
}

/// Wait until nothing has been signalled for `quiet`.  False when the channel is gone.
fn settle(rx: &Receiver<()>, quiet: Duration) -> bool {
    loop {
        match rx.recv_timeout(quiet) {
            Ok(()) => continue,
            Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// Make sure the files included by the config, and the directories they live in, are
/// watched.
fn watch_includes(
    watcher: &Arc<Mutex<Option<RecommendedWatcher>>>,
    state: &Arc<Mutex<WatchState>>,
    includes: &[PathBuf],
) {
    for inc in includes {
        let name = name_key(inc);
        let dir = inc.parent().map(|p| p.to_path_buf());
        let mut new_dir: Option<PathBuf> = None;
        {
            let mut st = state.lock().unwrap();
            if let Some(name) = name {
                st.names.insert(name);
            }
            if let Some(dir) = dir {
                let dir = if dir.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    dir
                };
                if st.dirs.insert(dir.clone()) {
                    new_dir = Some(dir);
                }
            }
        }
        if let Some(dir) = new_dir {
            if let Some(w) = watcher.lock().unwrap().as_mut() {
                if let Err(e) = w.watch(&dir, RecursiveMode::NonRecursive) {
                    tracing::warn!("cannot watch {}: {}", dir.display(), e);
                }
            }
        }
    }
}

// Config::load delegates to the one loader so there is a single code path.
impl Config {
    /// Read the config file at `path`.  A missing file is the default config.  Fails with the
    /// first error (`file:line:column: error: message`) when the file has any; every issue,
    /// warnings included, is written to the log.
    pub fn load(path: &Path) -> Result<Self> {
        let decoded = load_logged(path);
        into_result(decoded, &path.display().to_string())
    }

    pub fn load_sync(path: &Path) -> Result<Self> {
        Self::load(path)
    }

    /// Parse a KDL configuration string; `include "…"` is resolved against `base_dir` (the
    /// directory of the file the text came from).
    pub fn load_from_str_with_base(input: &str, base_dir: &Path) -> Result<Self> {
        into_result(decode::decode_text(input, "", Some(base_dir)), "")
    }
}

/// Return the first existing config file from the standard search order:
///   1. `$WIRI_CONFIG` env var (explicit override)
///   2. `%APPDATA%\wiri\config.kdl`            — Windows conventional location
///   3. `%USERPROFILE%\.config\wiri\config.kdl` — XDG-style fallback
///
/// Returns `None` when none of those paths exist; callers should fall back to
/// the bundled `default_config_path_for_creation()` location or
/// `Config::default()` and treat the absence as a fresh install.
pub fn default_config_path() -> Option<PathBuf> {
    // 1. Explicit override via environment variable.
    if let Ok(p) = std::env::var("WIRI_CONFIG") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Some(pb);
        }
    }

    // 2. %APPDATA%\wiri\config.kdl  (Windows conventional location)
    if let Ok(appdata) = std::env::var("APPDATA") {
        let pb = PathBuf::from(appdata).join("wiri").join("config.kdl");
        if pb.exists() {
            return Some(pb);
        }
    }

    // 3. %USERPROFILE%\.config\wiri\config.kdl  (XDG-style fallback)
    if let Ok(home) = std::env::var("USERPROFILE") {
        let pb = PathBuf::from(home)
            .join(".config")
            .join("wiri")
            .join("config.kdl");
        if pb.exists() {
            return Some(pb);
        }
    }

    None
}

/// Return the canonical wiri config directory (`%APPDATA%\wiri\`).  Unlike
/// [`default_config_path`], this does NOT require the directory to exist — the
/// tray menu's "Open config folder" entry uses it to create the directory on
/// demand before opening it in Explorer.
pub fn default_config_dir() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|a| PathBuf::from(a).join("wiri"))
}

#[cfg(test)]
mod include_tests {
    use super::*;
    use std::io::Write;

    /// Helper: write a file into a temporary directory and return its path.
    fn write_temp(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).expect("create temp include file");
        f.write_all(contents.as_bytes())
            .expect("write include body");
        p
    }

    /// Build a unique temp directory under `std::env::temp_dir()` so the tests
    /// don't collide when run in parallel.
    fn fresh_temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let unique = format!(
            "wiri_include_{}_{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        p.push(unique);
        std::fs::create_dir_all(&p).expect("create temp dir");
        p
    }

    #[test]
    fn test_include_relative_path_resolves_against_base_dir() {
        let dir = fresh_temp_dir("relative");
        let _ = write_temp(
            &dir,
            "binds.kdl",
            "binds {\n  Ctrl+Alt+Z spawn \"notepad.exe\"\n}\n",
        );

        let main = "input {\n  repeat-delay 333\n}\n\ninclude \"binds.kdl\"\n";
        let cfg = Config::load_from_str_with_base(main, &dir).expect("parse with include");
        assert_eq!(
            cfg.input.repeat_delay, 333,
            "host file's input block must survive"
        );
        assert_eq!(cfg.binds.hotkeys.len(), 1, "included binds must be parsed");
        assert_eq!(cfg.binds.hotkeys[0].command, "spawn");
        assert_eq!(cfg.binds.hotkeys[0].args, vec!["notepad.exe".to_string()]);
    }

    #[test]
    fn test_include_missing_file_is_an_error() {
        let dir = fresh_temp_dir("missing");
        let main = "include \"does_not_exist.kdl\"\n";
        let err =
            Config::load_from_str_with_base(main, &dir).expect_err("missing include must error");
        let msg = format!("{}", err);
        assert!(
            msg.contains("file not found") || msg.contains("not found"),
            "expected missing-file error, got: {}",
            msg
        );
    }

    #[test]
    fn test_include_cycle_is_detected() {
        let dir = fresh_temp_dir("cycle");
        // a.kdl includes b.kdl; b.kdl includes a.kdl
        let a_path = write_temp(&dir, "a.kdl", "include \"b.kdl\"\n");
        let _b_path = write_temp(&dir, "b.kdl", "include \"a.kdl\"\n");

        let host = format!(
            "include \"{}\"\n",
            a_path.file_name().unwrap().to_string_lossy()
        );
        let err =
            Config::load_from_str_with_base(&host, &dir).expect_err("cyclic include must error");
        let msg = format!("{}", err);
        assert!(
            msg.contains("cyclic include"),
            "expected cycle error, got: {}",
            msg
        );
    }

    #[test]
    fn test_include_depth_cap_is_honoured() {
        // Build chain a.kdl → b.kdl → c.kdl → … up to depth MAX+2.
        let dir = fresh_temp_dir("depth");
        let n = (MAX_INCLUDE_DEPTH as usize) + 2;
        for i in 0..n {
            let body = if i + 1 < n {
                format!("include \"f{}.kdl\"\n", i + 1)
            } else {
                "input { repeat-delay 42 }\n".to_string()
            };
            write_temp(&dir, &format!("f{}.kdl", i), &body);
        }
        let host = "include \"f0.kdl\"\n";
        let err = Config::load_from_str_with_base(host, &dir)
            .expect_err("over-deep include chain must error");
        let msg = format!("{}", err);
        assert!(
            msg.contains("depth exceeds"),
            "expected depth-cap error, got: {}",
            msg
        );
    }

    #[test]
    fn a_config_file_is_loaded_with_its_includes_and_reports_the_right_file() {
        let dir = fresh_temp_dir("file");
        write_temp(&dir, "extra.kdl", "layout {\n    gaps 9\n    gapz 1\n}\n");
        let main = write_temp(
            &dir,
            "config.kdl",
            "input { repeat-delay 111 }\ninclude \"extra.kdl\"\n",
        );
        let err = Config::load(&main).expect_err("the include has an error");
        let msg = err.to_string();
        assert!(msg.contains("extra.kdl:3:5: error"), "{}", msg);
        let decoded = load_quiet(&main);
        assert_eq!(decoded.includes.len(), 1);
        let summary = failure_summary(&main, &decoded.issues);
        assert!(summary.contains("extra.kdl:3:5"), "{}", summary);
        // Fixing the include makes the whole file load.
        write_temp(&dir, "extra.kdl", "layout {\n    gaps 9\n}\n");
        let cfg = Config::load(&main).expect("fixed");
        assert_eq!(cfg.input.repeat_delay, 111);
        assert_eq!(cfg.layout.inner_gaps, 9);
    }

    #[test]
    fn a_missing_config_file_is_the_default_config() {
        let dir = fresh_temp_dir("none");
        let cfg = Config::load(&dir.join("nope.kdl")).expect("defaults");
        assert_eq!(cfg.input.keyboard_layout, "us");
    }

    #[test]
    fn a_config_file_with_a_bom_or_in_utf16_loads() {
        let dir = fresh_temp_dir("bom");
        let p = dir.join("bom.kdl");
        std::fs::write(&p, b"\xEF\xBB\xBFinput { repeat-delay 77 }\n").unwrap();
        assert_eq!(Config::load(&p).unwrap().input.repeat_delay, 77);
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "input { repeat-delay 88 }\r\n".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        let p16 = dir.join("utf16.kdl");
        std::fs::write(&p16, utf16).unwrap();
        assert_eq!(Config::load(&p16).unwrap().input.repeat_delay, 88);
    }

    #[test]
    fn the_watcher_applies_only_the_finished_file_and_reports_a_broken_one() {
        let dir = fresh_temp_dir("watch");
        let p = write_temp(&dir, "config.kdl", "input { repeat-delay 100 }\n");
        let mut loader = ConfigLoader::new(p.clone()).with_quiet_period(Duration::from_millis(60));
        assert_eq!(loader.get_config().input.repeat_delay, 100);
        let mut rx = loader.subscribe();
        loader.start_watching().expect("watch");

        // A save in two steps: half a file, then the rest.  Only the finished one may arrive.
        std::fs::write(&p, "input {\n    repeat-del").unwrap();
        std::thread::sleep(Duration::from_millis(15));
        std::fs::write(&p, "input { repeat-delay 250 }\n").unwrap();
        let first = wait_for(&mut rx, Duration::from_secs(5));
        match first {
            Some(ReloadOutcome::Applied { config, .. }) => {
                assert_eq!(config.input.repeat_delay, 250)
            }
            other => panic!("expected the finished file to be applied, got {:?}", other),
        }
        assert_eq!(loader.get_config().input.repeat_delay, 250);

        // A real mistake: reported, and the last good config stays.
        std::fs::write(&p, "input { repeat-delay 250\n    nonsense 1\n}\n").unwrap();
        let second = wait_for(&mut rx, Duration::from_secs(5));
        match second {
            Some(ReloadOutcome::Failed { summary, issues }) => {
                assert!(summary.contains("config.kdl"), "{}", summary);
                assert!(summary.contains(":2:5"), "{}", summary);
                assert!(!issues.is_empty());
            }
            other => panic!("expected a failure, got {:?}", other),
        }
        assert_eq!(loader.get_config().input.repeat_delay, 250);

        // Fixing it applies again.
        std::fs::write(&p, "input { repeat-delay 300 }\n").unwrap();
        match wait_for(&mut rx, Duration::from_secs(5)) {
            Some(ReloadOutcome::Applied { config, .. }) => {
                assert_eq!(config.input.repeat_delay, 300)
            }
            other => panic!("expected the fix to be applied, got {:?}", other),
        }
        loader.stop_watching();
    }

    fn wait_for(
        rx: &mut broadcast::Receiver<ReloadOutcome>,
        timeout: Duration,
    ) -> Option<ReloadOutcome> {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            match rx.try_recv() {
                Ok(v) => return Some(v),
                Err(broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return None,
            }
        }
        None
    }
}
