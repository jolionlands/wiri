use crate::config::error::{ConfigError, Result};
use crate::config::types::Config;
use crate::config::parse;
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use tokio::sync::broadcast;

/// Maximum nesting depth for `include "…"` directives.  Prevents pathological
/// or deeply transitive include chains from exhausting the stack.  niri uses
/// the same kind of guard at a small constant — 8 is generous for real configs
/// (typical depth is 1-2 from a top-level + a `~/wiri/binds.kdl` etc.).
pub const MAX_INCLUDE_DEPTH: u8 = 8;

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
        let base = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Config::load_from_str_with_base(&content, &base)
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

    /// Parse a KDL configuration string, pre-processing `include "…"` lines
    /// against the given base directory.
    ///
    /// Include semantics (niri parity):
    /// * A line whose first non-whitespace token is `include` followed by a
    ///   quoted path is replaced verbatim with the contents of the referenced
    ///   file before KDL parsing.
    /// * The path is resolved relative to `base_dir` (the directory of the
    ///   file containing the directive) when not absolute.
    /// * Cycles are detected via a visited-set keyed on the canonicalised
    ///   path and reported as `ConfigError::ParseError`.
    /// * Nesting is capped at `MAX_INCLUDE_DEPTH`.
    /// * `// include "…"` (comment-prefixed) and `include` lines inside any
    ///   `{ … }` block are NOT expanded — only top-level (depth-0) directives.
    pub fn load_from_str_with_base(input: &str, base_dir: &Path) -> Result<Self> {
        let mut visited: HashSet<PathBuf> = HashSet::new();
        let preprocessed = preprocess_includes(input, base_dir, 0, &mut visited)?;
        parse::parse_kdl_config(&preprocessed)
    }
}

/// Recursively expand top-level `include "…"` lines in `input` against
/// `base_dir`.  Indentation/whitespace inside `{ … }` is honoured so we never
/// inline a file inside a nested block.
///
/// Returns the fully-expanded KDL text.  Errors out on cycle, depth cap, or
/// missing-file.
fn preprocess_includes(
    input: &str,
    base_dir: &Path,
    depth: u8,
    visited: &mut HashSet<PathBuf>,
) -> Result<String> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ConfigError::KdlParse(format!(
            "include depth exceeds {} (likely deep transitive include chain)",
            MAX_INCLUDE_DEPTH
        )));
    }

    let mut output = String::with_capacity(input.len());
    let mut brace_depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;

    for line in input.lines() {
        let trimmed = line.trim_start();

        // Try to handle `include "..."` at brace-depth 0 (not inside any block).
        if brace_depth == 0
            && !trimmed.starts_with("//")
            && (trimmed.starts_with("include ") || trimmed.starts_with("include\""))
        {
            if let Some(path_str) = parse_include_path(trimmed) {
                let raw_path = PathBuf::from(&path_str);
                let resolved = if raw_path.is_absolute() {
                    raw_path
                } else {
                    base_dir.join(raw_path)
                };

                if !resolved.exists() {
                    return Err(ConfigError::KdlParse(format!(
                        "include: file not found: {}",
                        resolved.display()
                    )));
                }

                // Canonicalise for cycle detection.  Fall back to the
                // un-canonicalised path on failure (e.g. permission denied)
                // so we still detect literal self-includes.
                let canon = std::fs::canonicalize(&resolved)
                    .unwrap_or_else(|_| resolved.clone());

                if visited.contains(&canon) {
                    return Err(ConfigError::KdlParse(format!(
                        "include: cyclic include detected for {}",
                        canon.display()
                    )));
                }

                let body = std::fs::read_to_string(&resolved)
                    .map_err(ConfigError::IoError)?;

                visited.insert(canon.clone());
                let nested_base = resolved
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| base_dir.to_path_buf());
                let expanded =
                    preprocess_includes(&body, &nested_base, depth + 1, visited)?;
                visited.remove(&canon);

                output.push_str(&expanded);
                // Always end with a newline so the included file's last line
                // doesn't accidentally merge with the next host line.
                if !expanded.ends_with('\n') {
                    output.push('\n');
                }
                continue;
            }
        }

        // Track brace depth for subsequent lines so we don't expand `include`
        // directives that happen to live inside a `{ … }` block (KDL parser
        // would reject them anyway, but we want a clean preprocess pass).
        // Skip characters inside double-quoted strings.
        for ch in line.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                in_string = !in_string;
                continue;
            }
            if !in_string {
                if ch == '{' {
                    brace_depth += 1;
                } else if ch == '}' {
                    brace_depth = (brace_depth - 1).max(0);
                }
            }
        }
        // Newlines reset the in_string state so a stray opening quote on one
        // line cannot run away across the whole file.
        in_string = false;
        escaped = false;

        output.push_str(line);
        output.push('\n');
    }

    Ok(output)
}

