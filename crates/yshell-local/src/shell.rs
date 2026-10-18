//! Default shell and home directory discovery.
//!
//! The rules mirror D3 in `problems.md`: an explicit override wins, then the
//! platform's notion of the user shell. On Windows the practical probe order is
//! `pwsh.exe` -> `powershell.exe` -> `cmd.exe`; on Unix `$SHELL` is used when it
//! is executable and `/bin/sh` is the fallback.
//!
//! Reading Windows Terminal's default profile is intentionally not implemented
//! here yet: the app settings layer can pass an explicit override once it can
//! resolve that profile, and the probe order above is D3's documented fallback.

use std::env;
use std::path::{Path, PathBuf};

/// Environment variable that overrides the default local shell.
///
/// A future settings UI can set this variable (or construct a
/// [`LocalPtyConfig`](crate::LocalPtyConfig) explicitly) without changing this
/// crate. The value is a program path; it is not validated here so that spawn
/// errors stay visible to the caller.
pub const SHELL_OVERRIDE_ENV: &str = "YSHELL_LOCAL_SHELL";

/// Where a resolved shell program came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellSource {
    /// Explicit override from [`SHELL_OVERRIDE_ENV`].
    Override,
    /// Platform environment, e.g. `$SHELL` on Unix or `%COMSPEC%` on Windows.
    Environment,
    /// A shell discovered by probing the system (Windows `pwsh`/PowerShell).
    SystemDefault,
    /// Hard-coded last resort (`/bin/sh`, `cmd.exe`).
    Fallback,
}

/// A shell program resolved for a local terminal, with no arguments by default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShell {
    /// Absolute path (or, for the last-resort Windows fallback, a bare name).
    pub program: PathBuf,
    /// Arguments to pass to the shell. Empty for interactive shells.
    pub args: Vec<String>,
    /// Why this shell was selected.
    pub source: ShellSource,
}

impl ResolvedShell {
    /// A short display name suitable for a tab title (`bash`, `pwsh`, `cmd`).
    pub fn name(&self) -> String {
        program_name(&self.program)
    }
}

/// A short display name for a shell program path (`/bin/bash` -> `bash`).
pub(crate) fn program_name(program: &Path) -> String {
    program
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| program.display().to_string())
}

/// Resolve the default local shell for this platform.
///
/// This function is cheap and never fails: an unusable override is surfaced by
/// [`LocalPty::spawn`](crate::LocalPty::spawn) with a readable error instead of
/// being silently replaced.
pub fn default_shell() -> ResolvedShell {
    resolve_shell(shell_override())
}

fn resolve_shell(override_program: Option<PathBuf>) -> ResolvedShell {
    if let Some(program) = override_program {
        return ResolvedShell {
            program,
            args: Vec::new(),
            source: ShellSource::Override,
        };
    }

    platform_default_shell()
}

/// The current user's home directory, if the environment provides one.
///
/// This is used as the default working directory for local shells. No passwd
/// database or shell API lookup is performed, which keeps the crate portable
/// and dependency free.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(profile) = non_empty_env("USERPROFILE") {
            return Some(PathBuf::from(profile));
        }
        let drive = non_empty_env("HOMEDRIVE")?;
        let path = non_empty_env("HOMEPATH")?;
        let mut home = PathBuf::from(drive);
        home.push(path);
        Some(home)
    }

    #[cfg(not(windows))]
    {
        non_empty_env("HOME").map(PathBuf::from)
    }
}

fn shell_override() -> Option<PathBuf> {
    non_empty_env(SHELL_OVERRIDE_ENV).map(PathBuf::from)
}

fn non_empty_env(name: &str) -> Option<std::ffi::OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(unix)]
fn platform_default_shell() -> ResolvedShell {
    resolve_unix_shell(env::var_os("SHELL"), is_executable)
}

