//! Release packaging: portable archives plus platform installers.
//!
//! `cargo xtask dist` is the single entry point used locally and by the
//! GitHub Actions release pipeline, so a release can be reproduced on any
//! machine with the same toolchain and packaging tools.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::Parsed;
use crate::context::{self, Context};
use crate::installers::{self, Format};

/// Options for `cargo xtask dist`.
pub struct DistOptions {
    pub target: Option<String>,
    pub platform: Option<String>,
    pub version: Option<String>,
    pub formats: Vec<Format>,
    pub out: Option<PathBuf>,
    pub no_build: bool,
    pub no_smoke: bool,
}

impl DistOptions {
    pub fn parse(parsed: &Parsed) -> Result<Self> {
        let formats = match parsed.value("formats") {
            None => vec![Format::Portable],
            Some(raw) => {
                let mut formats = Vec::new();
                for name in raw
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                {
                    let format = Format::parse(name)?;
                    if !formats.contains(&format) {
                        formats.push(format);
                    }
                }
                if formats.is_empty() {
                    bail!("--formats must name at least one format");
                }
                formats
            }
        };
        Ok(Self {
            target: parsed.value("target").map(str::to_string),
            platform: parsed.value("platform").map(str::to_string),
            version: parsed.value("version").map(str::to_string),
            formats,
            out: parsed.value("out").map(PathBuf::from),
            no_build: parsed.flag("no-build"),
            no_smoke: parsed.flag("no-smoke"),
        })
    }
}

/// Options for `cargo xtask checksums`.
pub struct ChecksumOptions {
    pub dir: Option<String>,
    pub output: Option<String>,
}

/// Operating system of the build target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetOs {
    Windows,
    Macos,
    Linux,
    Other(String),
}

impl TargetOs {
    fn parse(triple: &str) -> Result<Self> {
        let os = triple
            .split('-')
            .find(|part| {
                matches!(
                    *part,
                    "windows" | "darwin" | "linux" | "freebsd" | "netbsd" | "openbsd" | "android"
                )
            })
            .or_else(|| triple.split('-').nth(2))
            .with_context(|| format!("cannot detect the operating system in `{triple}`"))?;
        Ok(match os {
            "windows" => Self::Windows,
            "darwin" => Self::Macos,
            "linux" => Self::Linux,
            other => Self::Other(other.to_string()),
        })
    }

    pub fn slug(&self) -> &str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Other(os) => os,
        }
    }

    pub fn binary_name(&self) -> &str {
        match self {
            Self::Windows => "yshell.exe",
            _ => "yshell",
        }
    }
}

/// CPU architecture of the build target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    I686,
    Arm,
    Other(String),
}

impl Arch {
    fn parse(triple: &str) -> Result<Self> {
        let arch = triple
            .split('-')
            .next()
            .with_context(|| format!("cannot detect the architecture in `{triple}`"))?;
        Ok(match arch {
            "x86_64" => Self::X86_64,
            "aarch64" => Self::Aarch64,
            "i686" | "i586" => Self::I686,
            "arm" | "armv7" => Self::Arm,
            other => Self::Other(other.to_string()),
        })
    }

    pub fn slug(&self) -> &str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::I686 => "i686",
            Self::Arm => "armv7",
            Self::Other(arch) => arch,
        }
    }

    /// RPM architecture name.
    pub fn rpm(&self) -> &str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::I686 => "i686",
            Self::Arm => "armv7hl",
            Self::Other(arch) => arch,
        }
    }

    /// WiX `-arch` value.
    pub fn wix(&self) -> &str {
        match self {
            Self::X86_64 => "x64",
            Self::Aarch64 => "arm64",
            Self::I686 => "x86",
            Self::Arm => "arm",
            Self::Other(arch) => arch,
        }
    }
}

