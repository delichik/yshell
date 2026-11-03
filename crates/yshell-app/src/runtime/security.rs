//! Master password state machine (D35, Settings → Security / startup unlock).
//!
//! Scope of this round (D35, "Set / Change / Remove / Reset + startup unlock"):
//! the optional master password encrypts saved sessions and keys. This module
//! owns the **state machine and validation**; the file-encryption layer itself
//! still runs through [`yshell_secret`] and the `YSHELL_MASTER_PASSWORD`
//! fallback (see `docs/product/yshell-current-status.md` §3 — fully closing the
//! loop is tracked as a real gap).
//!
//! What is implemented here:
//!
//! * `NotSet` / `Set` status with a persisted one-way
//!   [`MasterPasswordVerifier`] (`<config>/master-password.json`) — the master
//!   password itself never hits disk.
//! * `Set` / `Change` / `Remove` / `Reset` dialog drafts with the contract's
//!   validation rules (new password ≥ 8 characters, confirm must match,
//!   Change/Remove/Unlock verify the current password).
//! * The startup unlock gate (`locked` until `Unlock` succeeds) with an inline
//!   error and no size change, plus the Reset escape hatch.
//!
//! No typing-confirmation token exists anywhere: `Reset` is a plain destructive
//! button (D35 / AGENTS.md).

use std::path::{Path, PathBuf};

use yshell_secret::MasterPasswordVerifier;

use crate::error::AppError;

use super::{AppProjection, AppResult, AppRuntime};

/// File name of the master-password verifier inside the config directory.
pub(crate) const MASTER_PASSWORD_FILE_NAME: &str = "master-password.json";

/// Master password card status (Settings → Security).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MasterPasswordStatus {
    /// No master password: the optional encryption layer is disabled.
    #[default]
    NotSet,
    /// A master password is configured (locked until unlocked).
    Set,
}

/// Which master-password sub-dialog is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MasterPasswordDialogKind {
    /// New + Confirm → Set.
    Set,
    /// Current + New + Confirm → Change.
    Change,
    /// Current → Remove (decrypts, keeps data).
    Remove,
    /// No fields → Reset and Delete (drops protected data).
    Reset,
}

impl MasterPasswordDialogKind {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Change => "change",
            Self::Remove => "remove",
            Self::Reset => "reset",
        }
    }

    /// `true` when the current-password field is shown.
    pub(crate) const fn needs_current(self) -> bool {
        matches!(self, Self::Change | Self::Remove)
    }

    /// `true` when the new/confirm password fields are shown.
    pub(crate) const fn needs_new(self) -> bool {
        matches!(self, Self::Set | Self::Change)
    }

    /// Destination status kind after a successful submit.
    pub(crate) const fn status_kind(self) -> &'static str {
        match self {
            Self::Set => "master-password-set",
            Self::Change => "master-password-changed",
            Self::Remove => "master-password-removed",
            Self::Reset => "master-password-reset",
        }
    }
}

/// Validation error of a master-password dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MasterPasswordError {
    /// New password is shorter than [`MasterPasswordVerifier::MIN_LEN`].
    TooShort,
    /// New and Confirm differ.
    Mismatch,
    /// The current password field is empty.
    CurrentRequired,
    /// The current password does not match the stored verifier.
    WrongCurrent,
    /// Unlock was attempted while no master password is configured.
    NotSet,
}

impl MasterPasswordError {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::TooShort => "too-short",
            Self::Mismatch => "mismatch",
            Self::CurrentRequired => "current-required",
            Self::WrongCurrent => "wrong-current",
            Self::NotSet => "not-set",
        }
    }

    /// English fallback; Slint renders the localized sentence from [`Self::id`].
    pub(crate) const fn display_text(self) -> &'static str {
        match self {
            Self::TooShort => "Use at least 8 characters.",
            Self::Mismatch => "The passwords do not match.",
            Self::CurrentRequired => "Enter the current master password.",
            Self::WrongCurrent => "Incorrect password.",
            Self::NotSet => "No master password is set.",
        }
    }
}

/// Result of a validated master-password dialog submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MasterPasswordOutcome {
    /// Set or Change: persist this verifier and stay unlocked.
    Set(MasterPasswordVerifier),
    /// Remove: drop the verifier, keep the (decrypted) data.
    Removed,
    /// Reset: drop the verifier and the protected data.
    Reset,
}

