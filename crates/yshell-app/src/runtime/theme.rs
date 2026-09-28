//! N5 terminal theme: appearance resolution, the Settings appearance dialog,
//! the Folder Editor state/model and the tri-state field model shared with the
//! Session Editor.
//!
//! Resolution rules (design §1, C0): a session resolves *session > nearest
//! ancestor folder > global > built-in defaults* per terminal field. Sessions
//! capture their [`TerminalAppearance`] once at creation (D17) and keep it for
//! their whole lifetime; later theme edits only affect terminals opened
//! afterwards.

use std::path::PathBuf;

use yshell_config::{FolderProfile, LoggingProfile, TerminalProfile};
use yshell_terminal::{
    color_scheme, FontSelection, PrimaryFont, TerminalCell, TerminalColor, TerminalFrame,
    TerminalPalette, TerminalRenderer, TerminalSnapshot, COLOR_SCHEMES, DEFAULT_FONT_SIZE,
};

use super::*;

/// `font_family` value that selects the bundled DejaVu Sans Mono family.
pub(crate) const BUNDLED_FONT_FAMILY: &str = "DejaVu Sans Mono";
/// Built-in palette id (also the fallback when `color_scheme` is unknown).
pub(crate) const DEFAULT_COLOR_SCHEME_ID: &str = "yshell-default";
/// Font size bounds accepted by the settings/editor forms.
pub(crate) const THEME_FONT_SIZE_MIN: u16 = 6;
/// Font size bounds accepted by the settings/editor forms.
pub(crate) const THEME_FONT_SIZE_MAX: u16 = 72;
/// Log format ids stored in `LoggingProfile::format`.
pub(crate) const LOG_FORMAT_RAW: &str = "raw";

// ---------------------------------------------------------------------------
// Appearance resolution
// ---------------------------------------------------------------------------

/// Frozen appearance of one terminal session (D17).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TerminalAppearance {
    pub(crate) palette: TerminalPalette,
    pub(crate) font: FontSelection,
    pub(crate) font_size: f32,
    /// Font files that failed to load when the appearance was captured; the
    /// renderer falls back to the bundled faces for these.
    pub(crate) rejected_fonts: Vec<String>,
}

/// Resolves a terminal profile into the renderer inputs.
pub(crate) fn terminal_appearance(profile: &TerminalProfile) -> TerminalAppearance {
    let (font, rejected_fonts) = resolve_font(profile);
    TerminalAppearance {
        palette: resolve_palette(profile),
        font,
        font_size: resolve_font_size(profile.font_size),
        rejected_fonts,
    }
}

impl Default for TerminalAppearance {
    fn default() -> Self {
        terminal_appearance(&TerminalProfile::default())
    }
}

/// Palette resolved from a profile: named scheme (or the built-in default) plus
/// the inline color overrides. Invalid hex values are ignored here; the config
/// loader clears them (`ConfigWarning`), and the editors validate before saving.
pub(crate) fn resolve_palette(profile: &TerminalProfile) -> TerminalPalette {
    let mut palette = profile
        .color_scheme
        .as_deref()
        .and_then(color_scheme)
        .map_or(TerminalPalette::DEFAULT, |scheme| scheme.palette);
    if let Some(color) = profile
        .foreground
        .as_deref()
        .and_then(TerminalColor::from_hex)
    {
        palette.foreground = color;
    }
    if let Some(color) = profile
        .background
        .as_deref()
        .and_then(TerminalColor::from_hex)
    {
        palette.background = color;
    }
    if let Some(color) = profile.cursor.as_deref().and_then(TerminalColor::from_hex) {
        palette.cursor = color;
    }
    if let Some(color) = profile
        .selection
        .as_deref()
        .and_then(TerminalColor::from_hex)
    {
        palette.selection = color;
    }
    if let Some(ansi) = profile.ansi.as_ref() {
        let parsed: Vec<TerminalColor> = ansi
            .iter()
            .map(|value| TerminalColor::from_hex(value))
            .collect::<Option<_>>()
            .unwrap_or_default();
        if let Ok(colors) = <[TerminalColor; 16]>::try_from(parsed) {
            palette.ansi = colors;
        }
    }
    palette
}

/// Font selection plus the paths that could not be loaded.
pub(crate) fn resolve_font(profile: &TerminalProfile) -> (FontSelection, Vec<String>) {
    let primary = match profile.font_family.as_deref().map(str::trim) {
        None | Some("") => PrimaryFont::BundledDejaVu,
        Some(name) if name.eq_ignore_ascii_case(BUNDLED_FONT_FAMILY) => PrimaryFont::BundledDejaVu,
        Some(path) => PrimaryFont::File(PathBuf::from(path)),
    };
    let fallback_files = profile
        .fallback_fonts
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect();
    let font = FontSelection {
        primary,
        fallback_files,
    };
    let (_, errors) = yshell_terminal::TerminalFontSet::from_selection(&font);
    let rejected = errors
        .into_iter()
        .map(|error| error.face_name().to_owned())
        .collect();
    (font, rejected)
}

/// Logical font size in pixels; missing/zero falls back to the default.
pub(crate) fn resolve_font_size(size: Option<u16>) -> f32 {
    match size {
        Some(value) if value > 0 => {
            f32::from(value.clamp(THEME_FONT_SIZE_MIN, THEME_FONT_SIZE_MAX))
        }
        _ => DEFAULT_FONT_SIZE,
    }
}

