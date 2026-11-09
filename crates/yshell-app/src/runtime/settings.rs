//! Settings dialog drafts and the single Save pipeline (D33 / D36 / D37).
//!
//! The Settings dialog is one window with a **single** `Cancel | Save` footer:
//! every section edits a draft, `Save` validates *all* drafts and persists them
//! once, and `Cancel` / `×` / `Esc` discard them (the draft is simply reloaded
//! from the persisted config). This module owns:
//!
//! * **App theme** draft (Mode System/Light/Dark + Accent Follow system + six
//!   presets) with a persisted baseline for the instant-preview / revert
//!   contract: selection updates the draft only, Save persists, Cancel reverts.
//! * **SFTP defaults** draft (Show hidden / Conflict action / External editor,
//!   including startup detection of well-known editors; only `Custom` needs a
//!   Browse path — D33).
//! * Terminal interaction index helpers for the draft fields that live in
//!   [`super::panels`] (middle/right click, multi-line paste, auto copy — D37).
//! * [`AppRuntime::save_settings_all`], which validates and persists every
//!   editable section in one config write and then closes the dialog.
//!
//! Persisted values stay in `[ui]` / `[sftp]` / `[terminal]` (`yshell-config`);
//! this module never writes config before Save.

use std::path::PathBuf;

use yshell_config::{
    AccentColor, AppearanceMode, MiddleClickAction, MultiLinePastePolicy, RightClickAction,
    SftpConflictAction, UiProfile,
};
use yshell_ui::appearance::{self, AccentColors, ThemeMode};

use super::panels::{
    parse_settings_usize, SettingsTerminalField, SettingsTerminalStatus,
    SETTINGS_SCROLLBACK_LINES_MAX, SETTINGS_SCROLLBACK_LINES_MIN,
    SETTINGS_SCROLLBACK_MAX_CELLS_MAX, SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
};
use super::theme::{clear_color_overrides, SettingsAppearanceStatus};
use super::*;

// ---------------------------------------------------------------------------
// App theme (D36)
// ---------------------------------------------------------------------------

/// Accent swatch ids in display order: `Follow system` + the six presets.
///
/// The Slint swatch row is rendered in this fixed order; the projection only
/// supplies the light/dark colors and the selected index.
pub(crate) const APP_ACCENT_IDS: [&str; 7] = [
    "follow", "blue", "teal", "purple", "green", "orange", "magenta",
];

/// Draft of the Settings → Appearance → App theme group.
///
/// `System` / `FollowSystem` are the defaults; the draft never touches the
/// persisted config until [`AppRuntime::save_settings_all`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AppThemeDraft {
    pub(crate) mode: AppearanceMode,
    pub(crate) accent: AccentColor,
}

impl AppThemeDraft {
    /// Loads the draft from persisted `[ui]` settings.
    pub(crate) fn from_ui(ui: &UiProfile) -> Self {
        Self {
            mode: ui.appearance_mode,
            accent: ui.appearance_accent,
        }
    }

    /// Mode segmented-control index (0 = System, 1 = Light, 2 = Dark).
    pub(crate) fn mode_index(self) -> i32 {
        match self.mode {
            AppearanceMode::System => 0,
            AppearanceMode::Light => 1,
            AppearanceMode::Dark => 2,
        }
    }

    /// Accent swatch index inside [`APP_ACCENT_IDS`].
    pub(crate) fn accent_index(self) -> i32 {
        accent_swatch_index(self.accent)
    }

    /// Applies a mode segmented-control click; out-of-range indexes are ignored.
    pub(crate) fn set_mode_index(&mut self, index: i32) -> bool {
        let Some(mode) = mode_from_index(index) else {
            return false;
        };
        self.mode = mode;
        true
    }

    /// Applies an accent swatch click; out-of-range indexes are ignored.
    pub(crate) fn set_accent_index(&mut self, index: i32) -> bool {
        let Some(accent) = accent_from_index(index) else {
            return false;
        };
        self.accent = accent;
        true
    }

    /// Slint `Theme.mode` value: 0 = system, 1 = light, 2 = dark.
    ///
    /// Consumed by [`super::projection`] (`settings-appearance-mode-index`) and
    /// pushed into the `Theme` global by the Settings instant preview.
    pub(crate) fn slint_mode_index(self) -> i32 {
        self.mode_index()
    }

    /// UI-side theme mode for preview / commit.
    pub(crate) fn theme_mode(self) -> ThemeMode {
        match self.mode {
            AppearanceMode::System => ThemeMode::System,
            AppearanceMode::Light => ThemeMode::Light,
            AppearanceMode::Dark => ThemeMode::Dark,
        }
    }

    /// Stable id used by `ThemeMode` and status kinds.
    pub(crate) fn mode_id(self) -> &'static str {
        self.theme_mode().id()
    }

    /// Stable accent id; `FollowSystem` maps to the default Windows Blue preset
    /// (Linux / macOS fall back to the current preset per design language §3.4).
    pub(crate) fn accent_id(self) -> &'static str {
        accent_id(self.accent)
    }

    /// Resolved accent colors for the live preview.
    pub(crate) fn accent_colors(self) -> AccentColors {
        appearance::resolve_accent(self.accent_id())
    }
}

