//! niri's explicit/user search policy, mapped to Windows. No profile mutation for
//! an explicit CLI or environment path, even when that path does not exist.
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPath {
    pub path: PathBuf,
    pub explicit: bool,
}

pub fn select(
    cli: Option<PathBuf>,
    wiri: Option<OsString>,
    niri: Option<OsString>,
    appdata: Option<OsString>,
) -> ConfigPath {
    let env = wiri
        .filter(|v| !v.is_empty())
        .or_else(|| niri.filter(|v| !v.is_empty()));
    if let Some(path) = cli.or_else(|| env.map(PathBuf::from)) {
        ConfigPath {
            path,
            explicit: true,
        }
    } else {
        ConfigPath {
            path: appdata
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("wiri")
                .join("config.kdl"),
            explicit: false,
        }
    }
}

pub fn resolve(cli: Option<PathBuf>) -> ConfigPath {
    select(
        cli,
        std::env::var_os("WIRI_CONFIG"),
        std::env::var_os("NIRI_CONFIG"),
        std::env::var_os("APPDATA"),
    )
}

impl ConfigPath {
    /// Returns true only for the process that created the implicit default.
    pub fn create_default(&self) -> std::io::Result<bool> {
        use std::io::Write;
        if self.explicit || self.path.exists() {
            return Ok(false);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                file.write_all(include_bytes!("../../resources/default_config.kdl"))?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn env(s: &str) -> Option<OsString> {
        Some(s.into())
    }
    #[test]
    fn explicit_order_and_empty_environment() {
        let cli = Some(PathBuf::from("cli.kdl"));
        assert_eq!(
            select(cli, env("wiri"), env("niri"), env("profile")).path,
            PathBuf::from("cli.kdl")
        );
        assert_eq!(
            select(None, env("wiri"), env("niri"), env("profile")).path,
            PathBuf::from("wiri")
        );
        assert_eq!(
            select(None, env(""), env("niri"), env("profile")).path,
            PathBuf::from("niri")
        );
        let regular = select(None, env(""), env(""), env("profile"));
        assert!(!regular.explicit);
        assert_eq!(
            regular.path,
            PathBuf::from("profile").join("wiri/config.kdl")
        );
    }
    #[test]
    fn explicit_missing_file_is_never_created() {
        let missing = ConfigPath {
            path: PathBuf::from(".scratch/nonexistent-c2-explicit/config.kdl"),
            explicit: true,
        };
        assert!(!missing.create_default().unwrap());
        assert!(!missing.path.exists());
    }
}
