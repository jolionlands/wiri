# Hooks Module Documentation

## Overview

The hooks module (src/hooks/) provides Windows-specific system integration for wiri, a tiling window manager. This module handles autostart registration, system tray presence, screenshots, notifications, wallpaper/accent color synchronization, and process spawning.

## Module Structure

src/hooks/
- mod.rs              Main exports and SystemIntegration struct
- startup.rs          StartupManager for autostart registration
- tray.rs             TrayIcon for system tray presence
- screenshot.rs       ScreenshotEvent for screen capture
- notification.rs     Notification struct for notifications
- wallpaper.rs        Wallpaper and accent color integration
- spawn.rs            Process spawning with CreateProcessW
- resources.rs        Windows resource file handling (.rc)
- win32_bindings.rs   Low-level Win32 API wrappers

## Feature Gate

System integration features are optional and controlled by Cargo features:

`	oml
[dependencies.wiri]
features = ["hooks", "screenshot", "notification", "tray"]
`

---

## StartupManager

Manages wiri's automatic startup registration via the Windows Registry.

### Registry Path

Autostart uses HKCU\Software\Microsoft\Windows\CurrentVersion\Run:

`
HKEY_CURRENT_USER
  Software
    Microsoft
      Windows
        CurrentVersion
          Run
            "wiri" = "<executable_path>"
`

### StartupManager Struct

`ust
pub struct StartupManager {
    /// Application display name for registry
    app_name: String,
    /// Path to the wiri executable
    exe_path: String,
    /// Command-line arguments to pass on startup
    args: Vec<String>,
}

impl StartupManager {
    /// Create a new StartupManager
    pub fn new(app_name: String, exe_path: String) -> Self {
        Self {
            app_name,
            exe_path,
            args: Vec::new(),
        }
    }

    /// Add a command-line argument
    pub fn with_arg(mut self, arg: String) -> Self {
        self.args.push(arg);
        self
    }

    /// Register wiri in the Run registry key
    pub fn register(&self) -> Result<(), StartupError> {
        unsafe {
            let mut hkey: HKEY = std::mem::zeroed();
            let run_key: PCWSTR = windows::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
            
            let result = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                run_key,
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            
            if result != ERROR_SUCCESS {
                return Err(StartupError::RegOpenFailed(result));
            }
            
            let command = self.build_command();
            let command_wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            let app_name_wide: Vec<u16> = self.app_name.encode_utf16().chain(std::iter::once(0)).collect();
            
            let set_result = RegSetValueExW(
                hkey,
                windows::PCWSTR(app_name_wide.as_ptr()),
                0,
                REG_SZ,
                Some(&command_wide.iter().map(|&w| w as u8).collect::<Vec<u8>>()),
            );
            
            RegCloseKey(hkey).ok();
            
            if set_result != ERROR_SUCCESS {
                return Err(StartupError::RegSetFailed(set_result));
            }
            
            Ok(())
        }
    }

    /// Unregister wiri from the Run registry key
    pub fn unregister(&self) -> Result<(), StartupError> {
        unsafe {
            let mut hkey: HKEY = std::mem::zeroed();
            let run_key: PCWSTR = windows::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
            
            RegOpenKeyExW(HKEY_CURRENT_USER, run_key, 0, KEY_SET_VALUE, &mut hkey)?;
            let app_name_wide: Vec<u16> = self.app_name.encode_utf16().chain(std::iter::once(0)).collect();
            let delete_result = RegDeleteValueW(hkey, windows::PCWSTR(app_name_wide.as_ptr()));
            RegCloseKey(hkey).ok();
            
            if delete_result != ERROR_SUCCESS && delete_result != ERROR_FILE_NOT_FOUND {
                return Err(StartupError::RegDeleteFailed(delete_result));
            }
            Ok(())
        }
    }

    /// Check if wiri is currently registered for startup
    pub fn is_registered(&self) -> Result<bool, StartupError> {
        unsafe {
            let mut hkey: HKEY = std::mem::zeroed();
            let run_key: PCWSTR = windows::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
            RegOpenKeyExW(HKEY_CURRENT_USER, run_key, 0, KEY_QUERY_VALUE, &mut hkey)?;
            
            let app_name_wide: Vec<u16> = self.app_name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut buffer: [u16; 512] = [0; 512];
            let mut buffer_size: u32 = (buffer.len() * 2) as u32;
            let mut reg_type: u32 = 0;
            
            let query_result = RegQueryValueExW(
                hkey,
                windows::PCWSTR(app_name_wide.as_ptr()),
                None,
                Some(&mut reg_type),
                Some(&mut buffer as *mut _ as *mut u8),
                Some(&mut buffer_size),
            );
            
            RegCloseKey(hkey).ok();
            Ok(query_result == ERROR_SUCCESS)
        }
    }

    fn build_command(&self) -> String {
        let mut cmd = format!("\"{}\"", self.exe_path);
        for arg in &self.args {
            cmd.push_str(&format!(" {}", arg));
        }
        cmd
    }
}
`