/// Mode index → [`AppearanceMode`] (0 System / 1 Light / 2 Dark).
pub(crate) fn mode_from_index(index: i32) -> Option<AppearanceMode> {
    match index {
        0 => Some(AppearanceMode::System),
        1 => Some(AppearanceMode::Light),
        2 => Some(AppearanceMode::Dark),
        _ => None,
    }
}

/// Accent swatch index → [`AccentColor`] (order = [`APP_ACCENT_IDS`]).
pub(crate) fn accent_from_index(index: i32) -> Option<AccentColor> {
    match index {
        0 => Some(AccentColor::FollowSystem),
        1 => Some(AccentColor::Blue),
        2 => Some(AccentColor::Teal),
        3 => Some(AccentColor::Purple),
        4 => Some(AccentColor::Green),
        5 => Some(AccentColor::Orange),
        6 => Some(AccentColor::Magenta),
        _ => None,
    }
}

/// [`AccentColor`] → index inside [`APP_ACCENT_IDS`].
pub(crate) fn accent_swatch_index(accent: AccentColor) -> i32 {
    match accent {
        AccentColor::FollowSystem => 0,
        AccentColor::Blue => 1,
        AccentColor::Teal => 2,
        AccentColor::Purple => 3,
        AccentColor::Green => 4,
        AccentColor::Orange => 5,
        AccentColor::Magenta => 6,
    }
}

/// Stable accent preset id; `FollowSystem` deliberately maps to `blue`.
pub(crate) fn accent_id(accent: AccentColor) -> &'static str {
    match accent {
        AccentColor::FollowSystem | AccentColor::Blue => "blue",
        AccentColor::Teal => "teal",
        AccentColor::Purple => "purple",
        AccentColor::Green => "green",
        AccentColor::Orange => "orange",
        AccentColor::Magenta => "magenta",
    }
}

/// `[u8; 3]` sRGB triple for a UI-side color (projection consumers).
pub(crate) fn rgb_parts(color: appearance::RgbColor) -> [u8; 3] {
    [color.red, color.green, color.blue]
}

/// Light-theme swatch colors for [`APP_ACCENT_IDS`] (dark variants are separate
/// so the row follows the resolved app theme, like the prototype board).
///
/// Index 0 (Follow system) is a placeholder: the Slint `AccentSwatch` paints a
/// half-white / half-dark gradient instead of this solid color.
pub(crate) fn accent_swatches_light() -> Vec<[u8; 3]> {
    let mut swatches = Vec::with_capacity(APP_ACCENT_IDS.len());
    for accent in (0..7).filter_map(accent_from_index) {
        if matches!(accent, AccentColor::FollowSystem) {
            swatches.push([0xff, 0xff, 0xff]);
            continue;
        }
        let colors = appearance::resolve_accent(accent_id(accent));
        swatches.push(rgb_parts(colors.light));
    }
    swatches
}

/// Dark-theme swatch colors for [`APP_ACCENT_IDS`].
///
/// Index 0 placeholder matches [`accent_swatches_light`]; UI uses the system
/// split gradient for Follow system.
pub(crate) fn accent_swatches_dark() -> Vec<[u8; 3]> {
    let mut swatches = Vec::with_capacity(APP_ACCENT_IDS.len());
    for accent in (0..7).filter_map(accent_from_index) {
        if matches!(accent, AccentColor::FollowSystem) {
            swatches.push([0x20, 0x20, 0x20]);
            continue;
        }
        let colors = appearance::resolve_accent(accent_id(accent));
        swatches.push(rgb_parts(colors.dark));
    }
    swatches
}

// ---------------------------------------------------------------------------
// SFTP defaults (D33)
// ---------------------------------------------------------------------------

/// A well-known editor detected on `PATH` at Settings-open time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetectedEditor {
    /// Stable id persisted in `[sftp] external_editor`.
    pub(crate) id: &'static str,
    /// Executable file names tried in every `PATH` entry (first hit wins).
    pub(crate) executables: &'static [&'static str],
}

/// Editors probed for Settings → SFTP → External editor.
///
/// Only the editors that are actually installed appear in the dropdown; unlisted
/// editors stay reachable through `Custom` + Browse.
pub(crate) const KNOWN_EXTERNAL_EDITORS: [DetectedEditor; 4] = [
    DetectedEditor {
        id: "vscode",
        executables: &["code.cmd", "code"],
    },
    DetectedEditor {
        id: "zed",
        executables: &["zed.exe", "zed"],
    },
    DetectedEditor {
        id: "notepad++",
        executables: &["notepad++.exe"],
    },
    DetectedEditor {
        id: "sublime",
        executables: &["sublime_text.exe", "sublime_text"],
    },
];

/// Which external editor the Settings → SFTP dropdown selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorChoice {
    /// Open remote edits with the OS default application (config `None`).
    SystemDefault,
    /// A detected editor id from [`KNOWN_EXTERNAL_EDITORS`].
    Detected(String),
    /// A user-picked executable path (config stores the path itself).
    Custom,
}