/// Short hex list for a palette (scheme chips).
pub(crate) fn palette_swatches(palette: TerminalPalette) -> Vec<[u8; 3]> {
    let rgb = |color: TerminalColor| [color.red, color.green, color.blue];
    let mut swatches = vec![
        rgb(palette.foreground),
        rgb(palette.background),
        rgb(palette.cursor),
        rgb(palette.selection),
    ];
    swatches.extend(
        [1u8, 2, 4, 3]
            .into_iter()
            .map(|index| rgb(palette.ansi(index))),
    );
    swatches
}

/// Four role colors (used where horizontal space is tight, e.g. editor rows).
pub(crate) fn palette_role_swatches(palette: TerminalPalette) -> Vec<[u8; 3]> {
    let rgb = |color: TerminalColor| [color.red, color.green, color.blue];
    vec![
        rgb(palette.foreground),
        rgb(palette.background),
        rgb(palette.cursor),
        rgb(palette.selection),
    ]
}

/// Swatch row of the *effective* palette (settings picker preview).
pub(crate) fn palette_parts(palette: TerminalPalette) -> Vec<[u8; 3]> {
    palette_swatches(palette)
}

/// One selectable color scheme (settings scheme grid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeSchemeOption {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) swatches: Vec<[u8; 3]>,
}

/// The built-in schemes as projection data. Scheme chips carry the four role
/// colors (foreground/background/cursor/selection) so the name stays readable.
pub(crate) fn scheme_options() -> Vec<ThemeSchemeOption> {
    COLOR_SCHEMES
        .iter()
        .map(|scheme| {
            let mut swatches = palette_swatches(scheme.palette);
            swatches.truncate(4);
            ThemeSchemeOption {
                id: scheme.id.to_owned(),
                name: scheme.name.to_owned(),
                swatches,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tri-state field model (Folder Editor + Session Editor + Settings)
// ---------------------------------------------------------------------------

/// One editable field of a three-state form: `inherit` (no local override) or
/// `explicit` (a value stored at the edited level).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ThemeFieldDraft {
    pub(crate) text: String,
    pub(crate) explicit: bool,
}

impl ThemeFieldDraft {
    fn value(text: impl Into<String>, explicit: bool) -> Self {
        Self {
            text: text.into(),
            explicit,
        }
    }
}

/// One action requested by a three-state field control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeFieldAction {
    /// Use the inherited value (clear the local override).
    SetInherit,
    /// Override locally (the editor keeps the current value as a starting point).
    SetExplicit,
    /// Drop the local value and fall back to the inherited one.
    Reset,
}

/// Where an inherited value comes from (for the "From …" hint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeSource {
    pub(crate) kind: &'static str,
    pub(crate) param: String,
}

impl ThemeSource {
    fn new(kind: &'static str, param: impl Into<String>) -> Self {
        Self {
            kind,
            param: param.into(),
        }
    }

    fn builtin() -> Self {
        Self::new("builtin", String::new())
    }
}

/// The eight terminal theme fields shared by every level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeDraft {
    pub(crate) color_scheme: ThemeFieldDraft,
    pub(crate) foreground: ThemeFieldDraft,
    pub(crate) background: ThemeFieldDraft,
    pub(crate) cursor: ThemeFieldDraft,
    pub(crate) selection: ThemeFieldDraft,
    pub(crate) font_family: ThemeFieldDraft,
    pub(crate) font_size: ThemeFieldDraft,
    pub(crate) fallback_fonts: ThemeFieldDraft,
}

