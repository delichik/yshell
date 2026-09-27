//! Development tasks: formatting, lints, tests, running the app and diagnostics.

use std::env;
use std::fs;

use anyhow::{bail, Context as _, Result};

use crate::context::{self, Context};
use crate::dist::TargetOs;
use crate::installers;

pub struct BuildOptions {
    pub release: bool,
    pub package: Option<String>,
}

pub struct TestOptions {
    pub live: bool,
    pub ssh_target: Option<String>,
    pub sftp_target: Option<String>,
}

pub struct RunOptions {
    pub release: bool,
    pub args: Vec<String>,
}

pub struct CleanOptions {
    pub all: bool,
}

/// `cargo fmt --all --check`.
pub fn fmt(ctx: &Context) -> Result<()> {
    let mut command = context::cargo(ctx);
    command.args(["fmt", "--all", "--check"]);
    context::run(command)
}

/// `cargo clippy` with the same flags CI uses.
pub fn lint(ctx: &Context) -> Result<()> {
    let mut command = context::cargo(ctx);
    command.args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--locked",
        "--",
        "-D",
        "warnings",
    ]);
    context::run(command)
}

/// Run every gate that does not need network access or containers.
pub fn check(ctx: &Context) -> Result<()> {
    let mut failures = Vec::new();
    if let Err(error) = fmt(ctx) {
        eprintln!("xtask: {error:#}");
        failures.push("fmt");
    }
    if let Err(error) = lint(ctx) {
        eprintln!("xtask: {error:#}");
        failures.push("clippy");
    }
    if failures.is_empty() {
        println!("==> check passed");
        Ok(())
    } else {
        bail!("check failed: {}", failures.join(", "));
    }
}

/// Build the workspace (or a single package).
pub fn build(ctx: &Context, options: &BuildOptions) -> Result<()> {
    let mut command = context::cargo(ctx);
    command.arg("build").arg("--locked").arg("--all-features");
    if options.release {
        command.arg("--release");
    }
    match &options.package {
        Some(package) => {
            command.arg("-p").arg(package);
        }
        None => {
            command.arg("--workspace");
        }
    }
    context::run(command)
}

/// Run the workspace tests, or the live SSH/SFTP smoke tests.
pub fn test(ctx: &Context, options: &TestOptions) -> Result<()> {
    let mut command = context::cargo(ctx);
    if options.live {
        let ssh_target = live_target(
            options.ssh_target.clone(),
            "YSHELL_LIVE_SSH_TARGET",
            "--ssh-target",
        )?;
        let sftp_target = match options
            .sftp_target
            .clone()
            .or_else(|| non_empty(env::var("YSHELL_LIVE_SFTP_TARGET").ok()))
        {
            Some(target) => target,
            None => ssh_target.clone(),
        };
        command
            .env("YSHELL_LIVE_SSH_TARGET", &ssh_target)
            .env("YSHELL_LIVE_SFTP_TARGET", &sftp_target);
        command.args([
            "test",
            "--locked",
            "--all-features",
            "-p",
            "yshell-ssh",
            "-p",
            "yshell-sftp",
            "-p",
            "yshell-app",
            "--",
            "live_native",
            "--nocapture",
        ]);
    } else {
        command.args(["test", "--workspace", "--all-features", "--locked"]);
    }
    context::run(command)
}

/// Run the native app from source.
pub fn run_app(ctx: &Context, options: &RunOptions) -> Result<()> {
    let mut command = context::cargo(ctx);
    command.args(["run", "--locked", "-p", "yshell-app"]);
    if options.release {
        command.arg("--release");
    }
    if !options.args.is_empty() {
        command.arg("--");
        command.args(&options.args);
    }
    context::run(command)
}

/// `cargo deny check`.
pub fn deny(ctx: &Context) -> Result<()> {
    if context::which("cargo-deny").is_none() {
        bail!(
            "cargo-deny is not installed; install it with `cargo install cargo-deny` \
             (CI installs it with taiki-e/install-action)"
        );
    }
    let mut command = context::cargo(ctx);
    command.args(["deny", "check"]);
    context::run(command)
}

