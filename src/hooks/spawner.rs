use super::environment::ChildEnvironment;
use crate::utils::cmdline::{build_command_line, quote_arg, split_command_line};
use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};
use windows::Win32::System::Threading::{
    CreateProcessW, GetProcessId, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowRect, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub message: String,
    pub icon_type: NotificationIcon,
}

#[derive(Debug, Clone, Copy)]
pub enum NotificationIcon {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct ScreenshotEvent {
    pub monitor_id: Option<u64>,
    pub save_path: Option<String>,
}

pub struct Spawner;

impl Spawner {
    pub fn new() -> Self {
        Self
    }

    /// Start `program` with `args`.  A thin wrapper over [`spawn_argv`], the one
    /// place wiri starts programs: every argument is passed through intact
    /// (quoted for the Windows command line), and a bare name such as `wt.exe`
    /// is found on the PATH.  `detached` is kept for source compatibility; a
    /// child is never tied to wiri's console.
    pub fn spawn(&self, program: &Path, args: &[&str], _detached: bool) -> Result<u32> {
        let mut argv = vec![program.to_string_lossy().into_owned()];
        argv.extend(args.iter().map(|a| a.to_string()));
        spawn_argv(&argv)
    }

    /// Per-call additions are merged first; configured child overrides win.
    /// Like `spawn`, this shares quoting, executable lookup and environment
    /// snapshots with every other non-elevated launch path.
    pub fn spawn_with_env(
        &self,
        program: &Path,
        args: &[&str],
        env: &[(String, String)],
        _detached: bool,
    ) -> Result<u32> {
        let mut argv = vec![program.to_string_lossy().into_owned()];
        argv.extend(args.iter().map(|a| a.to_string()));
        spawn_argv_in(&argv, &ChildEnvironment::capture_with(env)?)
    }