impl ThemeDraft {
    /// Merges a level's own profile over the resolved parent profile: explicit
    /// fields show the stored value, inherited fields show the parent value
    /// (display-only until the user flips the field to explicit).
    pub(crate) fn from_resolved(resolved: &TerminalProfile, own: Option<&TerminalProfile>) -> Self {
        let merge = |own: Option<String>, resolved: String| -> ThemeFieldDraft {
            match own {
                Some(value) => ThemeFieldDraft::value(value, true),
                None => ThemeFieldDraft::value(resolved, false),
            }
        };
        let own = own.cloned().unwrap_or_default();
        let palette = resolve_palette(resolved);
        Self {
            color_scheme: merge(
                own.color_scheme,
                resolved
                    .color_scheme
                    .clone()
                    .filter(|id| color_scheme(id).is_some())
                    .unwrap_or_else(|| DEFAULT_COLOR_SCHEME_ID.to_owned()),
            ),
            foreground: merge(own.foreground, palette.foreground.to_hex()),
            background: merge(own.background, palette.background.to_hex()),
            cursor: merge(own.cursor, palette.cursor.to_hex()),
            selection: merge(own.selection, palette.selection.to_hex()),
            font_family: merge(
                own.font_family,
                resolved
                    .font_family
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| BUNDLED_FONT_FAMILY.to_owned()),
            ),
            font_size: merge(
                own.font_size.map(|size| size.to_string()),
                resolve_font_size(resolved.font_size).round().to_string(),
            ),
            fallback_fonts: merge(
                own.fallback_fonts.map(|paths| paths.join("\n")),
                resolved
                    .fallback_fonts
                    .clone()
                    .unwrap_or_default()
                    .join("\n"),
            ),
        }
    }

    /// Refreshes the display text of every inherited field from a new parent
    /// resolution (the editor's target folder changed).
    pub(crate) fn refresh_inherited(&mut self, resolved: &TerminalProfile) {
        let palette = resolve_palette(resolved);
        let update = |field: &mut ThemeFieldDraft, text: String| {
            if !field.explicit {
                field.text = text;
            }
        };
        update(
            &mut self.color_scheme,
            resolved
                .color_scheme
                .clone()
                .filter(|id| color_scheme(id).is_some())
                .unwrap_or_else(|| DEFAULT_COLOR_SCHEME_ID.to_owned()),
        );
        update(&mut self.foreground, palette.foreground.to_hex());
        update(&mut self.background, palette.background.to_hex());
        update(&mut self.cursor, palette.cursor.to_hex());
        update(&mut self.selection, palette.selection.to_hex());
        update(
            &mut self.font_family,
            resolved
                .font_family
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| BUNDLED_FONT_FAMILY.to_owned()),
        );
        update(
            &mut self.font_size,
            resolve_font_size(resolved.font_size).round().to_string(),
        );
        update(
            &mut self.fallback_fonts,
            resolved
                .fallback_fonts
                .clone()
                .unwrap_or_default()
                .join("\n"),
        );
    }

    /// `true` when at least one theme field is overridden at this level.
    pub(crate) fn has_explicit_field(&self) -> bool {
        [
            &self.color_scheme,
            &self.foreground,
            &self.background,
            &self.cursor,
            &self.selection,
            &self.font_family,
            &self.font_size,
            &self.fallback_fonts,
        ]
        .into_iter()
        .any(|field| field.explicit)
    }

    /// Mutable access to one theme field by key.
    pub(crate) fn field_mut(&mut self, key: &str) -> Option<&mut ThemeFieldDraft> {
        match key {
            KEY_COLOR_SCHEME => Some(&mut self.color_scheme),
            KEY_FOREGROUND => Some(&mut self.foreground),
            KEY_BACKGROUND => Some(&mut self.background),
            KEY_CURSOR => Some(&mut self.cursor),
            KEY_SELECTION => Some(&mut self.selection),
            KEY_FONT_FAMILY => Some(&mut self.font_family),
            KEY_FONT_SIZE => Some(&mut self.font_size),
            KEY_FALLBACK_FONTS => Some(&mut self.fallback_fonts),
            _ => None,
        }
    }

    /// Writes the explicit fields into `profile`, clearing inherited ones.
    ///
    /// Inherited fields are taken from `inherited` (the parent-resolved
    /// profile), so the stored values never change a subtree silently.
    pub(crate) fn write_into(
        &self,
        profile: &mut TerminalProfile,
        inherited: &TerminalProfile,
    ) -> Result<(), ThemeFieldError> {
        profile.color_scheme = if self.color_scheme.explicit {
            let id = self.color_scheme.text.trim();
            if color_scheme(id).is_none() {
                return Err(ThemeFieldError::new(
                    "appearance.color_scheme",
                    "unknown color scheme id",
                ));
            }
            Some(id.to_owned())
        } else {
            inherited.color_scheme.clone()
        };
        profile.foreground = self.write_color(&self.foreground, inherited.foreground.clone())?;
        profile.background = self.write_color(&self.background, inherited.background.clone())?;
        profile.cursor = self.write_color(&self.cursor, inherited.cursor.clone())?;
        profile.selection = self.write_color(&self.selection, inherited.selection.clone())?;
        profile.font_family = if self.font_family.explicit {
            let text = self.font_family.text.trim().to_owned();
            if text.is_empty() || text.eq_ignore_ascii_case(BUNDLED_FONT_FAMILY) {
                None
            } else {
                Some(text)
            }
        } else {
            inherited.font_family.clone()
        };
        profile.font_size = if self.font_size.explicit {
            Some(self.parse_font_size()?)
        } else {
            inherited.font_size
        };
        profile.fallback_fonts = if self.fallback_fonts.explicit {
            let paths: Vec<String> = self
                .fallback_fonts
                .text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            (!paths.is_empty()).then_some(paths)
        } else {
            inherited.fallback_fonts.clone()
        };
        Ok(())
    }

    fn write_color(
        &self,
        field: &ThemeFieldDraft,
        inherited: Option<String>,
    ) -> Result<Option<String>, ThemeFieldError> {
        if !field.explicit {
            return Ok(inherited);
        }
        let text = field.text.trim();
        if text.is_empty() {
            return Ok(inherited);
        }
        if TerminalColor::from_hex(text).is_none() {
            return Err(ThemeFieldError::new(
                "appearance.color",
                format!("`{text}` is not a #RRGGBB color"),
            ));
        }
        Ok(Some(text.to_owned()))
    }

    pub(crate) fn parse_font_size(&self) -> Result<u16, ThemeFieldError> {
        let text = self.font_size.text.trim();
        let value = text.parse::<u16>().map_err(|_| {
            ThemeFieldError::new("appearance.font_size", "font size must be a number")
        })?;
        if !(THEME_FONT_SIZE_MIN..=THEME_FONT_SIZE_MAX).contains(&value) {
            return Err(ThemeFieldError::new(
                "appearance.font_size",
                format!(
                    "font size must be between {THEME_FONT_SIZE_MIN} and {THEME_FONT_SIZE_MAX}"
                ),
            ));
        }
        Ok(value)
    }
}

/// A rejected editor field with a message for the form status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeFieldError {
    pub(crate) field: &'static str,
    pub(crate) message: String,
}