/// Build release artifacts for one target.
pub fn run(ctx: &Context, options: &DistOptions) -> Result<()> {
    let target = match &options.target {
        Some(target) => target.clone(),
        None => context::host_triple()?,
    };
    let target_os = TargetOs::parse(&target)?;
    let arch = Arch::parse(&target)?;
    let platform = match &options.platform {
        Some(platform) => platform.clone(),
        None => format!("{}-{}", target_os.slug(), arch.slug()),
    };
    let version = resolve_version(ctx, options.version.as_deref())?;
    let out_dir = match &options.out {
        Some(dir) => dir.clone(),
        None => ctx.dist_dir(),
    };
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;
    let work_dir = out_dir.join(".work");
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    // Fail before the expensive release build when a requested format cannot
    // be produced here.
    for format in &options.formats {
        installers::ensure_buildable(*format, &target_os)?;
    }

    println!("==> target {target} ({platform}), version {version}");
    let binary = ctx.release_binary(&target, target_os.binary_name());
    if options.no_build {
        if !binary.is_file() {
            bail!(
                "{} does not exist; run without --no-build to build it first",
                binary.display()
            );
        }
        println!("==> reusing {}", binary.display());
    } else {
        let mut command = context::cargo(ctx);
        command.args([
            "build",
            "--release",
            "--locked",
            "--all-features",
            "--target",
            &target,
            "-p",
            "yshell-app",
        ]);
        context::run(command)?;
    }

    if !options.no_smoke {
        let mut command = Command::new(&binary);
        command.arg("--version");
        context::run(command).with_context(|| {
            format!(
                "{} failed to run (use --no-smoke to skip)",
                binary.display()
            )
        })?;
    }

    let mut artifacts: Vec<PathBuf> = Vec::new();
    let portable_name = format!("yshell-{version}-{platform}-portable");
    if options.formats.contains(&Format::Portable) {
        let staging = out_dir.join(&portable_name);
        prepare_portable_staging(&ctx.root, &binary, &target_os, &target, &version, &staging)?;
        let archive = archive_portable(&target_os, &staging, &out_dir, &portable_name)?;
        println!("==> portable archive {}", archive.display());
        artifacts.push(archive);
    }

    let installer_name = format!("yshell-{version}-{platform}");
    for format in &options.formats {
        if *format == Format::Portable {
            continue;
        }
        let installer = installers::InstallerContext {
            root: &ctx.root,
            out_dir: &out_dir,
            work_dir: &work_dir,
            binary: &binary,
            version: &version,
            target: &target,
            target_os: target_os.clone(),
            arch: arch.clone(),
            name: installer_name.clone(),
        };
        let artifact = installers::build(*format, &installer)?;
        println!("==> {} package {}", format.name(), artifact.display());
        artifacts.push(artifact);
    }

    for artifact in &artifacts {
        let digest = sha256_file(artifact)?;
        let sidecar = sidecar_path(artifact);
        let file_name = file_name(artifact)?;
        fs::write(&sidecar, format!("{digest}  {file_name}\n"))
            .with_context(|| format!("failed to write {}", sidecar.display()))?;
    }

    if let Some(summary) = env::var_os("GITHUB_OUTPUT") {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(summary)
            .context("failed to open $GITHUB_OUTPUT")?;
        let package = if options.formats.contains(&Format::Portable) {
            &portable_name
        } else {
            &installer_name
        };
        writeln!(file, "version={version}")?;
        writeln!(file, "package={package}")?;
        writeln!(file, "artifacts={}", files_joined(&artifacts))?;
    }

    println!("==> dist complete");
    for artifact in &artifacts {
        println!("    {}", artifact.display());
    }
    Ok(())
}

/// Write `SHA256SUMS.txt` for the archives and installers in a directory.
pub fn checksums(ctx: &Context, options: &ChecksumOptions) -> Result<()> {
    let dir = match &options.dir {
        Some(dir) => PathBuf::from(dir),
        None => ctx.dist_dir(),
    };
    let output = match &options.output {
        Some(output) => PathBuf::from(output),
        None => dir.join("SHA256SUMS.txt"),
    };
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }

    let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_release_artifact(path))
        .collect();
    files.sort();

    let mut contents = String::new();
    for file in &files {
        contents.push_str(&format!("{}  {}\n", sha256_file(file)?, file_name(file)?));
    }
    fs::write(&output, contents)
        .with_context(|| format!("failed to write {}", output.display()))?;
    println!("==> wrote {} ({} artifacts)", output.display(), files.len());
    Ok(())
}

