//! macOS code signing and Apple notarization helpers for `cargo xtask dist`.
//!
//! By default (when `YSHELL_MACOS_SIGN_IDENTITY` is unset or empty), binaries
//! and the `gTerm.app` bundle receive ad-hoc signatures (`codesign --sign -`).
//! Setting `YSHELL_MACOS_SIGN_IDENTITY` switches signing to Developer ID +
//! Hardened Runtime + secure timestamp, and setting either a notarytool
//! keychain profile or App Store Connect API credentials enables notarization
//! and stapling on the produced DMG.

#[cfg(any(target_os = "macos", test))]
use std::ffi::OsString;
#[cfg(any(target_os = "macos", test))]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::{env, process::Command};

#[cfg(target_os = "macos")]
use anyhow::Context as _;
#[cfg(any(target_os = "macos", test))]
use anyhow::{bail, Result};

#[cfg(target_os = "macos")]
use crate::context;

/// Signing identity mode for macOS binaries, `.app` bundles and `.dmg` images.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIdentity {
    AdHoc,
    DeveloperId(String),
}

/// Resolved macOS code-signing configuration.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacosSignConfig {
    pub identity: SignIdentity,
}

#[cfg(any(target_os = "macos", test))]
impl MacosSignConfig {
    /// Load signing configuration from `YSHELL_MACOS_SIGN_IDENTITY`.
    #[cfg(target_os = "macos")]
    pub fn from_env() -> Self {
        Self::from_lookup(|key| env::var(key).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let identity = non_empty(&lookup, "YSHELL_MACOS_SIGN_IDENTITY")
            .map(SignIdentity::DeveloperId)
            .unwrap_or(SignIdentity::AdHoc);
        Self { identity }
    }

    pub fn is_developer_id(&self) -> bool {
        matches!(self.identity, SignIdentity::DeveloperId(_))
    }

    pub fn codesign_binary_args(&self, path: &Path) -> Vec<OsString> {
        let mut args = Vec::new();
        args.push(OsString::from("--force"));
        match &self.identity {
            SignIdentity::AdHoc => {
                args.push(OsString::from("--sign"));
                args.push(OsString::from("-"));
            }
            SignIdentity::DeveloperId(identity) => {
                args.push(OsString::from("--options"));
                args.push(OsString::from("runtime"));
                args.push(OsString::from("--timestamp"));
                args.push(OsString::from("--sign"));
                args.push(OsString::from(identity));
            }
        }
        args.push(path.as_os_str().to_os_string());
        args
    }

    pub fn codesign_bundle_args(&self, app_path: &Path) -> Vec<OsString> {
        self.codesign_binary_args(app_path)
    }

    pub fn codesign_dmg_args(&self, dmg_path: &Path) -> Option<Vec<OsString>> {
        match &self.identity {
            SignIdentity::AdHoc => None,
            SignIdentity::DeveloperId(identity) => Some(vec![
                OsString::from("--force"),
                OsString::from("--timestamp"),
                OsString::from("--sign"),
                OsString::from(identity),
                dmg_path.as_os_str().to_os_string(),
            ]),
        }
    }

    pub fn codesign_verify_args(app_path: &Path) -> Vec<OsString> {
        vec![
            OsString::from("--verify"),
            OsString::from("--deep"),
            OsString::from("--strict"),
            OsString::from("--verbose=2"),
            app_path.as_os_str().to_os_string(),
        ]
    }
}

/// Authentication method for Apple `xcrun notarytool submit`.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacosNotaryConfig {
    KeychainProfile(String),
    ApiKey {
        key_path: PathBuf,
        key_id: String,
        issuer_id: String,
    },
    AppleId {
        apple_id: String,
        team_id: String,
        app_password: String,
    },
}

#[cfg(any(target_os = "macos", test))]
impl MacosNotaryConfig {
    /// Load notarization credentials from `YSHELL_MACOS_*` environment variables.
    #[cfg(target_os = "macos")]
    pub fn from_env() -> Result<Option<Self>> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let profile = non_empty(&lookup, "YSHELL_MACOS_NOTARY_PROFILE");

