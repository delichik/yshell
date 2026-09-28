//! N5 Folder Editor: folder-level terminal/logging defaults with per-field
//! three-state editing (inherit / explicit / reset), backed by
//! `FolderProfile::terminal` and `FolderProfile::logging`.
//!
//! Group semantics for the *concrete* C0 fields (`scrollback_*`, logging
//! `enabled`/`format`): they are `explicit` when the folder stores a terminal /
//! logging override at all. Saving always seeds the stored profile from the
//! parent-resolved values first, so adding a theme override at a folder never
//! changes the effective scrollback/logging of its subtree.

use yshell_config::{FolderProfile, LoggingProfile, SessionProfile, TerminalProfile};

use super::panels::{
    SETTINGS_SCROLLBACK_LINES_MAX, SETTINGS_SCROLLBACK_LINES_MIN,
    SETTINGS_SCROLLBACK_MAX_CELLS_MAX, SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
};
use super::theme::{
    color_scheme_id_at, parse_scrollback, resolve_palette, theme_field_rows, ThemeDraft,
    ThemeFieldAction, ThemeFieldDraft, ThemeSources, TriStateFieldData, KEY_LOGGING_DIRECTORY,
    KEY_LOGGING_ENABLED, KEY_LOGGING_FORMAT, KEY_SCROLLBACK_LINES, KEY_SCROLLBACK_MAX_CELLS,
    LOG_FORMAT_RAW,
};
use super::*;

/// Editable state of the Folder Editor dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderEditorState {
    pub(crate) visible: bool,
    pub(crate) folder_id: String,
    pub(crate) folder_name: String,
    pub(crate) folder_path: String,
    /// Parent-resolved terminal profile (seeds inherited fields on save).
    pub(crate) parent_terminal: TerminalProfile,
    /// Parent-resolved logging profile.
    pub(crate) parent_logging: LoggingProfile,
    /// Inherit sources for the "From …" hints.
    pub(crate) sources: ThemeSources,
    pub(crate) theme: ThemeDraft,
    pub(crate) scrollback_lines: ThemeFieldDraft,
    pub(crate) scrollback_max_cells: ThemeFieldDraft,
    pub(crate) logging_enabled: ThemeFieldDraft,
    pub(crate) logging_format: ThemeFieldDraft,
    pub(crate) logging_directory: ThemeFieldDraft,
    pub(crate) status_text: String,
}

impl Default for FolderEditorState {
    fn default() -> Self {
        let parent_terminal = TerminalProfile::default();
        let parent_logging = LoggingProfile::default();
        Self {
            visible: false,
            folder_id: String::new(),
            folder_name: String::new(),
            folder_path: String::new(),
            theme: ThemeDraft::from_resolved(&parent_terminal, None),
            scrollback_lines: ThemeFieldDraft {
                text: parent_terminal.scrollback_lines.to_string(),
                explicit: false,
            },
            scrollback_max_cells: ThemeFieldDraft {
                text: parent_terminal.scrollback_max_cells.to_string(),
                explicit: false,
            },
            logging_enabled: ThemeFieldDraft {
                text: if parent_logging.enabled { "on" } else { "off" }.to_owned(),
                explicit: false,
            },
            logging_format: ThemeFieldDraft {
                text: parent_logging.format.clone(),
                explicit: false,
            },
            logging_directory: ThemeFieldDraft {
                text: parent_logging.directory.clone().unwrap_or_default(),
                explicit: false,
            },
            sources: ThemeSources::resolve(&[], &parent_terminal, &parent_logging),
            parent_terminal,
            parent_logging,
            status_text: String::new(),
        }
    }
}

