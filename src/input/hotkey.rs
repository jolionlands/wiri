use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT};

/// Windows key modifier (VK_LWIN or VK_RWIN)
#[allow(dead_code)]
const MOD_WIN: windows::Win32::UI::Input::KeyboardAndMouse::HOT_KEY_MODIFIERS =
    windows::Win32::UI::Input::KeyboardAndMouse::HOT_KEY_MODIFIERS(8);

use super::{Action, ModKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HotkeyId(u32);

impl HotkeyId {
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HotkeyBinding {
    pub id: HotkeyId,
    pub modifiers: Vec<ModKey>,
    pub key_code: u32,
    pub action: Action,
}

impl HotkeyBinding {
    pub fn new(id: u32, modifiers: Vec<ModKey>, key_code: u32, action: Action) -> Self {
        Self {
            id: HotkeyId(id),
            modifiers,
            key_code,
            action,
        }
    }

    /// Convert modifiers to Windows MOD_* flags
    pub fn modifier_flags(&self) -> u32 {
        let mut flags = 0u32;
        for m in &self.modifiers {
            flags |= match m {
                ModKey::Alt => MOD_ALT.0,
                ModKey::Ctrl => MOD_CONTROL.0,
                ModKey::Shift => MOD_SHIFT.0,
                ModKey::Win => 8, // MOD_WIN = 8
            };
        }
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modifier_flags() {
        let binding = HotkeyBinding::new(0, vec![ModKey::Win, ModKey::Shift], 0x41, Action::Quit);
        let flags = binding.modifier_flags();
        assert_eq!(flags, MOD_WIN.0 | MOD_SHIFT.0);
    }

    #[test]
    fn test_hotkey_binding() {
        let binding = HotkeyBinding::new(1, vec![ModKey::Ctrl], 0x41, Action::CloseWindow);
        assert_eq!(binding.id.as_u32(), 1);
        assert_eq!(binding.key_code, 0x41);
        assert_eq!(binding.action, Action::CloseWindow);
    }

    #[test]
    fn test_modifier_flags_all() {
        let binding = HotkeyBinding::new(0, vec![ModKey::Ctrl, ModKey::Alt, ModKey::Shift, ModKey::Win], 0x41, Action::Quit);
        let flags = binding.modifier_flags();
        assert_eq!(flags, MOD_CONTROL.0 | MOD_ALT.0 | MOD_SHIFT.0 | MOD_WIN.0);
    }

    #[test]
    fn test_modifier_flags_empty() {
        let binding = HotkeyBinding::new(0, vec![], 0x41, Action::Quit);
        assert_eq!(binding.modifier_flags(), 0);
    }

    #[test]
    fn test_hotkey_binding_equality() {
        let b1 = HotkeyBinding::new(1, vec![ModKey::Ctrl], 0x41, Action::CloseWindow);
        let b2 = HotkeyBinding::new(1, vec![ModKey::Ctrl], 0x41, Action::CloseWindow);
        assert_eq!(b1, b2);
    }

    #[test]
    fn test_hotkey_id() {
        let binding = HotkeyBinding::new(42, vec![], 0x41, Action::Quit);
        assert_eq!(binding.id.as_u32(), 42);
    }

}