impl EditorChoice {
    /// Maps the persisted `[sftp] external_editor` value onto a choice.
    pub(crate) fn from_config(value: Option<&str>, detected: &[DetectedEditor]) -> Self {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Self::SystemDefault;
        };
        if detected
            .iter()
            .any(|editor| editor.id.eq_ignore_ascii_case(value))
        {
            return Self::Detected(value.to_owned());
        }
        Self::Custom
    }
}

/// Finds the known editors whose executable exists in one of `search_dirs`.
///
/// Pure on purpose: the production caller passes `PATH` entries, tests pass a
/// temp directory.
pub(crate) fn external_editor_candidates(search_dirs: &[PathBuf]) -> Vec<DetectedEditor> {
    KNOWN_EXTERNAL_EDITORS
        .iter()
        .copied()
        .filter(|editor| {
            editor.executables.iter().any(|executable| {
                search_dirs
                    .iter()
                    .any(|dir| !dir.as_os_str().is_empty() && dir.join(executable).is_file())
            })
        })
        .collect()
}

/// `PATH` entries as directories (empty entries are skipped).
fn executable_search_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|dir| !dir.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Detects installed editors on the current `PATH`.
pub(crate) fn detect_external_editors() -> Vec<DetectedEditor> {
    external_editor_candidates(&executable_search_dirs())
}

/// Editable state of Settings → SFTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsSftpDraft {
    pub(crate) show_hidden: bool,
    pub(crate) conflict_action: SftpConflictAction,
    pub(crate) editor: EditorChoice,
    /// Custom editor path (only used by [`EditorChoice::Custom`]).
    pub(crate) editor_path_text: String,
    /// Editors detected on `PATH`; refreshed whenever Settings opens.
    pub(crate) detected_editors: Vec<DetectedEditor>,
}

impl Default for SettingsSftpDraft {
    fn default() -> Self {
        Self::from_profile(&yshell_config::SftpProfile::default(), Vec::new())
    }
}

impl SettingsSftpDraft {
    /// Loads the draft from persisted `[sftp]` defaults.
    pub(crate) fn from_profile(
        profile: &yshell_config::SftpProfile,
        detected_editors: Vec<DetectedEditor>,
    ) -> Self {
        let editor =
            EditorChoice::from_config(profile.external_editor.as_deref(), &detected_editors);
        let editor_path_text = match (&editor, profile.external_editor.as_deref()) {
            (EditorChoice::Custom, Some(value)) => value.to_owned(),
            _ => String::new(),
        };
        Self {
            show_hidden: profile.show_hidden,
            conflict_action: profile.conflict_action,
            editor,
            editor_path_text,
            detected_editors,
        }
    }

    /// Conflict dropdown index (Ask 0 / Replace 1 / Skip 2 / Stop 3).
    pub(crate) fn conflict_index(&self) -> i32 {
        match self.conflict_action {
            SftpConflictAction::Ask => 0,
            SftpConflictAction::Replace => 1,
            SftpConflictAction::Skip => 2,
            SftpConflictAction::Stop => 3,
        }
    }

    /// Applies a conflict dropdown selection; unknown indexes are ignored.
    pub(crate) fn set_conflict_index(&mut self, index: i32) -> bool {
        let Some(action) = conflict_action_from_index(index) else {
            return false;
        };
        self.conflict_action = action;
        true
    }

    /// External editor dropdown model: stable id for the two fixed choices
    /// (`system-default` / `custom`) plus the detected editors' display names.
    ///
    /// The ids are rendered by `TextFormats.settings-sftp-editor-label` in
    /// `ui/pages/settings.slint`; detected editor names are proper nouns and are
    /// shown as-is.
    pub(crate) fn editor_options(&self) -> Vec<String> {
        let mut options = Vec::with_capacity(self.detected_editors.len() + 2);
        options.push("system-default".to_owned());
        for editor in &self.detected_editors {
            options.push(editor_label(editor.id).to_owned());
        }
        options.push("custom".to_owned());
        options
    }

    /// Dropdown index for the current choice.
    pub(crate) fn editor_index(&self) -> i32 {
        match &self.editor {
            EditorChoice::SystemDefault => 0,
            EditorChoice::Detected(id) => self
                .detected_editors
                .iter()
                .position(|editor| editor.id.eq_ignore_ascii_case(id))
                .map_or(0, |index| index as i32 + 1),
            EditorChoice::Custom => self.editor_options().len() as i32 - 1,
        }
    }

    /// Applies a dropdown selection; unknown indexes are ignored.
    pub(crate) fn set_editor_index(&mut self, index: i32) -> bool {
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        if index == 0 {
            self.editor = EditorChoice::SystemDefault;
            return true;
        }
        if index == self.detected_editors.len() + 1 {
            self.editor = EditorChoice::Custom;
            return true;
        }
        match index
            .checked_sub(1)
            .and_then(|index| self.detected_editors.get(index))
        {
            Some(editor) => {
                self.editor = EditorChoice::Detected(editor.id.to_owned());
                true
            }
            None => false,
        }
    }

    /// `true` when the Custom path row (input + Browse) must be shown.
    pub(crate) fn custom_path_visible(&self) -> bool {
        matches!(self.editor, EditorChoice::Custom)
    }