        let api_key_path = non_empty(&lookup, "YSHELL_MACOS_API_KEY_PATH");
        let api_key_id = non_empty(&lookup, "YSHELL_MACOS_API_KEY_ID");
        let api_issuer_id = non_empty(&lookup, "YSHELL_MACOS_API_ISSUER_ID");
        let any_api = api_key_path.is_some() || api_key_id.is_some() || api_issuer_id.is_some();

        let apple_id = non_empty(&lookup, "YSHELL_MACOS_APPLE_ID");
        let apple_team_id = non_empty(&lookup, "YSHELL_MACOS_APPLE_TEAM_ID");
        let apple_app_password = non_empty(&lookup, "YSHELL_MACOS_APPLE_APP_PASSWORD");
        let any_apple =
            apple_id.is_some() || apple_team_id.is_some() || apple_app_password.is_some();

        if any_api {
            let (Some(key_path), Some(key_id), Some(issuer_id)) =
                (api_key_path, api_key_id, api_issuer_id)
            else {
                bail!(
                    "incomplete macOS App Store Connect API notary config: set \
                     YSHELL_MACOS_API_KEY_PATH, YSHELL_MACOS_API_KEY_ID and \
                     YSHELL_MACOS_API_ISSUER_ID"
                );
            };
            return Ok(Some(Self::ApiKey {
                key_path: PathBuf::from(key_path),
                key_id,
                issuer_id,
            }));
        }

        if any_apple {
            let (Some(apple_id), Some(team_id), Some(app_password)) =
                (apple_id, apple_team_id, apple_app_password)
            else {
                bail!(
                    "incomplete macOS Apple ID notary config: set \
                     YSHELL_MACOS_APPLE_ID, YSHELL_MACOS_APPLE_TEAM_ID and \
                     YSHELL_MACOS_APPLE_APP_PASSWORD"
                );
            };
            return Ok(Some(Self::AppleId {
                apple_id,
                team_id,
                app_password,
            }));
        }

        Ok(profile.map(Self::KeychainProfile))
    }

    pub fn notarytool_submit_args(&self, dmg_path: &Path) -> Vec<OsString> {
        let mut args = vec![
            OsString::from("notarytool"),
            OsString::from("submit"),
            dmg_path.as_os_str().to_os_string(),
        ];
        match self {
            Self::KeychainProfile(profile) => {
                args.push(OsString::from("--keychain-profile"));
                args.push(OsString::from(profile));
            }
            Self::ApiKey {
                key_path,
                key_id,
                issuer_id,
            } => {
                args.push(OsString::from("--key"));
                args.push(key_path.as_os_str().to_os_string());
                args.push(OsString::from("--key-id"));
                args.push(OsString::from(key_id));
                args.push(OsString::from("--issuer"));
                args.push(OsString::from(issuer_id));
            }
            Self::AppleId {
                apple_id,
                team_id,
                app_password,
            } => {
                args.push(OsString::from("--apple-id"));
                args.push(OsString::from(apple_id));
                args.push(OsString::from("--team-id"));
                args.push(OsString::from(team_id));
                args.push(OsString::from("--password"));
                args.push(OsString::from(app_password));
            }
        }
        args.push(OsString::from("--wait"));
        args
    }
}

