//! Installer generation: MSI (WiX), DMG (macOS), deb (cargo-deb) and rpm
//! (rpmbuild).
//!
//! Installers are always built natively - the platform runner in CI has the
//! tooling (WiX on Windows, hdiutil on macOS, cargo-deb and rpmbuild on
//! Linux) and `cargo xtask doctor` reports what is available locally.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::context;
use crate::dist::{render_template, Arch, TargetOs};

/// A release artifact format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Portable,
    Msi,
    Dmg,
    Deb,
    Rpm,
}

impl Format {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "portable" => Self::Portable,
            "msi" => Self::Msi,
            "dmg" => Self::Dmg,
            "deb" => Self::Deb,
            "rpm" => Self::Rpm,
            other => bail!("unknown format `{other}` (expected portable, msi, dmg, deb or rpm)"),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Msi => "msi",
            Self::Dmg => "dmg",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
        }
    }

    /// Platforms the format can be produced for.
    pub fn supports(&self, os: &TargetOs) -> bool {
        match self {
            Self::Portable => true,
            Self::Msi => matches!(os, TargetOs::Windows),
            Self::Dmg => matches!(os, TargetOs::Macos),
            Self::Deb | Self::Rpm => matches!(os, TargetOs::Linux),
        }
    }

    fn tool(&self) -> &'static str {
        match self {
            Self::Portable => "zip/tar.gz (built in)",
            Self::Msi => "WiX Toolset v3 (candle.exe/light.exe)",
            Self::Dmg => "hdiutil (macOS)",
            Self::Deb => "cargo-deb",
            Self::Rpm => "rpmbuild",
        }
    }

    /// Human readable tool requirement, used by `cargo xtask doctor`.
    pub fn tool_description(&self) -> &'static str {
        self.tool()
    }

    /// Target platform the format belongs to.
    fn platform_label(&self) -> &'static str {
        match self {
            Self::Portable => "any",
            Self::Msi => "Windows",
            Self::Dmg => "macOS",
            Self::Deb | Self::Rpm => "Linux",
        }
    }

    fn install_hint(&self) -> &'static str {
        match self {
            Self::Portable => "",
            Self::Msi => "install with `choco install wixtoolset`",
            Self::Dmg => "dmg packages can only be built on macOS",
            Self::Deb => "install with `cargo install cargo-deb`",
            Self::Rpm => "install with `sudo apt-get install rpm` on Debian/Ubuntu",
        }
    }

    /// Tool requirement hint, used by `cargo xtask doctor`.
    pub fn hint(&self) -> &'static str {
        self.install_hint()
    }
}

/// Path of the tool that produces a format on the host, if it is installed.
pub fn tool_path(format: Format) -> Option<PathBuf> {
    match format {
        Format::Portable => None,
        Format::Msi => find_wix_tool("candle.exe"),
        Format::Dmg => context::which("hdiutil"),
        Format::Deb => context::which("cargo-deb"),
        Format::Rpm => context::which("rpmbuild"),
    }
}

/// Every format the build tool knows about, in a stable order.
pub const ALL_FORMATS: [Format; 5] = [
    Format::Portable,
    Format::Msi,
    Format::Dmg,
    Format::Deb,
    Format::Rpm,
];

/// Everything an installer needs to know about the build being packaged.
pub struct InstallerContext<'a> {
    pub root: &'a Path,
    pub out_dir: &'a Path,
    pub work_dir: &'a Path,
    pub binary: &'a Path,
    pub version: &'a str,
    pub target: &'a str,
    pub target_os: TargetOs,
    pub arch: Arch,
    /// Artifact base name without extension, e.g. `yshell-v0.1.0-linux-x86_64`.
    pub name: String,
}

/// Check that a format can be produced for `os` on this host, with an install
/// hint when the packaging tool is missing.
pub fn ensure_buildable(format: Format, os: &TargetOs) -> Result<()> {
    if format == Format::Portable {
        return Ok(());
    }
    if !format.supports(os) {
        bail!(
            "`{}` packages are built for {} targets only",
            format.name(),
            format.platform_label()
        );
    }
    if tool_path(format).is_none() {
        bail!(
            "`{}` packages need {} ({})",
            format.name(),
            format.tool_description(),
            format.hint()
        );
    }
    Ok(())
}