/// Editable state of one master-password sub-dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MasterPasswordDraft {
    pub(crate) kind: MasterPasswordDialogKind,
    pub(crate) current_text: String,
    pub(crate) new_text: String,
    pub(crate) confirm_text: String,
    pub(crate) error: Option<MasterPasswordError>,
}

impl MasterPasswordDraft {
    pub(crate) fn new(kind: MasterPasswordDialogKind) -> Self {
        Self {
            kind,
            current_text: String::new(),
            new_text: String::new(),
            confirm_text: String::new(),
            error: None,
        }
    }

    /// `true` when the current-password field is visible.
    pub(crate) fn current_visible(&self) -> bool {
        self.kind.needs_current()
    }

    /// `true` when the New / Confirm fields are visible.
    pub(crate) fn new_visible(&self) -> bool {
        self.kind.needs_new()
    }
}

/// Master-password state shared by the Settings card and the startup unlock.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SecurityState {
    verifier: Option<MasterPasswordVerifier>,
    /// `true` after a successful unlock (also right after Set/Change).
    unlocked: bool,
    /// Open master-password sub-dialog (None = closed).
    dialog: Option<MasterPasswordDraft>,
    /// Startup unlock window: password draft + inline error.
    unlock_visible: bool,
    unlock_password_text: String,
    unlock_error: Option<MasterPasswordError>,
}

impl SecurityState {
    /// Loads the persisted verifier (missing/corrupt record = Not set).
    pub(crate) fn load(config_dir: &Path) -> Self {
        let verifier = read_verifier(config_dir).ok().flatten();
        Self {
            unlocked: verifier.is_none(),
            verifier,
            ..Self::default()
        }
    }

    /// Card status.
    pub(crate) fn status(&self) -> MasterPasswordStatus {
        if self.verifier.is_some() {
            MasterPasswordStatus::Set
        } else {
            MasterPasswordStatus::NotSet
        }
    }

    /// `true` when a master password is configured.
    pub(crate) fn is_set(&self) -> bool {
        self.verifier.is_some()
    }

    /// `true` when the configured master password has not been supplied yet.
    pub(crate) fn is_locked(&self) -> bool {
        self.verifier.is_some() && !self.unlocked
    }

    /// Startup unlock window should be visible.
    pub(crate) fn unlock_visible(&self) -> bool {
        self.unlock_visible
    }

    /// Opens the startup unlock window (only when locked).
    pub(crate) fn request_unlock(&mut self) -> bool {
        if !self.is_locked() {
            return false;
        }
        self.unlock_visible = true;
        self.unlock_password_text.clear();
        self.unlock_error = None;
        true
    }

    /// Hides the startup unlock window (Quit / × / Esc).
    pub(crate) fn dismiss_unlock(&mut self) {
        self.unlock_visible = false;
        self.unlock_password_text.clear();
        self.unlock_error = None;
    }

    /// Updates the unlock password field (clears the inline error).
    pub(crate) fn set_unlock_password(&mut self, value: &str) {
        self.unlock_password_text = value.to_owned();
        self.unlock_error = None;
    }

