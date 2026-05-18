# Input Module Documentation

## Overview

The input module handles all user input devices in wiri including keyboard, pointer (mouse), touch, and global hotkeys. It provides the interface between Windows input APIs and wiri window management actions.

## Module Structure

src/input/
mod.rs - Main input processing, Action enum, InputConfig
hotkey.rs - Global hotkey registration via RegisterHotKey
keyboard.rs - Keyboard state, GetAsyncKeyState, keyboard layout
mouse.rs - Mouse tracking, GetCursorPos, SetCursorPos
pointer.rs - Pointer visibility and WM_POINTER handling
touch.rs - Touch input handling
grab.rs - Input grab system (move/resize)
input_grab.rs - Base grab trait and implementations
hooks.rs - Low-level hooks (WH_KEYBOARD_LL, WH_MOUSE_LL)

## Key Types

### ModKey Enum

Represents modifier keys used in hotkey bindings.

pub enum ModKey { Alt, Ctrl, Shift, Win }

### Action Enum

All input bindings resolve to Action enum values:

pub enum Action {
    Quit,
    Spawn(String),
    Screenshot,
    CloseWindow,
    FullscreenWindow,
    FocusWindow(WindowId),
    MoveColumnLeft,
    MoveColumnRight,
    FocusColumnLeft,
    FocusColumnRight,
    MoveWindowUp,
    MoveWindowDown,
    FocusWorkspace(i32),
    FocusWorkspaceNext,
    FocusWorkspacePrev,
    MoveToWorkspace(i32),
    ToggleFloat,
    ToggleSticky,
    InteractiveMove,
    InteractiveResize,
    PickWindow,
    ReloadConfig,
}

### PointerVisibility Enum

Controls pointer visibility during operations.

pub enum PointerVisibility { Visible, Hidden, Disabled }


### HotkeyBinding Struct

Represents a binding between a key combination and an action.

pub struct HotkeyBinding { modifiers: EnumSet<ModKey>, key: VirtualKey, action: Action, description: String }


### InputConfig Struct

Configuration for all input devices.

pub struct InputConfig { keyboard_layout: String, keyboard_options: String, repeat_rate: u32, repeat_delay: u32, scroll_speed: f64, acceleration: f64, sensitivity: f64, natural_scroll: bool, touch_enabled: bool, tap_to_click: bool, focus_follows_mouse: bool, warp_on_focus: bool, workspace_auto_back_and_forth: bool }

## Global Hotkey Registration

### Overview

wiri uses RegisterHotKey / UnregisterHotKey with WM_HOTKEY messages for global hotkeys that work even when wiri does not have focus.

### HotkeyManager

pub struct HotkeyManager { hotkeys: HashMap<HotkeyId, HotkeyBinding>, next_id: u32 }

### Registration

pub fn register(modifiers: EnumSet<ModKey>, key: VirtualKey, binding: HotkeyBinding) -> Result<HotkeyId, HotkeyError>
pub fn unregister(id: HotkeyId) -> Result<(), HotkeyError>

### WM_HOTKEY Handling

pub fn process_wm_hotkey(hotkey_id: u32)

### Modifier Mask Calculation

fn calculate_modifier_mask(modifiers: EnumSet<ModKey>) -> u32
## Keyboard Handling

### KeyboardState

Tracks current keyboard state using GetAsyncKeyState.

pub struct KeyboardState { last_pressed: HashMap<VirtualKey, KeyState> }


### Key State Tracking

pub fn update()
pub fn is_pressed(key: VirtualKey) -> bool
pub fn is_toggled(key: VirtualKey) -> bool

### Keyboard Layout

wiri retrieves keyboard layout information via GetKeyboardLayout.

pub fn get_keyboard_layout(thread_id: u32) -> Option<HKL>
pub fn get_keyboard_layout_name(hkl: HKL) -> String

### Low-Level Keyboard Hook

For advanced keyboard processing, wiri may use WH_KEYBOARD_LL.