#[cfg(unix)]
fn resolve_unix_shell(
    shell_env: Option<std::ffi::OsString>,
    is_executable: impl Fn(&std::path::Path) -> bool,
) -> ResolvedShell {
    if let Some(value) = shell_env.filter(|value| !value.is_empty()) {
        let program = PathBuf::from(value);
        if is_executable(&program) {
            return ResolvedShell {
                program,
                args: Vec::new(),
                source: ShellSource::Environment,
            };
        }
    }

    ResolvedShell {
        program: PathBuf::from("/bin/sh"),
        args: Vec::new(),
        source: ShellSource::Fallback,
    }
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn platform_default_shell() -> ResolvedShell {
    for (name, well_known) in [
        ("pwsh.exe", pwsh_candidates()),
        ("powershell.exe", powershell_candidates()),
    ] {
        if let Some(program) = find_windows_program(name, &well_known) {
            return ResolvedShell {
                program,
                args: Vec::new(),
                source: ShellSource::SystemDefault,
            };
        }
    }

    if let Some(program) = comspec_program() {
        return ResolvedShell {
            program,
            args: Vec::new(),
            source: ShellSource::Environment,
        };
    }

    if let Some(program) = find_windows_program("cmd.exe", &cmd_candidates()) {
        return ResolvedShell {
            program,
            args: Vec::new(),
            source: ShellSource::SystemDefault,
        };
    }

    ResolvedShell {
        program: PathBuf::from("cmd.exe"),
        args: Vec::new(),
        source: ShellSource::Fallback,
    }
}

#[cfg(windows)]
fn find_windows_program(name: &str, well_known: &[PathBuf]) -> Option<PathBuf> {
    find_on_path(name).or_else(|| {
        well_known
            .iter()
            .find(|candidate| candidate.is_file())
            .cloned()
    })
}

#[cfg(windows)]
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(windows)]
fn comspec_program() -> Option<PathBuf> {
    let program = PathBuf::from(non_empty_env("COMSPEC")?);
    program.is_file().then_some(program)
}

#[cfg(windows)]
fn system_root() -> Option<PathBuf> {
    non_empty_env("SystemRoot")
        .or_else(|| non_empty_env("windir"))
        .map(PathBuf::from)
}

#[cfg(windows)]
fn pwsh_candidates() -> Vec<PathBuf> {
    let Some(program_files) = non_empty_env("ProgramFiles") else {
        return Vec::new();
    };
    let program_files = PathBuf::from(program_files);
    vec![
        program_files.join("PowerShell").join("7").join("pwsh.exe"),
        program_files
            .join("PowerShell")
            .join("7-preview")
            .join("pwsh.exe"),
    ]
}

#[cfg(windows)]
fn powershell_candidates() -> Vec<PathBuf> {
    system_root()
        .map(|root| {
            vec![root
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")]
        })
        .unwrap_or_default()
}

#[cfg(windows)]
fn cmd_candidates() -> Vec<PathBuf> {
    system_root()
        .map(|root| vec![root.join("System32").join("cmd.exe")])
        .unwrap_or_default()
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::*;

    fn executable_only(expected: &'static str) -> impl Fn(&Path) -> bool {
        move |path| path == Path::new(expected)
    }

    #[test]
    fn executable_shell_env_is_selected() {
        let resolved = resolve_unix_shell(
            Some(OsString::from("/opt/yshell-test-shell")),
            executable_only("/opt/yshell-test-shell"),
        );

        assert_eq!(resolved.program, PathBuf::from("/opt/yshell-test-shell"));
        assert_eq!(resolved.source, ShellSource::Environment);
        assert!(resolved.args.is_empty());
        assert_eq!(resolved.name(), "yshell-test-shell");
    }

    #[test]
    fn missing_shell_env_falls_back_to_bin_sh() {
        let resolved = resolve_unix_shell(
            Some(OsString::from("/opt/yshell-missing-shell")),
            executable_only("/bin/sh"),
        );

        assert_eq!(resolved.program, PathBuf::from("/bin/sh"));
        assert_eq!(resolved.source, ShellSource::Fallback);
    }

    #[test]
    fn empty_shell_env_falls_back_to_bin_sh() {
        let resolved = resolve_unix_shell(Some(OsString::new()), |_| false);

        assert_eq!(resolved.program, PathBuf::from("/bin/sh"));
        assert_eq!(resolved.source, ShellSource::Fallback);
    }

    #[test]
    fn override_wins_and_uses_the_program_stem_as_name() {
        // The override path is deliberately not validated here; spawn reports
        // unusable explicit paths instead of silently replacing them.
        let resolved = resolve_shell(Some(PathBuf::from("/opt/yshell-override-shell")));

        assert_eq!(
            resolved.program,
            PathBuf::from("/opt/yshell-override-shell")
        );
        assert_eq!(resolved.source, ShellSource::Override);
        assert_eq!(resolved.name(), "yshell-override-shell");
        assert!(resolved.args.is_empty());
    }
}