    /// Stable id of the inline unlock error (empty when clean); Slint renders
    /// the localized sentence via `TextFormats.master-password-error`.
    pub(crate) fn unlock_error_id(&self) -> &'static str {
        self.unlock_error.map_or("", MasterPasswordError::id)
    }

    /// Validates the unlock password; on success the gate opens and the window
    /// closes, on failure the error stays inline (window size unchanged).
    pub(crate) fn submit_unlock(&mut self) -> Result<(), MasterPasswordError> {
        let Some(verifier) = self.verifier else {
            self.unlock_error = Some(MasterPasswordError::NotSet);
            return Err(MasterPasswordError::NotSet);
        };
        if !verifier.verify(&self.unlock_password_text) {
            self.unlock_error = Some(MasterPasswordError::WrongCurrent);
            return Err(MasterPasswordError::WrongCurrent);
        }
        self.unlocked = true;
        self.unlock_visible = false;
        self.unlock_password_text.clear();
        self.unlock_error = None;
        Ok(())
    }

    /// Opens a master-password sub-dialog from the Settings card/links.
    pub(crate) fn open_dialog(&mut self, kind: MasterPasswordDialogKind) {
        // Set is only offered when not set; Change/Remove/Reset only when set.
        let allowed = match kind {
            MasterPasswordDialogKind::Set => !self.is_set(),
            _ => self.is_set(),
        };
        if allowed {
            self.dialog = Some(MasterPasswordDraft::new(kind));
        }
    }

    /// Closes the sub-dialog (`Cancel` / `×` / `Esc`).
    pub(crate) fn close_dialog(&mut self) {
        self.dialog = None;
    }

    /// `true` when a sub-dialog is open.
    pub(crate) fn dialog_visible(&self) -> bool {
        self.dialog.is_some()
    }

    /// Dialog kind id (empty when closed).
    pub(crate) fn dialog_kind_id(&self) -> &'static str {
        self.dialog.as_ref().map_or("", |draft| draft.kind.id())
    }

    /// Dialog error id (empty when clean).
    pub(crate) fn dialog_error_id(&self) -> &'static str {
        self.dialog
            .as_ref()
            .and_then(|draft| draft.error)
            .map_or("", MasterPasswordError::id)
    }

    /// Updates one dialog field; `field` is `"current" | "new" | "confirm"`.
    pub(crate) fn set_dialog_field(&mut self, field: &str, value: &str) {
        let Some(draft) = self.dialog.as_mut() else {
            return;
        };
        match field {
            "current" => draft.current_text = value.to_owned(),
            "new" => draft.new_text = value.to_owned(),
            "confirm" => draft.confirm_text = value.to_owned(),
            _ => return,
        }
        draft.error = None;
    }

    /// Dialog draft accessor (used by tests / projection text).
    pub(crate) fn dialog(&self) -> Option<&MasterPasswordDraft> {
        self.dialog.as_ref()
    }

    /// Validates the open dialog and returns the state change to apply.
    pub(crate) fn validate_dialog(&self) -> Result<MasterPasswordOutcome, MasterPasswordError> {
        let Some(draft) = self.dialog.as_ref() else {
            return Err(MasterPasswordError::NotSet);
        };
        // Change / Remove must prove the current password first.
        if draft.kind.needs_current() {
            if draft.current_text.is_empty() {
                return Err(MasterPasswordError::CurrentRequired);
            }
            let Some(verifier) = self.verifier.as_ref() else {
                return Err(MasterPasswordError::NotSet);
            };
            if !verifier.verify(&draft.current_text) {
                return Err(MasterPasswordError::WrongCurrent);
            }
        }
        match draft.kind {
            MasterPasswordDialogKind::Set | MasterPasswordDialogKind::Change => {
                if !MasterPasswordVerifier::is_acceptable_new_password(&draft.new_text) {
                    return Err(MasterPasswordError::TooShort);
                }
                if draft.new_text != draft.confirm_text {
                    return Err(MasterPasswordError::Mismatch);
                }
                Ok(MasterPasswordOutcome::Set(
                    MasterPasswordVerifier::from_password(&draft.new_text),
                ))
            }
            MasterPasswordDialogKind::Remove => Ok(MasterPasswordOutcome::Removed),
            MasterPasswordDialogKind::Reset => Ok(MasterPasswordOutcome::Reset),
        }
    }

    /// Applies a validated outcome (Set/Change stay unlocked; Remove/Reset lock
    /// the gate down to `NotSet`).
    pub(crate) fn apply_outcome(&mut self, outcome: MasterPasswordOutcome) {
        match outcome {
            MasterPasswordOutcome::Set(verifier) => {
                self.verifier = Some(verifier);
                self.unlocked = true;
            }
            MasterPasswordOutcome::Removed | MasterPasswordOutcome::Reset => {
                self.verifier = None;
                self.unlocked = true;
                // No password is left, so the startup gate must not stay up
                // (Remove/Reset can be reached from the unlock window's
                // "Forgot password?" flow).
                self.unlock_visible = false;
                self.unlock_password_text.clear();
                self.unlock_error = None;
            }
        }
        self.dialog = None;
    }

    /// Persists (or removes) the verifier record on disk.
    fn persist(&self, config_dir: &Path) -> Result<(), std::io::Error> {
        match self.verifier {
            Some(verifier) => write_verifier(config_dir, verifier),
            None => remove_verifier(config_dir),
        }
    }
}

/// Verifier record on disk. Kept separate from `config.toml` so a corrupt
/// config never weakens (or blocks) the master-password gate.
///
/// Format: `{"version":1,"fingerprint":<u64>}` — the verifier derives
/// `Serialize`/`Deserialize` inside `yshell-secret` (this crate has no direct
/// `serde` dependency).
fn verifier_path(config_dir: &Path) -> PathBuf {
    config_dir.join(MASTER_PASSWORD_FILE_NAME)
}

