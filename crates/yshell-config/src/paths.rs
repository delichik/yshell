//! Cross-platform configuration directory discovery.
//!
//! Discovery is intentionally injectable so tests can validate platform rules
//! without mutating the process environment.

use std::{collections::HashMap, env, error::Error, fmt, path::PathBuf};

/// Environment variable used to override the `YShell` configuration directory.
pub const CONFIG_DIR_ENV: &str = "YSHELL_CONFIG_DIR";

const APP_DIR_NAME: &str = "yshell";

/// Operating-system families that affect default configuration locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    /// Linux and other Unix-like platforms that follow XDG conventions.
    Unix,
    /// Apple platforms using `~/Library/Application Support`.
    MacOs,
    /// Windows platforms using `%APPDATA%`.
    Windows,
}

impl OperatingSystem {
    /// Returns the current target operating system family.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Unix
        }
    }
}

/// Error returned when no configuration directory can be discovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigPathError {
    /// No usable platform-specific base directory exists in the provided
    /// environment.
    MissingBaseDirectory { os: OperatingSystem },
}

impl fmt::Display for ConfigPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBaseDirectory { os } => {
                write!(f, "could not discover a YShell config directory for {os:?}")
            }
        }
    }
}

impl Error for ConfigPathError {}

/// Testable configuration path resolver.
#[derive(Debug, Clone)]
pub struct ConfigPathResolver {
    os: OperatingSystem,
    env: HashMap<String, String>,
}

impl ConfigPathResolver {
    /// Builds a resolver from the current process environment and target OS.
    #[must_use]
    pub fn from_current_env() -> Self {
        Self {
            os: OperatingSystem::current(),
            env: env::vars().collect(),
        }
    }

    /// Builds a resolver with explicit OS and environment values.
    #[must_use]
    pub fn new(os: OperatingSystem, env: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            os,
            env: env.into_iter().collect(),
        }
    }

    /// Discovers the configuration directory.
    ///
    /// Precedence:
    /// 1. `YSHELL_CONFIG_DIR`
    /// 2. Platform default (`XDG_CONFIG_HOME`, `%APPDATA%`, or macOS app support)
    /// 3. Home-directory fallback for Unix/macOS when applicable
    ///
    /// Empty environment variables are ignored.
    pub fn discover_config_dir(&self) -> Result<PathBuf, ConfigPathError> {
        if let Some(override_dir) = self.non_empty_env(CONFIG_DIR_ENV) {
            return Ok(PathBuf::from(override_dir));
        }

        match self.os {
            OperatingSystem::Windows => self
                .non_empty_env("APPDATA")
                .map(|base| PathBuf::from(base).join("YShell"))
                .ok_or(ConfigPathError::MissingBaseDirectory { os: self.os }),
            OperatingSystem::MacOs => self
                .non_empty_env("HOME")
                .map(|home| {
                    PathBuf::from(home)
                        .join("Library")
                        .join("Application Support")
                        .join("YShell")
                })
                .ok_or(ConfigPathError::MissingBaseDirectory { os: self.os }),
            OperatingSystem::Unix => self
                .non_empty_env("XDG_CONFIG_HOME")
                .map(|base| PathBuf::from(base).join(APP_DIR_NAME))
                .or_else(|| {
                    self.non_empty_env("HOME")
                        .map(|home| PathBuf::from(home).join(".config").join(APP_DIR_NAME))
                })
                .ok_or(ConfigPathError::MissingBaseDirectory { os: self.os }),
        }
    }

    fn non_empty_env(&self, key: &str) -> Option<&str> {
        self.env
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}

/// Discovers the configuration directory from the current process environment.
pub fn discover_config_dir() -> Result<PathBuf, ConfigPathError> {
    ConfigPathResolver::from_current_env().discover_config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(os: OperatingSystem, values: &[(&str, &str)]) -> ConfigPathResolver {
        ConfigPathResolver::new(
            os,
            values
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        )
    }

    #[test]
    fn explicit_override_wins_on_unix() {
        let path = resolver(
            OperatingSystem::Unix,
            &[
                (CONFIG_DIR_ENV, "/tmp/custom"),
                ("XDG_CONFIG_HOME", "/tmp/xdg"),
            ],
        )
        .discover_config_dir()
        .expect("override should resolve");

        assert_eq!(path, PathBuf::from("/tmp/custom"));
    }

    #[test]
    fn unix_uses_xdg_config_home() {
        let path = resolver(
            OperatingSystem::Unix,
            &[("XDG_CONFIG_HOME", "/home/me/.xdg")],
        )
        .discover_config_dir()
        .expect("xdg config should resolve");

        assert_eq!(path, PathBuf::from("/home/me/.xdg").join(APP_DIR_NAME));
    }

    #[test]
    fn unix_falls_back_to_home_config() {
        let path = resolver(OperatingSystem::Unix, &[("HOME", "/home/me")])
            .discover_config_dir()
            .expect("home fallback should resolve");

        assert_eq!(path, PathBuf::from("/home/me/.config").join(APP_DIR_NAME));
    }

    #[test]
    fn windows_uses_appdata() {
        let path = resolver(
            OperatingSystem::Windows,
            &[("APPDATA", r"C:\Users\me\AppData\Roaming")],
        )
        .discover_config_dir()
        .expect("appdata should resolve");

        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\me\AppData\Roaming").join("YShell")
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let path = resolver(OperatingSystem::MacOs, &[("HOME", "/Users/me")])
            .discover_config_dir()
            .expect("macOS home should resolve");

        assert_eq!(
            path,
            PathBuf::from("/Users/me")
                .join("Library")
                .join("Application Support")
                .join("YShell")
        );
    }

    #[test]
    fn empty_values_are_ignored() {
        let error = resolver(OperatingSystem::Unix, &[(CONFIG_DIR_ENV, ""), ("HOME", "")])
            .discover_config_dir()
            .expect_err("empty values should not resolve");

        assert_eq!(
            error,
            ConfigPathError::MissingBaseDirectory {
                os: OperatingSystem::Unix
            }
        );
    }
}