impl ThemeFieldError {
    pub(crate) fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

/// Plain-data mirror of the Slint `TriStateField` struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriStateFieldData {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) hint: String,
    /// `text` | `number` | `hex` | `toggle` | `choice`
    pub(crate) kind: &'static str,
    /// `inherit` | `explicit`
    pub(crate) state: &'static str,
    pub(crate) value: String,
    /// `builtin` | `global` | `folder` | `scheme` | `local-folder` | `local-session`
    pub(crate) source_kind: &'static str,
    pub(crate) source_param: String,
    pub(crate) options: Vec<String>,
    pub(crate) selected: i32,
    pub(crate) swatches: Vec<[u8; 3]>,
}

impl TriStateFieldData {
    pub(crate) fn new(
        key: &str,
        label: &str,
        hint: &str,
        kind: &'static str,
        field: &ThemeFieldDraft,
        source: &ThemeSource,
        local_kind: &'static str,
    ) -> Self {
        let (state, source_kind, source_param) = if field.explicit {
            ("explicit", local_kind, String::new())
        } else {
            ("inherit", source.kind, source.param.clone())
        };
        Self {
            key: key.to_owned(),
            label: label.to_owned(),
            hint: hint.to_owned(),
            kind,
            state,
            value: field.text.clone(),
            source_kind,
            source_param,
            options: Vec::new(),
            selected: -1,
            swatches: Vec::new(),
        }
    }

    pub(crate) fn with_choice(mut self, options: Vec<String>, selected: i32) -> Self {
        self.options = options;
        self.selected = selected;
        self
    }

    pub(crate) fn with_swatches(mut self, swatches: Vec<[u8; 3]>) -> Self {
        self.swatches = swatches;
        self
    }
}

/// Field key → the tri-state field data model used by the editors.
pub(crate) const KEY_COLOR_SCHEME: &str = "appearance.color_scheme";
pub(crate) const KEY_FOREGROUND: &str = "appearance.foreground";
pub(crate) const KEY_BACKGROUND: &str = "appearance.background";
pub(crate) const KEY_CURSOR: &str = "appearance.cursor";
pub(crate) const KEY_SELECTION: &str = "appearance.selection";
pub(crate) const KEY_FONT_FAMILY: &str = "appearance.font_family";
pub(crate) const KEY_FONT_SIZE: &str = "appearance.font_size";
pub(crate) const KEY_FALLBACK_FONTS: &str = "appearance.fallback_fonts";
pub(crate) const KEY_SCROLLBACK_LINES: &str = "terminal.scrollback_lines";
pub(crate) const KEY_SCROLLBACK_MAX_CELLS: &str = "terminal.scrollback_max_cells";
pub(crate) const KEY_LOGGING_ENABLED: &str = "logging.enabled";
pub(crate) const KEY_LOGGING_FORMAT: &str = "logging.format";
pub(crate) const KEY_LOGGING_DIRECTORY: &str = "logging.directory";

/// Scheme labels offered by the choice editor (ids resolved by index).
pub(crate) fn color_scheme_options() -> Vec<String> {
    COLOR_SCHEMES
        .iter()
        .map(|scheme| scheme.name.to_owned())
        .collect()
}

/// Index of a scheme id inside [`color_scheme_options`].
pub(crate) fn color_scheme_index(id: &str) -> i32 {
    COLOR_SCHEMES
        .iter()
        .position(|scheme| scheme.id == id)
        .map_or(0, |index| index as i32)
}

pub(crate) fn color_scheme_id_at(index: i32) -> Option<&'static str> {
    usize::try_from(index)
        .ok()
        .and_then(|index| COLOR_SCHEMES.get(index))
        .map(|scheme| scheme.id)
}

/// Nearest level that defines one terminal theme field, nearest-first chain.
fn theme_field_source(
    chain: &[&FolderProfile],
    global: &TerminalProfile,
    is_set: impl Fn(&TerminalProfile) -> bool,
) -> ThemeSource {
    for folder in chain {
        if folder.terminal.as_ref().is_some_and(&is_set) {
            return ThemeSource::new("folder", folder.name.clone());
        }
    }
    if is_set(global) {
        ThemeSource::new("global", String::new())
    } else {
        ThemeSource::builtin()
    }
}

/// Inherit-source of every editor field, resolved against the levels *above*
/// the edited one (nearest folder first, then global, then built-in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeSources {
    pub(crate) color_scheme: ThemeSource,
    pub(crate) foreground: ThemeSource,
    pub(crate) background: ThemeSource,
    pub(crate) cursor: ThemeSource,
    pub(crate) selection: ThemeSource,
    pub(crate) font_family: ThemeSource,
    pub(crate) font_size: ThemeSource,
    pub(crate) fallback_fonts: ThemeSource,
    pub(crate) scrollback_lines: ThemeSource,
    pub(crate) scrollback_max_cells: ThemeSource,
    pub(crate) logging_enabled: ThemeSource,
    pub(crate) logging_format: ThemeSource,
    pub(crate) logging_directory: ThemeSource,
}

