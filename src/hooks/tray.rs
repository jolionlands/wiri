use anyhow::{Context, Result};
use std::os::windows::ffi::OsStrExt;
use std::sync::{mpsc, Arc};
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, AppendMenuW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    TrackPopupMenu, TranslateMessage, CreateWindowExW,
    CS_HREDRAW, CS_VREDRAW, MF_SEPARATOR, MF_STRING,
    TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RIGHTBUTTON,
    WM_COMMAND, WM_DESTROY, WM_USER, WNDCLASSW,
    WINDOW_EX_STYLE, WINDOW_STYLE,
};
// NIF_INFO, NIIF_INFO, NIM_MODIFY etc. come from the Shell glob below
use windows::Win32::UI::Shell::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::PCWSTR;

const WM_TRAYICON: u32 = WM_USER + 100;
const ID_TRAY_SHOW_HIDE: usize = 1001;
const ID_TRAY_RELOAD_CONFIG: usize = 1002;
const ID_TRAY_QUIT: usize = 1003;
const ID_TRAY_OPEN_CONFIG_DIR: usize = 1004;
const ID_TRAY_ABOUT: usize = 1005;
const ID_TRAY_FOCUS_PREV: usize = 1006;

/// Actions that the tray icon can trigger
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    /// Show/hide all wiri-managed windows (toggle).
    ShowHide,
    /// Reload the on-disk configuration file.
    ReloadConfig,
    /// Open the wiri config folder in Explorer.
    OpenConfigDir,
    /// Show the About dialog (balloon notification with version + GitHub URL).
    About,
    /// Bring focus to the most recently focused tile.
    FocusPrevious,
    /// Quit wiri.
    Quit,
}

pub struct TrayIcon {
    thread_handle: Option<std::thread::JoinHandle<()>>,
    action_tx: mpsc::Sender<TrayAction>,
    action_rx: Option<mpsc::Receiver<TrayAction>>,
    /// Raw HWND value (as isize) of the tray message-only window.
    /// Populated after the tray thread creates its window.
    hwnd_raw: Arc<Mutex<Option<isize>>>,
}

impl TrayIcon {
    pub fn new() -> Self {
        let (action_tx, action_rx) = mpsc::channel();
        Self {
            thread_handle: None,
            action_tx,
            action_rx: Some(action_rx),
            hwnd_raw: Arc::new(Mutex::new(None)),
        }
    }

    /// Setup the tray icon (starts the message loop thread)
    pub fn setup(&mut self) -> Result<()> {
        let action_tx = self.action_tx.clone();
        let hwnd_raw = self.hwnd_raw.clone();
        let handle = std::thread::spawn(move || {
            if let Err(e) = Self::run_tray_loop(action_tx, hwnd_raw) {
                warn!("Tray icon error: {}", e);
            }
        });
        self.thread_handle = Some(handle);
        info!("System tray icon initialized");
        Ok(())
    }

    fn run_tray_loop(action_tx: mpsc::Sender<TrayAction>, hwnd_raw: Arc<Mutex<Option<isize>>>) -> Result<()> {
        unsafe {
            let hinstance = GetModuleHandleW(None).context("GetModuleHandleW failed")?;

            // Register window class
            let class_name: Vec<u16> = "WiriTrayWnd\0".encode_utf16().collect();
            let wnd_class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(Self::tray_window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance.into(),
                hIcon: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
                hCursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR::default(),
                hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
            };
            RegisterClassW(&wnd_class);

            // Create message-only window
            let window_name: Vec<u16> = "WiriTrayWindow\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(window_name.as_ptr()),
                WINDOW_STYLE(0),
                0, 0, 0, 0,
                None, None, hinstance, None,
            );
            let hwnd = match hwnd {
                Ok(w) => w,
                Err(e) => {
                    warn!("Failed to create tray window: {:?}", e);
                    return Err(anyhow::anyhow!("CreateWindowExW failed: {:?}", e));
                }
            };

            // Publish the HWND so that destroy()/show_balloon() can reach it
            *hwnd_raw.lock() = Some(hwnd.0 as isize);

            // Store action_tx in window user data for callback access
            let tx_ptr = Box::into_raw(Box::new(action_tx));
            windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                tx_ptr as isize,
            );