pub struct KeyboardHook { hook: HHOOK, callback: Box<dyn Fn(KeyEvent)> }
## Mouse Handling

### MouseState

pub struct MouseState { position: Point<i32>, last_click: Option<ClickInfo>, is_tracking: bool }

### Cursor Position

Using GetCursorPos and SetCursorPos.

pub fn get_position() -> Point<i32>
pub fn set_position(point: Point<i32>)

### Hover Tracking

Using TrackMouseEvent for hover detection.
pub fn start_hover_tracking(hwnd: HWND, rect: RECT)

### Low-Level Mouse Hook

pub struct MouseHook { hook: HHOOK, callback: Box<dyn Fn(MouseEvent)> }
## Pointer Handling

### WM_POINTER Messages

Enable pointer messages via EnableMouseInPointer(TRUE).
pub fn enable_pointer_messages()

### PointerEvent

pub enum PointerEvent { PointerDown, PointerUpdate, PointerUp, PointerCancel, PointerWheel, PointerHWheel }


### PointerVisibility Management

pub fn set_pointer_visibility(visibility: PointerVisibility)
## Touch Handling

### TouchState

pub struct TouchState { active_touches: HashMap<DWORD, TouchInfo>, gesture_recognizer: GestureRecognizer }

### WM_TOUCH Handling

pub fn process_wm_touch(wparam: WPARAM, lparam: LPARAM)
## Input Grab System

### Grab Trait

pub trait InputGrab { fn name(), fn on_pointer_move(), fn on_pointer_release(), fn on_key_event(), fn cursor_hint() }

### MoveGrab

Intercepts pointer for window movement.
pub struct MoveGrab { window: WindowId, initial_cursor: Point<i32>, initial_window_pos: Point<i32> }


### ResizeGrab

Intercepts pointer for window resizing.
pub enum ResizeEdge { Left, Right, Top, Bottom, TopLeft, TopRight, BottomLeft, BottomRight }
pub struct ResizeGrab { window: WindowId, edge: ResizeEdge, initial_cursor: Point<i32>, initial_window_rect: Rect<i32> }

### SpatialMovementGrab

Keyboard-driven window movement (for accessibility).
pub struct SpatialMovementGrab { window: WindowId, direction: Direction, distance: i32 }

### GrabResult

pub enum GrabResult { Consumed, Defer, Done(Option<Action>) }
## Mouse Capture

During interactive operations (move/resize), wiri uses SetCapture to ensure all mouse events go to the capture window.

### CaptureGuard

pub struct CaptureGuard { original_wnd: Option<HWND> }
pub fn new(hwnd: HWND) -> Self
impl Drop for CaptureGuard { fn drop() }


### Usage in Grabs

pub fn start(window: WindowId, cursor_pos: Point<i32>) -> Option<(Self, CaptureGuard)>
## Input Configuration

### Default Configuration

impl Default for InputConfig

### Configuration File Syntax

[input]
keyboard_layout = 00000409
repeat_rate = 30
repeat_delay = 250

[mouse]
scroll_speed = 1.0
acceleration = 1.0
natural_scroll = false

[touch]
enabled = true
tap_to_click = true

[focus]
follow_mouse = false
warp_on_focus = false
workspace_auto_back_and_forth = false
## Integration Points

### With Layout Module
- Grabs modify window positions and sizes
- Window focus changes trigger layout updates
- Overview mode disables normal input routing

### With Backend Module
- Input events originate from Windows message queue
- Window handles (HWND) obtained from backend

### With Config Module
- InputConfig deserialized from config file
- Hotkey bindings registered during startup

### With Workspace Module
- Workspace switching via hotkeys
- Focus follows mouse behavior per workspace

## Thread Safety

InputManager is not Send or Sync due to raw Windows handles.

## Error Handling

### HotkeyError

pub enum HotkeyError { RegistrationFailed, UnregistrationFailed, DuplicateBinding }


### HookError

pub enum HookError { CreationFailed, ThreadNotFound }