/// Remove locally produced artifacts.
pub fn clean(ctx: &Context, options: &CleanOptions) -> Result<()> {
    let dist = ctx.dist_dir();
    if dist.is_dir() {
        for entry in
            fs::read_dir(&dist).with_context(|| format!("failed to read {}", dist.display()))?
        {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Only touch what `cargo xtask dist` produces; other content in
            // dist/ (screenshots, scratch dirs) belongs to the developer.
            let produced_by_dist =
                name == ".work" || name == "SHA256SUMS.txt" || name.starts_with("yshell-");
            if !produced_by_dist {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
            } else {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
            }
            println!("==> removed {}", path.display());
        }
    }
    if options.all {
        remove_dir_if_exists(&ctx.target_dir())?;
    }
    Ok(())
}

/// Reproduce the CI gates locally.
pub fn ci(ctx: &Context) -> Result<()> {
    check(ctx)?;
    test(
        ctx,
        &TestOptions {
            live: false,
            ssh_target: None,
            sftp_target: None,
        },
    )
}

/// Print toolchain, packaging tool and workspace diagnostics.
pub fn doctor(ctx: &Context) -> Result<()> {
    let host = context::host_triple().unwrap_or_else(|_| "unknown".to_string());
    println!("xtask      : {}", env!("CARGO_PKG_VERSION"));
    println!("repo root  : {}", ctx.root.display());
    println!("host triple: {host}");
    println!(
        "cargo      : {}",
        context::cargo_version().unwrap_or_else(|| "MISSING".to_string())
    );
    println!(
        "rustc      : {}",
        context::rustc_version().unwrap_or_else(|| "MISSING".to_string())
    );
    println!("target dir : {}", ctx.target_dir().display());
    println!("dist dir   : {}", ctx.dist_dir().display());
    println!(
        "commit     : {}",
        context::git_describe(&ctx.root).unwrap_or_else(|| "unknown".to_string())
    );

    println!();
    println!("support tools:");
    let tools: &[(&str, &str)] = &[
        ("git", "tags, versions and commit labels"),
        ("docker", "live SSH/SFTP smoke tests"),
        ("cargo-deny", "license and advisory checks"),
    ];
    for (tool, purpose) in tools {
        let status = match context::which(tool) {
            Some(path) => format!("ok ({})", path.display()),
            None => "missing".to_string(),
        };
        println!("  {tool:<12} {status:<44} {purpose}");
    }

    println!();
    println!("installer formats buildable on this host:");
    let host_os = host_target_os();
    for format in installers::ALL_FORMATS {
        if !format.supports(&host_os) {
            continue;
        }
        let status = if format == installers::Format::Portable {
            "ok (built in)".to_string()
        } else {
            match installers::tool_path(format) {
                Some(path) => format!("ok ({})", path.display()),
                None => format!("missing ({}; {})", format.tool_description(), format.hint()),
            }
        };
        println!("  {:<9} {status}", format.name());
    }

    if cfg!(target_os = "linux") {
        println!();
        println!("Linux build dependencies (needed to compile):");
        println!("  sudo apt-get install -y --no-install-recommends build-essential \\");
        println!("    pkg-config libssl-dev libx11-dev libxcb1-dev libxcb-render0-dev \\");
        println!("    libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig1-dev");
        println!();
        println!("Linux GUI runtime libraries (needed to run, not to build):");
        println!("  sudo apt-get install -y --no-install-recommends libx11-xcb1 \\");
        println!("    libxcursor1 libxrandr2 libxi6 libxinerama1 libxrender1 \\");
        println!("    libxfixes3 libxkbcommon0 libxkbcommon-x11-0 libfontconfig1 \\");
        println!("    fonts-dejavu-core");
        if ctx.root.starts_with("/mnt/") {
            println!();
            println!(
                "hint: the repository is on a Windows mount; set \
                 CARGO_TARGET_DIR=$HOME/yshell-target to speed up builds"
            );
        }
    }
    Ok(())
}

fn host_target_os() -> TargetOs {
    if cfg!(windows) {
        TargetOs::Windows
    } else if cfg!(target_os = "macos") {
        TargetOs::Macos
    } else {
        TargetOs::Linux
    }
}

fn live_target(cli: Option<String>, variable: &str, flag: &str) -> Result<String> {
    let target = cli.or_else(|| non_empty(env::var(variable).ok()));
    match target {
        Some(target) => Ok(target),
        None => bail!("live tests need `{flag} <user@host:port>` or `{variable}`"),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn remove_dir_if_exists(path: &std::path::Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
        println!("==> removed {}", path.display());
    }
    Ok(())
}
