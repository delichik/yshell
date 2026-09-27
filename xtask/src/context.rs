//! Shared context: workspace paths, process helpers and tool lookup.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _, Result};

/// Paths the build tool operates on.
pub struct Context {
    /// Repository root (the directory containing the workspace `Cargo.toml`).
    pub root: PathBuf,
}

impl Context {
    /// Locate the repository root from the xtask manifest location.
    pub fn discover() -> Result<Self> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest
            .parent()
            .with_context(|| format!("{} has no parent directory", manifest.display()))?
            .to_path_buf();
        Ok(Self { root })
    }

    /// Cargo's target directory, honouring `CARGO_TARGET_DIR`.
    pub fn target_dir(&self) -> PathBuf {
        match env::var_os("CARGO_TARGET_DIR") {
            Some(dir) if !dir.is_empty() => {
                let dir = PathBuf::from(dir);
                if dir.is_absolute() {
                    dir
                } else {
                    self.root.join(dir)
                }
            }
            _ => self.root.join("target"),
        }
    }

    /// Directory for release artifacts (`dist/`).
    pub fn dist_dir(&self) -> PathBuf {
        self.root.join("dist")
    }

    /// Path of the release binary for a target triple.
    pub fn release_binary(&self, target: &str, binary_name: &str) -> PathBuf {
        self.target_dir()
            .join(target)
            .join("release")
            .join(binary_name)
    }
}

/// A `cargo` invocation rooted at the repository.
pub fn cargo(ctx: &Context) -> Command {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    command.current_dir(&ctx.root);
    command
}

/// Run a command, streaming its output, and fail on a non-zero exit status.
pub fn run(mut command: Command) -> Result<()> {
    eprintln!("==> {}", command_line(&command));
    let status = command
        .status()
        .with_context(|| format!("failed to run `{}`", command_line(&command)))?;
    if !status.success() {
        bail!("`{}` failed with {status}", command_line(&command));
    }
    Ok(())
}

/// Run a command capturing stdout; `None` when it is missing or fails.
pub fn capture(mut command: Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Human readable command line for log output.
pub fn command_line(command: &Command) -> String {
    let mut parts = vec![command.get_program().to_string_lossy().into_owned()];
    for argument in command.get_args() {
        let argument = argument.to_string_lossy();
        if argument.contains(' ') {
            parts.push(format!("\"{argument}\""));
        } else {
            parts.push(argument.into_owned());
        }
    }
    parts.join(" ")
}

/// Look up an executable the way a shell would (honours `PATHEXT` on Windows).
pub fn which(tool: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let candidates = executable_names(tool);
    for directory in env::split_paths(&path) {
        for candidate in &candidates {
            let candidate = directory.join(candidate);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn executable_names(tool: &str) -> Vec<String> {
    if Path::new(tool).extension().is_some() {
        return vec![tool.to_string()];
    }
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut names = vec![tool.to_string()];
    for extension in pathext.split(';') {
        let extension = extension.trim();
        if !extension.is_empty() {
            names.push(format!("{tool}{}", extension.to_ascii_lowercase()));
            names.push(format!("{tool}{extension}"));
        }
    }
    names.dedup();
    names
}

#[cfg(not(windows))]
fn executable_names(tool: &str) -> Vec<String> {
    vec![tool.to_string()]
}

/// The host target triple reported by rustc.
pub fn host_triple() -> Result<String> {
    let mut command = Command::new(env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc")));
    command.arg("-vV");
    let output = capture(command).context("failed to query the host triple with `rustc -vV`")?;
    for line in output.lines() {
        if let Some(host) = line.strip_prefix("host: ") {
            return Ok(host.trim().to_string());
        }
    }
    bail!("`rustc -vV` did not report a host triple");
}

/// `rustc --version`, if a toolchain is available.
pub fn rustc_version() -> Option<String> {
    let mut command = Command::new(env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc")));
    command.arg("--version");
    capture(command)
}

/// `cargo --version`, if a toolchain is available.
pub fn cargo_version() -> Option<String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    command.arg("--version");
    capture(command)
}

/// Short commit hash of the checkout, with a `-dirty` marker when modified.
pub fn git_describe(root: &Path) -> Option<String> {
    let mut commit = Command::new("git");
    commit
        .current_dir(root)
        .args(["rev-parse", "--short", "HEAD"]);
    let commit = capture(commit)?;
    let mut status = Command::new("git");
    status.current_dir(root).args(["diff", "--quiet"]);
    let dirty = match status.status() {
        Ok(status) => !status.success(),
        Err(_) => false,
    };
    Some(if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    })
}
