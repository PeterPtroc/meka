use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crossterm::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Permission {
    None = 0,
    Read = 1,
    Ask = 2,
    Write = 3,
}

impl Permission {
    pub fn cycle_next(self) -> Permission {
        match self {
            Permission::None => Permission::Read,
            Permission::Read => Permission::Ask,
            Permission::Ask => Permission::Write,
            Permission::Write => Permission::None,
        }
    }

    pub fn indicator(self) -> &'static str {
        match self {
            Permission::None => "n",
            Permission::Read => "r",
            Permission::Ask => "a",
            Permission::Write => "w",
        }
    }

    pub fn indicator_color(self) -> Color {
        match self {
            Permission::None => Color::Green,
            Permission::Read => Color::Yellow,
            Permission::Ask => Color::Magenta,
            Permission::Write => Color::Red,
        }
    }

    /// Returns true if this permission level allows using a tool that requires
    /// `required`. Ask and Write both allow all tools; Read allows Read and
    /// None; None allows only None.
    pub fn allows(self, required: Permission) -> bool {
        match self {
            Permission::None => required == Permission::None,
            Permission::Read => matches!(required, Permission::None | Permission::Read),
            Permission::Ask | Permission::Write => true,
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Permission::None => write!(f, "none"),
            Permission::Read => write!(f, "read"),
            Permission::Ask => write!(f, "ask"),
            Permission::Write => write!(f, "write"),
        }
    }
}

impl FromStr for Permission {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" | "n" => Ok(Permission::None),
            "read" | "r" => Ok(Permission::Read),
            "ask" | "a" => Ok(Permission::Ask),
            "write" | "w" => Ok(Permission::Write),
            other => Err(format!(
                "invalid permission mode '{other}': expected 'none', 'read', 'ask', or 'write'"
            )),
        }
    }
}

#[derive(Clone)]
pub struct SharedPermission {
    inner: Arc<AtomicU8>,
}

impl SharedPermission {
    pub fn new(initial: Permission) -> Self {
        Self {
            inner: Arc::new(AtomicU8::new(initial as u8)),
        }
    }

    pub fn get(&self) -> Permission {
        match self.inner.load(Ordering::Relaxed) {
            0 => Permission::None,
            1 => Permission::Read,
            2 => Permission::Ask,
            3 => Permission::Write,
            _ => Permission::None,
        }
    }

    pub fn set(&self, permission: Permission) {
        self.inner.store(permission as u8, Ordering::Relaxed);
    }

    pub fn cycle(&self) -> Permission {
        let current = self.get();
        let next = current.cycle_next();
        self.set(next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_allows() {
        assert!(Permission::Write.allows(Permission::None));
        assert!(Permission::Write.allows(Permission::Read));
        assert!(Permission::Write.allows(Permission::Ask));
        assert!(Permission::Write.allows(Permission::Write));

        assert!(Permission::Ask.allows(Permission::None));
        assert!(Permission::Ask.allows(Permission::Read));
        assert!(Permission::Ask.allows(Permission::Ask));
        assert!(Permission::Ask.allows(Permission::Write));

        assert!(Permission::Read.allows(Permission::None));
        assert!(Permission::Read.allows(Permission::Read));
        assert!(!Permission::Read.allows(Permission::Ask));
        assert!(!Permission::Read.allows(Permission::Write));

        assert!(Permission::None.allows(Permission::None));
        assert!(!Permission::None.allows(Permission::Read));
        assert!(!Permission::None.allows(Permission::Ask));
        assert!(!Permission::None.allows(Permission::Write));
    }

    #[test]
    fn test_permission_cycle() {
        assert_eq!(Permission::None.cycle_next(), Permission::Read);
        assert_eq!(Permission::Read.cycle_next(), Permission::Ask);
        assert_eq!(Permission::Ask.cycle_next(), Permission::Write);
        assert_eq!(Permission::Write.cycle_next(), Permission::None);
    }

    #[test]
    fn test_permission_from_str() {
        assert_eq!(Permission::from_str("none"), Ok(Permission::None));
        assert_eq!(Permission::from_str("read"), Ok(Permission::Read));
        assert_eq!(Permission::from_str("ask"), Ok(Permission::Ask));
        assert_eq!(Permission::from_str("write"), Ok(Permission::Write));
        assert_eq!(Permission::from_str("n"), Ok(Permission::None));
        assert_eq!(Permission::from_str("r"), Ok(Permission::Read));
        assert_eq!(Permission::from_str("a"), Ok(Permission::Ask));
        assert_eq!(Permission::from_str("w"), Ok(Permission::Write));
        assert!(Permission::from_str("invalid").is_err());
    }

    #[test]
    fn test_permission_display() {
        assert_eq!(Permission::None.to_string(), "none");
        assert_eq!(Permission::Read.to_string(), "read");
        assert_eq!(Permission::Ask.to_string(), "ask");
        assert_eq!(Permission::Write.to_string(), "write");
    }

    #[test]
    fn test_shared_permission() {
        let shared = SharedPermission::new(Permission::Read);
        assert_eq!(shared.get(), Permission::Read);

        shared.set(Permission::Write);
        assert_eq!(shared.get(), Permission::Write);

        let next = shared.cycle();
        assert_eq!(next, Permission::None);
        assert_eq!(shared.get(), Permission::None);
    }

    #[test]
    fn test_shared_permission_clone() {
        let shared = SharedPermission::new(Permission::Read);
        let cloned = shared.clone();

        shared.set(Permission::Write);
        assert_eq!(cloned.get(), Permission::Write);
    }

    #[test]
    fn test_shared_permission_ask() {
        let shared = SharedPermission::new(Permission::Ask);
        assert_eq!(shared.get(), Permission::Ask);
        assert_eq!(shared.get().indicator(), "a");
    }
}