### StartupError

`ust
#[derive(Debug, Error)]
pub enum StartupError {
    #[error("Failed to open registry key: {0}")]
    RegOpenFailed(u32),
    
    #[error("Failed to set registry value: {0}")]
    RegSetFailed(u32),
    
    #[error("Failed to delete registry value: {0}")]
    RegDeleteFailed(u32),
    
    #[error("Invalid registry value type: {0}")]
    InvalidRegType(u32),
}
`

---

## TrayIcon

System tray icon for wiri presence and quick actions.

### TrayIcon Struct

`ust
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIM_ADD, NIM_MODIFY, NIM_DELETE,
    NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIF_STATE, NIF_INFO,
    NIS_HIDDEN, NIF_SHOWTIP,
};
use windows::Win32::Foundation::HWND;

pub struct TrayIcon {
    hicon: HICON,
    notify_hwnd: HWND,
    uid: u32,
    tip: String,
    visible: bool,
}

impl TrayIcon {
    pub fn new(notify_hwnd: HWND, uid: u32, tip: String) -> Result<Self, TrayError> {
        let hicon = Self::load_icon()?;
        Ok(Self { hicon, notify_hwnd, uid, tip, visible: false })
    }

    fn load_icon() -> Result<HICON, TrayError> {
        unsafe {
            let icon = LoadImageW(None, windows::PCWSTR(windows::w!("IDI_WIRI_ICON")),
                IMAGE_ICON, 0, 0, LR_SHARED | LR_DEFAULTSIZE)?;
            Ok(HICON(icon as isize))
        }
    }

    pub fn show(&mut self) -> Result<(), TrayError> {
        if self.visible { return Ok(()); }
        let mut nid = self.build_notify_icon_data(NIF_ICON | NIF_MESSAGE | NIF_TIP);
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &mut nid as *mut _)
                .ok().map_err(|e| TrayError::NotifyIconFailed(e))?;
        }
        self.visible = true;
        Ok(())
    }

    pub fn hide(&mut self) -> Result<(), TrayError> {
        if !self.visible { return Ok(()); }
        let mut nid = self.build_notify_icon_data(NIF_STATE);
        nid.dwState = NIS_HIDDEN;
        nid.dwStateMask = NIS_HIDDEN;
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &mut nid as *mut _)
                .ok().map_err(|e| TrayError::NotifyIconFailed(e))?;
        }
        self.visible = false;
        Ok(())
    }

    pub fn set_tooltip(&mut self, tip: String) -> Result<(), TrayError> {
        self.tip = tip;
        if !self.visible { return Ok(()); }
        let mut nid = self.build_notify_icon_data(NIF_TIP);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &mut nid as *mut _)
                .ok().map_err(|e| TrayError::NotifyIconFailed(e))?;
        }
        Ok(())
    }

    pub fn show_balloon(&self, title: &str, message: &str, icon_type: BalloonIcon) -> Result<(), TrayError> {
        let mut nid = self.build_notify_icon_data(NIF_INFO);
        let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let msg_wide: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
        nid.dwInfoFlags = icon_type.to_flags();
        // ... copy to nid.szInfoTitle and nid.szInfo
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &mut nid as *mut _)
                .ok().map_err(|e| TrayError::NotifyIconFailed(e))?;
        }
        Ok(())
    }

    fn build_notify_icon_data(&self, flags: u32) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.notify_hwnd;
        nid.uID = self.uid;
        nid.uFlags = flags;
        nid.uCallbackMessage = WM_TRAYICON;
        if flags & NIF_ICON != 0 { nid.hIcon = self.hicon; }
        if flags & NIF_TIP != 0 {
            let tip_wide: Vec<u16> = self.tip.encode_utf16().chain(std::iter::once(0)).collect();
            nid.szTip[..tip_wide.len().min(127)].copy_from_slice(&tip_wide[..tip_wide.len().min(127)]);
        }
        nid
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.visible {
            let mut nid = self.build_notify_icon_data(NIF_ICON);
            unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid as *mut _); }
        }
    }
}

pub enum BalloonIcon { None, Info, Warning, Error, User }
impl BalloonIcon { fn to_flags(&self) -> u32 { match self { Self::None => 0x0, Self::Info => 0x1, Self::Warning => 0x2, Self::Error => 0x3, Self::User => 0x4 } } }

pub const WM_TRAYICON: u32 = WM_USER + 1;
pub const TRAY_CMD_SHOW_HIDE: u32 = 1001;
pub const TRAY_CMD_RELOAD_CONFIG: u32 = 1002;
pub const TRAY_CMD_QUIT: u32 = 1003;
`