#[cfg(any(target_os = "macos", test))]
fn non_empty(lookup: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    lookup(key).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

/// Sign a standalone macOS binary (used for portable archives).
#[cfg(target_os = "macos")]
pub fn sign_binary(path: &Path) -> Result<()> {
    let config = MacosSignConfig::from_env();
    let codesign = find_codesign()?;
    let mut command = Command::new(codesign);
    command.args(config.codesign_binary_args(path));
    context::run(command)
}

/// Sign nested helper executables inside `gTerm.app`, then sign and strictly
/// verify the `.app` bundle itself.
#[cfg(target_os = "macos")]
pub fn sign_app_bundle(app_path: &Path, helper_binaries: &[PathBuf]) -> Result<MacosSignConfig> {
    let config = MacosSignConfig::from_env();
    let codesign = find_codesign()?;

    for helper in helper_binaries {
        let mut command = Command::new(&codesign);
        command.args(config.codesign_binary_args(helper));
        context::run(command)?;
    }

    let mut bundle_cmd = Command::new(&codesign);
    bundle_cmd.args(config.codesign_bundle_args(app_path));
    context::run(bundle_cmd)?;

    let mut verify = Command::new(&codesign);
    verify.args(MacosSignConfig::codesign_verify_args(app_path));
    context::run(verify)?;

    if config.is_developer_id() {
        eprintln!(
            "==> signed {} with Developer ID + Hardened Runtime",
            app_path.display()
        );
    } else {
        eprintln!(
            "==> signed {} ad-hoc (set YSHELL_MACOS_SIGN_IDENTITY for Developer ID)",
            app_path.display()
        );
    }
    Ok(config)
}

/// Optionally sign and notarize the produced `.dmg` image when Developer ID
/// credentials are configured.
#[cfg(target_os = "macos")]
pub fn sign_and_notarize_dmg(dmg_path: &Path, config: &MacosSignConfig) -> Result<()> {
    if let Some(args) = config.codesign_dmg_args(dmg_path) {
        let codesign = find_codesign()?;
        let mut command = Command::new(codesign);
        command.args(args);
        context::run(command)?;
    }

    if !config.is_developer_id() {
        eprintln!(
            "==> macOS DMG signed ad-hoc (notarization skipped; Gatekeeper requires `xattr -dr com.apple.quarantine` on first launch)"
        );
        return Ok(());
    }

    let Some(notary) = MacosNotaryConfig::from_env()? else {
        eprintln!(
            "==> macOS DMG signed with Developer ID (notarization skipped; set YSHELL_MACOS_NOTARY_PROFILE or YSHELL_MACOS_API_KEY_* to notarize)"
        );
        return Ok(());
    };

    let xcrun = context::which("xcrun")
        .context("xcrun was not found; Xcode Command Line Tools are required for notarization")?;
    // Do not route through `context::run`: Apple ID mode passes `--password` on
    // argv and must never be echoed into CI logs.
    eprintln!("==> xcrun notarytool submit {} --wait", dmg_path.display());
    let status = Command::new(&xcrun)
        .args(notary.notarytool_submit_args(dmg_path))
        .status()
        .context("failed to run `xcrun notarytool submit`")?;
    if !status.success() {
        bail!(
            "`xcrun notarytool submit {}` failed with {status}",
            dmg_path.display()
        );
    }

    let mut staple = Command::new(&xcrun);
    staple.args(["stapler", "staple"]).arg(dmg_path);
    context::run(staple)?;

    let mut validate = Command::new(&xcrun);
    validate.args(["stapler", "validate"]).arg(dmg_path);
    context::run(validate)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn find_codesign() -> Result<PathBuf> {
    context::which("codesign").context("codesign was not found on macOS")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn map_lookup(entries: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn defaults_to_adhoc_signing_when_identity_unset_or_blank() {
        let unset = MacosSignConfig::from_lookup(map_lookup(&[]));
        assert_eq!(unset.identity, SignIdentity::AdHoc);
        assert!(!unset.is_developer_id());

        let blank =
            MacosSignConfig::from_lookup(map_lookup(&[("YSHELL_MACOS_SIGN_IDENTITY", "  ")]));
        assert_eq!(blank.identity, SignIdentity::AdHoc);
        assert_eq!(
            blank.codesign_binary_args(Path::new("/tmp/gterm")),
            vec![
                OsString::from("--force"),
                OsString::from("--sign"),
                OsString::from("-"),
                OsString::from("/tmp/gterm"),
            ]
        );
        assert!(blank
            .codesign_dmg_args(Path::new("/tmp/gTerm.dmg"))
            .is_none());
    }

    #[test]
    fn developer_id_enables_hardened_runtime_timestamp_and_dmg_signing() {
        let config = MacosSignConfig::from_lookup(map_lookup(&[(
            "YSHELL_MACOS_SIGN_IDENTITY",
            "  Developer ID Application: Example (TEAM123)  ",
        )]));
        assert!(config.is_developer_id());
        assert_eq!(
            config.codesign_bundle_args(Path::new("/tmp/gTerm.app")),
            vec![
                OsString::from("--force"),
                OsString::from("--options"),
                OsString::from("runtime"),
                OsString::from("--timestamp"),
                OsString::from("--sign"),
                OsString::from("Developer ID Application: Example (TEAM123)"),
                OsString::from("/tmp/gTerm.app"),
            ]
        );
        assert_eq!(
            config.codesign_dmg_args(Path::new("/tmp/gTerm.dmg")),
            Some(vec![
                OsString::from("--force"),
                OsString::from("--timestamp"),
                OsString::from("--sign"),
                OsString::from("Developer ID Application: Example (TEAM123)"),
                OsString::from("/tmp/gTerm.dmg"),
            ])
        );
        assert_eq!(
            MacosSignConfig::codesign_verify_args(Path::new("/tmp/gTerm.app")),
            vec![
                OsString::from("--verify"),
                OsString::from("--deep"),
                OsString::from("--strict"),
                OsString::from("--verbose=2"),
                OsString::from("/tmp/gTerm.app"),
            ]
        );
    }

    #[test]
    fn notary_config_parses_all_three_modes_and_rejects_partial() {
        assert_eq!(
            MacosNotaryConfig::from_lookup(map_lookup(&[])).expect("empty"),
            None
        );

        let profile = MacosNotaryConfig::from_lookup(map_lookup(&[(
            "YSHELL_MACOS_NOTARY_PROFILE",
            "gterm-notary",
        )]))
        .expect("profile")
        .expect("some");
        assert_eq!(
            profile.notarytool_submit_args(Path::new("/tmp/gTerm.dmg")),
            vec![
                OsString::from("notarytool"),
                OsString::from("submit"),
                OsString::from("/tmp/gTerm.dmg"),
                OsString::from("--keychain-profile"),
                OsString::from("gterm-notary"),
                OsString::from("--wait"),
            ]
        );

        let api = MacosNotaryConfig::from_lookup(map_lookup(&[
            ("YSHELL_MACOS_API_KEY_PATH", "/tmp/AuthKey.p8"),
            ("YSHELL_MACOS_API_KEY_ID", "KEY123"),
            ("YSHELL_MACOS_API_ISSUER_ID", "ISSUER456"),
        ]))
        .expect("api")
        .expect("some");
        assert_eq!(
            api.notarytool_submit_args(Path::new("/tmp/gTerm.dmg")),
            vec![
                OsString::from("notarytool"),
                OsString::from("submit"),
                OsString::from("/tmp/gTerm.dmg"),
                OsString::from("--key"),
                OsString::from("/tmp/AuthKey.p8"),
                OsString::from("--key-id"),
                OsString::from("KEY123"),
                OsString::from("--issuer"),
                OsString::from("ISSUER456"),
                OsString::from("--wait"),
            ]
        );

        let apple = MacosNotaryConfig::from_lookup(map_lookup(&[
            ("YSHELL_MACOS_APPLE_ID", "dev@example.com"),
            ("YSHELL_MACOS_APPLE_TEAM_ID", "TEAM123"),
            ("YSHELL_MACOS_APPLE_APP_PASSWORD", "abcd-efgh"),
        ]))
        .expect("apple")
        .expect("some");
        assert_eq!(
            apple.notarytool_submit_args(Path::new("/tmp/gTerm.dmg")),
            vec![
                OsString::from("notarytool"),
                OsString::from("submit"),
                OsString::from("/tmp/gTerm.dmg"),
                OsString::from("--apple-id"),
                OsString::from("dev@example.com"),
                OsString::from("--team-id"),
                OsString::from("TEAM123"),
                OsString::from("--password"),
                OsString::from("abcd-efgh"),
                OsString::from("--wait"),
            ]
        );

        assert!(MacosNotaryConfig::from_lookup(map_lookup(&[(
            "YSHELL_MACOS_API_KEY_ID",
            "KEY123"
        )]))
        .is_err());
        assert!(MacosNotaryConfig::from_lookup(map_lookup(&[(
            "YSHELL_MACOS_APPLE_ID",
            "dev@example.com"
        )]))
        .is_err());
    }
}