/// Build one installer and return the produced file.
pub fn build(format: Format, ctx: &InstallerContext<'_>) -> Result<PathBuf> {
    ensure_buildable(format, &ctx.target_os)?;
    match format {
        Format::Portable => bail!("portable archives are assembled by `cargo xtask dist` itself"),
        Format::Msi => msi(ctx),
        Format::Dmg => dmg(ctx),
        Format::Deb => deb(ctx),
        Format::Rpm => rpm(ctx),
    }
}

/// MSI installer built with the WiX Toolset v3 that ships with the Windows
/// GitHub runners (and is available through `choco install wixtoolset`).
fn msi(ctx: &InstallerContext<'_>) -> Result<PathBuf> {
    let candle = find_wix_tool("candle.exe")
        .with_context(|| format!("could not find WiX v3; {}", Format::Msi.install_hint()))?;
    let light = find_wix_tool("light.exe")
        .context("could not find WiX v3 (light.exe); install with `choco install wixtoolset`")?;

    let work = ctx.work_dir.join("wix");
    recreate_dir(&work)?;

    let license_rtf_path = work.join("license.rtf");
    fs::write(&license_rtf_path, license_rtf(&ctx.root.join("LICENSE"))?)?;

    let template = ctx.root.join("packaging/windows/yshell.wxs");
    let rendered = render_template(
        &template,
        &[
            ("@VERSION@", &msi_version(ctx.version)),
            ("@BINARY@", &absolute_path(ctx.binary)?),
            ("@README@", &absolute_path(&ctx.root.join("README.md"))?),
            ("@LICENSE@", &absolute_path(&ctx.root.join("LICENSE"))?),
            ("@LICENSE_RTF@", &absolute_path(&license_rtf_path)?),
        ],
    )?;
    let source = work.join("yshell.wxs");
    fs::write(&source, rendered)?;

    let object = work.join("yshell.wixobj");
    let mut compile = Command::new(&candle);
    compile
        .args(["-nologo", "-arch"])
        .arg(ctx.arch.wix())
        .arg("-out")
        .arg(&object)
        .arg(&source);
    context::run(compile)?;

    let dest = ctx.out_dir.join(format!("{}.msi", ctx.name));
    let mut link = Command::new(&light);
    link.args([
        "-nologo",
        "-ext",
        "WixUIExtension",
        "-cultures:en-us",
        "-out",
    ])
    .arg(&dest)
    .arg(&object);
    context::run(link)?;
    Ok(dest)
}

/// DMG disk image containing a `YShell.app` bundle and an `/Applications`
/// symlink, built with the `hdiutil` that ships with macOS.
fn dmg(ctx: &InstallerContext<'_>) -> Result<PathBuf> {
    let hdiutil = context::which("hdiutil")
        .with_context(|| format!("hdiutil was not found; {}", Format::Dmg.install_hint()))?;

    let stage = ctx.work_dir.join("dmg");
    recreate_dir(&stage)?;
    let app = stage.join("YShell.app");
    let macos_dir = app.join("Contents/MacOS");
    let resources_dir = app.join("Contents/Resources");
    fs::create_dir_all(&macos_dir)?;
    fs::create_dir_all(&resources_dir)?;

    copy_file(ctx.binary, &macos_dir.join("yshell"), true)?;

    let icon_source = ctx.root.join("assets/icons/yshell.icns");
    let icon_entry = if icon_source.is_file() {
        copy_file(&icon_source, &resources_dir.join("yshell.icns"), false)?;
        "  <key>CFBundleIconFile</key>\n  <string>yshell</string>\n".to_string()
    } else {
        String::new()
    };

    let info_plist = render_template(
        &ctx.root.join("packaging/macos/Info.plist"),
        &[
            ("@VERSION@", &short_version(ctx.version)),
            ("@BUNDLE_VERSION@", &numeric_version(ctx.version, 3)),
            ("@ICON_ENTRY@", &icon_entry),
        ],
    )?;
    fs::write(app.join("Contents/Info.plist"), info_plist)?;
    copy_file(
        &ctx.root.join("LICENSE"),
        &resources_dir.join("LICENSE"),
        false,
    )?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let applications = stage.join("Applications");
        let _ = fs::remove_file(&applications);
        symlink("/Applications", &applications)?;
    }
    #[cfg(not(unix))]
    bail!("dmg packages can only be built on macOS");

    let dest = ctx.out_dir.join(format!("{}.dmg", ctx.name));
    let mut command = Command::new(hdiutil);
    command
        .args(["create", "-volname"])
        .arg(format!("YShell {}", short_version(ctx.version)))
        .arg("-srcfolder")
        .arg(&stage)
        .args(["-ov", "-format", "UDZO"])
        .arg(&dest);
    context::run(command)?;
    Ok(dest)
}