impl AppRuntime {
    /// Opens the Folder Editor for `folder_id` with the current defaults.
    pub fn open_folder_editor(&mut self, folder_id: &str) -> AppResult<AppProjection> {
        let Some(folder) = self.config_document.find_folder(folder_id) else {
            return Err(AppError::new(format!(
                "saved-session folder `{folder_id}` was not found"
            )));
        };
        let folder_name = folder.name.clone();
        let folder_path = self.folder_path_label(folder_id);
        let own_terminal = folder.terminal.clone();
        let own_logging = folder.logging.clone();
        let parent_terminal = self.folder_editor_parent_terminal(folder_id);
        let parent_logging = self.folder_editor_parent_logging(folder_id);
        // Inherit hints come from the levels above the edited folder.
        let parent_chain: Vec<&FolderProfile> = self
            .config_document
            .folder_chain_to(folder_id)
            .into_iter()
            .skip(1)
            .collect();
        let sources = ThemeSources::resolve(
            &parent_chain,
            &self.config_document.terminal,
            &self.config_document.logging,
        );
        let terminal_explicit = own_terminal.is_some();
        let logging_explicit = own_logging.is_some();
        let current_terminal = own_terminal.as_ref().unwrap_or(&parent_terminal);
        let current_logging = own_logging.as_ref().unwrap_or(&parent_logging);

        self.folder_editor = FolderEditorState {
            visible: true,
            folder_id: folder_id.to_owned(),
            folder_name: folder_name.clone(),
            folder_path,
            theme: ThemeDraft::from_resolved(&parent_terminal, own_terminal.as_ref()),
            scrollback_lines: ThemeFieldDraft {
                text: current_terminal.scrollback_lines.to_string(),
                explicit: terminal_explicit,
            },
            scrollback_max_cells: ThemeFieldDraft {
                text: current_terminal.scrollback_max_cells.to_string(),
                explicit: terminal_explicit,
            },
            logging_enabled: ThemeFieldDraft {
                text: if current_logging.enabled { "on" } else { "off" }.to_owned(),
                explicit: logging_explicit,
            },
            logging_format: ThemeFieldDraft {
                text: current_logging.format.clone(),
                explicit: logging_explicit,
            },
            logging_directory: ThemeFieldDraft {
                text: current_logging.directory.clone().unwrap_or_default(),
                explicit: logging_explicit,
            },
            sources,
            parent_terminal,
            parent_logging,
            status_text: format!(
                "Editing defaults for `{folder_name}`. Changes apply to terminals opened afterwards."
            ),
        };
        self.status_text = format!("Opened folder defaults for `{folder_name}`.");
        Ok(self.projection())
    }

    pub fn close_folder_editor(&mut self) -> AppProjection {
        self.folder_editor.visible = false;
        self.projection()
    }

    /// Applies a three-state control action to one field.
    pub fn folder_editor_field_action(
        &mut self,
        key: &str,
        action: ThemeFieldAction,
    ) -> AppProjection {
        let parent_terminal = self.folder_editor.parent_terminal.clone();
        let parent_logging = self.folder_editor.parent_logging.clone();
        let explicit = match action {
            ThemeFieldAction::SetExplicit => true,
            ThemeFieldAction::SetInherit | ThemeFieldAction::Reset => false,
        };
        if let Some(field) = self.folder_editor.theme.field_mut(key) {
            field.explicit = explicit;
        }
        match key {
            KEY_SCROLLBACK_LINES => {
                let field = &mut self.folder_editor.scrollback_lines;
                field.explicit = explicit;
                if !explicit {
                    field.text = parent_terminal.scrollback_lines.to_string();
                }
            }
            KEY_SCROLLBACK_MAX_CELLS => {
                let field = &mut self.folder_editor.scrollback_max_cells;
                field.explicit = explicit;
                if !explicit {
                    field.text = parent_terminal.scrollback_max_cells.to_string();
                }
            }
            KEY_LOGGING_ENABLED => {
                let field = &mut self.folder_editor.logging_enabled;
                field.explicit = explicit;
                if !explicit {
                    field.text = if parent_logging.enabled { "on" } else { "off" }.to_owned();
                }
            }
            KEY_LOGGING_FORMAT => {
                let field = &mut self.folder_editor.logging_format;
                field.explicit = explicit;
                if !explicit {
                    field.text = parent_logging.format.clone();
                }
            }
            KEY_LOGGING_DIRECTORY => {
                let field = &mut self.folder_editor.logging_directory;
                field.explicit = explicit;
                if !explicit {
                    field.text = parent_logging.directory.clone().unwrap_or_default();
                }
            }
            _ => {}
        }
        self.folder_editor.status_text.clear();
        self.projection()
    }