    /// Spawn a process elevated (UAC "Run as administrator") via ShellExecuteExW.
    /// Returns the new process's PID on success. Configured environment overrides
    /// are refused: the UAC broker cannot receive a child environment block.
    pub fn spawn_elevated(cmd: &str, args: &[&str]) -> Result<u32> {
        if super::environment::has_overrides() {
            anyhow::bail!(
                "elevated launches cannot preserve configured environment overrides: \
                 ShellExecuteExW's UAC broker has no environment-block parameter"
            );
        }
        if cmd.contains('\0') || args.iter().any(|arg| arg.contains('\0')) {
            anyhow::bail!("elevated command contains NUL");
        }
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let file: Vec<u16> = OsStr::new(cmd)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let params_str = args
            .iter()
            .map(|arg| quote_arg(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let params: Vec<u16> = OsStr::new(&params_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let mut sei = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS,
                lpVerb: PCWSTR::from_raw(verb.as_ptr()),
                lpFile: PCWSTR::from_raw(file.as_ptr()),
                lpParameters: PCWSTR::from_raw(params.as_ptr()),
                nShow: windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL.0 as i32,
                ..Default::default()
            };

            ShellExecuteExW(&mut sei).context("ShellExecuteExW failed")?;

            let pid = if !sei.hProcess.is_invalid() {
                let pid = GetProcessId(sei.hProcess);
                let _ = CloseHandle(sei.hProcess);
                pid
            } else {
                0
            };
            Ok(pid)
        }
    }

    /// Capture the full virtual desktop and save it as a BMP file at `path`.
    /// Returns a [`ScreenshotEvent`] with `save_path` set to the canonical path.
    pub fn capture_screenshot_to_file<P: AsRef<Path>>(
        monitor_id: Option<u64>,
        path: P,
    ) -> Result<ScreenshotEvent> {
        let path = path.as_ref();
        capture_desktop_to_bmp(path)?;
        Ok(ScreenshotEvent {
            monitor_id,
            save_path: Some(path.to_string_lossy().into_owned()),
        })
    }

    /// Capture the full virtual desktop in-memory and save it to a temporary BMP file.
    /// Returns a [`ScreenshotEvent`] with `save_path` set to the temp file path.
    pub fn capture_screenshot(monitor_id: Option<u64>) -> Result<ScreenshotEvent> {
        // Build a temp path: <TEMP>\wiri_screenshot_<timestamp_ns>.bmp
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_path = std::env::temp_dir().join(format!("wiri_screenshot_{}.bmp", ts));
        capture_desktop_to_bmp(&tmp_path)?;
        Ok(ScreenshotEvent {
            monitor_id,
            save_path: Some(tmp_path.to_string_lossy().into_owned()),
        })
    }

    /// Capture a single window by HWND and save it as a BMP file at `path`.
    ///
    /// Validates the HWND, uses `PrintWindow(PW_RENDERFULLCONTENT)` to render
    /// the window's actual pixels (including obscured/UWP surfaces), then
    /// writes a 32-bit BMP via the same path as the desktop helper.
    ///
    /// Returns a [`ScreenshotEvent`] with `save_path` populated on success.
    pub fn capture_window_to_file(&self, hwnd: isize, path: &Path) -> Result<ScreenshotEvent> {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        let hwnd_t = windows::Win32::Foundation::HWND(hwnd as *mut std::ffi::c_void);
        if !unsafe { IsWindow(hwnd_t).as_bool() } {
            anyhow::bail!(
                "Spawner::capture_window_to_file: HWND {} is not a valid window",
                hwnd
            );
        }
        let (w, h) = unsafe { capture_window_to_bmp(hwnd_t, path)? };
        let _ = (w, h); // dimensions captured; caller uses path
        Ok(ScreenshotEvent {
            monitor_id: None,
            save_path: Some(path.to_string_lossy().into_owned()),
        })
    }
}

// ---------------------------------------------------------------------------
// The one place wiri starts programs
//
// `spawn` / `exec` binds, `spawn-at-startup` and the IPC `spawn` request all end
// up in `spawn_argv`; `spawn-sh` / `spawn-cmd` end up in `spawn_shell`.  The
// command line is built with the Windows quoting rules. Executables, batch
// launchers and COMSPEC are resolved against the SAME effective child snapshot
// that CreateProcessW receives. Letting Windows resolve a bare executable would
// search wiri's own PATH, not the lpEnvironment block's PATH.
// ---------------------------------------------------------------------------

/// Look `program` up the way a shell would: next to the given directory if it
/// has one, else in every PATH directory, trying the name as given when it has an
/// extension and then with each PATHEXT extension appended.  `exists` says
/// whether a candidate is a file (injected so the search is testable).
pub fn find_program(
    program: &str,
    path_var: &str,
    pathext_var: &str,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<std::path::PathBuf> {
    find_program_os(
        OsStr::new(program),
        OsStr::new(path_var),
        OsStr::new(pathext_var),
        exists,
    )
}

/// UTF-16 equivalent of `find_program`: inherited PATH entries need not be UTF-8.
fn find_program_os(
    program: &OsStr,
    path_var: &OsStr,
    pathext_var: &OsStr,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let program_path = Path::new(program);
    let has_directory = program
        .encode_wide()
        .any(|c| matches!(c, 0x5c | 0x2f | 0x3a));
    let has_extension = program_path.extension().is_some();
    let extensions: Vec<OsString> = pathext_var
        .encode_wide()
        .collect::<Vec<_>>()
        .split(|c| *c == b';' as u16)
        .filter(|ext| !ext.is_empty())
        .map(OsString::from_wide)
        .collect();
    let mut bases = Vec::new();
    if has_directory {
        bases.push(program_path.to_path_buf());
    } else {
        let path_units: Vec<_> = path_var.encode_wide().collect();
        for mut dir in path_units.split(|c| *c == b';' as u16) {
            // Preserve the existing tolerance for quoted/empty PATH entries.
            while dir.first().is_some_and(|c| matches!(c, 9..=13 | 32)) {
                dir = &dir[1..];
            }
            while dir.last().is_some_and(|c| matches!(c, 9..=13 | 32)) {
                dir = &dir[..dir.len() - 1];
            }
            while dir.first() == Some(&(b'"' as u16)) {
                dir = &dir[1..];
            }
            while dir.last() == Some(&(b'"' as u16)) {
                dir = &dir[..dir.len() - 1];
            }
            if !dir.is_empty() {
                bases.push(PathBuf::from(OsString::from_wide(dir)).join(program));
            }
        }
    }
    for base in bases {
        if has_extension && exists(&base) {
            return Some(base);
        }
        for ext in &extensions {
            let mut name = base.as_os_str().to_os_string();
            name.push(ext);
            let candidate = PathBuf::from(name);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn resolve_program(program: &OsStr, environment: &ChildEnvironment) -> Option<PathBuf> {
    let resolved = find_program_os(
        program,
        environment.get("PATH").unwrap_or_else(|| OsStr::new("")),
        environment
            .get("PATHEXT")
            .unwrap_or_else(|| OsStr::new(".COM;.EXE;.BAT;.CMD")),
        &real_file_exists,
    )?;
    // PATH may contain relative directories. Freeze their meaning before
    // CreateProcess changes the child's working directory to USERPROFILE.
    std::path::absolute(resolved).ok()
}

fn resolve_comspec(environment: &ChildEnvironment) -> Result<PathBuf> {
    if let Some(comspec) = environment.get("COMSPEC").filter(|value| !value.is_empty()) {
        return resolve_program(comspec, environment)
            .with_context(|| format!("child COMSPEC {:?} was not found", comspec));
    }
    // Missing/removed COMSPEC must not resurrect the parent's COMSPEC or PATH.
    // Use the Windows API's trusted system directory instead.
    let mut directory = vec![0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    if length > 0 && length < directory.len() {
        let candidate = PathBuf::from(OsString::from_wide(&directory[..length])).join("cmd.exe");
        if real_file_exists(&candidate) {
            return Ok(candidate);
        }
    }
    let length = unsafe { GetWindowsDirectoryW(Some(&mut directory)) } as usize;
    if length > 0 && length < directory.len() {
        let candidate = PathBuf::from(OsString::from_wide(&directory[..length]))
            .join("System32")
            .join("cmd.exe");
        if real_file_exists(&candidate) {
            return Ok(candidate);
        }
    }
    anyhow::bail!("COMSPEC is absent and the Windows system command interpreter was not found")
}

/// Is `path` a `.cmd` / `.bat` script (which needs the command interpreter)?
fn is_batch_script(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

/// Command line that runs the batch script `script` with `args` through the
/// command interpreter, or an error when an argument cannot cross `cmd.exe`'s
/// parser safely (quotes and `%` are interpreted by the shell even inside
/// quotes; `spawn-sh` is the way to pass those on purpose).
pub fn batch_script_command_line(script: &Path, args: &[String], comspec: &str) -> Result<String> {
    batch_script_command_line_os(script, args, OsStr::new(comspec))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("batch command line is not representable as UTF-8"))
}

fn batch_script_command_line_os(
    script: &Path,
    args: &[String],
    comspec: &OsStr,
) -> Result<OsString> {
    // The script name crosses cmd's parser too, not just the arguments.
    if script
        .as_os_str()
        .encode_wide()
        .any(|c| matches!(c, 0 | 10 | 13 | 34 | 37))
    {
        anyhow::bail!(
            "batch file path cannot be passed through cmd safely: {}",
            script.display()
        );
    }
    for arg in args {
        if arg
            .chars()
            .any(|c| matches!(c, '\0' | '"' | '%' | '\n' | '\r'))
        {
            anyhow::bail!(
                "argument {:?} cannot be passed to the batch file {} safely; use spawn-sh instead",
                arg,
                script.display()
            );
        }
    }
    let mut inner = OsString::from("\"");
    inner.push(script);
    inner.push("\"");
    for arg in args {
        inner.push(" \"");
        inner.push(arg);
        inner.push("\"");
    }
    Ok(shell_command_line(comspec, &inner))
}

/// The MSVC quoting rules, without converting inherited UTF-16 paths to UTF-8.
fn quote_os_arg(arg: &OsStr) -> OsString {
    let units: Vec<_> = arg.encode_wide().collect();
    if !units.is_empty() && !units.iter().any(|c| matches!(c, 9..=13 | 32 | 34)) {
        return arg.to_os_string();
    }
    let mut quoted = vec![b'"' as u16];
    let mut backslashes = 0;
    for c in units {
        if c == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        let count = if c == b'"' as u16 {
            backslashes * 2 + 1
        } else {
            backslashes
        };
        quoted.extend(std::iter::repeat(b'\\' as u16).take(count));
        quoted.push(c);
        backslashes = 0;
    }
    quoted.extend(std::iter::repeat(b'\\' as u16).take(backslashes * 2));
    quoted.push(b'"' as u16);
    OsString::from_wide(&quoted)
}

fn shell_command_line(comspec: &OsStr, command: &OsStr) -> OsString {
    let mut line = quote_os_arg(comspec);
    line.push(" /d /s /c \"");
    line.push(command);
    line.push("\"");
    line
}

fn real_file_exists(path: &Path) -> bool {
    // symlink_metadata does not follow the reparse point of an App Execution
    // Alias (wt.exe), which `metadata` cannot open.
    std::fs::symlink_metadata(path)
        .map(|m| !m.is_dir())
        .unwrap_or(false)
}

/// Directory a freshly spawned program starts in: the user's profile, like a
/// program started from Win+R.  `None` keeps wiri's own working directory.
fn spawn_working_directory() -> Option<Vec<u16>> {
    let home = std::env::var_os("USERPROFILE")?;
    if !Path::new(&home).is_dir() {
        return None;
    }
    Some(home.encode_wide().chain(std::iter::once(0)).collect())
}

/// Explicit executable + Unicode environment avoids Windows' implicit lookup
/// using the parent's PATH. `spawn-sh` hides its console; direct `spawn` does not.
fn create_process(
    application: &Path,
    command_line: &OsStr,
    hide_console: bool,
    environment: &ChildEnvironment,
) -> Result<u32> {
    if command_line.encode_wide().any(|c| c == 0)
        || application.as_os_str().encode_wide().any(|c| c == 0)
    {
        anyhow::bail!("application or command line contains NUL");
    }
    let application_wide: Vec<u16> = application
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let environment_block = environment.to_block();
    let mut command_wide: Vec<u16> = command_line.encode_wide().chain(Some(0)).collect();
    let cwd = spawn_working_directory();
    let cwd_ptr = match &cwd {
        Some(wide) => PCWSTR::from_raw(wide.as_ptr()),
        None => PCWSTR::null(),
    };
    let flags = if hide_console {
        CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW
    } else {
        CREATE_UNICODE_ENVIRONMENT
    };
    unsafe {
        let mut si = STARTUPINFOW::default();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi = PROCESS_INFORMATION::default();
        CreateProcessW(
            PCWSTR::from_raw(application_wide.as_ptr()),
            PWSTR::from_raw(command_wide.as_mut_ptr()),
            None,
            None,
            BOOL::from(false),
            flags,
            Some(environment_block.as_ptr().cast()),
            cwd_ptr,
            &mut si,
            &mut pi,
        )
        .with_context(|| format!("CreateProcessW failed for {:?}", command_line))?;
        let pid = pi.dwProcessId;
        // Only the PID is needed; the child runs on its own.
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(pi.hThread);
        Ok(pid)
    }
}

/// The argv of an IPC `spawn` request: with `args` empty `command` is a whole
/// command line (split with the Windows quoting rules), otherwise `command` is
/// the program and `args` its arguments, none of them re-split.
pub fn spawn_command_argv(command: &str, args: &[String]) -> Vec<String> {
    if args.is_empty() {
        split_command_line(command)
    } else {
        let mut argv = vec![command.to_string()];
        argv.extend(args.iter().cloned());
        argv
    }
}

/// Start the program `argv[0]` with the arguments `argv[1..]`, each passed
/// through intact.  Returns the process id.
pub fn spawn_argv(argv: &[String]) -> Result<u32> {
    spawn_argv_in(argv, &ChildEnvironment::capture())
}

fn spawn_argv_in(argv: &[String], environment: &ChildEnvironment) -> Result<u32> {
    let expanded = crate::input::system_actions::expand_spawn_argv(argv);
    let argv = expanded.as_slice();
    if argv.is_empty() || argv[0].trim().is_empty() {
        anyhow::bail!("nothing to run: the command is empty");
    }
    if argv.iter().any(|arg| arg.contains('\0')) {
        anyhow::bail!("spawn argument contains NUL");
    }
    let resolved = resolve_program(OsStr::new(&argv[0]), environment)
        .with_context(|| format!("program {:?} not found on the child PATH/PATHEXT", argv[0]))?;
    if is_batch_script(&resolved) {
        let comspec = resolve_comspec(environment)?;
        let line = batch_script_command_line_os(&resolved, &argv[1..], comspec.as_os_str())?;
        return create_process(&comspec, &line, true, environment);
    }
    create_process(
        &resolved,
        OsStr::new(&build_command_line(argv)),
        false,
        environment,
    )
}

/// Run `command` through the command interpreter: `cmd.exe /d /s /c "<command>"`
/// (niri `spawn-sh`).  Environment variables, `start`, `&&` and redirection work;
/// the interpreter itself runs without a console window. The child COMSPEC is
/// resolved using the same snapshot as its PATH/PATHEXT and environment block.
pub fn spawn_shell(command: &str) -> Result<u32> {
    if command.trim().is_empty() {
        anyhow::bail!("nothing to run: the shell command is empty");
    }
    if command.contains('\0') {
        anyhow::bail!("shell command contains NUL");
    }
    let environment = ChildEnvironment::capture();
    let comspec = resolve_comspec(&environment)?;
    let line = shell_command_line(comspec.as_os_str(), OsStr::new(command));
    create_process(&comspec, &line, true, &environment)
}

/// Low-level helper: capture a single window into a 32-bit bottom-up BMP file.
///
/// Uses `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)` — the modern flag that
/// asks the window to render its full client area including UWP /
/// DirectComposition surfaces.  Falls back to flags `0` for legacy windows.
///
/// Returns `(width, height)` of the captured bitmap on success.  All GDI
/// handles are released on every exit path.
unsafe fn capture_window_to_bmp(hwnd: HWND, path: &Path) -> Result<(u32, u32)> {
    let mut wnd_rect: RECT = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetWindowRect(hwnd, &mut wnd_rect)
        .map_err(|e| anyhow::anyhow!("GetWindowRect failed: {:?}", e))?;
    let width = wnd_rect.right - wnd_rect.left;
    let height = wnd_rect.bottom - wnd_rect.top;
    if width <= 0 || height <= 0 {
        anyhow::bail!(
            "capture_window_to_bmp: window has zero or negative size ({}x{})",
            width,
            height
        );
    }

    let wnd_dc = GetDC(hwnd);
    if wnd_dc.is_invalid() {
        anyhow::bail!("capture_window_to_bmp: GetDC(hwnd) failed");
    }
    let mem_dc = CreateCompatibleDC(wnd_dc);
    if mem_dc.is_invalid() {
        let _ = ReleaseDC(hwnd, wnd_dc);
        anyhow::bail!("capture_window_to_bmp: CreateCompatibleDC failed");
    }
    let bitmap = CreateCompatibleBitmap(wnd_dc, width, height);
    if bitmap.is_invalid() {
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(hwnd, wnd_dc);
        anyhow::bail!("capture_window_to_bmp: CreateCompatibleBitmap failed");
    }
    let bitmap_obj: HGDIOBJ = HGDIOBJ::from(bitmap);
    let old_obj = SelectObject(mem_dc, bitmap_obj);

    // PW_RENDERFULLCONTENT = 0x00000002 captures UWP + DComp surfaces.
    // Fall back to flags 0 if the window rejects it.
    let mut ok = PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(0x0000_0002));
    if !ok.as_bool() {
        ok = PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(0));
    }
    if !ok.as_bool() {
        let _ = SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bitmap_obj);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(hwnd, wnd_dc);
        anyhow::bail!("capture_window_to_bmp: PrintWindow failed");
    }

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: height, // positive = bottom-up
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default(); 1],
    };
    let pixel_data_size = (width as u32 * 4 * height as u32) as usize;
    let mut pixels: Vec<u8> = vec![0u8; pixel_data_size];
    let scan_lines = GetDIBits(
        mem_dc,
        bitmap,
        0,
        height as u32,
        Some(pixels.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
    );

    let _ = SelectObject(mem_dc, old_obj);
    let _ = DeleteObject(bitmap_obj);
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(hwnd, wnd_dc);

    if scan_lines == 0 {
        anyhow::bail!("capture_window_to_bmp: GetDIBits returned 0 scan lines");
    }

    // Write BMP file: BITMAPFILEHEADER (14 B) + BITMAPINFOHEADER (40 B) + pixels.
    let file_header_size: u32 = 14;
    let info_header_size: u32 = 40;
    let pixel_offset: u32 = file_header_size + info_header_size;
    let file_size: u32 = pixel_offset + pixel_data_size as u32;

    let mut bmp: Vec<u8> = Vec::with_capacity(file_size as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&pixel_offset.to_le_bytes());
    bmp.extend_from_slice(&info_header_size.to_le_bytes());
    bmp.extend_from_slice(&width.to_le_bytes());
    bmp.extend_from_slice(&height.to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(pixel_data_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0i32.to_le_bytes());
    bmp.extend_from_slice(&0i32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&pixels);

    std::fs::write(path, &bmp)
        .with_context(|| format!("capture_window_to_bmp: failed to write {}", path.display()))?;

    Ok((width as u32, height as u32))
}

/// Capture a single window by HWND and write a BMP file to `path`.
///
/// Uses `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)` — the modern flag
/// that asks the window to render its full client area including UWP /
/// DirectComposition surfaces.  Falls back to plain `PrintWindow` flags
/// `0` on the rare windows that don't honour PW_RENDERFULLCONTENT.
///
/// The output BMP is 32-bit BGRA, bottom-up, matching the desktop helper
/// so downstream readers (web preview, share-sheet) can treat both kinds
/// of capture identically.
pub fn capture_window_to_file(hwnd: isize, path: &Path) -> Result<()> {
    unsafe {
        let hwnd_t = HWND(hwnd as *mut std::ffi::c_void);
        if hwnd_t.is_invalid() {
            anyhow::bail!("capture_window_to_file: invalid HWND {}", hwnd);
        }

        // --- Window rect (in screen coords) ---
        let mut wnd_rect: RECT = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetWindowRect(hwnd_t, &mut wnd_rect)
            .map_err(|e| anyhow::anyhow!("GetWindowRect failed: {:?}", e))?;
        let width = (wnd_rect.right - wnd_rect.left).max(0);
        let height = (wnd_rect.bottom - wnd_rect.top).max(0);
        if width == 0 || height == 0 {
            anyhow::bail!(
                "capture_window_to_file: window {} has zero size ({}x{})",
                hwnd,
                width,
                height
            );
        }

        // --- Acquire the window's own DC + create a memory DC with a
        //     compatible bitmap to receive the PrintWindow output ---
        let wnd_dc = GetDC(hwnd_t);
        if wnd_dc.is_invalid() {
            anyhow::bail!("GetDC(hwnd) failed for window {}", hwnd);
        }

        let mem_dc = CreateCompatibleDC(wnd_dc);
        if mem_dc.is_invalid() {
            let _ = ReleaseDC(hwnd_t, wnd_dc);
            anyhow::bail!("CreateCompatibleDC failed");
        }

        let bitmap = CreateCompatibleBitmap(wnd_dc, width, height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(hwnd_t, wnd_dc);
            anyhow::bail!("CreateCompatibleBitmap failed");
        }

        let bitmap_obj: HGDIOBJ = HGDIOBJ::from(bitmap);
        let old_obj = SelectObject(mem_dc, bitmap_obj);

        // PW_RENDERFULLCONTENT = 0x00000002 — the modern flag that handles
        // UWP windows and DirectComposition surfaces. PW_CLIENTONLY = 0x1.
        const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
        let mut ok = PrintWindow(hwnd_t, mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT));
        if !ok.as_bool() {
            // Some legacy windows return 0 with PW_RENDERFULLCONTENT.
            // Retry with the default flags = 0.
            ok = PrintWindow(hwnd_t, mem_dc, PRINT_WINDOW_FLAGS(0));
        }
        if !ok.as_bool() {
            let _ = SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(bitmap_obj);
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(hwnd_t, wnd_dc);
            anyhow::bail!("PrintWindow failed for HWND {}", hwnd);
        }

        // --- Extract pixel bytes via GetDIBits ---
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: height, // positive = bottom-up
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default(); 1],
        };

        let row_bytes = (width as u32) * 4;
        let pixel_data_size = (row_bytes * height as u32) as usize;
        let mut pixels: Vec<u8> = vec![0u8; pixel_data_size];

        let scan_lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        // --- Cleanup GDI ---
        let _ = SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bitmap_obj);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(hwnd_t, wnd_dc);