/// Debian package built with cargo-deb; the metadata lives in the
/// `[package.metadata.deb]` section of `crates/yshell-app/Cargo.toml`.
fn deb(ctx: &InstallerContext<'_>) -> Result<PathBuf> {
    if context::which("cargo-deb").is_none() {
        bail!("cargo-deb was not found; {}", Format::Deb.install_hint());
    }
    // cargo-deb names the package `yshell_<version>_<arch>.deb`; build it in a
    // scratch directory and copy it to the release name.
    let staging = ctx.work_dir.join("deb");
    recreate_dir(&staging)?;
    let mut command = cargo(ctx);
    command
        .args(["deb", "--no-build", "--no-strip", "-p", "yshell-app"])
        .arg("--target")
        .arg(ctx.target)
        .arg("--deb-version")
        .arg(deb_version(ctx.version))
        .arg("--output")
        .arg(&staging);
    context::run(command)?;

    let produced = find_file(&staging, "deb")?.with_context(|| {
        format!(
            "cargo-deb did not produce a .deb package in {}",
            staging.display()
        )
    })?;
    let dest = ctx.out_dir.join(format!("{}.deb", ctx.name));
    fs::copy(&produced, &dest).with_context(|| {
        format!(
            "failed to copy {} to {}",
            produced.display(),
            dest.display()
        )
    })?;
    Ok(dest)
}

/// RPM package built with rpmbuild and `packaging/linux/yshell.spec.in`.
fn rpm(ctx: &InstallerContext<'_>) -> Result<PathBuf> {
    let rpmbuild = context::which("rpmbuild")
        .with_context(|| format!("rpmbuild was not found; {}", Format::Rpm.install_hint()))?;

    let top = ctx.work_dir.join("rpmbuild");
    recreate_dir(&top)?;
    for dir in ["BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"] {
        fs::create_dir_all(top.join(dir))?;
    }
    let sources = top.join("SOURCES");
    copy_file(ctx.binary, &sources.join("yshell"), true)?;
    copy_file(
        &ctx.root.join("packaging/linux/yshell.desktop"),
        &sources.join("yshell.desktop"),
        false,
    )?;
    copy_file(&ctx.root.join("LICENSE"), &sources.join("LICENSE"), false)?;
    copy_file(
        &ctx.root.join("README.md"),
        &sources.join("README.md"),
        false,
    )?;

    let (version, release) = rpm_version(ctx.version);
    let spec = render_template(
        &ctx.root.join("packaging/linux/yshell.spec.in"),
        &[
            ("@VERSION@", &version),
            ("@RELEASE@", &release),
            ("@ARCH@", ctx.arch.rpm()),
        ],
    )?;
    let spec_path = top.join("SPECS/yshell.spec");
    fs::write(&spec_path, spec)?;

    let mut command = Command::new(&rpmbuild);
    command
        .arg("-bb")
        .arg("--define")
        .arg(format!("_topdir {}", top.display()))
        .arg(&spec_path);
    context::run(command)?;

    let produced = find_file(&top.join("RPMS"), "rpm")?
        .with_context(|| format!("rpmbuild did not produce a package in {}", top.display()))?;
    let dest = ctx.out_dir.join(format!("{}.rpm", ctx.name));
    fs::copy(&produced, &dest).with_context(|| {
        format!(
            "failed to copy {} to {}",
            produced.display(),
            dest.display()
        )
    })?;
    Ok(dest)
}

fn cargo(ctx: &InstallerContext<'_>) -> Command {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(ctx.root);
    command
}

/// Locate a WiX v3 tool: on `PATH`, then in the standard install locations.
fn find_wix_tool(tool: &str) -> Option<PathBuf> {
    if let Some(path) = context::which(tool) {
        return Some(path);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Some(base) = env::var_os(variable) else {
            continue;
        };
        let base = PathBuf::from(base);
        roots.push(base.join("WiX Toolset v3.14").join("bin"));
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("WiX Toolset v3") {
                    roots.push(entry.path().join("bin"));
                }
            }
        }
    }
    roots
        .into_iter()
        .map(|root| root.join(tool))
        .find(|candidate| candidate.is_file())
}