/// Version label used in artifact names.
pub fn resolve_version(ctx: &Context, explicit: Option<&str>) -> Result<String> {
    if let Some(version) = explicit.filter(|value| !value.trim().is_empty()) {
        return Ok(version.to_string());
    }
    if let Some(version) = env::var("YSHELL_RELEASE_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(version);
    }
    if env::var("GITHUB_REF_TYPE").ok().as_deref() == Some("tag") {
        if let Some(name) = env::var("GITHUB_REF_NAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(name);
        }
    }
    let mut describe = Command::new("git");
    describe
        .current_dir(&ctx.root)
        .args(["describe", "--tags", "--exact-match"]);
    if let Some(tag) = context::capture(describe) {
        return Ok(tag);
    }
    let mut rev = Command::new("git");
    rev.current_dir(&ctx.root)
        .args(["rev-parse", "--short", "HEAD"]);
    if let Some(sha) = context::capture(rev) {
        return Ok(format!("0.0.0-dev.{sha}"));
    }
    Ok("0.0.0-dev.local".to_string())
}

/// Render a template by replacing `@PLACEHOLDER@` markers.
pub fn render_template(path: &Path, replacements: &[(&str, &str)]) -> Result<String> {
    let mut text = fs::read_to_string(path)
        .with_context(|| format!("failed to read template {}", path.display()))?;
    for (placeholder, value) in replacements {
        text = text.replace(placeholder, value);
    }
    Ok(text)
}

fn prepare_portable_staging(
    root: &Path,
    binary: &Path,
    target_os: &TargetOs,
    target: &str,
    version: &str,
    staging: &Path,
) -> Result<()> {
    if staging.exists() {
        fs::remove_dir_all(staging)
            .with_context(|| format!("failed to remove {}", staging.display()))?;
    }
    fs::create_dir_all(staging)
        .with_context(|| format!("failed to create {}", staging.display()))?;

    copy_file(binary, &staging.join(target_os.binary_name()), true)?;
    copy_file(&root.join("README.md"), &staging.join("README.md"), false)?;
    copy_file(&root.join("LICENSE"), &staging.join("LICENSE"), false)?;
    copy_file(
        &root.join("packaging/PORTABLE.txt"),
        &staging.join("PORTABLE.txt"),
        false,
    )?;
    let launcher = match target_os {
        TargetOs::Windows => "yshell-portable.cmd",
        _ => "yshell-portable.sh",
    };
    copy_file(
        &root.join("packaging").join(launcher),
        &staging.join(launcher),
        true,
    )?;

    let mut build_info = String::new();
    build_info.push_str("YShell portable build\n");
    build_info.push_str(&format!("version : {version}\n"));
    build_info.push_str(&format!("target  : {target}\n"));
    build_info.push_str(&format!(
        "rustc   : {}\n",
        context::rustc_version().unwrap_or_else(|| "unknown".to_string())
    ));
    build_info.push_str(&format!(
        "commit  : {}\n",
        context::git_describe(root).unwrap_or_else(|| "unknown".to_string())
    ));
    fs::write(staging.join("BUILD.txt"), build_info)?;
    Ok(())
}

fn archive_portable(
    target_os: &TargetOs,
    staging: &Path,
    out_dir: &Path,
    package: &str,
) -> Result<PathBuf> {
    let entries = collect_files(staging)?;
    match target_os {
        TargetOs::Windows => {
            let dest = out_dir.join(format!("{package}.zip"));
            write_zip(&dest, &entries)?;
            Ok(dest)
        }
        _ => {
            let dest = out_dir.join(format!("{package}.tar.gz"));
            write_tar_gz(&dest, &entries)?;
            Ok(dest)
        }
    }
}