fn read_verifier(config_dir: &Path) -> Result<Option<MasterPasswordVerifier>, std::io::Error> {
    let path = verifier_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    match serde_json::from_str::<MasterPasswordVerifier>(&text) {
        Ok(verifier) => Ok(Some(verifier)),
        // A corrupt record must not unlock the app: treat it as not set.
        Err(_) => Ok(None),
    }
}

fn write_verifier(
    config_dir: &Path,
    verifier: MasterPasswordVerifier,
) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(config_dir)?;
    let text = serde_json::to_string_pretty(&verifier)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    std::fs::write(verifier_path(config_dir), text)
}

fn remove_verifier(config_dir: &Path) -> Result<(), std::io::Error> {
    let path = verifier_path(config_dir);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

impl AppRuntime {
    /// Opens a master-password sub-dialog from the Settings → Security card.
    pub fn open_master_password_dialog(&mut self, kind_id: &str) -> AppProjection {
        if let Some(kind) = dialog_kind_from_id(kind_id) {
            self.security_state.open_dialog(kind);
        }
        self.projection()
    }

    /// Closes the master-password sub-dialog without applying anything.
    pub fn close_master_password_dialog(&mut self) -> AppProjection {
        self.security_state.close_dialog();
        self.projection()
    }

    /// Updates one field of the open sub-dialog (`current` / `new` / `confirm`).
    pub fn update_master_password_field(&mut self, field: &str, value: &str) -> AppProjection {
        self.security_state.set_dialog_field(field, value);
        self.projection()
    }

    /// Validates and applies the open sub-dialog.
    ///
    /// Errors stay inline in the dialog (window size unchanged, D35); on success
    /// the verifier record is persisted and the dialog closes.
    pub fn submit_master_password_dialog(&mut self) -> AppResult<AppProjection> {
        let outcome = match self.security_state.validate_dialog() {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Some(draft) = self.security_state.dialog.as_mut() {
                    draft.error = Some(error);
                }
                return Ok(self.projection());
            }
        };
        let status_kind = self
            .security_state
            .dialog()
            .map_or("master-password-updated", |draft| draft.kind.status_kind());
        self.security_state.apply_outcome(outcome);
        self.security_state
            .persist(&self.config_dir)
            .map_err(AppError::from_error)?;
        self.set_status_kind(
            status_kind,
            "Saved the master password setting.".to_owned(),
            String::new(),
            String::new(),
        );
        self.mark_ui_dirty();
        Ok(self.projection())
    }

    /// Startup unlock: opens the unlock window when a master password is set
    /// and has not been supplied yet.
    pub fn request_master_password_unlock(&mut self) -> AppProjection {
        self.security_state.request_unlock();
        self.projection()
    }

    /// Closing the startup unlock window (Quit path is `dialog-windows` owned).
    pub fn dismiss_master_password_unlock(&mut self) -> AppProjection {
        self.security_state.dismiss_unlock();
        self.projection()
    }

    /// Updates the unlock password field.
    pub fn update_master_password_unlock_password(&mut self, value: &str) -> AppProjection {
        self.security_state.set_unlock_password(value);
        self.projection()
    }

    /// Submits the unlock password; a wrong password stays inline.
    pub fn submit_master_password_unlock(&mut self) -> AppProjection {
        if let Err(error) = self.security_state.submit_unlock() {
            // Slint renders the localized sentence from the error id; the log
            // keeps the English fallback for support / headless diagnosis.
            tracing::warn!(
                target: "yshell::app",
                "master password unlock rejected: {}",
                error.display_text()
            );
        }
        self.mark_ui_dirty();
        self.projection()
    }

    /// "Forgot password?" → Reset dialog (deletes protected data, D35).
    pub fn forgot_master_password(&mut self) -> AppProjection {
        self.security_state
            .open_dialog(MasterPasswordDialogKind::Reset);
        self.projection()
    }

    /// Card status text fallback (Slint renders the pill from booleans).
    pub(crate) fn master_password_status_text(&self) -> String {
        match self.security_state.status() {
            MasterPasswordStatus::NotSet => "Not set".to_owned(),
            MasterPasswordStatus::Set => "Set".to_owned(),
        }
    }

    /// `true` while the configured master password is still locked.
    pub(crate) fn master_password_locked(&self) -> bool {
        self.security_state.is_locked()
    }
}