impl ThemeSources {
    pub(crate) fn resolve(
        chain: &[&FolderProfile],
        global: &TerminalProfile,
        global_logging: &LoggingProfile,
    ) -> Self {
        let builtin = TerminalProfile::default();
        let builtin_logging = LoggingProfile::default();
        let terminal = |is_set: &dyn Fn(&TerminalProfile) -> bool| {
            theme_field_source(chain, global, |profile| is_set(profile))
        };
        let concrete = |global_value: usize,
                        builtin_value: usize,
                        folder_value: &dyn Fn(&FolderProfile) -> usize| {
            if let Some(folder) = chain
                .iter()
                .find(|folder| folder.terminal.is_some() && folder_value(folder) != builtin_value)
            {
                ThemeSource::new("folder", folder.name.clone())
            } else if global_value != builtin_value {
                ThemeSource::new("global", String::new())
            } else {
                ThemeSource::builtin()
            }
        };
        let logging_field = |is_set: &dyn Fn(&LoggingProfile) -> bool| {
            if let Some(folder) = chain
                .iter()
                .find(|folder| folder.logging.as_ref().is_some_and(is_set))
            {
                return ThemeSource::new("folder", folder.name.clone());
            }
            if is_set(global_logging) {
                ThemeSource::new("global", String::new())
            } else {
                ThemeSource::builtin()
            }
        };
        Self {
            color_scheme: terminal(&|profile| profile.color_scheme.is_some()),
            foreground: terminal(&|profile| profile.foreground.is_some()),
            background: terminal(&|profile| profile.background.is_some()),
            cursor: terminal(&|profile| profile.cursor.is_some()),
            selection: terminal(&|profile| profile.selection.is_some()),
            font_family: terminal(&|profile| profile.font_family.is_some()),
            font_size: terminal(&|profile| profile.font_size.is_some()),
            fallback_fonts: terminal(&|profile| profile.fallback_fonts.is_some()),
            scrollback_lines: concrete(
                global.scrollback_lines,
                builtin.scrollback_lines,
                &|folder| {
                    folder
                        .terminal
                        .as_ref()
                        .map_or(builtin.scrollback_lines, |terminal| {
                            terminal.scrollback_lines
                        })
                },
            ),
            scrollback_max_cells: concrete(
                global.scrollback_max_cells,
                builtin.scrollback_max_cells,
                &|folder| {
                    folder
                        .terminal
                        .as_ref()
                        .map_or(builtin.scrollback_max_cells, |terminal| {
                            terminal.scrollback_max_cells
                        })
                },
            ),
            logging_enabled: logging_field(&|logging| logging.enabled != builtin_logging.enabled),
            logging_format: logging_field(&|logging| logging.format != builtin_logging.format),
            logging_directory: logging_field(&|logging| logging.directory.is_some()),
        }
    }

    pub(crate) fn theme_field(&self, key: &str) -> &ThemeSource {
        match key {
            KEY_COLOR_SCHEME => &self.color_scheme,
            KEY_FOREGROUND => &self.foreground,
            KEY_BACKGROUND => &self.background,
            KEY_CURSOR => &self.cursor,
            KEY_SELECTION => &self.selection,
            KEY_FONT_FAMILY => &self.font_family,
            KEY_FONT_SIZE => &self.font_size,
            KEY_FALLBACK_FONTS => &self.fallback_fonts,
            KEY_SCROLLBACK_LINES => &self.scrollback_lines,
            KEY_SCROLLBACK_MAX_CELLS => &self.scrollback_max_cells,
            KEY_LOGGING_ENABLED => &self.logging_enabled,
            KEY_LOGGING_FORMAT => &self.logging_format,
            KEY_LOGGING_DIRECTORY => &self.logging_directory,
            _ => &self.color_scheme,
        }
    }
}

/// Builds the eight appearance rows for a folder/session editor.
pub(crate) fn theme_field_rows(
    draft: &ThemeDraft,
    sources: &ThemeSources,
    local_kind: &'static str,
    palette: TerminalPalette,
    scheme_label: String,
) -> Vec<TriStateFieldData> {
    let mut scheme_field = draft.color_scheme.clone();
    if !scheme_field.explicit {
        // The inherit column shows the resolved scheme *name*.
        scheme_field.text = scheme_label;
    }
    let row = |key: &str, label: &str, hint: &str, kind: &'static str, field: &ThemeFieldDraft| {
        TriStateFieldData::new(
            key,
            label,
            hint,
            kind,
            field,
            sources.theme_field(key),
            local_kind,
        )
    };
    let swatches = palette_role_swatches(palette);
    vec![
        row(
            KEY_COLOR_SCHEME,
            "Color scheme",
            "Built-in palette",
            "choice",
            &scheme_field,
        )
        .with_choice(
            color_scheme_options(),
            color_scheme_index(&scheme_field.text),
        )
        .with_swatches(swatches.clone()),
        row(
            KEY_FOREGROUND,
            "Foreground",
            "Hex color",
            "hex",
            &draft.foreground,
        )
        .with_swatches(vec![rgb_of(palette.foreground)]),
        row(
            KEY_BACKGROUND,
            "Background",
            "Hex color",
            "hex",
            &draft.background,
        )
        .with_swatches(vec![rgb_of(palette.background)]),
        row(KEY_CURSOR, "Cursor", "Hex color", "hex", &draft.cursor)
            .with_swatches(vec![rgb_of(palette.cursor)]),
        row(
            KEY_SELECTION,
            "Selection",
            "Hex color",
            "hex",
            &draft.selection,
        )
        .with_swatches(vec![rgb_of(palette.selection)]),
        row(
            KEY_FONT_FAMILY,
            "Font family",
            "Bundled DejaVu or an imported file",
            "text",
            &draft.font_family,
        ),
        row(
            KEY_FONT_SIZE,
            "Font size",
            "Logical pixels",
            "number",
            &draft.font_size,
        ),
        row(
            KEY_FALLBACK_FONTS,
            "Fallback fonts",
            "One font file per line, tried in order",
            "text",
            &draft.fallback_fonts,
        ),
    ]
}