    /// Updates the draft text of one field (typing in the editor). Editing a
    /// field implies an explicit override at this level.
    pub fn update_folder_editor_field(&mut self, key: &str, value: &str) -> AppProjection {
        if let Some(field) = self.folder_editor.theme.field_mut(key) {
            field.text = value.to_owned();
            field.explicit = true;
        } else {
            let field = match key {
                KEY_SCROLLBACK_LINES => Some(&mut self.folder_editor.scrollback_lines),
                KEY_SCROLLBACK_MAX_CELLS => Some(&mut self.folder_editor.scrollback_max_cells),
                KEY_LOGGING_ENABLED => Some(&mut self.folder_editor.logging_enabled),
                KEY_LOGGING_FORMAT => Some(&mut self.folder_editor.logging_format),
                KEY_LOGGING_DIRECTORY => Some(&mut self.folder_editor.logging_directory),
                _ => None,
            };
            if let Some(field) = field {
                field.text = value.to_owned();
                field.explicit = true;
            }
        }
        self.folder_editor.status_text.clear();
        self.projection()
    }

    /// Toggles the "session logging" field of the Folder Editor draft.
    pub fn toggle_folder_editor_logging(&mut self) -> AppProjection {
        let field = &mut self.folder_editor.logging_enabled;
        field.text = if field.text == "on" { "off" } else { "on" }.to_owned();
        field.explicit = true;
        self.folder_editor.status_text.clear();
        self.projection()
    }

    /// Picks a color scheme from the option list (index into `COLOR_SCHEMES`).
    pub fn select_folder_editor_scheme_index(&mut self, index: i32) -> AppProjection {
        if let Some(id) = color_scheme_id_at(index) {
            let field = &mut self.folder_editor.theme.color_scheme;
            field.text = id.to_owned();
            field.explicit = true;
            if let Some(scheme) = yshell_terminal::color_scheme(id) {
                let theme = &mut self.folder_editor.theme;
                theme.foreground.text = scheme.palette.foreground.to_hex();
                theme.background.text = scheme.palette.background.to_hex();
                theme.cursor.text = scheme.palette.cursor.to_hex();
                theme.selection.text = scheme.palette.selection.to_hex();
            }
        }
        self.folder_editor.status_text.clear();
        self.projection()
    }

    /// Picks the logging format from the option list (`raw` first).
    pub fn select_folder_editor_log_format_index(&mut self, index: i32) -> AppProjection {
        let field = &mut self.folder_editor.logging_format;
        field.text = if index == 0 {
            LOG_FORMAT_RAW
        } else {
            "sanitized"
        }
        .to_owned();
        field.explicit = true;
        self.folder_editor.status_text.clear();
        self.projection()
    }

    /// Drops every override of the folder (terminal + logging).
    pub fn reset_all_folder_editor(&mut self) -> AppProjection {
        let parent_terminal = self.folder_editor.parent_terminal.clone();
        let parent_logging = self.folder_editor.parent_logging.clone();
        self.folder_editor.theme = ThemeDraft::from_resolved(&parent_terminal, None);
        self.folder_editor.scrollback_lines = ThemeFieldDraft {
            text: parent_terminal.scrollback_lines.to_string(),
            explicit: false,
        };
        self.folder_editor.scrollback_max_cells = ThemeFieldDraft {
            text: parent_terminal.scrollback_max_cells.to_string(),
            explicit: false,
        };
        self.folder_editor.logging_enabled = ThemeFieldDraft {
            text: if parent_logging.enabled { "on" } else { "off" }.to_owned(),
            explicit: false,
        };
        self.folder_editor.logging_format = ThemeFieldDraft {
            text: parent_logging.format.clone(),
            explicit: false,
        };
        self.folder_editor.logging_directory = ThemeFieldDraft {
            text: parent_logging.directory.clone().unwrap_or_default(),
            explicit: false,
        };
        self.folder_editor.status_text =
            "Cleared every override for this folder; Save persists the reset.".to_owned();
        self.projection()
    }

