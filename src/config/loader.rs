use crate::config::error::{ConfigError, Result};
use crate::config::types::Config;
use crate::config::parse;
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use tokio::sync::broadcast;

pub struct ConfigLoader {
    config: Arc<RwLock<Config>>,
    path: PathBuf,
    watcher: Mutex<Option<RecommendedWatcher>>,
    /// Sender half of the reload broadcast channel.
    reload_tx: broadcast::Sender<Config>,
    last_reload: Arc<Mutex<Instant>>,
    debounce_duration: Duration,
}

impl ConfigLoader {
    pub fn new(path: PathBuf) -> Self {
        let config = load_sync_inner(&path).unwrap_or_else(|_| Config::default_config());
        // Bug fix #1/#3: use broadcast so subscribe() hands out real receivers and
        // the sender is kept alive for the lifetime of ConfigLoader.
        let (reload_tx, _) = broadcast::channel(16);

        Self {
            config: Arc::new(RwLock::new(config)),
            path,
            watcher: Mutex::new(None),
            reload_tx,
            last_reload: Arc::new(Mutex::new(Instant::now())),
            // Bug fix #9: debounce is 100 ms per SPEC.md (was 300 ms).
            debounce_duration: Duration::from_millis(100),
        }
    }

    pub fn with_debounce(mut self, duration: Duration) -> Self {
        self.debounce_duration = duration;
        self
    }

    // Bug fix #4: single canonical load_sync; Config::load_sync (duplicate) delegates here.
    fn load_sync(path: &Path) -> Result<Config> {
        load_sync_inner(path)
    }

    pub fn load(&mut self) -> Result<()> {
        let new_config = Self::load_sync(&self.path)?;
        *self.config.write() = new_config;
        Ok(())
    }

    pub fn reload(&self) -> Result<Config> {
        let now = Instant::now();
        let mut last = self.last_reload.lock().unwrap();

        if now.duration_since(*last) < self.debounce_duration {
            return Err(ConfigError::ReloadError(
                "Reload debounced".to_string(),
            ));
        }

        *last = now;
        drop(last);

        let new_config = Self::load_sync(&self.path)?;
        *self.config.write() = new_config.clone();

        Ok(new_config)
    }

    pub fn start_watching(&mut self) -> Result<()> {
        let path = self.path.clone();
        let debounce = self.debounce_duration;

        let (tx, rx) = channel::<Result<Config>>();

        // Bug fix #2: Instant-based gating instead of thread::sleep inside the
        // notify callback (which runs on notify's internal thread).
        let last_reload = Arc::clone(&self.last_reload);

        let watcher_result = RecommendedWatcher::new(
            move |res: std::result::Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        notify::EventKind::Create(_) |
                        notify::EventKind::Modify(_) |
                        notify::EventKind::Remove(_) => {
                            let now = Instant::now();
                            let mut last = last_reload.lock().unwrap();
                            if now.duration_since(*last) < debounce {
                                return; // debounced — no sleep, just skip
                            }
                            *last = now;
                            drop(last);

                            match load_sync_inner(&path) {
                                Ok(config) => { let _ = tx.send(Ok(config)); }
                                Err(e)     => { let _ = tx.send(Err(e)); }
                            }
                        }
                        _ => {}
                    }
                }
            },
            NotifyConfig::default(),
        );

        match watcher_result {
            Ok(mut watcher) => {
                if let Some(parent) = self.path.parent() {
                    watcher.watch(parent, RecursiveMode::NonRecursive)
                        .map_err(|e| ConfigError::WatchError(e.to_string()))?;
                }

                let _ = watcher.watch(
                    self.path.as_path(),
                    RecursiveMode::NonRecursive,
                );

                *self.watcher.lock().unwrap() = Some(watcher);
                self.spawn_reload_handler(rx);
                Ok(())
            }
            Err(e) => Err(ConfigError::WatchError(e.to_string())),
        }
    }

    fn spawn_reload_handler(&self, rx: Receiver<Result<Config>>) {
        let config = Arc::clone(&self.config);
        let tx = self.reload_tx.clone();

        std::thread::spawn(move || {
            while let Ok(result) = rx.recv() {
                if let Ok(new_config) = result {
                    let config_clone = new_config.clone();
                    *config.write() = new_config;
                    // broadcast::Sender::send fails only when there are no receivers —
                    // that is fine; we keep running so future subscribers still work.
                    let _ = tx.send(config_clone);
                }
            }
        });
    }

    pub fn get_config(&self) -> Config {
        self.config.read().clone()
    }

    pub fn config(&self) -> Arc<RwLock<Config>> {
        Arc::clone(&self.config)
    }

    /// Bug fix #3: return a real receiver from the stored broadcast channel.
    /// Each call produces an independent subscription (broadcast semantics).
    pub fn subscribe(&self) -> broadcast::Receiver<Config> {
        self.reload_tx.subscribe()
    }

    pub fn stop_watching(&mut self) {
        *self.watcher.lock().unwrap() = None;
    }
}

// ── free function so both ConfigLoader::load_sync and Config::load_sync
// can delegate without duplicating logic (bug fix #4).
fn load_sync_inner(path: &Path) -> Result<Config> {
    if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(ConfigError::IoError)?;
        parse::parse_kdl_config(&content)
    } else {
        Ok(Config::default_config())
    }
}

// Bug fix #4: Config::load_sync now delegates; body is no longer duplicated.
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        load_sync_inner(path)
    }

    pub fn load_sync(path: &Path) -> Result<Self> {
        load_sync_inner(path)
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
        let pb = PathBuf::from(home).join(".config").join("wiri").join("config.kdl");
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
    std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join("wiri"))
}