            // Add tray icon
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            nid.uFlags = NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_TRAYICON;
            let tip: Vec<u16> = "wiri - Tiling WM\0".encode_utf16().collect();
            nid.szTip[..tip.len().min(128)].copy_from_slice(&tip[..tip.len().min(128)]);

            // Use a default system icon
            nid.hIcon = windows::Win32::UI::WindowsAndMessaging::LoadIconW(
                None,
                windows::Win32::UI::WindowsAndMessaging::IDI_APPLICATION,
            )
            .ok()
            .unwrap_or_default();
            if nid.hIcon.is_invalid() {
                nid.hIcon = windows::Win32::UI::WindowsAndMessaging::HICON::default();
            }

            if !Shell_NotifyIconW(NIM_ADD, &nid as *const _ as *const _).as_bool() {
                warn!("Shell_NotifyIconW NIM_ADD failed");
            }
            info!("System tray icon added");

            // Message loop
            let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // Remove tray icon before exiting
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid as *const _ as *const _);
        }
        Ok(())
    }

    unsafe extern "system" fn tray_window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TRAYICON => {
                let event = lparam.0 as u32;
                // WM_RBUTTONUP (0x205): show context menu.
                // WM_LBUTTONUP (0x202): single left click → focus the most
                //   recently focused tile (matches what most WM trays do).
                if event == 0x205 {
                    Self::show_context_menu(hwnd);
                } else if event == 0x202 {
                    let tx_ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                        hwnd,
                        windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                    ) as *mut mpsc::Sender<TrayAction>;
                    if !tx_ptr.is_null() {
                        let tx = &*tx_ptr;
                        let _ = tx.send(TrayAction::FocusPrevious);
                    }
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let cmd_id = (wparam.0 & 0xFFFF) as usize;
                // Get the action_tx from window user data
                let tx_ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                    hwnd,
                    windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                ) as *mut mpsc::Sender<TrayAction>;
                if !tx_ptr.is_null() {
                    let tx = &*tx_ptr;
                    match cmd_id {
                        ID_TRAY_SHOW_HIDE => { let _ = tx.send(TrayAction::ShowHide); }
                        ID_TRAY_RELOAD_CONFIG => { let _ = tx.send(TrayAction::ReloadConfig); }
                        ID_TRAY_OPEN_CONFIG_DIR => { let _ = tx.send(TrayAction::OpenConfigDir); }
                        ID_TRAY_ABOUT => { let _ = tx.send(TrayAction::About); }
                        ID_TRAY_FOCUS_PREV => { let _ = tx.send(TrayAction::FocusPrevious); }
                        ID_TRAY_QUIT => { let _ = tx.send(TrayAction::Quit); }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // Clean up the stored Sender pointer
                let tx_ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                    hwnd,
                    windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                ) as *mut mpsc::Sender<TrayAction>;
                if !tx_ptr.is_null() {
                    drop(Box::from_raw(tx_ptr));
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn show_context_menu(hwnd: HWND) {
        unsafe {
            let hmenu = match CreatePopupMenu() {
                Ok(m) => m,
                Err(e) => {
                    warn!("CreatePopupMenu failed: {:?}", e);
                    return;
                }
            };

            // Build all menu strings as null-terminated UTF-16. Storing in
            // locals keeps the wide strings alive for the duration of the
            // AppendMenuW calls.
            let focus_prev: Vec<u16> = "Focus Last Tile\0".encode_utf16().collect();
            let show_hide_text: Vec<u16> = "Show/Hide\0".encode_utf16().collect();
            let reload_text: Vec<u16> = "Reload Config\0".encode_utf16().collect();
            let open_dir_text: Vec<u16> = "Open Config Folder…\0".encode_utf16().collect();
            let about_text: Vec<u16> = "About wiri…\0".encode_utf16().collect();
            let quit_text: Vec<u16> = "Quit\0".encode_utf16().collect();

            AppendMenuW(hmenu, MF_STRING, ID_TRAY_FOCUS_PREV, PCWSTR(focus_prev.as_ptr())).ok();
            AppendMenuW(hmenu, MF_STRING, ID_TRAY_SHOW_HIDE, PCWSTR(show_hide_text.as_ptr())).ok();
            AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
            AppendMenuW(hmenu, MF_STRING, ID_TRAY_RELOAD_CONFIG, PCWSTR(reload_text.as_ptr())).ok();
            AppendMenuW(hmenu, MF_STRING, ID_TRAY_OPEN_CONFIG_DIR, PCWSTR(open_dir_text.as_ptr())).ok();
            AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
            AppendMenuW(hmenu, MF_STRING, ID_TRAY_ABOUT, PCWSTR(about_text.as_ptr())).ok();
            AppendMenuW(hmenu, MF_STRING, ID_TRAY_QUIT, PCWSTR(quit_text.as_ptr())).ok();

            let mut pt = POINT { x: 0, y: 0 };
            let _ = GetCursorPos(&mut pt);

            // Set foreground window so the menu dismisses properly
            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(
                hmenu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                pt.x,
                pt.y,
                0,
                hwnd,
                None,
            )
            .ok();
            DestroyMenu(hmenu).ok();
        }
    }

    /// Open the wiri config folder in Explorer, creating it if missing.
    /// Used by the tray menu's "Open Config Folder…" item.
    pub fn open_config_folder() -> Result<()> {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let dir = crate::config::default_config_dir()
            .ok_or_else(|| anyhow::anyhow!("APPDATA environment variable not set"))?;
        // Best-effort: create the folder if missing.
        let _ = std::fs::create_dir_all(&dir);

        let dir_w: Vec<u16> = dir.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let verb_w: Vec<u16> = "open\0".encode_utf16().collect();

        unsafe {
            let _ = ShellExecuteW(
                HWND::default(),
                PCWSTR::from_raw(verb_w.as_ptr()),
                PCWSTR::from_raw(dir_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
        Ok(())
    }

    /// Try to receive a tray action (non-blocking).
    /// Returns None if no action is pending.
    pub fn try_recv_action(&mut self) -> Option<TrayAction> {
        self.action_rx.as_mut()?.try_recv().ok()
    }

    /// Get a clone of the action sender for dispatching tray actions
    pub fn action_sender(&self) -> mpsc::Sender<TrayAction> {
        self.action_tx.clone()
    }

    pub async fn run(&self) -> Result<()> {
        tokio::signal::ctrl_c().await.ok();
        Ok(())
    }

    /// Show a balloon notification from the tray icon.
    pub fn show_balloon(&self, title: &str, message: &str) -> Result<()> {
        let hwnd_raw = *self.hwnd_raw.lock();
        let hwnd_val = match hwnd_raw {
            Some(v) => v,
            None => {
                warn!("show_balloon: tray window not yet created");
                return Ok(());
            }
        };
        let hwnd = HWND(hwnd_val as *mut std::ffi::c_void);

        unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            nid.uFlags = NIF_INFO;
            nid.dwInfoFlags = NIIF_INFO;

            // szInfoTitle (max 64 chars including null)
            let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let title_len = title_wide.len().min(nid.szInfoTitle.len());
            nid.szInfoTitle[..title_len].copy_from_slice(&title_wide[..title_len]);

            // szInfo (max 256 chars including null)
            let msg_wide: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
            let msg_len = msg_wide.len().min(nid.szInfo.len());
            nid.szInfo[..msg_len].copy_from_slice(&msg_wide[..msg_len]);

            if !Shell_NotifyIconW(NIM_MODIFY, &nid as *const _ as *const _).as_bool() {
                warn!("show_balloon: Shell_NotifyIconW NIM_MODIFY failed");
            }
        }
        Ok(())
    }

    pub fn set_icon(&mut self, _icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Result<()> {
        Ok(())
    }

    pub fn hide(&self) -> Result<()> {
        Ok(())
    }

    pub fn show(&self) -> Result<()> {
        Ok(())
    }

    /// Destroy the tray icon: post WM_DESTROY to the message-only window so
    /// its loop exits (which triggers Shell_NotifyIconW NIM_DELETE inside the
    /// loop), then wait for the thread to finish.
    pub fn destroy(&self) -> Result<()> {
        let hwnd_raw = *self.hwnd_raw.lock();
        if let Some(raw) = hwnd_raw {
            let hwnd = HWND(raw as *mut std::ffi::c_void);
            unsafe {
                let _ = PostMessageW(hwnd, WM_DESTROY, WPARAM(0), LPARAM(0));
            }
        }
        Ok(())
    }
}

impl Default for TrayIcon {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        // Ask the message loop to exit and join the thread.
        let _ = self.destroy();
        if let Some(handle) = self.thread_handle.take() {
            // Give the thread a moment to process WM_DESTROY, then join.
            let _ = handle.join();
        }
    }
}
