use std::process::Command;

fn main() {
    // Cargo runs build scripts on the platform's main-thread stack. On Windows
    // MSVC that is ~1 MB, and slint-build's recursive parser/layout passes over
    // the full UI tree can exhaust it (STATUS_STACK_OVERFLOW, 0xc00000fd).
    // Give the build work a dedicated large stack so the host OS stack limit is
    // irrelevant; the final binary's stack is configured separately below.
    let result = std::thread::Builder::new()
        .name("yshell-build-script".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(run)
        .expect("spawn yshell build-script thread")
        .join();
    if result.is_err() {
        std::process::exit(1);
    }
}

fn run() {
    // slint-build emits rerun-if-changed for the .slint files it compiles, but
    // not for the .po files it embeds: without this, editing translations would
    // leave a stale translation table in the binary.
    println!("cargo:rerun-if-changed=../../translations");
    println!("cargo:rerun-if-env-changed=YSHELL_RELEASE_VERSION");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");

    let version = resolve_display_version();
    println!("cargo:rustc-env=YSHELL_VERSION={version}");
    embed_windows_version_info(&version);

    // Windows MSVC defaults the main thread to ~1MB; deep Slint trees (terminal
    // split) can overflow it. Match the common 8MB Unix default as a safety net
    // alongside conditional TerminalView mounting in workspace_split.slint.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some()
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }

    // Pin the widget style so the built-in controls (Button/LineEdit/...) do not
    // drift with the host platform: the design language is WinUI 3 / Fluent 2.
    // Bundle the gettext translations at compile time so the portable build can
    // switch languages at runtime without any file IO.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_bundled_translations("../../translations");
    slint_build::compile_with_config("../../ui/main_window.slint", config)
        .expect("compile Slint UI");
}

/// Same labeling as `xtask dist`: release env, else `0.0.0-dev.<short-sha>`,
/// else crate version. Keeps About / `--version` aligned with artifact names.
fn resolve_display_version() -> String {
    if let Ok(version) = std::env::var("YSHELL_RELEASE_VERSION") {
        let trimmed = version.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    if let Some(sha) = git_short_sha() {
        return format!("0.0.0-dev.{sha}");
    }
    std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned())
}

fn git_short_sha() -> Option<String> {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let root = manifest.parent()?.parent()?;
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let sha = sha.trim();
    if sha.is_empty() {
        None
    } else {
        Some(sha.to_owned())
    }
}

/// Embed PE FileVersion / ProductVersion so Explorer and installers see a real
/// version string (not empty). Best-effort: skip quietly when the host has no
/// `rc` / `windres` (e.g. incomplete mingw).
fn embed_windows_version_info(version: &str) {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let (major, minor, patch, build) = parse_pe_version_parts(version);
    let mut resource = winresource::WindowsResource::new();
    resource.set("FileVersion", version);
    resource.set("ProductVersion", version);
    resource.set("ProductName", "YShell");
    resource.set("FileDescription", "YShell SSH terminal and SFTP client");
    resource.set("LegalCopyright", "YShell contributors");
    resource.set_version_info(winresource::VersionInfo::FILEVERSION, pe_u64(major, minor, patch, build));
    resource.set_version_info(
        winresource::VersionInfo::PRODUCTVERSION,
        pe_u64(major, minor, patch, build),
    );
    if let Err(error) = resource.compile() {
        println!("cargo:warning=skipping Windows version resource: {error}");
    }
}

fn parse_pe_version_parts(version: &str) -> (u16, u16, u16, u16) {
    let stripped = version.strip_prefix('v').unwrap_or(version);
    let numeric = stripped.split(|c: char| !c.is_ascii_digit() && c != '.').next().unwrap_or("");
    let mut parts = numeric.split('.').filter_map(|part| part.parse::<u16>().ok());
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn pe_u64(major: u16, minor: u16, patch: u16, build: u16) -> u64 {
    ((u64::from(major)) << 48)
        | ((u64::from(minor)) << 32)
        | ((u64::from(patch)) << 16)
        | u64::from(build)
}
