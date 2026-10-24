//! Logging for a program that has no console.
//!
//! `wiri.exe` is a GUI-subsystem program (starting it must not open a console
//! window, which wiri would then tile).  A GUI process has no console to print
//! to, so its log goes to `%LOCALAPPDATA%\wiri\wiri.log`, rotated by size.  When
//! wiri is started from a shell (`wiri -v`) it additionally attaches to the
//! shell's console and mirrors the log there.

use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing_subscriber::fmt::MakeWriter;

/// Rotate the log when it grows past this size.
pub const LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// `%LOCALAPPDATA%\wiri\wiri.log`.
pub fn default_log_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("wiri").join("wiri.log"))
}

/// `wiri.log` -> `wiri.log.1`: where the previous log goes when it rotates.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// An append-only log file that rotates itself: when the next write would push
/// it past `max_bytes`, the current file becomes `<name>.1` (replacing the
/// previous backup) and a fresh file is started.  At most two files, so the log
/// directory never grows beyond about twice `max_bytes`.
pub struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
}

impl RotatingFile {
    /// Open (creating directories as needed) the log at `path`.  A file that is
    /// already over the limit from a previous run is rotated straight away.
    pub fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut log = Self {
            path,
            file: None,
            size: 0,
            max_bytes,
        };
        log.reopen()?;
        if log.size >= log.max_bytes {
            log.rotate()?;
        }
        Ok(log)
    }

    fn reopen(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.size = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Close the file first: Windows will not rename a file that is open
        // without FILE_SHARE_DELETE, and we do not want to depend on that.
        self.file = None;
        let backup = backup_path(&self.path);
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::rename(&self.path, &backup);
        self.reopen()
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.file.is_none() {
            self.reopen()?;
        }
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        if let Some(file) = self.file.as_mut() {
            file.write_all(buf)?;
            self.size += buf.len() as u64;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// A [`RotatingFile`] shared between all threads that log, in the form the
/// `tracing` fmt layer wants (`MakeWriter`).
#[derive(Clone)]
pub struct SharedLog(Arc<Mutex<RotatingFile>>);

impl SharedLog {
    pub fn new(file: RotatingFile) -> Self {
        Self(Arc::new(Mutex::new(file)))
    }
}

/// One `write` handle onto a [`SharedLog`].
pub struct LogWriter(Arc<Mutex<RotatingFile>>);

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // A log that cannot be written must never take the window manager
        // down (or spam the error handler): pretend the write succeeded.
        let _ = self.0.lock().write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.0.lock().flush();
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedLog {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

/// True when standard error leads somewhere: the process was started with a
/// redirect (`2> file`, `Start-Process -RedirectStandardError`) or from a
/// console.  A GUI-subsystem process started from Explorer has no stderr.
pub fn stderr_is_usable() -> bool {
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
    match unsafe { GetStdHandle(STD_ERROR_HANDLE) } {
        Ok(handle) => !handle.is_invalid() && !handle.0.is_null(),
        Err(_) => false,
    }
}

/// Attach to the console of the process that started wiri, if it has one, and
/// point any standard stream that is not already redirected at it.
///
/// A GUI-subsystem program starts with no console, so `wiri -v`, `--help` and
/// command-line errors would print into the void.  This makes them appear in
/// the shell that launched wiri, without ever creating a console window of
/// our own.  Streams that were redirected (`2> wiri.log`) are left alone.
/// Returns true when a console was attached.
pub fn attach_parent_console() -> bool {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            // Started by Explorer, the Run key or a service: no console to use.
            return false;
        }
        for std_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let usable = match GetStdHandle(std_handle) {
                Ok(h) => !h.is_invalid() && !h.0.is_null(),
                Err(_) => false,
            };
            if usable {
                continue;
            }
            if let Ok(console) = OpenOptions::new().write(true).open("CONOUT$") {
                // The handle intentionally lives for the rest of the process.
                let _ = SetStdHandle(std_handle, HANDLE(console.into_raw_handle()));
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wiri-log-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_backup_path_appends_dot_one() {
        assert_eq!(
            backup_path(Path::new(r"C:\Users\x\AppData\Local\wiri\wiri.log")),
            PathBuf::from(r"C:\Users\x\AppData\Local\wiri\wiri.log.1")
        );
    }

    #[test]
    fn test_rotating_file_rotates_when_the_limit_is_reached() {
        let dir = temp_dir("rotate");
        let path = dir.join("wiri.log");
        let mut log = RotatingFile::open(path.clone(), 100).unwrap();

        log.write_all(&[b'a'; 60]).unwrap();
        log.write_all(&[b'b'; 30]).unwrap(); // 90 bytes: still one file
        assert!(!backup_path(&path).exists());

        log.write_all(&[b'c'; 30]).unwrap(); // would reach 120: rotate first
        log.flush().unwrap();
        let old = std::fs::read(backup_path(&path)).unwrap();
        let new = std::fs::read(&path).unwrap();
        assert_eq!(old.len(), 90);
        assert!(old.starts_with(b"aaa") && old.ends_with(b"bbb"));
        assert_eq!(new, vec![b'c'; 30]);

        // A second rotation replaces the backup, it does not pile up files.
        log.write_all(&[b'd'; 80]).unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), vec![b'c'; 30]);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rotating_file_rotates_an_oversized_file_at_startup() {
        let dir = temp_dir("startup");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wiri.log");
        std::fs::write(&path, vec![b'x'; 500]).unwrap();

        let mut log = RotatingFile::open(path.clone(), 100).unwrap();
        log.write_all(b"fresh").unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), vec![b'x'; 500]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rotating_file_appends_to_a_small_existing_log() {
        let dir = temp_dir("append");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wiri.log");
        std::fs::write(&path, b"old\n").unwrap();

        let mut log = RotatingFile::open(path.clone(), 1000).unwrap();
        log.write_all(b"new\n").unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old\nnew\n");
        assert!(!backup_path(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_shared_log_writer_never_fails() {
        let dir = temp_dir("shared");
        let log = SharedLog::new(RotatingFile::open(dir.join("wiri.log"), 1000).unwrap());
        let mut writer = log.make_writer();
        assert_eq!(writer.write(b"hello").unwrap(), 5);
        writer.flush().unwrap();
        assert_eq!(std::fs::read(dir.join("wiri.log")).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