/// Parses a scrollback draft value, falling back to `inherited` when the field
/// is not overridden at this level.
pub(crate) fn parse_scrollback(
    field: &ThemeFieldDraft,
    inherited: usize,
    minimum: usize,
    maximum: usize,
    key: &'static str,
) -> Result<usize, ThemeFieldError> {
    if !field.explicit {
        return Ok(inherited);
    }
    let text = field.text.trim();
    if text.is_empty() {
        return Ok(inherited);
    }
    let value = text
        .parse::<usize>()
        .map_err(|_| ThemeFieldError::new(key, "value must be a number"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(ThemeFieldError::new(
            key,
            format!("value must be between {minimum} and {maximum}"),
        ));
    }
    Ok(value)
}

fn rgb_of(color: TerminalColor) -> [u8; 3] {
    [color.red, color.green, color.blue]
}

// ---------------------------------------------------------------------------
// Session appearance capture (D17)
// ---------------------------------------------------------------------------

impl AppRuntime {
    /// Resolves the appearance a *new* terminal for `runtime` would use.
    pub(crate) fn appearance_for_runtime(&self, runtime: &SessionRuntime) -> TerminalAppearance {
        let profile = self.resolved_terminal_profile_for_runtime(runtime);
        terminal_appearance(&profile)
    }

    /// Captures the resolved appearance into the session (called once, when the
    /// session is activated — D17: later edits must not repaint it).
    pub(crate) fn capture_runtime_appearance(&self, runtime: &mut SessionRuntime) {
        let appearance = self.appearance_for_runtime(runtime);
        runtime.configure_appearance(appearance);
    }

    /// The active session's captured appearance (renderer sync).
    pub(crate) fn active_terminal_appearance(&self) -> Option<TerminalAppearance> {
        self.active_session_id
            .as_ref()
            .and_then(|key| self.sessions.get(key))
            .map(|runtime| runtime.appearance().clone())
    }
}

// ---------------------------------------------------------------------------
// Settings → Terminal Appearance dialog
// ---------------------------------------------------------------------------

/// Editable state of the Settings appearance dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsAppearanceDraft {
    pub(crate) color_scheme: String,
    pub(crate) foreground: String,
    pub(crate) background: String,
    pub(crate) cursor: String,
    pub(crate) selection: String,
    pub(crate) font_family: String,
    pub(crate) font_size_text: String,
    pub(crate) fallback_fonts_text: String,
}

impl Default for SettingsAppearanceDraft {
    fn default() -> Self {
        Self::from_terminal_profile(&TerminalProfile::default())
    }
}

impl SettingsAppearanceDraft {
    pub(crate) fn from_terminal_profile(profile: &TerminalProfile) -> Self {
        let palette = resolve_palette(profile);
        let (font, _) = resolve_font(profile);
        Self {
            color_scheme: profile
                .color_scheme
                .clone()
                .filter(|id| color_scheme(id).is_some())
                .unwrap_or_else(|| DEFAULT_COLOR_SCHEME_ID.to_owned()),
            foreground: palette.foreground.to_hex(),
            background: palette.background.to_hex(),
            cursor: palette.cursor.to_hex(),
            selection: palette.selection.to_hex(),
            font_family: match font.primary {
                PrimaryFont::BundledDejaVu => BUNDLED_FONT_FAMILY.to_owned(),
                PrimaryFont::File(path) => path.display().to_string(),
            },
            font_size_text: resolve_font_size(profile.font_size).round().to_string(),
            fallback_fonts_text: profile
                .fallback_fonts
                .clone()
                .unwrap_or_default()
                .join("\n"),
        }
    }

    /// Base scheme palette (before the hex overrides).
    fn scheme_palette(&self) -> TerminalPalette {
        color_scheme(&self.color_scheme).map_or(TerminalPalette::DEFAULT, |scheme| scheme.palette)
    }

    /// Effective palette: scheme base plus the hex fields.
    pub(crate) fn palette(&self) -> TerminalPalette {
        let mut palette = self.scheme_palette();
        if let Some(color) = TerminalColor::from_hex(&self.foreground) {
            palette.foreground = color;
        }
        if let Some(color) = TerminalColor::from_hex(&self.background) {
            palette.background = color;
        }
        if let Some(color) = TerminalColor::from_hex(&self.cursor) {
            palette.cursor = color;
        }
        if let Some(color) = TerminalColor::from_hex(&self.selection) {
            palette.selection = color;
        }
        palette
    }

    pub(crate) fn font_selection(&self) -> FontSelection {
        let primary = {
            let text = self.font_family.trim();
            if text.is_empty() || text.eq_ignore_ascii_case(BUNDLED_FONT_FAMILY) {
                PrimaryFont::BundledDejaVu
            } else {
                PrimaryFont::File(PathBuf::from(text))
            }
        };
        FontSelection {
            primary,
            fallback_files: self
                .fallback_fonts_text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect(),
        }
    }