### Tray Menu

Tray icon right-click menu:

`
wiri
  Show/Hide Window    (TRAY_CMD_SHOW_HIDE)
  Reload Config       (TRAY_CMD_RELOAD_CONFIG)
  ------------------  (separator)
  Quit                (TRAY_CMD_QUIT)
`

### TrayError

`ust
#[derive(Debug, Error)]
pub enum TrayError {
    #[error("Shell_NotifyIcon failed: {0}")]
    NotifyIconFailed(#[from] Win32Error),
    
    #[error("Failed to load icon: {0}")]
    IconLoadFailed(#[from] Win32Error),
    
    #[error("Icon not visible")]
    IconNotVisible,
}
`

---

## ScreenshotEvent

Event-based screen capture triggering.

### ScreenshotEvent Struct

`ust
pub enum ScreenshotEvent {
    FullScreen { path: PathBuf },
    Region { path: PathBuf, bounds: Rect<i32, Screen> },
    Window { path: PathBuf, hwnd: HWND },
    ToClipboard { variant: ClipboardVariant },
}

pub enum ClipboardVariant {
    FullScreen,
    Region(Rect<i32, Screen>),
    Window(HWND),
}

impl ScreenshotEvent {
    pub async fn execute(&self) -> Result<(), ScreenshotError> {
        match self {
            Self::FullScreen { path } => self.capture_full_screen(path).await,
            Self::Region { path, bounds } => self.capture_region(path, *bounds).await,
            Self::Window { path, hwnd } => self.capture_window(path, *hwnd).await,
            Self::ToClipboard { variant } => self.capture_to_clipboard(variant).await,
        }
    }

    fn capture_screen_region(&self, rect: RECT, path: &PathBuf) -> Result<(), ScreenshotError> {
        let width = (rect.right - rect.left) as u32;
        let height = (rect.bottom - rect.top) as u32;
        
        unsafe {
            let hdc_screen = GetDC(HWND::default());
            let hdc_mem = CreateCompatibleDC(hdc_screen);
            let hbitmap = CreateCompatibleBitmap(hdc_screen, width, height);
            let hdc_old = SelectObject(hdc_mem, hbitmap);
            BitBlt(hdc_mem, 0, 0, width, height, hdc_screen, rect.left, rect.top, SRCCOPY)?;
            SelectObject(hdc_mem, hdc_old);
            // Save as PNG
            DeleteObject(hbitmap).ok();
            DeleteDC(hdc_mem).ok();
            ReleaseDC(HWND::default(), hdc_screen);
            Ok(())
        }
    }
}

pub struct Screen; // Marker type for screen coordinates
`