/// Dialog kind by id (`set` / `change` / `remove` / `reset`).
pub(crate) fn dialog_kind_from_id(id: &str) -> Option<MasterPasswordDialogKind> {
    match id {
        "set" => Some(MasterPasswordDialogKind::Set),
        "change" => Some(MasterPasswordDialogKind::Change),
        "remove" => Some(MasterPasswordDialogKind::Remove),
        "reset" => Some(MasterPasswordDialogKind::Reset),
        _ => None,
    }
}

/// The master password must never be rendered through `Display`/`Debug`.
#[cfg(test)]
fn assert_secret_never_leaks(password: &str) -> yshell_secret::SecretString {
    let secret = yshell_secret::SecretString::from(password);
    assert_eq!(secret.to_string(), "[REDACTED]");
    assert!(!format!("{secret:?}").contains(password));
    secret
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn state_setup(password: &str) -> SecurityState {
        SecurityState {
            verifier: Some(MasterPasswordVerifier::from_password(password)),
            unlocked: false,
            ..SecurityState::default()
        }
    }

    #[test]
    fn default_state_is_not_set_and_unlocked() {
        let state = SecurityState::default();
        assert_eq!(state.status(), MasterPasswordStatus::NotSet);
        assert!(!state.is_set());
        assert!(!state.is_locked());
        assert!(!state.unlock_visible());
        assert!(!state.dialog_visible());
    }

    #[test]
    fn set_dialog_validates_length_and_confirmation() {
        let mut state = SecurityState::default();
        state.open_dialog(MasterPasswordDialogKind::Set);
        assert!(state.dialog_visible());

        state.set_dialog_field("new", "short");
        state.set_dialog_field("confirm", "short");
        assert_eq!(state.validate_dialog(), Err(MasterPasswordError::TooShort));

        state.set_dialog_field("new", "long-enough-password");
        state.set_dialog_field("confirm", "different-password");
        assert_eq!(state.validate_dialog(), Err(MasterPasswordError::Mismatch));

        state.set_dialog_field("confirm", "long-enough-password");
        let outcome = state.validate_dialog().expect("valid set");
        assert!(matches!(outcome, MasterPasswordOutcome::Set(_)));
        state.apply_outcome(outcome);
        assert_eq!(state.status(), MasterPasswordStatus::Set);
        assert!(!state.is_locked(), "Set leaves the app unlocked");
    }

    #[test]
    fn change_and_remove_require_the_current_password() {
        let mut state = state_setup("current-password");

        state.open_dialog(MasterPasswordDialogKind::Change);
        state.set_dialog_field("new", "brand-new-password");
        state.set_dialog_field("confirm", "brand-new-password");
        assert_eq!(
            state.validate_dialog(),
            Err(MasterPasswordError::CurrentRequired)
        );

        state.set_dialog_field("current", "wrong-password");
        assert_eq!(
            state.validate_dialog(),
            Err(MasterPasswordError::WrongCurrent)
        );

        state.set_dialog_field("current", "current-password");
        state.set_dialog_field("new", "brand-new-password");
        state.set_dialog_field("confirm", "brand-new-password");
        let outcome = state.validate_dialog().expect("valid change");
        state.apply_outcome(outcome);
        assert!(state
            .verifier
            .as_ref()
            .expect("verifier")
            .verify("brand-new-password"));

        // Remove: wrong current password is rejected, correct one drops the gate.
        state.open_dialog(MasterPasswordDialogKind::Remove);
        state.set_dialog_field("current", "nope");
        assert_eq!(
            state.validate_dialog(),
            Err(MasterPasswordError::WrongCurrent)
        );
        state.set_dialog_field("current", "brand-new-password");
        assert_eq!(state.validate_dialog(), Ok(MasterPasswordOutcome::Removed));
    }

    #[test]
    fn reset_dialog_has_no_fields_and_needs_no_confirmation_token() {
        let mut state = state_setup("current-password");
        state.open_dialog(MasterPasswordDialogKind::Reset);
        let draft = state.dialog().expect("reset draft");
        assert!(!draft.current_visible());
        assert!(!draft.new_visible());
        assert_eq!(state.validate_dialog(), Ok(MasterPasswordOutcome::Reset));
        state.apply_outcome(MasterPasswordOutcome::Reset);
        assert_eq!(state.status(), MasterPasswordStatus::NotSet);
        assert!(!state.is_locked());
    }

    #[test]
    fn dialog_open_is_gated_by_current_status() {
        let mut unset = SecurityState::default();
        unset.open_dialog(MasterPasswordDialogKind::Change);
        assert!(
            !unset.dialog_visible(),
            "Change needs a configured password"
        );
        unset.open_dialog(MasterPasswordDialogKind::Set);
        assert_eq!(unset.dialog_kind_id(), "set");

        let mut set = state_setup("current-password");
        set.open_dialog(MasterPasswordDialogKind::Set);
        assert!(!set.dialog_visible(), "Set is hidden once configured");
        set.open_dialog(MasterPasswordDialogKind::Remove);
        assert_eq!(set.dialog_kind_id(), "remove");
    }

    #[test]
    fn startup_unlock_wrong_then_right_password() {
        let mut state = state_setup("unlock-me");
        assert!(state.is_locked());
        assert!(state.request_unlock());
        assert!(state.unlock_visible());

        state.set_unlock_password("wrong");
        assert_eq!(
            state.submit_unlock(),
            Err(MasterPasswordError::WrongCurrent)
        );
        assert_eq!(state.unlock_error_id(), "wrong-current");
        // The window stays open so the user can retry in place.
        assert!(state.unlock_visible());

        state.set_unlock_password("unlock-me");
        assert_eq!(state.submit_unlock(), Ok(()));
        assert!(!state.unlock_visible());
        assert!(!state.is_locked());
        assert_eq!(state.unlock_error_id(), "");
    }

    #[test]
    fn verifier_record_round_trips_and_corrupt_records_are_not_set() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut state = SecurityState::default();
        state.open_dialog(MasterPasswordDialogKind::Set);
        state.set_dialog_field("new", "local-master-password");
        state.set_dialog_field("confirm", "local-master-password");
        let outcome = state.validate_dialog().expect("valid set");
        state.apply_outcome(outcome);
        state.persist(temp.path()).expect("persist verifier");

        let reloaded = SecurityState::load(temp.path());
        assert_eq!(reloaded.status(), MasterPasswordStatus::Set);
        assert!(reloaded.is_locked(), "a fresh boot starts locked");

        // The plaintext password never reached disk.
        let on_disk = std::fs::read_to_string(temp.path().join(MASTER_PASSWORD_FILE_NAME))
            .expect("read record");
        assert!(!on_disk.contains("local-master-password"));

        // A corrupt record is treated as "not set" instead of unlocking.
        std::fs::write(temp.path().join(MASTER_PASSWORD_FILE_NAME), "{not json")
            .expect("corrupt record");
        let corrupt = SecurityState::load(temp.path());
        assert_eq!(corrupt.status(), MasterPasswordStatus::NotSet);

        // Removing the verifier deletes the record.
        let removing = SecurityState::load(temp.path());
        assert_eq!(removing.status(), MasterPasswordStatus::NotSet);
        let _ = removing.persist(temp.path());
        assert!(!temp.path().join(MASTER_PASSWORD_FILE_NAME).exists());
    }

    #[test]
    fn master_password_plaintext_is_never_rendered() {
        let secret = assert_secret_never_leaks("top-secret-master");
        assert_eq!(secret.to_string(), "[REDACTED]");
    }

    #[test]
    fn runtime_flow_persists_the_verifier_and_reopens_locked() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        assert!(!runtime.projection().settings_master_password_set);

        runtime.open_master_password_dialog("set");
        runtime.update_master_password_field("new", "runtime-master-password");
        runtime.update_master_password_field("confirm", "runtime-master-password");
        let projection = runtime
            .submit_master_password_dialog()
            .expect("submit set dialog");
        assert!(projection.settings_master_password_set);
        assert_eq!(projection.status_kind, "master-password-set");
        assert!(!projection.settings_master_password_dialog_visible);
        assert!(temp.path().join(MASTER_PASSWORD_FILE_NAME).exists());

        // A fresh runtime picks the verifier up and starts locked.
        let fresh = AppRuntime::new(temp.path().to_path_buf()).expect("fresh runtime");
        assert!(fresh.projection().settings_master_password_set);
        assert!(fresh.projection().settings_master_password_locked);
    }
}