    /// Parsed font size, or `None` when the text is empty/invalid.
    pub(crate) fn font_size(&self) -> Option<u16> {
        let value = self.font_size_text.trim().parse::<u16>().ok()?;
        (THEME_FONT_SIZE_MIN..=THEME_FONT_SIZE_MAX)
            .contains(&value)
            .then_some(value)
    }

    /// Validates the draft and returns the theme fields to write into the
    /// global `TerminalProfile`.
    pub(crate) fn theme_fields(&self) -> Result<SettingsAppearanceTheme, ThemeFieldError> {
        if color_scheme(&self.color_scheme).is_none() {
            return Err(ThemeFieldError::new(
                "appearance.color_scheme",
                "unknown color scheme id",
            ));
        }
        let base = self.scheme_palette();
        let parsed = |field: &'static str, value: &str| -> Result<TerminalColor, ThemeFieldError> {
            TerminalColor::from_hex(value).ok_or_else(|| {
                ThemeFieldError::new(field, format!("`{value}` is not a #RRGGBB color"))
            })
        };
        let foreground = parsed("appearance.foreground", &self.foreground)?;
        let background = parsed("appearance.background", &self.background)?;
        let cursor = parsed("appearance.cursor", &self.cursor)?;
        let selection = parsed("appearance.selection", &self.selection)?;
        let font_size = self.font_size().ok_or_else(|| {
            ThemeFieldError::new(
                "appearance.font_size",
                format!(
                    "font size must be between {THEME_FONT_SIZE_MIN} and {THEME_FONT_SIZE_MAX}"
                ),
            )
        })?;
        let font_family = {
            let text = self.font_family.trim();
            if text.is_empty() || text.eq_ignore_ascii_case(BUNDLED_FONT_FAMILY) {
                None
            } else {
                Some(text.to_owned())
            }
        };
        if let Some(path) = font_family.as_deref() {
            if !Path::new(path).is_file() {
                return Err(ThemeFieldError::new(
                    "appearance.font_family",
                    format!("font file `{path}` was not found"),
                ));
            }
        }
        let fallback_fonts: Vec<String> = self
            .fallback_fonts_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if let Some(missing) = fallback_fonts
            .iter()
            .find(|path| !Path::new(path.as_str()).is_file())
        {
            return Err(ThemeFieldError::new(
                "appearance.fallback_fonts",
                format!("font file `{missing}` was not found"),
            ));
        }
        Ok(SettingsAppearanceTheme {
            color_scheme: self.color_scheme.clone(),
            foreground: (foreground != base.foreground).then(|| foreground.to_hex()),
            background: (background != base.background).then(|| background.to_hex()),
            cursor: (cursor != base.cursor).then(|| cursor.to_hex()),
            selection: (selection != base.selection).then(|| selection.to_hex()),
            font_family,
            font_size: Some(font_size),
            fallback_fonts: (!fallback_fonts.is_empty()).then_some(fallback_fonts),
        })
    }
}

/// Validated theme fields produced by the Settings appearance form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsAppearanceTheme {
    pub(crate) color_scheme: String,
    pub(crate) foreground: Option<String>,
    pub(crate) background: Option<String>,
    pub(crate) cursor: Option<String>,
    pub(crate) selection: Option<String>,
    pub(crate) font_family: Option<String>,
    pub(crate) font_size: Option<u16>,
    pub(crate) fallback_fonts: Option<Vec<String>>,
}

/// Status line of the Settings appearance dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsAppearanceStatus {
    Hint,
    Saved,
    Rejected(ThemeFieldError),
}

impl SettingsAppearanceStatus {
    /// Full status line (rejected values include the field and message).
    pub(crate) fn display_text(&self) -> String {
        match self {
            Self::Hint => {
                "Changes apply to terminals opened after saving; open terminals keep their theme."
                    .to_owned()
            }
            Self::Saved => "Saved the terminal appearance defaults.".to_owned(),
            Self::Rejected(error) => format!("{}: {}", error.field, error.message),
        }
    }
}

impl AppRuntime {
    /// Opens the appearance dialog with the current global defaults.
    pub fn open_settings_appearance(&mut self) -> AppProjection {
        self.settings_appearance =
            SettingsAppearanceDraft::from_terminal_profile(&self.config_document.terminal);
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.settings_appearance_visible = true;
        self.projection()
    }

    pub fn close_settings_appearance(&mut self) -> AppProjection {
        self.settings_appearance_visible = false;
        self.projection()
    }

    /// Picks a named scheme; the hex fields follow the scheme until the user
    /// overrides them again.
    pub fn select_settings_appearance_scheme(&mut self, id: &str) -> AppProjection {
        if let Some(scheme) = color_scheme(id) {
            self.settings_appearance.color_scheme = scheme.id.to_owned();
            self.settings_appearance.foreground = scheme.palette.foreground.to_hex();
            self.settings_appearance.background = scheme.palette.background.to_hex();
            self.settings_appearance.cursor = scheme.palette.cursor.to_hex();
            self.settings_appearance.selection = scheme.palette.selection.to_hex();
            self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        }
        self.projection()
    }