    /// Validates the draft and returns the `[sftp] external_editor` value.
    ///
    /// `Err(())` means the Custom variant is selected but the path is empty.
    pub(crate) fn external_editor_config_value(&self) -> Result<Option<String>, ()> {
        match &self.editor {
            EditorChoice::SystemDefault => Ok(None),
            EditorChoice::Detected(id) => Ok(Some(id.clone())),
            EditorChoice::Custom => {
                let path = self.editor_path_text.trim();
                if path.is_empty() {
                    Err(())
                } else {
                    Ok(Some(path.to_owned()))
                }
            }
        }
    }
}

/// Conflict dropdown index → [`SftpConflictAction`].
pub(crate) fn conflict_action_from_index(index: i32) -> Option<SftpConflictAction> {
    match index {
        0 => Some(SftpConflictAction::Ask),
        1 => Some(SftpConflictAction::Replace),
        2 => Some(SftpConflictAction::Skip),
        3 => Some(SftpConflictAction::Stop),
        _ => None,
    }
}

/// Display label for a detected editor id.
fn editor_label(id: &str) -> &'static str {
    match id {
        "vscode" => "Visual Studio Code",
        "zed" => "Zed",
        "notepad++" => "Notepad++",
        "sublime" => "Sublime Text",
        _ => "",
    }
}

/// Inline status of the Settings → SFTP section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsSftpStatus {
    Hint,
    /// Custom is selected but no executable path was picked yet.
    CustomEditorPathRequired,
    Saved,
}