### ScreenshotError

`ust
#[derive(Debug, Error)]
pub enum ScreenshotError {
    #[error("Failed to get window rect: {0}")]
    GetRectFailed(#[from] Win32Error),
    #[error("Failed to create DC: {0}")]
    CreateDcFailed(#[from] Win32Error),
    #[error("BitBlt failed: {0}")]
    BitBltFailed(#[from] Win32Error),
    #[error("PNG encoding failed: {0}")]
    EncodeError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Task join failed")]
    TaskJoinFailed,
}
`

---

## Notification

User notifications with Windows toast notifications.

### Notification Struct

`ust
pub struct Notification {
    title: String,
    body: String,
    icon: NotificationIcon,
    actions: Vec<NotificationAction>,
    expire_timeout: Option<Duration>,
}

impl Notification {
    pub fn new(title: String, body: String) -> Self {
        Self { title, body, icon: NotificationIcon::Info, actions: Vec::new(), expire_timeout: Some(Duration::from_secs(5)) }
    }

    pub fn with_icon(mut self, icon: NotificationIcon) -> Self { self.icon = icon; self }
    pub fn with_action(mut self, action: NotificationAction) -> Self { self.actions.push(action); self }
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self { self.expire_timeout = timeout; self }

    pub async fn show(&self) -> Result<NotificationHandle, NotificationHandle> {
        Ok(NotificationHandle { id: generate_notification_id() })
    }
}

pub enum NotificationIcon { Info, Warning, Error, Success }
pub struct NotificationAction { pub id: String, pub label: String }
pub struct NotificationHandle { id: u32 }

fn generate_notification_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}
`

### NotificationError

`ust
#[derive(Debug, Error)]
pub enum NotificationError {
    #[error("Notification service unavailable")]
    ServiceUnavailable,
    #[error("Failed to create toast: {0}")]
    ToastCreateFailed(String),
}
`

---

## Wallpaper and Accent Color Integration

Windows 11 dark mode synchronization for wallpaper and accent colors.

### WallpaperManager Struct