fn license_rtf(license: &Path) -> Result<String> {
    let text = fs::read_to_string(license)
        .with_context(|| format!("failed to read {}", license.display()))?;
    let mut rtf = String::from(
        "{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0\\fmodern Consolas;}}\\viewkind4\\uc1\\pard\\f0\\fs18 ",
    );
    for line in text.lines() {
        let escaped = line
            .replace('\\', "\\\\")
            .replace('{', "\\{")
            .replace('}', "\\}");
        for character in escaped.chars() {
            if character.is_ascii() {
                rtf.push(character);
            } else {
                rtf.push('?');
            }
        }
        rtf.push_str("\\par\r\n");
    }
    rtf.push('}');
    Ok(rtf)
}

fn recreate_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(())
}

/// First file with the given extension below `dir` (sorted for determinism).
fn find_file(dir: &Path, extension: &str) -> Result<Option<PathBuf>> {
    let mut found = Vec::new();
    collect_files(dir, extension, &mut found)?;
    found.sort();
    Ok(found.into_iter().next())
}

fn collect_files(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, extension, found)?;
        } else if path
            .extension()
            .map(|value| value.eq_ignore_ascii_case(extension))
            .unwrap_or(false)
        {
            found.push(path);
        }
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    Ok(absolute.to_string_lossy().into_owned())
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

/// The first up-to-three numeric components of a version, used for formats
/// with strict version requirements (MSI, CFBundleVersion).
fn numeric_components(version: &str) -> Vec<u64> {
    let Some(start) = version.find(|character: char| character.is_ascii_digit()) else {
        return Vec::new();
    };
    let mut components = Vec::new();
    let mut current = String::new();
    for character in version[start..].chars() {
        if character.is_ascii_digit() {
            current.push(character);
        } else if character == '.' && !current.is_empty() {
            components.push(std::mem::take(&mut current));
        } else {
            break;
        }
    }
    if !current.is_empty() {
        components.push(current);
    }
    components
        .into_iter()
        .filter_map(|component| component.parse().ok())
        .collect()
}

fn numeric_version(version: &str, max_components: usize) -> String {
    let components = numeric_components(version);
    if components.is_empty() {
        return "0".to_string();
    }
    components
        .into_iter()
        .take(max_components)
        .map(|component| component.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// MSI requires `major.minor.build` with bounded numbers.
pub fn msi_version(version: &str) -> String {
    let components = numeric_components(version);
    let major = components.first().copied().unwrap_or(0).min(255);
    let minor = components.get(1).copied().unwrap_or(0).min(255);
    let build = components.get(2).copied().unwrap_or(0).min(65535);
    format!("{major}.{minor}.{build}")
}

/// dpkg versions must start with a digit and use a restricted character set.
pub fn deb_version(version: &str) -> String {
    let trimmed = version.strip_prefix('v').unwrap_or(version);
    let valid = trimmed.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '+' | '-' | '~' | ':')
    }) && trimmed.starts_with(|character: char| character.is_ascii_digit());
    if valid {
        return trimmed.to_string();
    }
    let components = numeric_components(version);
    if components.is_empty() {
        "0.0.0".to_string()
    } else {
        components
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// RPM versions must not contain `-`; the label lives in the release instead.
pub fn rpm_version(version: &str) -> (String, String) {
    let components = numeric_components(version);
    if components.is_empty() {
        ("0.0.0".to_string(), "1".to_string())
    } else {
        (
            components
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("."),
            "1".to_string(),
        )
    }
}

fn short_version(version: &str) -> String {
    version.strip_prefix('v').unwrap_or(version).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msi_versions_are_three_bounded_numbers() {
        assert_eq!(msi_version("v0.1.0"), "0.1.0");
        assert_eq!(msi_version("portable-0.0.5"), "0.0.5");
        assert_eq!(msi_version("0.0.0-dev.abc1234"), "0.0.0");
        assert_eq!(msi_version("v1.2.3-rc.4"), "1.2.3");
        assert_eq!(msi_version("300.300.70000"), "255.255.65535");
    }

    #[test]
    fn deb_versions_start_with_a_digit() {
        assert_eq!(deb_version("v0.1.0"), "0.1.0");
        assert_eq!(deb_version("0.0.0-dev.abc1234"), "0.0.0-dev.abc1234");
        assert_eq!(deb_version("portable-0.0.5"), "0.0.5");
    }

    #[test]
    fn rpm_versions_never_contain_hyphens() {
        assert_eq!(
            rpm_version("v1.2.3-rc.1"),
            ("1.2.3".to_string(), "1".to_string())
        );
        assert_eq!(
            rpm_version("portable-0.0.5"),
            ("0.0.5".to_string(), "1".to_string())
        );
    }
}