    /// Validates and persists the folder defaults.
    pub fn save_folder_editor(&mut self) -> AppResult<AppProjection> {
        let folder_id = self.folder_editor.folder_id.clone();
        if self.config_document.find_folder(&folder_id).is_none() {
            return Err(AppError::new(format!(
                "saved-session folder `{folder_id}` was not found"
            )));
        }
        let parent_terminal = self.folder_editor.parent_terminal.clone();
        let parent_logging = self.folder_editor.parent_logging.clone();

        let mut terminal = parent_terminal.clone();
        if let Err(error) = self
            .folder_editor
            .theme
            .write_into(&mut terminal, &parent_terminal)
        {
            self.folder_editor.status_text = format!("{}: {}", error.field, error.message);
            return Ok(self.projection());
        }
        terminal.scrollback_lines = match parse_scrollback(
            &self.folder_editor.scrollback_lines,
            parent_terminal.scrollback_lines,
            SETTINGS_SCROLLBACK_LINES_MIN,
            SETTINGS_SCROLLBACK_LINES_MAX,
            KEY_SCROLLBACK_LINES,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.folder_editor.status_text = format!("{}: {}", error.field, error.message);
                return Ok(self.projection());
            }
        };
        terminal.scrollback_max_cells = match parse_scrollback(
            &self.folder_editor.scrollback_max_cells,
            parent_terminal.scrollback_max_cells,
            SETTINGS_SCROLLBACK_MAX_CELLS_MIN,
            SETTINGS_SCROLLBACK_MAX_CELLS_MAX,
            KEY_SCROLLBACK_MAX_CELLS,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.folder_editor.status_text = format!("{}: {}", error.field, error.message);
                return Ok(self.projection());
            }
        };
        let terminal_override = self.folder_editor.theme.has_explicit_field()
            || self.folder_editor.scrollback_lines.explicit
            || self.folder_editor.scrollback_max_cells.explicit;

        let mut logging = parent_logging.clone();
        logging.enabled = if self.folder_editor.logging_enabled.explicit {
            match self.folder_editor.logging_enabled.text.as_str() {
                "on" => true,
                "off" => false,
                other => {
                    self.folder_editor.status_text =
                        format!("{KEY_LOGGING_ENABLED}: `{other}` is not `on` or `off`");
                    return Ok(self.projection());
                }
            }
        } else {
            parent_logging.enabled
        };
        logging.format = if self.folder_editor.logging_format.explicit {
            let format = self.folder_editor.logging_format.text.trim();
            if format != LOG_FORMAT_RAW && format != "sanitized" {
                self.folder_editor.status_text =
                    format!("{KEY_LOGGING_FORMAT}: format must be `raw` or `sanitized`");
                return Ok(self.projection());
            }
            format.to_owned()
        } else {
            parent_logging.format.clone()
        };
        logging.directory = if self.folder_editor.logging_directory.explicit {
            let text = self.folder_editor.logging_directory.text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        } else {
            parent_logging.directory.clone()
        };
        let logging_override = self.folder_editor.logging_enabled.explicit
            || self.folder_editor.logging_format.explicit
            || self.folder_editor.logging_directory.explicit;

        let folder = self
            .config_document
            .find_folder_mut(&folder_id)
            .expect("folder was checked above");
        folder.terminal = terminal_override.then_some(terminal);
        folder.logging = logging_override.then_some(logging);
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;

        let folder_name = self.folder_editor.folder_name.clone();
        self.folder_editor.status_text = format!(
            "Saved folder defaults for `{folder_name}`. New terminals use them; open terminals keep their theme."
        );
        self.set_status_kind(
            "folder-defaults-saved",
            format!("Saved folder defaults for `{folder_name}`."),
            folder_name,
            String::new(),
        );
        Ok(self.projection())
    }

    /// Path label (`Root / Child`) of a saved-session folder.
    pub(crate) fn folder_path_label(&self, folder_id: &str) -> String {
        fn walk(
            folders: &[FolderProfile],
            id: &str,
            ancestors: &mut Vec<String>,
        ) -> Option<String> {
            for folder in folders {
                ancestors.push(folder.name.clone());
                if folder.id == id {
                    return Some(ancestors.join(" / "));
                }
                if let Some(found) = walk(&folder.folders, id, ancestors) {
                    return Some(found);
                }
                ancestors.pop();
            }
            None
        }
        walk(&self.config_document.folders, folder_id, &mut Vec::new()).unwrap_or_default()
    }

    /// Terminal profile resolved *without* the edited folder (parent chain).
    pub(crate) fn folder_editor_parent_terminal(&self, folder_id: &str) -> TerminalProfile {
        let chain = self.config_document.folder_chain_to(folder_id);
        let parent_id = chain.get(1).map(|folder| folder.id.clone());
        self.terminal_profile_for_draft(parent_id.as_deref())
    }

    /// Logging profile resolved without the edited folder.
    pub(crate) fn folder_editor_parent_logging(&self, folder_id: &str) -> LoggingProfile {
        let chain = self.config_document.folder_chain_to(folder_id);
        let parent_id = chain.get(1).map(|folder| folder.id.clone());
        self.resolved_logging_for_draft(parent_id.as_deref())
    }

    /// Resolves a terminal profile for a folder id via the config resolver.
    pub(crate) fn terminal_profile_for_draft(&self, folder_id: Option<&str>) -> TerminalProfile {
        let draft = SessionProfile::new("folder-editor-draft", "", "");
        self.config_document
            .resolve_session_draft(&draft, folder_id.unwrap_or_default())
            .terminal
    }

    /// Resolves a logging profile for a folder id via the config resolver.
    pub(crate) fn resolved_logging_for_draft(&self, folder_id: Option<&str>) -> LoggingProfile {
        let draft = SessionProfile::new("folder-editor-draft", "", "");
        self.config_document
            .resolve_session_draft(&draft, folder_id.unwrap_or_default())
            .logging
    }

    /// The eight Appearance rows of the Folder Editor.
    pub(crate) fn folder_editor_appearance_fields(&self) -> Vec<TriStateFieldData> {
        let state = &self.folder_editor;
        let resolved = self.folder_editor_resolved_terminal();
        let palette = resolve_palette(&resolved);
        let scheme_label = resolved
            .color_scheme
            .as_deref()
            .and_then(yshell_terminal::color_scheme)
            .map_or("YShell Default", |scheme| scheme.name)
            .to_owned();
        theme_field_rows(
            &state.theme,
            &state.sources,
            "local-folder",
            palette,
            scheme_label,
        )
    }

    /// Terminal profile the folder editor is currently showing (draft applied
    /// over the parent resolution).
    pub(crate) fn folder_editor_resolved_terminal(&self) -> TerminalProfile {
        let mut terminal = self.folder_editor.parent_terminal.clone();
        let _ = self
            .folder_editor
            .theme
            .write_into(&mut terminal, &self.folder_editor.parent_terminal);
        terminal
    }

    /// The scrollback rows of the Folder Editor (Terminal block).
    pub(crate) fn folder_editor_terminal_fields(&self) -> Vec<TriStateFieldData> {
        let state = &self.folder_editor;
        vec![
            TriStateFieldData::new(
                KEY_SCROLLBACK_LINES,
                "Scrollback lines",
                "Per session",
                "number",
                &state.scrollback_lines,
                &state.sources.scrollback_lines,
                "local-folder",
            ),
            TriStateFieldData::new(
                KEY_SCROLLBACK_MAX_CELLS,
                "Scrollback memory cap",
                "Cells kept per session",
                "number",
                &state.scrollback_max_cells,
                &state.sources.scrollback_max_cells,
                "local-folder",
            ),
        ]
    }

    /// The Logging rows of the Folder Editor.
    pub(crate) fn folder_editor_logging_fields(&self) -> Vec<TriStateFieldData> {
        let state = &self.folder_editor;
        vec![
            TriStateFieldData::new(
                KEY_LOGGING_ENABLED,
                "Session logging",
                "Write sessions to disk",
                "toggle",
                &state.logging_enabled,
                &state.sources.logging_enabled,
                "local-folder",
            ),
            TriStateFieldData::new(
                KEY_LOGGING_FORMAT,
                "Format",
                "Raw keeps escape sequences",
                "choice",
                &state.logging_format,
                &state.sources.logging_format,
                "local-folder",
            )
            .with_choice(
                vec![LOG_FORMAT_RAW.to_owned(), "sanitized".to_owned()],
                i32::from(state.logging_format.text != LOG_FORMAT_RAW),
            ),
            TriStateFieldData::new(
                KEY_LOGGING_DIRECTORY,
                "Directory",
                "Log output directory",
                "text",
                &state.logging_directory,
                &state.sources.logging_directory,
                "local-folder",
            ),
        ]
    }
}