    pub fn update_settings_appearance_color(&mut self, field: &str, value: &str) -> AppProjection {
        match field {
            "foreground" => self.settings_appearance.foreground = value.to_owned(),
            "background" => self.settings_appearance.background = value.to_owned(),
            "cursor" => self.settings_appearance.cursor = value.to_owned(),
            "selection" => self.settings_appearance.selection = value.to_owned(),
            _ => return self.projection(),
        }
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    pub fn update_settings_appearance_font_family(&mut self, value: &str) -> AppProjection {
        self.settings_appearance.font_family = value.to_owned();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    pub fn update_settings_appearance_font_size(&mut self, value: &str) -> AppProjection {
        self.settings_appearance.font_size_text = value.to_owned();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    pub fn update_settings_appearance_fallback_fonts(&mut self, value: &str) -> AppProjection {
        self.settings_appearance.fallback_fonts_text = value.to_owned();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    /// Appends one picked fallback font file (rfd result).
    pub fn add_settings_appearance_fallback_font(&mut self, path: &str) -> AppProjection {
        let path = path.trim();
        if !path.is_empty() {
            let text = &mut self.settings_appearance.fallback_fonts_text;
            if !text.lines().any(|line| line.trim() == path) {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(path);
            }
        }
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    pub fn clear_settings_appearance_fallback_fonts(&mut self) -> AppProjection {
        self.settings_appearance.fallback_fonts_text.clear();
        self.projection()
    }

    /// Restores the built-in defaults into the form (saved by `save_...`).
    pub fn reset_settings_appearance_defaults(&mut self) -> AppProjection {
        self.settings_appearance = SettingsAppearanceDraft::default();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    /// Validates and persists the global terminal appearance defaults.
    pub fn save_settings_appearance(&mut self) -> AppResult<AppProjection> {
        let theme = match self.settings_appearance.theme_fields() {
            Ok(theme) => theme,
            Err(error) => {
                self.settings_appearance_status = SettingsAppearanceStatus::Rejected(error);
                return Ok(self.projection());
            }
        };
        let terminal = &mut self.config_document.terminal;
        terminal.color_scheme = Some(theme.color_scheme);
        terminal.foreground = theme.foreground;
        terminal.background = theme.background;
        terminal.cursor = theme.cursor;
        terminal.selection = theme.selection;
        terminal.font_family = theme.font_family;
        terminal.font_size = theme.font_size;
        terminal.fallback_fonts = theme.fallback_fonts;
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.settings_appearance_status = SettingsAppearanceStatus::Saved;
        self.settings_appearance =
            SettingsAppearanceDraft::from_terminal_profile(&self.config_document.terminal);
        self.set_status_kind(
            "terminal-appearance-saved",
            "Saved the global terminal appearance defaults.".to_owned(),
            self.settings_appearance.color_scheme.clone(),
            String::new(),
        );
        Ok(self.projection())
    }

    /// Palette of the current draft (settings preview / swatches).
    pub(crate) fn settings_appearance_palette(&self) -> TerminalPalette {
        self.settings_appearance.palette()
    }

    /// Renders the live preview frame for the appearance dialog.
    ///
    /// Returns `None` while the dialog is closed. The dedicated preview
    /// renderer keeps the live terminal's glyph cache untouched.
    pub(crate) fn settings_appearance_preview_frame(&self) -> Option<TerminalFrame> {
        if !self.settings_appearance_visible {
            return None;
        }
        let palette = self.settings_appearance.palette();
        let font = self.settings_appearance.font_selection();
        let font_size = resolve_font_size(self.settings_appearance.font_size());
        let mut renderer = self.settings_preview_renderer.borrow_mut();
        let renderer = renderer.get_or_insert_with(TerminalRenderer::new);
        renderer.set_font_size(font_size.clamp(8.0, 20.0));
        renderer.apply_appearance(palette, font);
        let lines = preview_lines(palette);
        let snapshot = TerminalSnapshot {
            lines: lines
                .iter()
                .map(|line| std::borrow::Cow::Borrowed(line.as_slice()))
                .collect(),
            cursor: Some((12, 1)),
            selection: None,
        };
        Some(renderer.render(&snapshot))
    }
}

/// Sample text of the live preview window (colors taken from the palette).
pub(crate) fn preview_lines(palette: TerminalPalette) -> Vec<Vec<TerminalCell>> {
    let cell = |text: &str, foreground: TerminalColor, bold: bool| {
        let mut cell = TerminalCell {
            grapheme: text.to_owned(),
            ..TerminalCell::default()
        };
        cell.foreground = foreground;
        cell.bold = bold;
        cell
    };
    let line = |text: &str, foreground: TerminalColor| -> Vec<TerminalCell> {
        text.chars()
            .map(|character| cell(&character.to_string(), foreground, false))
            .collect()
    };
    let mut prompt = line("user@host:~$ ", palette.ansi(2));
    prompt.extend(line("ls --color", palette.foreground));
    let mut listing = line("drwxr-xr-x  ", palette.foreground);
    listing.extend(line("src", palette.ansi(4)));
    listing.extend(line("   notes.md", palette.foreground));
    let mut ansi = line("ansi ", palette.foreground);
    for index in 0..8u8 {
        let mut swatch = cell("\u{2588}", palette.ansi(index), false);
        swatch.background = palette.ansi(index);
        ansi.push(swatch);
    }
    let selection = line("selection row", palette.foreground);
    let mut status = line("connected ", palette.ansi(10));
    status.extend(line("| idle ", palette.ansi(11)));
    status.extend(line("| failed", palette.ansi(9)));
    vec![
        line("YShell terminal theme preview", palette.foreground),
        prompt,
        listing,
        ansi,
        selection,
        status,
    ]
}