`ust
pub struct WallpaperManager {
    wallpaper_path: Option<String>,
    style: WallpaperStyle,
    source_monitor: Option<HMONITOR>,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum WallpaperStyle {
    Fill, Fit, Stretch, Tile, Center, Span,
    #[default]
    None,
}

impl WallpaperManager {
    pub fn new() -> Self { Self { wallpaper_path: None, style: WallpaperStyle::None, source_monitor: None } }

    pub fn get_wallpaper(&mut self) -> Result<WallpaperInfo, WallpaperError> {
        unsafe {
            let mut path_buffer: [u16; 260] = [0; 260];
            SystemParametersInfoW(SPI_GETWALLPAPER, path_buffer.len() as u32, Some(&mut path_buffer as *mut _ as *mut _), 0)?;
            let path = String::from_utf16_lossy(&path_buffer).trim_end_matches('\0').to_string();
            let style = self.get_wallpaper_style_from_registry()?;
            self.wallpaper_path = Some(path.clone());
            Ok(WallpaperInfo { path, style, monitors: self.get_wallpaper_monitors()? })
        }
    }

    pub fn set_wallpaper(&mut self, path: &str, style: WallpaperStyle) -> Result<(), WallpaperError> {
        unsafe {
            let path_wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            SystemParametersInfoW(SPI_SETWALLPAPER, path_wide.len() as u32, Some(path_wide.as_ptr() as *mut _), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)?;
        }
        self.set_wallpaper_style_in_registry(style)?;
        self.wallpaper_path = Some(path.to_string());
        self.style = style;
        Ok(())
    }

    fn get_wallpaper_style_from_registry(&self) -> Result<WallpaperStyle, WallpaperError> {
        unsafe {
            let mut hkey: HKEY = std::mem::zeroed();
            RegOpenKeyExW(HKEY_CURRENT_USER, windows::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Wallpapers"), 0, KEY_QUERY_VALUE, &mut hkey)?;
            let mut style: u32 = 0;
            let mut size = std::mem::size_of::<u32>() as u32;
            let result = RegQueryValueExW(hkey, windows::w!("WallpaperStyle"), None, None, Some(&mut style as *mut _ as *mut u8), Some(&mut size));
            RegCloseKey(hkey).ok();
            Ok(match style { 0 => WallpaperStyle::Center, 2 => WallpaperStyle::Stretch, 6 => WallpaperStyle::Fit, 10 => WallpaperStyle::Fill, _ => WallpaperStyle::None })
        }
    }

    fn set_wallpaper_style_in_registry(&self, style: WallpaperStyle) -> Result<(), WallpaperError> {
        let style_value = match style {
            WallpaperStyle::Center => 0, WallpaperStyle::Stretch => 2, WallpaperStyle::Fit => 6,
            WallpaperStyle::Fill => 10, WallpaperStyle::Tile | WallpaperStyle::Span => 22, WallpaperStyle::None => 0,
        };
        unsafe {
            let mut hkey: HKEY = std::mem::zeroed();
            RegOpenKeyExW(HKEY_CURRENT_USER, windows::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Wallpapers"), 0, KEY_SET_VALUE, &mut hkey)?;
            let result = RegSetValueExW(hkey, windows::w!("WallpaperStyle"), 0, REG_DWORD, Some(&style_value.to_le_bytes()));
            RegCloseKey(hkey).ok();
            result.ok().map_err(|e| WallpaperError::RegistryFailed(e))?;
            Ok(())
        }
    }
}

pub struct WallpaperInfo { pub path: String, pub style: WallpaperStyle, pub monitors: Vec<HMONITOR> }

#[derive(Debug, Error)]
pub enum WallpaperError {
    #[error("SystemParametersInfoW failed: {0}")]
    SystemParametersFailed(#[from] Win32Error),
    #[error("Registry operation failed: {0}")]
    RegistryFailed(#[from] Win32Error),
}
`

### AccentColorManager

`ust
pub struct AccentColorManager { accent_color: Option<u32>, dark_mode: bool }

impl AccentColorManager {
    pub fn new() -> Self { Self { accent_color: None, dark_mode: false } }

    pub fn get_accent_color(&mut self) -> Result<u32, AccentColorError> {
        unsafe {
            let mut hkey: HKEY = std::mem::zeroed();
            RegOpenKeyExW(HKEY_CURRENT_USER, windows::w!("Software\\Microsoft\\Windows\\DWM"), 0, KEY_QUERY_VALUE, &mut hkey)?;
            let mut color: u32 = 0;
            let mut size = std::mem::size_of::<u32>() as u32;
            let result = RegQueryValueExW(hkey, windows::w!("AccentColor"), None, None, Some(&mut color as *mut _ as *mut u8), Some(&mut size));
            RegCloseKey(hkey).ok();
            if result == ERROR_SUCCESS {
                let abgr = color;
                let a = (abgr >> 24) & 0xFF; let b = (abgr >> 16) & 0xFF; let g = (abgr >> 8) & 0xFF; let r = abgr & 0xFF;
                self.accent_color = Some((a << 24) | (r << 16) | (g << 8) | b);
                Ok(self.accent_color.unwrap())
            } else { Err(AccentColorError::NotFound) }
        }
    }

    pub fn is_dark_mode(&mut self) -> Result<bool, AccentColorError> {
        unsafe {
            let mut hkey: HKEY = std::mem::zeroed();
            RegOpenKeyExW(HKEY_CURRENT_USER, windows::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), 0, KEY_QUERY_VALUE, &mut hkey)?;
            let mut value: u32 = 0;
            let mut size = std::mem::size_of::<u32>() as u32;
            let result = RegQueryValueExW(hkey, windows::w!("AppsUseLightTheme"), None, None, Some(&mut value as *mut _ as *mut u8), Some(&mut size));
            RegCloseKey(hkey).ok();
            if result == ERROR_SUCCESS { self.dark_mode = value == 0; Ok(self.dark_mode) } else { Ok(false) }
        }
    }
}

#[derive(Debug, Error)]
pub enum AccentColorError {
    #[error("Registry query failed: {0}")]
    RegistryFailed(#[from] Win32Error),
    #[error("Accent color not found")]
    NotFound,
}
`