/// Extract the path argument from an `include "…"` directive line.
///
/// Accepts both `include "path"` and `include="path"` (KDL-equals style).
/// Returns `None` when the line isn't a well-formed include directive (e.g.
/// missing quotes, unrecognised token).
fn parse_include_path(line: &str) -> Option<String> {
    let line = line.trim();
    // Strip `include` keyword and optional `=`.
    let rest = if let Some(r) = line.strip_prefix("include") {
        r.trim_start().trim_start_matches('=').trim_start()
    } else {
        return None;
    };

    // Strip trailing inline comment (// …) outside the quoted string.
    let rest = strip_trailing_comment(rest);

    // Must be a quoted string token.
    if !rest.starts_with('"') {
        return None;
    }
    let body = &rest[1..];
    let end = body.find('"')?;
    Some(body[..end].to_string())
}

fn strip_trailing_comment(input: &str) -> &str {
    let mut in_string = false;
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_string = !in_string;
        } else if !in_string && b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            return input[..i].trim_end();
        }
        i += 1;
    }
    input.trim_end()
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

#[cfg(test)]
mod include_tests {
    use super::*;
    use std::io::Write;

    /// Helper: write a file into a temporary directory and return its path.
    fn write_temp(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).expect("create temp include file");
        f.write_all(contents.as_bytes()).expect("write include body");
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
        assert_eq!(cfg.input.repeat_delay, 333, "host file's input block must survive");
        assert_eq!(cfg.binds.hotkeys.len(), 1, "included binds must be parsed");
        assert_eq!(cfg.binds.hotkeys[0].command, "spawn");
        assert_eq!(cfg.binds.hotkeys[0].args, vec!["notepad.exe".to_string()]);
    }

    #[test]
    fn test_include_missing_file_is_an_error() {
        let dir = fresh_temp_dir("missing");
        let main = "include \"does_not_exist.kdl\"\n";
        let err = Config::load_from_str_with_base(main, &dir)
            .expect_err("missing include must error");
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

        // Load via path so the visited set seeds correctly.
        let host = format!("include \"{}\"\n", a_path.file_name().unwrap().to_string_lossy());
        let err = Config::load_from_str_with_base(&host, &dir)
            .expect_err("cyclic include must error");
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
    fn test_include_inside_block_is_not_expanded() {
        // An include sitting inside a `layout { … }` block must be treated as
        // a normal (unknown) KDL line, not pre-processed.
        let dir = fresh_temp_dir("nested");
        let _ = write_temp(&dir, "shouldnotload.kdl", "FAILURE_TOKEN_42 nope\n");
        let main = "layout {\n    include \"shouldnotload.kdl\"\n}\n";
        let cfg = Config::load_from_str_with_base(main, &dir)
            .expect("nested include must be a no-op, not crash");
        // The default layout config should be untouched (no FAILURE_TOKEN leaked into a field).
        assert_eq!(cfg.layout.inner_gaps, 16);
    }

    #[test]
    fn test_strip_trailing_comment_helper() {
        assert_eq!(strip_trailing_comment("\"a/b\""), "\"a/b\"");
        assert_eq!(strip_trailing_comment("\"a\"  // tail"), "\"a\"");
        assert_eq!(strip_trailing_comment("no comment"), "no comment");
    }

    #[test]
    fn test_parse_include_path_quoted() {
        assert_eq!(parse_include_path("include \"foo.kdl\""), Some("foo.kdl".to_string()));
        assert_eq!(parse_include_path("include=\"bar.kdl\""), Some("bar.kdl".to_string()));
        assert_eq!(parse_include_path("includes \"x\""), None);
        assert_eq!(parse_include_path("include without_quotes"), None);
    }
}