impl SettingsSftpStatus {
    pub(crate) const fn kind_id(&self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::CustomEditorPathRequired => "custom-editor-path-required",
            Self::Saved => "saved",
        }
    }

    /// English fallback (Slint renders `TextFormats.settings-sftp-status`).
    pub(crate) fn display_text(&self) -> String {
        match self {
            Self::Hint => "Defaults apply to new SFTP tabs.".to_owned(),
            Self::CustomEditorPathRequired => {
                "Choose the editor executable before saving.".to_owned()
            }
            Self::Saved => "Saved the SFTP defaults.".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal interaction index helpers (D37)
// ---------------------------------------------------------------------------

/// Middle-click dropdown index (Paste 0 / Do nothing 1).
pub(crate) fn middle_click_index(action: MiddleClickAction) -> i32 {
    match action {
        MiddleClickAction::Paste => 0,
        MiddleClickAction::DoNothing => 1,
    }
}

/// Right-click dropdown index (Context menu 0 / Paste 1 / Do nothing 2).
pub(crate) fn right_click_index(action: RightClickAction) -> i32 {
    match action {
        RightClickAction::ContextMenu => 0,
        RightClickAction::Paste => 1,
        RightClickAction::DoNothing => 2,
    }
}

/// Multi-line paste dropdown index (Ask 0 / Paste 1 / Block 2).
pub(crate) fn multi_line_paste_index(policy: MultiLinePastePolicy) -> i32 {
    match policy {
        MultiLinePastePolicy::Ask => 0,
        MultiLinePastePolicy::Paste => 1,
        MultiLinePastePolicy::Block => 2,
    }
}

/// Terminal style (global color-scheme profile) names for the Appearance
/// dropdown; the row is a `Style` picker, font/color editing stays in the
/// Terminal Styles manager (D36).
pub(crate) fn terminal_style_options(catalog: &yshell_config::ThemeCatalog) -> Vec<String> {
    theme::color_scheme_option_names(catalog)
}

/// Index of the global terminal style inside [`terminal_style_options`].
pub(crate) fn terminal_style_index(
    draft: &super::theme::SettingsAppearanceDraft,
    catalog: &yshell_config::ThemeCatalog,
) -> i32 {
    theme::color_scheme_index(catalog, &draft.color_scheme)
}

// ---------------------------------------------------------------------------
// AppRuntime: draft lifecycle + whole-dialog Save
// ---------------------------------------------------------------------------

impl AppRuntime {
    /// Reloads the Appearance / SFTP drafts from the persisted config.
    ///
    /// Called when the dialog opens (shows the persisted values) and as part of
    /// the full draft reload on cancel.
    pub(crate) fn reload_settings_theme_drafts(&mut self) {
        self.settings_app_theme = AppThemeDraft::from_ui(&self.config_document.ui);
        self.settings_app_theme_baseline = self.settings_app_theme;
        self.settings_sftp_draft =
            SettingsSftpDraft::from_profile(&self.config_document.sftp, detect_external_editors());
        self.settings_sftp_status = SettingsSftpStatus::Hint;
    }

    /// Reloads every Settings draft from the persisted config.
    ///
    /// Called when the dialog is cancelled: `Cancel` / `×` / `Esc` simply throw
    /// the drafts away by reloading the baseline, which is what reverts the App
    /// theme instant preview (and the terminal / SFTP edits).
    pub(crate) fn reload_settings_drafts(&mut self) {
        self.reload_settings_theme_drafts();
        self.settings_terminal_status = SettingsTerminalStatus::Hint;
        self.settings_scrollback_lines_text =
            self.config_document.terminal.scrollback_lines.to_string();
        self.settings_scrollback_max_cells_text = self
            .config_document
            .terminal
            .scrollback_max_cells
            .to_string();
        self.settings_terminal_middle_click = self.config_document.ui.terminal_middle_click;
        self.settings_terminal_right_click = self.config_document.ui.terminal_right_click;
        self.settings_terminal_multi_line_paste = self.config_document.ui.terminal_multi_line_paste;
        self.settings_terminal_auto_copy = self.config_document.ui.terminal_auto_copy;
    }

    /// Reverts the App theme instant preview to the persisted baseline.
    pub(crate) fn revert_settings_app_theme(&mut self) {
        self.settings_app_theme = self.settings_app_theme_baseline;
    }

    /// Settings → Appearance → App theme: Mode segmented control.
    pub fn update_settings_appearance_mode(&mut self, index: i32) -> AppProjection {
        if self.settings_app_theme.set_mode_index(index) {
            // Keep the combined Appearance draft in sync: `save_settings_all`
            // persists the section from this draft (single source of truth).
            self.settings_appearance.app_mode = self.settings_app_theme.mode;
            self.settings_appearance_status = SettingsAppearanceStatus::Hint;
            self.mark_ui_dirty();
        }
        self.projection()
    }

    /// Settings → Appearance → App theme: Accent swatch row.
    pub fn update_settings_appearance_accent(&mut self, index: i32) -> AppProjection {
        if self.settings_app_theme.set_accent_index(index) {
            self.settings_appearance.app_accent = self.settings_app_theme.accent;
            self.settings_appearance_status = SettingsAppearanceStatus::Hint;
            self.mark_ui_dirty();
        }
        self.projection()
    }

    /// Settings → Appearance → Terminal style dropdown (global profile).
    pub fn select_settings_appearance_style(&mut self, index: i32) -> AppProjection {
        if let Some(id) = theme::color_scheme_id_at(&self.theme_catalog, index) {
            self.settings_appearance.color_scheme = id;
            self.settings_appearance_status = SettingsAppearanceStatus::Hint;
            self.mark_ui_dirty();
        }
        self.projection()
    }

    /// Settings → Appearance → Manage terminal styles (Terminal Styles manager).
    pub fn open_settings_terminal_styles_manager(&mut self) -> AppProjection {
        self.open_theme_manager()
    }

    /// Settings → SFTP: Show hidden (default for new SFTP tabs).
    pub fn update_settings_sftp_show_hidden(&mut self, show_hidden: bool) -> AppProjection {
        self.settings_sftp_draft.show_hidden = show_hidden;
        self.settings_sftp_status = SettingsSftpStatus::Hint;
        self.projection()
    }

    /// Settings → SFTP: Conflict default action dropdown.
    pub fn update_settings_sftp_conflict_action(&mut self, index: i32) -> AppProjection {
        if self.settings_sftp_draft.set_conflict_index(index) {
            self.settings_sftp_status = SettingsSftpStatus::Hint;
        }
        self.projection()
    }

    /// Settings → SFTP: External editor dropdown.
    pub fn update_settings_sftp_external_editor(&mut self, index: i32) -> AppProjection {
        if self.settings_sftp_draft.set_editor_index(index) {
            self.settings_sftp_status = SettingsSftpStatus::Hint;
        }
        self.projection()
    }

    /// Settings → SFTP: Custom editor path (free text; Browse writes the picker
    /// result through the same method).
    pub fn update_settings_sftp_external_editor_path(&mut self, value: &str) -> AppProjection {
        self.settings_sftp_draft.editor_path_text = value.to_owned();
        self.settings_sftp_status = SettingsSftpStatus::Hint;
        self.projection()
    }

    /// Validates and persists every editable Settings section in one config
    /// write, then closes the dialog (`Save` = save and close, D36/§14.1).
    ///
    /// Any validation failure keeps the dialog open and leaves the persisted
    /// config untouched; the offending section projects an inline status.
    pub fn save_settings_all(&mut self) -> AppResult<AppProjection> {
        // 1. Appearance: terminal style + font fields (validated by theme.rs).
        let appearance_theme = match self.settings_appearance.theme_fields(&self.theme_catalog) {
            Ok(theme) => theme,
            Err(error) => {
                self.settings_appearance_status = SettingsAppearanceStatus::Rejected(error);
                // Keep the dialog open and push the inline status to the UI.
                self.mark_ui_dirty();
                return Ok(self.projection());
            }
        };
        // 2. Terminal: scrollback limits (ranges from the contract §14.5).
        let scrollback_lines = match parse_settings_usize(
            SettingsTerminalField::ScrollbackLines,
            &self.settings_scrollback_lines_text,
            SETTINGS_SCROLLBACK_LINES_MIN,
            SETTINGS_SCROLLBACK_LINES_MAX,
        ) {
            Ok(lines) => lines,
            Err(status) => {
                self.settings_terminal_status = status;
                self.mark_ui_dirty();
                return Ok(self.projection());
            }
        };
        let scrollback_max_cells = match parse_settings_usize(
            SettingsTerminalField::ScrollbackMaxCells,
            &self.settings_scrollback_max_cells_text,
            SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
            SETTINGS_SCROLLBACK_MAX_CELLS_MAX,
        ) {
            Ok(max_cells) => max_cells,
            Err(status) => {
                self.settings_terminal_status = status;
                self.mark_ui_dirty();
                return Ok(self.projection());
            }
        };
        // 3. SFTP: external editor (Custom requires a path).
        let external_editor = match self.settings_sftp_draft.external_editor_config_value() {
            Ok(value) => value,
            Err(()) => {
                self.settings_sftp_status = SettingsSftpStatus::CustomEditorPathRequired;
                self.mark_ui_dirty();
                return Ok(self.projection());
            }
        };

        // App theme (D36): persisted, not preview-only. The combined Appearance
        // draft keeps `app_mode` / `app_accent` in sync with the preview draft.
        self.config_document.ui.appearance_mode = self.settings_appearance.app_mode;
        self.config_document.ui.appearance_accent = self.settings_appearance.app_accent;
        // Terminal interactions (D37).
        self.config_document.ui.terminal_middle_click = self.settings_terminal_middle_click;
        self.config_document.ui.terminal_right_click = self.settings_terminal_right_click;
        self.config_document.ui.terminal_multi_line_paste = self.settings_terminal_multi_line_paste;
        self.config_document.ui.terminal_auto_copy = self.settings_terminal_auto_copy;
        // Terminal profile: scrollback + global style / font (D36 §14.6).
        {
            let terminal = &mut self.config_document.terminal;
            terminal.scrollback_lines = scrollback_lines;
            terminal.scrollback_max_cells = scrollback_max_cells;
            terminal.color_scheme = Some(appearance_theme.color_scheme.clone());
            clear_color_overrides(terminal);
            terminal.font_family = appearance_theme.font_family.clone();
            terminal.font_size = appearance_theme.font_size;
            terminal.backup_font = appearance_theme.backup_font.clone();
            terminal.legacy_fallback_fonts = None;
        }
        // SFTP defaults (D33).
        self.config_document.sftp.show_hidden = self.settings_sftp_draft.show_hidden;
        self.config_document.sftp.conflict_action = self.settings_sftp_draft.conflict_action;
        self.config_document.sftp.external_editor = external_editor;

        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;

        // Apply the saved limits to open sessions (new terminals already read
        // the persisted profile).
        let terminal_profile = self.config_document.terminal.clone();
        for runtime in self.sessions.values_mut() {
            runtime.configure_terminal_limits(&terminal_profile);
        }

        self.settings_scrollback_lines_text = scrollback_lines.to_string();
        self.settings_scrollback_max_cells_text = scrollback_max_cells.to_string();
        self.settings_terminal_status = SettingsTerminalStatus::Saved {
            lines: scrollback_lines,
            max_cells: scrollback_max_cells,
        };
        self.settings_app_theme_baseline = self.settings_app_theme;
        self.settings_sftp_draft = SettingsSftpDraft::from_profile(
            &self.config_document.sftp,
            std::mem::take(&mut self.settings_sftp_draft.detected_editors),
        );
        self.settings_sftp_status = SettingsSftpStatus::Saved;
        self.settings_appearance_status = SettingsAppearanceStatus::Saved;

        self.set_status_kind(
            "settings-saved",
            format!(
                "Saved settings (app theme={}, accent={}).",
                self.settings_app_theme.mode_id(),
                self.settings_app_theme.accent_id()
            ),
            self.settings_app_theme.mode_id().to_owned(),
            self.settings_app_theme.accent_id().to_owned(),
        );
        // Save = save and close.
        self.settings_appearance_visible = false;
        self.theme_editor = None;
        Ok(self.projection())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir_runtime() -> (tempfile::TempDir, AppRuntime) {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        (temp, runtime)
    }

    #[test]
    fn app_theme_draft_defaults_and_index_mapping() {
        let draft = AppThemeDraft::default();
        assert_eq!(draft.mode, AppearanceMode::System);
        assert_eq!(draft.accent, AccentColor::FollowSystem);
        assert_eq!(draft.mode_index(), 0);
        assert_eq!(draft.accent_index(), 0);
        assert_eq!(draft.mode_id(), "system");
        assert_eq!(draft.accent_id(), "blue");

        let mut draft = draft;
        assert!(draft.set_mode_index(2));
        assert!(draft.set_accent_index(2));
        assert_eq!(draft.mode, AppearanceMode::Dark);
        assert_eq!(draft.accent, AccentColor::Teal);
        assert_eq!(draft.mode_index(), 2);
        assert_eq!(draft.accent_index(), 2);
        assert_eq!(draft.accent_id(), "teal");

        // Out-of-range indexes are ignored (dropdown contract).
        assert!(!draft.set_mode_index(3));
        assert!(!draft.set_mode_index(-1));
        assert!(!draft.set_accent_index(7));
        assert_eq!(draft.mode, AppearanceMode::Dark);
        assert_eq!(draft.accent, AccentColor::Teal);
    }

    #[test]
    fn app_theme_maps_to_slint_mode_and_resolved_accent() {
        assert_eq!(AppThemeDraft::default().slint_mode_index(), 0);
        let mut draft = AppThemeDraft::default();
        draft.set_mode_index(1);
        assert_eq!(draft.theme_mode(), ThemeMode::Light);
        assert_eq!(draft.slint_mode_index(), 1);
        draft.set_mode_index(2);
        assert_eq!(draft.theme_mode(), ThemeMode::Dark);
        assert_eq!(draft.slint_mode_index(), 2);

        draft.set_accent_index(6);
        assert_eq!(draft.accent, AccentColor::Magenta);
        assert_eq!(draft.accent_colors(), appearance::resolve_accent("magenta"));

        // Follow system resolves to the Windows Blue preset fallback.
        let follow = AppThemeDraft::default();
        assert_eq!(follow.accent_colors(), appearance::resolve_accent("blue"));

        // The swatch rows are 1:1 with APP_ACCENT_IDS in both themes.
        // Index 0 is Follow system (placeholder; UI paints a split gradient).
        assert_eq!(accent_swatches_light().len(), APP_ACCENT_IDS.len());
        assert_eq!(accent_swatches_dark().len(), APP_ACCENT_IDS.len());
        assert_eq!(accent_swatches_light()[0], [0xff, 0xff, 0xff]);
        assert_eq!(accent_swatches_dark()[0], [0x20, 0x20, 0x20]);
        assert_ne!(
            accent_swatches_light()[0],
            rgb_parts(appearance::resolve_accent("blue").light)
        );
        assert_eq!(
            accent_swatches_light()[1],
            rgb_parts(appearance::resolve_accent("blue").light)
        );
        assert_eq!(
            accent_swatches_dark()[1],
            rgb_parts(appearance::resolve_accent("blue").dark)
        );
    }

    #[test]
    fn external_editor_detection_requires_an_executable_on_disk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().to_path_buf();

        assert!(external_editor_candidates(std::slice::from_ref(&dir)).is_empty());

        // Only known editor executables count; unrelated files are ignored.
        std::fs::write(dir.join("code.cmd"), b"@echo off").expect("write code");
        std::fs::write(dir.join("notepad++.exe"), b"").expect("write npp");
        std::fs::write(dir.join("README.txt"), b"").expect("write readme");
        let detected = external_editor_candidates(&[dir.join("missing"), dir]);
        let ids: Vec<&str> = detected.iter().map(|editor| editor.id).collect();
        assert_eq!(ids, vec!["vscode", "notepad++"]);

        // Empty search dirs are skipped without panicking.
        assert!(external_editor_candidates(&[]).is_empty());
        assert!(external_editor_candidates(&[PathBuf::new()]).is_empty());
    }

    #[test]
    fn sftp_editor_selection_maps_to_config_values() {
        let detected = vec![KNOWN_EXTERNAL_EDITORS[0], KNOWN_EXTERNAL_EDITORS[1]];
        let profile = yshell_config::SftpProfile::default();
        let mut draft = SettingsSftpDraft::from_profile(&profile, detected);
        // Defaults: show hidden on, conflict Ask, system default editor.
        assert!(draft.show_hidden);
        assert_eq!(draft.conflict_index(), 0);
        assert_eq!(draft.editor, EditorChoice::SystemDefault);
        assert_eq!(
            draft.editor_options(),
            vec![
                "system-default".to_owned(),
                "Visual Studio Code".to_owned(),
                "Zed".to_owned(),
                "custom".to_owned(),
            ]
        );
        assert_eq!(draft.external_editor_config_value(), Ok(None));

        // Detected editor → persisted id.
        assert!(draft.set_editor_index(2));
        assert_eq!(draft.editor, EditorChoice::Detected("zed".to_owned()));
        assert_eq!(
            draft.external_editor_config_value(),
            Ok(Some("zed".to_owned()))
        );

        // Custom without a path fails validation; a picked path is stored as-is.
        assert!(draft.set_editor_index(3));
        assert!(draft.custom_path_visible());
        assert_eq!(draft.external_editor_config_value(), Err(()));
        draft.editor_path_text = "  C:\\Tools\\editor.exe  ".to_owned();
        assert_eq!(
            draft.external_editor_config_value(),
            Ok(Some("C:\\Tools\\editor.exe".to_owned()))
        );

        // Loading a persisted custom path selects Custom with the path filled.
        let profile = yshell_config::SftpProfile {
            external_editor: Some("C:\\Tools\\editor.exe".to_owned()),
            ..Default::default()
        };
        let reloaded = SettingsSftpDraft::from_profile(&profile, Vec::new());
        assert_eq!(reloaded.editor, EditorChoice::Custom);
        assert_eq!(reloaded.editor_path_text, "C:\\Tools\\editor.exe");

        // Conflict mapping covers all four actions.
        for (index, expected) in [
            (0, SftpConflictAction::Ask),
            (1, SftpConflictAction::Replace),
            (2, SftpConflictAction::Skip),
            (3, SftpConflictAction::Stop),
        ] {
            assert!(draft.set_conflict_index(index));
            assert_eq!(draft.conflict_action, expected);
        }
        assert!(!draft.set_conflict_index(4));
    }

    #[test]
    fn terminal_interaction_indexes_match_dropdown_order() {
        assert_eq!(middle_click_index(MiddleClickAction::Paste), 0);
        assert_eq!(middle_click_index(MiddleClickAction::DoNothing), 1);
        assert_eq!(right_click_index(RightClickAction::ContextMenu), 0);
        assert_eq!(right_click_index(RightClickAction::Paste), 1);
        assert_eq!(right_click_index(RightClickAction::DoNothing), 2);
        assert_eq!(multi_line_paste_index(MultiLinePastePolicy::Ask), 0);
        assert_eq!(multi_line_paste_index(MultiLinePastePolicy::Paste), 1);
        assert_eq!(multi_line_paste_index(MultiLinePastePolicy::Block), 2);
    }

    #[test]
    fn whole_dialog_save_persists_app_theme_sftp_and_terminal_drafts() {
        let (temp, mut runtime) = tempdir_runtime();
        runtime.open_settings_appearance();
        assert!(runtime.projection().settings_appearance_visible);

        runtime.update_settings_appearance_mode(2);
        runtime.update_settings_appearance_accent(2);
        runtime.update_settings_sftp_show_hidden(false);
        runtime.update_settings_sftp_conflict_action(1);
        runtime.update_settings_terminal_auto_copy(true);
        runtime.update_settings_scrollback_lines("25000");

        let saved = runtime.save_settings_all().expect("save settings");
        assert!(!saved.settings_appearance_visible);
        assert_eq!(saved.status_kind, "settings-saved");

        let document = ConfigStore::new(temp.path())
            .load_or_recover()
            .expect("reload config")
            .document;
        assert_eq!(document.ui.appearance_mode, AppearanceMode::Dark);
        assert_eq!(document.ui.appearance_accent, AccentColor::Teal);
        assert!(!document.sftp.show_hidden);
        assert_eq!(document.sftp.conflict_action, SftpConflictAction::Replace);
        assert!(document.ui.terminal_auto_copy);
        assert_eq!(document.terminal.scrollback_lines, 25_000);

        // Reopening reloads the persisted values into the drafts.
        let reopened = runtime.open_settings_appearance();
        assert_eq!(reopened.settings_appearance_mode_index, 2);
        assert_eq!(reopened.settings_appearance_accent_index, 2);
        assert!(!reopened.settings_sftp_show_hidden);
        assert_eq!(reopened.settings_sftp_conflict_index, 1);
    }

    #[test]
    fn cancel_reverts_the_app_theme_preview_to_the_baseline() {
        let (_temp, mut runtime) = tempdir_runtime();
        runtime.open_settings_appearance();
        runtime.update_settings_appearance_mode(2);
        runtime.update_settings_appearance_accent(4);

        // × / Esc / Cancel all end in `close_settings_appearance`.
        runtime.close_settings_appearance();
        let projection = runtime.projection();
        assert!(!projection.settings_appearance_visible);
        assert_eq!(projection.settings_appearance_mode_index, 0);
        assert_eq!(projection.settings_appearance_accent_index, 0);
        assert_eq!(
            runtime.settings_app_theme,
            runtime.settings_app_theme_baseline
        );

        // The persisted config was never touched.
        let config = runtime.config_document.ui.clone();
        assert_eq!(config.appearance_mode, AppearanceMode::System);
        assert_eq!(config.appearance_accent, AccentColor::FollowSystem);
    }

    #[test]
    fn invalid_terminal_values_block_the_whole_dialog_save() {
        let (temp, mut runtime) = tempdir_runtime();
        runtime.open_settings_appearance();
        runtime.update_settings_appearance_mode(2);
        runtime.update_settings_scrollback_lines("99");

        let blocked = runtime.save_settings_all().expect("save attempt");
        assert!(blocked.settings_appearance_visible);
        assert_eq!(runtime.settings_terminal_status.kind_id(), "out-of-range");
        // Nothing was persisted, including the valid App theme change.
        let document = ConfigStore::new(temp.path())
            .load_or_recover()
            .expect("reload config")
            .document;
        assert_eq!(document.ui.appearance_mode, AppearanceMode::System);
    }

    #[test]
    fn custom_sftp_editor_without_a_path_blocks_saving() {
        let (_temp, mut runtime) = tempdir_runtime();
        runtime.open_settings_appearance();
        runtime.update_settings_sftp_external_editor(1_000); // no such row → ignored
        assert_eq!(
            runtime.settings_sftp_draft.editor,
            EditorChoice::SystemDefault
        );

        runtime.settings_sftp_draft.detected_editors.clear();
        runtime.update_settings_sftp_external_editor(1); // Custom row
        assert!(runtime.settings_sftp_draft.custom_path_visible());
        let blocked = runtime.save_settings_all().expect("save attempt");
        assert!(blocked.settings_appearance_visible);
        assert_eq!(
            runtime.settings_sftp_status,
            SettingsSftpStatus::CustomEditorPathRequired
        );
        assert_eq!(
            blocked.settings_sftp_status_kind_text,
            "custom-editor-path-required"
        );
        assert_eq!(
            blocked.settings_sftp_status_text,
            "Choose the editor executable before saving."
        );
    }
}