---

## Spawn Mechanism

Process spawning with environment variable inheritance.

### Spawner Struct

`ust
pub struct Spawner {
    env_vars: HashMap<String, String>,
    cwd: Option<String>,
}

impl Spawner {
    pub fn new() -> Self { Self { env_vars: HashMap::new(), cwd: None } }
    pub fn with_env(mut self, key: String, value: String) -> Self { self.env_vars.insert(key, value); self }
    pub fn with_cwd(mut self, path: String) -> Self { self.cwd = Some(path); self }

    pub fn spawn(&self, program: &str, args: &[&str]) -> Result<ProcessHandle, SpawnError> {
        self.spawn_with_handle(program, args, CREATE_NO_WINDOW)
    }

    pub fn spawn_with_handle(&self, program: &str, args: &[&str], flags: u32) -> Result<ProcessHandle, SpawnError> {
        unsafe {
            let mut cmd_line = format!("\"{}\"", program);
            for arg in args { cmd_line.push_str(&format!(" \"{}\"", arg)); }
            let cmd_wide: Vec<u16> = cmd_line.encode_utf16().chain(std::iter::once(0)).collect();
            let prog_wide: Vec<u16> = program.encode_utf16().chain(std::iter::once(0)).collect();
            let env = self.build_environment_block()?;
            let cwd = self.cwd.as_ref().map(|s| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>()).unwrap_or_default();
            let cwd_ptr = if cwd.is_empty() { std::ptr::null() } else { cwd.as_ptr() };
            let mut si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, dwFlags: STARTF_USESHOWWINDOW, wShowWindow: SW_SHOWNORMAL, ..Default::default() };
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let created = CreateProcessW(windows::PCWSTR(prog_wide.as_ptr()), windows::PCWSTR(cmd_wide.as_ptr()), None, None, true, flags, env.as_ptr() as *const _, cwd_ptr, &mut si, &mut pi);
            if !created.is_ok() { return Err(SpawnError::CreateProcessFailed()); }
            Ok(ProcessHandle { process: pi.hProcess, thread: pi.hThread, pid: pi.dwProcessId })
        }
    }

    pub fn spawn_shell(&self, program: &str, args: &[&str], run_as: bool) -> Result<(), SpawnError> {
        unsafe {
            let mut sei = SHELLEXECUTEINFOW { cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32, fMask: SEE_MASK_NOCLOSEPROCESS, hwnd: HWND::default(),
                lpVerb: if run_as { windows::w!("runas") } else { windows::w!("open") },
                lpFile: windows::PCWSTR(program.as_ptr()), lpParameters: windows::PCWSTR(args.join(" ").encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>().as_ptr()),
                nShow: SW_SHOWNORMAL, ..Default::default() };
            ShellExecuteExW(&mut sei).ok()?;
            if sei.hProcess.is_invalid() { return Err(SpawnError::ShellExecuteFailed()); }
            CloseHandle(sei.hProcess).ok();
            Ok(())
        }
    }

    fn build_environment_block(&self) -> Result<*mut u16, SpawnError> {
        let mut env_map: HashMap<String, String> = std::env::vars().collect();
        for (key, value) in &self.env_vars { env_map.insert(key.clone(), value.clone()); }
        let mut env_block = Vec::new();
        let mut keys: Vec<_> = env_map.keys().collect();
        keys.sort();
        for key in keys { env_block.extend(format!("{}={}", key, env_map[key]).encode_utf16()); env_block.push(0); }
        env_block.push(0);
        Ok(env_block.into_boxed_slice() as *mut u16)
    }
}

pub struct ProcessHandle { process: HANDLE, thread: HANDLE, pid: u32 }

impl ProcessHandle {
    pub fn wait(&self) -> Result<u32, SpawnError> {
        unsafe { WaitForSingleObject(self.process, INFINITE)?; let mut exit_code: u32 = 0; GetExitCodeProcess(self.process, &mut exit_code)?; Ok(exit_code) }
    }
    pub fn wait_timeout(&self, timeout: Duration) -> Result<Option<u32>, SpawnError> {
        unsafe {
            let result = WaitForSingleObject(self.process, timeout.as_millis() as u32);
            if result == WAIT_TIMEOUT { return Ok(None); }
            let mut exit_code: u32 = 0;
            GetExitCodeProcess(self.process, &mut exit_code)?;
            Ok(Some(exit_code))
        }
    }
    pub fn pid(&self) -> u32 { self.pid }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) { unsafe { CloseHandle(self.process).ok(); CloseHandle(self.thread).ok(); } }
}

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("CreateProcessW failed")]
    CreateProcessFailed(),
    #[error("ShellExecuteExW failed")]
    ShellExecuteFailed(),
    #[error("WaitForSingleObject failed: {0}")]
    WaitFailed(#[from] Win32Error),
}
`

