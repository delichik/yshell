//! Configuration for a local PTY shell.

use std::ffi::OsString;
use std::path::PathBuf;

use yshell_ssh::PtySize;

use crate::shell::{default_shell, home_dir};

/// Everything needed to start a local shell in a PTY.
///
/// The environment is a list of overrides on top of the process environment
/// inherited by the child; it is not a complete environment. On Unix the
/// constructor adds `TERM=xterm-256color` unless the caller overrides `TERM`.
///
/// ```
/// use yshell_local::LocalPtyConfig;
///
/// let config = LocalPtyConfig::new("/bin/sh")
///     .with_args(["-c", "echo hello"])
///     .with_size(120, 40);
/// assert_eq!(config.size.columns, 120);
/// assert_eq!(config.size.rows, 40);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPtyConfig {
    /// Shell program to start. Absolute paths are preferred; relative names are
    /// resolved through `PATH` by the backend.
    pub program: PathBuf,
    /// Arguments passed to the shell.
    pub args: Vec<String>,
    /// Working directory. `None` means "let the backend use the user's home
    /// directory".
    pub cwd: Option<PathBuf>,
    /// Environment variable overrides applied to the child.
    pub env: Vec<(OsString, OsString)>,
    /// Initial PTY size in character cells.
    pub size: PtySize,
}

impl LocalPtyConfig {
    /// Start building a configuration for `program`.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: default_env(),
            size: PtySize::default(),
        }
    }

    /// Configuration for the platform default shell (see D3) in the user's home
    /// directory.
    ///
    /// This is the entry point a local terminal tab should use. It does not
    /// read application settings; an override from settings should be applied
    /// by constructing the config explicitly or by setting
    /// [`SHELL_OVERRIDE_ENV`](crate::SHELL_OVERRIDE_ENV).
    pub fn for_default_shell() -> Self {
        let shell = default_shell();
        let mut config = Self::new(shell.program).with_args(shell.args);
        config.cwd = home_dir().filter(|directory| directory.is_dir());
        config
    }

    /// Append one shell argument.
    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append shell arguments.
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the working directory.
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add or replace an environment variable override.
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let key = key.into();
        self.env.retain(|(existing, _)| existing != &key);
        self.env.push((key, value.into()));
        self
    }

    /// Set the initial PTY size in character cells.
    pub fn with_size(mut self, columns: u16, rows: u16) -> Self {
        self.size = PtySize {
            columns,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        };
        self
    }

    /// Set the initial PTY size, including pixel dimensions when known.
    pub fn with_pty_size(mut self, size: PtySize) -> Self {
        self.size = size;
        self
    }

    /// A short display name for the configured shell (`/bin/bash` -> `bash`).
    pub fn shell_name(&self) -> String {
        crate::shell::program_name(&self.program)
    }
}

impl Default for LocalPtyConfig {
    fn default() -> Self {
        Self::for_default_shell()
    }
}

fn default_env() -> Vec<(OsString, OsString)> {
    #[cfg(unix)]
    {
        // Inherited TERM may be unset (for example when launched from a desktop
        // launcher). A 256-color value keeps `ls`/vim colors working; a caller
        // supplied TERM still wins because `with_env` replaces the entry.
        vec![("TERM".into(), "xterm-256color".into())]
    }

    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_sets_size_args_cwd_and_env() {
        let config = LocalPtyConfig::new("/bin/sh")
            .with_arg("-i")
            .with_args(["-c", "true"])
            .with_cwd("/tmp")
            .with_env("YSHELL_TEST", "one")
            .with_env("YSHELL_TEST", "two")
            .with_size(132, 43);

        assert_eq!(config.program, PathBuf::from("/bin/sh"));
        assert_eq!(config.args, ["-i", "-c", "true"]);
        assert_eq!(config.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(config.size.columns, 132);
        assert_eq!(config.size.rows, 43);
        assert_eq!(config.shell_name(), "sh");
        assert_eq!(
            config
                .env
                .iter()
                .filter(|(key, _)| key == "YSHELL_TEST")
                .count(),
            1
        );
        assert!(config
            .env
            .iter()
            .any(|(key, value)| key == "YSHELL_TEST" && value == "two"));
    }

    #[test]
    fn default_config_has_a_program_and_a_nonzero_size() {
        let config = LocalPtyConfig::default();

        assert!(!config.program.as_os_str().is_empty());
        assert!(config.size.columns > 0);
        assert!(config.size.rows > 0);
    }

    #[cfg(unix)]
    #[test]
    fn default_env_sets_term_but_explicit_term_wins() {
        let config = LocalPtyConfig::new("/bin/sh").with_env("TERM", "screen-256color");

        assert!(config
            .env
            .iter()
            .any(|(key, value)| key == "TERM" && value == "screen-256color"));
        assert_eq!(
            config.env.iter().filter(|(key, _)| key == "TERM").count(),
            1
        );
    }
}