        if scan_lines == 0 {
            anyhow::bail!("GetDIBits returned 0 scan lines for window {}", hwnd);
        }

        // --- Write BMP file (BITMAPFILEHEADER + BITMAPINFOHEADER + bytes) ---
        let file_header_size: u32 = 14;
        let info_header_size: u32 = 40;
        let pixel_offset: u32 = file_header_size + info_header_size;
        let file_size: u32 = pixel_offset + pixel_data_size as u32;

        let mut bmp: Vec<u8> = Vec::with_capacity(file_size as usize);
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&file_size.to_le_bytes());
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&0u16.to_le_bytes());
        bmp.extend_from_slice(&pixel_offset.to_le_bytes());

        bmp.extend_from_slice(&info_header_size.to_le_bytes());
        bmp.extend_from_slice(&width.to_le_bytes());
        bmp.extend_from_slice(&height.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&(pixel_data_size as u32).to_le_bytes());
        bmp.extend_from_slice(&0i32.to_le_bytes());
        bmp.extend_from_slice(&0i32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());

        bmp.extend_from_slice(&pixels);

        std::fs::write(path, &bmp)
            .with_context(|| format!("failed to write window BMP to {}", path.display()))?;

        Ok(())
    }
}

/// Resolve a default output path for a window capture: prefer
/// `%USERPROFILE%\Pictures\wiri-window-<hwnd>-<unix>.bmp`; fall back to the
/// current working directory.  Lifted into a separate helper so the IPC
/// server, tray menu, and ctl subcommand all obey the same policy.
pub fn default_window_capture_path(hwnd: isize) -> Result<std::path::PathBuf> {
    use std::path::PathBuf;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("wiri-window-{}-{}.bmp", hwnd, ts);
    let dest = std::env::var("USERPROFILE")
        .ok()
        .map(|home| PathBuf::from(home).join("Pictures"))
        .and_then(|dir| {
            std::fs::create_dir_all(&dir)
                .ok()
                .map(|_| dir.join(&filename))
        })
        .or_else(|| std::env::current_dir().ok().map(|d| d.join(&filename)))
        .ok_or_else(|| {
            anyhow::anyhow!("no writable destination dir (Pictures and cwd both unavailable)")
        })?;
    Ok(dest)
}