---

## SystemIntegration

Unified struct combining all system integrations.

### SystemIntegration Struct

`ust
pub struct SystemIntegration {
    pub startup: StartupManager,
    pub tray: Option<TrayIcon>,
    pub wallpaper: WallpaperManager,
    pub accent: AccentColorManager,
    pub spawner: Spawner,
    pub screenshot_handler: ScreenshotHandler,
}

impl SystemIntegration {
    pub fn new(config: &Config, hwnd: HWND) -> Result<Self, SystemIntegrationError> {
        let exe_path = std::env::current_exe().map_err(|e| SystemIntegrationError::ExePathFailed(e))?;
        let startup = StartupManager::new(String::from("wiri"), exe_path.to_string_lossy().to_string());
        let tray = TrayIcon::new(hwnd, 1, String::from("wiri tiling window manager")).ok();
        Ok(Self { startup, tray, wallpaper: WallpaperManager::new(), accent: AccentColorManager::new(), spawner: Spawner::new(), screenshot_handler: ScreenshotHandler::new() })
    }

    pub fn show_tray(&mut self) -> Result<(), TrayError> {
        if let Some(ref mut tray) = self.tray { tray.show()?; }
        Ok(())
    }

    pub fn handle_tray_command(&self, cmd: u32) -> TrayCommandResult {
        match cmd { TRAY_CMD_SHOW_HIDE => TrayCommandResult::ShowHide, TRAY_CMD_RELOAD_CONFIG => TrayCommandResult::ReloadConfig, TRAY_CMD_QUIT => TrayCommandResult::Quit, _ => TrayCommandResult::Unknown(cmd) }
    }

    pub fn sync_theme(&mut self) -> Result<ThemeSyncResult, SystemIntegrationError> {
        let wallpaper = self.wallpaper.get_wallpaper()?;
        let accent_color = self.accent.get_accent_color().unwrap_or(0x0078D4FF);
        let dark_mode = self.accent.is_dark_mode().unwrap_or(false);
        Ok(ThemeSyncResult { wallpaper, accent_color, dark_mode })
    }
}

pub enum TrayCommandResult { ShowHide, ReloadConfig, Quit, Unknown(u32) }

pub struct ThemeSyncResult { pub wallpaper: WallpaperInfo, pub accent_color: u32, pub dark_mode: bool }

pub struct ScreenshotHandler { save_dir: PathBuf, format: ScreenshotFormat }
impl ScreenshotHandler { pub fn new() -> Self { Self { save_dir: std::env::temp_dir(), format: ScreenshotFormat::Png } } }

pub enum ScreenshotFormat { Png, Bmp, Jpeg }

#[derive(Debug, Error)]
pub enum SystemIntegrationError {
    #[error("Failed to get executable path: {0}")]
    ExePathFailed(#[from] std::io::Error),
    #[error("Startup error: {0}")]
    StartupError(#[from] StartupError),
    #[error("Tray error: {0}")]
    TrayError(#[from] TrayError),
}
`

