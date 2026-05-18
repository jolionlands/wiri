use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(u64);

impl OutputId {
    pub fn from_name(name: &str) -> Self {
        let hash = name.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        Self(hash)
    }

    /// Return the inner u64 — useful for cross-thread event payloads
    /// (`BackendEvent::MonitorConnected { id: u64 }`).
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(isize);

impl WindowId {
    pub fn new(hwnd: isize) -> Self {
        Self(hwnd)
    }

    pub fn as_isize(self) -> isize {
        self.0
    }
}

impl fmt::Display for OutputId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Output:{:016x}", self.0)
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Window:{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_id_deterministic() {
        let a = OutputId::from_name("Monitor1");
        let b = OutputId::from_name("Monitor1");
        assert_eq!(a, b);
    }

    #[test]
    fn test_output_id_different_names() {
        let a = OutputId::from_name("Monitor1");
        let b = OutputId::from_name("Monitor2");
        assert_ne!(a, b);
    }

    #[test]
    fn test_output_id_display() {
        let id = OutputId::from_name("Test");
        let s = format!("{}", id);
        assert!(s.starts_with("Output:"));
    }

    #[test]
    fn test_output_id_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(OutputId::from_name("M1"));
        set.insert(OutputId::from_name("M2"));
        set.insert(OutputId::from_name("M1")); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_window_id_new() {
        let id = WindowId::new(12345);
        assert_eq!(id.as_isize(), 12345);
    }

    #[test]
    fn test_window_id_display() {
        let id = WindowId::new(999);
        let s = format!("{}", id);
        assert!(s.contains("999"));
    }

    #[test]
    fn test_window_id_equality() {
        assert_eq!(WindowId::new(100), WindowId::new(100));
        assert_ne!(WindowId::new(100), WindowId::new(101));
    }

    #[test]
    fn test_window_id_zero() {
        let id = WindowId::new(0);
        assert_eq!(id.as_isize(), 0);
    }

    #[test]
    fn test_window_id_negative() {
        let id = WindowId::new(-1);
        assert_eq!(id.as_isize(), -1);
    }
}