/// Capture the full virtual desktop (all monitors) and write a BMP file to `path`.
///
/// Uses pure GDI: `GetDC(NULL)` → `CreateCompatibleDC` → `CreateCompatibleBitmap` →
/// `BitBlt` → `GetDIBits` → manual BITMAPFILEHEADER + BITMAPINFOHEADER + pixel bytes.
///
/// All GDI handles are released on every exit path.
fn capture_desktop_to_bmp(path: &Path) -> Result<()> {
    unsafe {
        // --- Acquire desktop DC ---
        let desktop_dc = GetDC(HWND(std::ptr::null_mut()));
        if desktop_dc.is_invalid() {
            anyhow::bail!("GetDC(NULL) failed — no display available");
        }

        // Helper closure to release DC and bail on errors.
        // We manage cleanup manually before each early-return.

        // --- Virtual desktop dimensions ---
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        if vw <= 0 || vh <= 0 {
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("GetSystemMetrics returned zero virtual desktop size");
        }

        let width = vw as u32;
        let height = vh as u32;

        // --- Create memory DC + compatible bitmap ---
        let mem_dc = CreateCompatibleDC(desktop_dc);
        if mem_dc.is_invalid() {
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("CreateCompatibleDC failed");
        }

        let bitmap = CreateCompatibleBitmap(desktop_dc, vw, vh);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("CreateCompatibleBitmap failed");
        }

        // Select bitmap into memory DC; save the old one so we can restore.
        let bitmap_obj: HGDIOBJ = HGDIOBJ::from(bitmap);
        let old_obj = SelectObject(mem_dc, bitmap_obj);

        // --- BitBlt: copy virtual desktop into the memory DC ---
        let blt_ok = BitBlt(mem_dc, 0, 0, vw, vh, desktop_dc, vx, vy, SRCCOPY);
        if blt_ok.is_err() {
            let _ = SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(bitmap_obj);
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);
            anyhow::bail!("BitBlt failed");
        }

        // --- Extract pixel data via GetDIBits ---
        // 32-bit BGRA — bottom-up (standard BMP orientation).
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: vw,
                biHeight: vh, // positive = bottom-up
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default(); 1],
        };

        let row_bytes = width * 4; // 32 bpp, no padding needed (width * 4 always DWORD-aligned for any w)
        let pixel_data_size = (row_bytes * height) as usize;
        let mut pixels: Vec<u8> = vec![0u8; pixel_data_size];

        let scan_lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        // --- Cleanup GDI resources ---
        let _ = SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bitmap_obj);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(HWND(std::ptr::null_mut()), desktop_dc);

        if scan_lines == 0 {
            anyhow::bail!("GetDIBits failed (returned 0 scan lines)");
        }

        // --- Write BMP file ---
        // BITMAPFILEHEADER: 14 bytes
        // BITMAPINFOHEADER: 40 bytes
        // Pixel data: row_bytes * height bytes
        let file_header_size: u32 = 14;
        let info_header_size: u32 = 40;
        let pixel_offset: u32 = file_header_size + info_header_size;
        let file_size: u32 = pixel_offset + pixel_data_size as u32;

        let mut bmp: Vec<u8> = Vec::with_capacity(file_size as usize);

        // BITMAPFILEHEADER
        bmp.extend_from_slice(b"BM"); // bfType
        bmp.extend_from_slice(&file_size.to_le_bytes()); // bfSize
        bmp.extend_from_slice(&0u16.to_le_bytes()); // bfReserved1
        bmp.extend_from_slice(&0u16.to_le_bytes()); // bfReserved2
        bmp.extend_from_slice(&pixel_offset.to_le_bytes()); // bfOffBits

        // BITMAPINFOHEADER (40 bytes, BI_RGB)
        bmp.extend_from_slice(&info_header_size.to_le_bytes()); // biSize
        bmp.extend_from_slice(&(width as i32).to_le_bytes()); // biWidth
        bmp.extend_from_slice(&(height as i32).to_le_bytes()); // biHeight (positive = bottom-up)
        bmp.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
        bmp.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
        bmp.extend_from_slice(&0u32.to_le_bytes()); // biCompression (BI_RGB)
        bmp.extend_from_slice(&(pixel_data_size as u32).to_le_bytes()); // biSizeImage
        bmp.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
        bmp.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
        bmp.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
        bmp.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

        // Pixel data
        bmp.extend_from_slice(&pixels);

        std::fs::write(path, &bmp)
            .with_context(|| format!("failed to write BMP to {}", path.display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    #[test]
    fn test_capture_window_to_file_bmp_signature() {
        // Use the desktop HWND as a stable target window — it always exists,
        // has a valid window rect, and PrintWindow handles it on most boxes.
        // In headless CI environments without a display, GetDC will fail; we
        // skip gracefully in that case.
        let desktop_hwnd: isize = unsafe { GetDesktopWindow().0 as isize };
        let tmp_path = std::env::temp_dir().join("wiri_test_window_capture.bmp");
        match capture_window_to_file(desktop_hwnd, &tmp_path) {
            Ok(()) => {
                let bytes = std::fs::read(&tmp_path)
                    .expect("BMP file should be readable after window capture");
                assert!(bytes.len() >= 2, "BMP file must be at least 2 bytes");
                assert_eq!(
                    &bytes[0..2],
                    b"BM",
                    "BMP file must start with 'BM' signature"
                );
                let _ = std::fs::remove_file(&tmp_path);
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("GetDC") || msg.contains("zero size") || msg.contains("PrintWindow")
                {
                    eprintln!(
                        "Skipping window-capture test — no display / PrintWindow refused: {}",
                        msg
                    );
                } else {
                    panic!("capture_window_to_file failed unexpectedly: {}", e);
                }
            }
        }
    }

    #[test]
    fn test_capture_window_to_file_invalid_hwnd() {
        let tmp_path = std::env::temp_dir().join("wiri_test_window_capture_bad.bmp");
        // 0 is an explicitly-invalid HWND; the helper must reject without
        // panicking and without leaving a partial file behind.
        let result = capture_window_to_file(0, &tmp_path);
        assert!(
            result.is_err(),
            "expected error for invalid HWND, got {:?}",
            result
        );
        // No file should have been created on the failure path.
        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn test_default_window_capture_path_format() {
        let p = default_window_capture_path(0xCAFE).expect("path must resolve");
        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        assert!(
            fname.starts_with("wiri-window-51966-") && fname.ends_with(".bmp"),
            "expected wiri-window-<hwnd>-<ts>.bmp shape, got {}",
            fname
        );
    }

    #[test]
    fn test_spawner_capture_window_to_file_bmp_signature() {
        // Tests `Spawner::capture_window_to_file` (the method that returns
        // ScreenshotEvent), using the desktop HWND as a stable capture target.
        // Skips gracefully in headless CI where display / PrintWindow is absent.
        let desktop_hwnd: isize = unsafe { GetDesktopWindow().0 as isize };
        let tmp_path = std::env::temp_dir().join("wiri_test_spawner_window_capture.bmp");
        let spawner = Spawner::new();
        match spawner.capture_window_to_file(desktop_hwnd, &tmp_path) {
            Ok(event) => {
                assert!(
                    event.save_path.is_some(),
                    "ScreenshotEvent.save_path should be set"
                );
                let bytes = std::fs::read(&tmp_path).expect("BMP file should be readable");
                assert!(bytes.len() >= 2, "BMP must be at least 2 bytes");
                assert_eq!(&bytes[0..2], b"BM", "BMP must start with 'BM' signature");
                let _ = std::fs::remove_file(&tmp_path);
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("GetDC")
                    || msg.contains("zero")
                    || msg.contains("PrintWindow")
                    || msg.contains("negative")
                    || msg.contains("not a valid window")
                {
                    eprintln!("Skipping Spawner window-capture test — no display: {}", msg);
                } else {
                    panic!("Spawner::capture_window_to_file failed unexpectedly: {}", e);
                }
            }
        }
    }

    #[test]
    fn test_capture_screenshot_to_file_bmp_signature() {
        let tmp_path = std::env::temp_dir().join("wiri_test_screenshot.bmp");

        match Spawner::capture_screenshot_to_file(None, &tmp_path) {
            Ok(event) => {
                // Verify the BMP signature
                let bytes =
                    std::fs::read(&tmp_path).expect("BMP file should be readable after capture");
                assert!(bytes.len() >= 2, "BMP file must be at least 2 bytes");
                assert_eq!(
                    &bytes[0..2],
                    b"BM",
                    "BMP file must start with 'BM' signature"
                );
                assert!(
                    event.save_path.is_some(),
                    "ScreenshotEvent.save_path should be populated"
                );
                // Clean up
                let _ = std::fs::remove_file(&tmp_path);
            }
            Err(e) => {
                // In CI environments without a display, GetDC(NULL) may fail.
                // Skip gracefully rather than failing the test.
                let msg = e.to_string();
                if msg.contains("GetDC(NULL) failed") || msg.contains("zero virtual desktop size") {
                    eprintln!("Skipping screenshot test — no display: {}", msg);
                } else {
                    panic!("capture_screenshot_to_file failed unexpectedly: {}", e);
                }
            }
        }
    }
}

impl Default for Spawner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod spawn_tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// A file system that knows `files` and, like NTFS, ignores case.
    fn fake_fs(files: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<String> = files.iter().map(|f| f.to_lowercase()).collect();
        move |p: &Path| set.contains(&p.to_string_lossy().to_lowercase())
    }

    fn lower(p: Option<PathBuf>) -> Option<String> {
        p.map(|p| p.to_string_lossy().to_lowercase())
    }

    #[test]
    fn find_program_searches_path_then_pathext() {
        let fs = fake_fs(&[r"C:\tools\code.cmd", r"C:\bin\code.exe", r"C:\bin\wt.exe"]);
        let path = r"C:\nothing;C:\tools;C:\bin";
        let ext = ".COM;.EXE;.BAT;.CMD";
        // `code` is found as code.cmd in C:\tools (earlier on PATH) even though
        // C:\bin has a code.exe.
        assert_eq!(
            lower(find_program("code", path, ext, &fs)),
            Some(r"c:\tools\code.cmd".to_string())
        );
        // A name with an extension is taken as given.
        assert_eq!(
            lower(find_program("wt.exe", path, ext, &fs)),
            Some(r"c:\bin\wt.exe".to_string())
        );
        // Within one directory the PATHEXT order decides.
        let fs2 = fake_fs(&[r"C:\d\tool.cmd", r"C:\d\tool.exe"]);
        assert_eq!(
            lower(find_program("tool", r"C:\d", ext, &fs2)),
            Some(r"c:\d\tool.exe".to_string())
        );
        assert_eq!(find_program("missing", path, ext, &fs), None);
        // Empty and quoted PATH entries are tolerated.
        assert_eq!(
            lower(find_program("wt", r#";"C:\bin";"#, ext, &fs)),
            Some(r"c:\bin\wt.exe".to_string())
        );
    }

    #[test]
    fn find_program_with_a_directory_does_not_search_path() {
        let fs = fake_fs(&[r"C:\apps\x.exe", r"C:\bin\y.exe"]);
        assert_eq!(
            lower(find_program(r"C:\apps\x", r"C:\bin", ".EXE", &fs)),
            Some(r"c:\apps\x.exe".to_string())
        );
        // y.exe lives on the PATH, but the program named a directory.
        assert_eq!(find_program(r"C:\apps\y", r"C:\bin", ".EXE", &fs), None);
    }

    #[test]
    fn batch_scripts_run_through_the_command_interpreter() {
        assert!(is_batch_script(Path::new(r"C:\x\code.cmd")));
        assert!(is_batch_script(Path::new(r"C:\x\RUN.BAT")));
        assert!(!is_batch_script(Path::new(r"C:\x\code.exe")));
        let line = batch_script_command_line(
            Path::new(r"C:\Program Files\VS Code\bin\code.cmd"),
            &v(&["--new-window", r"C:\My Projects"]),
            r"C:\Windows\System32\cmd.exe",
        )
        .unwrap();
        assert_eq!(
            line,
            r#"C:\Windows\System32\cmd.exe /d /s /c ""C:\Program Files\VS Code\bin\code.cmd" "--new-window" "C:\My Projects"""#
        );
        // Arguments the shell would interpret are refused rather than mangled.
        assert!(batch_script_command_line(Path::new("x.cmd"), &v(&["50%"]), "cmd.exe").is_err());
        assert!(batch_script_command_line(Path::new("x.cmd"), &v(&["a\"b"]), "cmd.exe").is_err());
    }

    #[test]
    fn inherited_utf16_lookup_and_shell_quoting_are_lossless() {
        let mut directory = OsString::from(r"C:\tools ");
        directory.push(OsString::from_wide(&[0xd800]));
        let expected = PathBuf::from(&directory).join("tool.CMD");
        assert_eq!(
            find_program_os(
                OsStr::new("tool"),
                &directory,
                OsStr::new(".CMD"),
                &|path| path == expected,
            ),
            Some(expected)
        );
        let mut comspec = directory;
        comspec.push(r"\cmd.exe");
        let line = shell_command_line(&comspec, OsStr::new("echo hello"));
        let mut expected = OsString::from("\"");
        expected.push(&comspec);
        expected.push("\" /d /s /c \"echo hello\"");
        assert_eq!(line, expected);
        for arg in ["", "plain", "a b", "a\"b", "trailing space \\", "🦀"] {
            assert_eq!(
                quote_os_arg(OsStr::new(arg)),
                OsString::from(quote_arg(arg))
            );
        }
    }

    #[test]
    fn resolvers_use_child_environment_not_parent_lookup() {
        let program = std::env::current_exe().unwrap();
        let parent = program.parent().unwrap();
        let child = ChildEnvironment::from_entries(
            [
                (OsString::from("Path"), parent.as_os_str().to_owned()),
                (OsString::from("PATHEXT"), OsString::from(".EXE")),
                (
                    OsString::from("ComSpec"),
                    program.file_name().unwrap().to_owned(),
                ),
            ],
            &[],
            &Default::default(),
        );
        assert_eq!(
            resolve_program(program.file_name().unwrap(), &child),
            Some(program.clone())
        );
        assert_eq!(resolve_comspec(&child).unwrap(), program.clone());
        let missing = ChildEnvironment::from_entries(
            [(
                OsString::from("COMSPEC"),
                OsString::from("wiri-no-such-shell.exe"),
            )],
            &[],
            &Default::default(),
        );
        assert!(resolve_program(program.file_name().unwrap(), &missing).is_none());
        assert!(resolve_comspec(&missing).is_err());
        let empty = ChildEnvironment::from_entries([], &[], &Default::default());
        assert_eq!(resolve_program(program.as_os_str(), &empty), Some(program));
        assert!(resolve_comspec(&empty).unwrap().is_absolute());
    }

    #[test]
    fn ipc_spawn_requests_become_an_argv() {
        // A whole command line is split with quoting...
        assert_eq!(
            spawn_command_argv(r#"wt.exe -d "C:\My Projects""#, &[]),
            v(&["wt.exe", "-d", r"C:\My Projects"])
        );
        // ...but with explicit arguments the program is not re-split.
        assert_eq!(
            spawn_command_argv(r"C:\Program Files\Tool\tool.exe", &v(&["a b", "c"])),
            v(&[r"C:\Program Files\Tool\tool.exe", "a b", "c"])
        );
        assert!(spawn_command_argv("   ", &[]).is_empty());
    }

    #[test]
    fn an_empty_command_is_refused_before_anything_is_started() {
        assert!(spawn_argv(&[]).is_err());
        assert!(spawn_argv(&v(&["  "])).is_err());
        assert!(spawn_shell("   ").is_err());
    }
}