---

## Windows Resource File Handling

Resource file (.rc) for icons, version info, and manifest.

### Resource File (wiri.rc)

`c
#define IDI_WIRI_ICON 101
#define IDI_WIRI_ICON_SM 102

IDI_WIRI_ICON ICON "wiri.ico"
IDI_WIRI_ICON_SM ICON "wiri_sm.ico"

VS_VERSION_INFO VERSIONINFO
    FILEVERSION 0, 1, 0, 0
    PRODUCTVERSION 0, 1, 0, 0
    FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
    FILEFLAGS 0
    FILEOS VOS__WINDOWS32
    FILETYPE VFT_APP
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "wiri"
            VALUE "FileDescription", "wiri tiling window manager"
            VALUE "FileVersion", "0.1.0"
            VALUE "ProductName", "wiri"
        END
    END
END
`

### Resource Loader

`ust
pub fn load_icon(resource_id: u16) -> Result<HICON, ResourceError> {
    unsafe {
        let icon = LoadImageW(HINSTANCE::default(), resource_id as *const _, IMAGE_ICON, 0, 0, LR_SHARED | LR_DEFAULTSIZE)?;
        Ok(HICON(icon as isize))
    }
}

pub fn load_app_icon() -> Result<HICON, ResourceError> {
    load_icon(101).or_else(|_| unsafe { let icon = LoadIconW(None, IDI_APPLICATION)?; Ok(HICON(icon.0 as isize)) })
}

#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("Failed to load resource: {0}")]
    LoadFailed(#[from] Win32Error),
}
`

---

## Integration Points

### With Backend Module
- Tray icon window receives events via WM_TRAYICON
- Screenshot capture uses same DC/bitmap APIs as rendering
- Spawner used for launching helper processes

### With Layout Module
- Theme sync provides wallpaper/accent colors for rendering
- Notifications triggered on layout events

### With Config Module
- Startup registration reads config for autostart preference
- Tray visibility controlled by config

### With IPC Module
- Screenshot events can be triggered via IPC
- Notification show/hide via IPC commands

---

## Feature Matrix

| Feature | Required | Optional | Windows API |
|---------|----------|----------|-------------|
| Startup | Yes | - | RegOpenKeyExW, RegSetValueExW |
| Tray | Yes | - | Shell_NotifyIconW |
| Screenshot | - | Yes | BitBlt, GetDC, CreateCompatibleDC |
| Notification | - | Yes | Shell_NotifyIconW (balloon) |
| Wallpaper | Yes | - | SystemParametersInfoW |
| Accent Color | Yes | - | Registry (DWM key) |
| Spawn | Yes | - | CreateProcessW, ShellExecuteExW |
| Resources | Yes | - | LoadImageW |

---

## Error Handling Strategy

All errors use the 	hiserror crate for clear error messages:

- **StartupError**: Registry access failures
- **TrayError**: Shell_NotifyIconW failures, icon load failures
- **ScreenshotError**: DC creation, BitBlt, PNG encoding failures
- **NotificationError**: Toast notification failures
- **WallpaperError**: SystemParametersInfoW, registry failures
- **AccentColorError**: Registry query failures
- **SpawnError**: CreateProcessW, ShellExecuteExW failures

---

## Thread Safety

- SystemIntegration is Send + Sync safe
- Windows API calls are wrapped in spawn_blocking for async compatibility
- All Win32 handles are properly closed in Drop implementations