struct ArchiveEntry {
    name: String,
    path: PathBuf,
    mode: u32,
}

fn collect_files(dir: &Path) -> Result<Vec<ArchiveEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let executable = name == "yshell" || name == "yshell.exe" || name.ends_with(".sh");
        entries.push(ArchiveEntry {
            name,
            path,
            mode: if executable { 0o755 } else { 0o644 },
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

fn write_zip(dest: &Path, entries: &[ArchiveEntry]) -> Result<()> {
    let file =
        File::create(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    let mut writer = zip::ZipWriter::new(file);
    for entry in entries {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(entry.mode);
        writer
            .start_file(entry.name.clone(), options)
            .with_context(|| format!("failed to add {} to the zip", entry.name))?;
        writer.write_all(&fs::read(&entry.path)?)?;
    }
    writer
        .finish()
        .with_context(|| format!("failed to finish {}", dest.display()))?;
    Ok(())
}

fn write_tar_gz(dest: &Path, entries: &[ArchiveEntry]) -> Result<()> {
    let file =
        File::create(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::new(6));
    let mut builder = tar::Builder::new(encoder);
    let mtime = source_date_epoch();
    for entry in entries {
        let data = fs::read(&entry.path)?;
        let mut header = tar::Header::new_gnu();
        header.set_path(&entry.name)?;
        header.set_size(data.len() as u64);
        header.set_mode(entry.mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(mtime);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append(&header, data.as_slice())
            .with_context(|| format!("failed to add {} to the archive", entry.name))?;
    }
    let encoder = builder
        .into_inner()
        .context("failed to finish the tar stream")?;
    encoder
        .finish()
        .with_context(|| format!("failed to finish {}", dest.display()))?;
    Ok(())
}

/// `SOURCE_DATE_EPOCH` when set, otherwise 0, so archives stay reproducible.
fn source_date_epoch() -> u64 {
    env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

fn copy_file(source: &Path, dest: &Path, executable: bool) -> Result<()> {
    fs::copy(source, dest)
        .with_context(|| format!("failed to copy {} to {}", source.display(), dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(dest, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
    }
    Ok(())
}

/// SHA-256 digest as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

fn sidecar_path(artifact: &Path) -> PathBuf {
    let mut name = artifact
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".sha256");
    artifact.with_file_name(name)
}

fn file_name(path: &Path) -> Result<String> {
    Ok(path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy()
        .into_owned())
}

fn files_joined(files: &[PathBuf]) -> String {
    files
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_release_artifact(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    [".zip", ".tar.gz", ".msi", ".dmg", ".deb", ".rpm"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_slugs_follow_the_release_naming_scheme() {
        let cases = [
            ("x86_64-pc-windows-msvc", "windows-x86_64", "yshell.exe"),
            ("aarch64-apple-darwin", "macos-aarch64", "yshell"),
            ("x86_64-unknown-linux-gnu", "linux-x86_64", "yshell"),
            ("x86_64-unknown-freebsd", "freebsd-x86_64", "yshell"),
        ];
        for (triple, slug, binary) in cases {
            let os = TargetOs::parse(triple).unwrap();
            let arch = Arch::parse(triple).unwrap();
            assert_eq!(format!("{}-{}", os.slug(), arch.slug()), slug, "{triple}");
            assert_eq!(os.binary_name(), binary, "{triple}");
        }
    }

    #[test]
    fn package_architectures_match_the_platform_conventions() {
        let x86_64 = Arch::parse("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(x86_64.rpm(), "x86_64");
        assert_eq!(x86_64.wix(), "x64");
        let arm64 = Arch::parse("aarch64-apple-darwin").unwrap();
        assert_eq!(arm64.rpm(), "aarch64");
        assert_eq!(arm64.wix(), "arm64");
    }
}
