//! N5 terminal theme: appearance resolution, the Settings Appearance form,
//! the Folder Editor state/model and the tri-state field model shared with the
//! Session Editor.
//!
//! Resolution rules (design §1, C0): a session resolves *session > nearest
//! ancestor folder > global > built-in defaults* per terminal field. Sessions
//! capture their [`TerminalAppearance`] once at creation (D17) and keep it for
//! their whole lifetime; later theme edits only affect terminals opened
//! afterwards.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use yshell_config::{
    ColorTheme, ConfigDocument, ConfigWarning, FolderProfile, LoggingProfile, TerminalProfile,
    ThemeCatalog,
};
use yshell_terminal::{
    color_scheme, FontFamilyFaces, FontSelection, GridPoint, PrimaryFont, SelectionRange,
    TerminalCell, TerminalColor, TerminalFrame, TerminalPalette, TerminalRenderer,
    TerminalSnapshot, COLOR_SCHEMES, DEFAULT_FONT_SIZE, DEFAULT_SCALE_FACTOR,
};

use super::*;

/// `font_family` value that selects the bundled DejaVu Sans Mono family.
pub(crate) const BUNDLED_FONT_FAMILY: &str = "DejaVu Sans Mono";
/// Windows Terminal / PowerShell–style mono (preferred when installed).
#[allow(dead_code)]
pub(crate) const CASCADIA_MONO_FAMILY: &str = "Cascadia Mono";
/// Classic Windows console mono (always present on modern Windows).
#[allow(dead_code)]
pub(crate) const CONSOLAS_FAMILY: &str = "Consolas";
/// Built-in palette id (also the fallback when `color_scheme` is unknown).
pub(crate) const DEFAULT_COLOR_SCHEME_ID: &str = "yshell-default";
/// Font size bounds accepted by the settings/editor forms.
pub(crate) const THEME_FONT_SIZE_MIN: u16 = 6;
/// Font size bounds accepted by the settings/editor forms.
pub(crate) const THEME_FONT_SIZE_MAX: u16 = 72;
/// Log format ids stored in `LoggingProfile::format`.
pub(crate) const LOG_FORMAT_RAW: &str = "raw";

/// D15 presets discovered from fixed system paths (not a full font catalog).
fn system_mono_presets() -> Vec<FontFamilyFaces> {
    #[cfg(not(windows))]
    {
        Vec::new()
    }
    #[cfg(windows)]
    {
        let mut presets = Vec::new();
        let fonts = windows_fonts_dir();
        if let Some(faces) = load_windows_family(
            CASCADIA_MONO_FAMILY,
            &fonts,
            &[
                "CascadiaMono.ttf",
                "CascadiaMono-Regular.ttf",
                "CascadiaMonoNF.ttf",
            ],
            &[
                "CascadiaMono-Bold.ttf",
                "CascadiaMonoNF-Bold.ttf",
                "CascadiaMono-Bold.otf",
            ],
            &[
                "CascadiaMono-Italic.ttf",
                "CascadiaMonoNF-Italic.ttf",
                "CascadiaMonoItalic.ttf",
            ],
            &[
                "CascadiaMono-BoldItalic.ttf",
                "CascadiaMonoNF-BoldItalic.ttf",
                "CascadiaMono-BoldItalic.otf",
            ],
        ) {
            presets.push(faces);
        }
        if let Some(faces) = load_windows_family(
            CONSOLAS_FAMILY,
            &fonts,
            &["consola.ttf"],
            &["consolab.ttf"],
            &["consolai.ttf"],
            &["consolaz.ttf"],
        ) {
            presets.push(faces);
        }
        presets
    }
}

#[cfg(windows)]
fn windows_fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts")
}

#[cfg(windows)]
fn first_existing(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(windows)]
fn load_windows_family(
    label: &str,
    fonts: &Path,
    regular: &[&str],
    bold: &[&str],
    italic: &[&str],
    bold_italic: &[&str],
) -> Option<FontFamilyFaces> {
    let regular = first_existing(fonts, regular)?;
    Some(FontFamilyFaces {
        label: label.to_owned(),
        regular,
        bold: first_existing(fonts, bold),
        italic: first_existing(fonts, italic),
        bold_italic: first_existing(fonts, bold_italic),
    })
}

/// Labels for the Settings font ComboBox: system presets first, then bundled.
pub(crate) fn appearance_font_preset_labels() -> Vec<String> {
    let mut labels: Vec<String> = system_mono_presets()
        .into_iter()
        .map(|faces| faces.label)
        .collect();
    labels.push(BUNDLED_FONT_FAMILY.to_owned());
    labels
}

/// Platform default primary font: bundled DejaVu (portable). System presets
/// stay available in Settings for users who prefer Consolas / Cascadia Mono.
pub(crate) fn default_primary_font() -> PrimaryFont {
    PrimaryFont::BundledDejaVu
}

/// Display label for [`default_primary_font`].
pub(crate) fn default_font_family_label() -> String {
    match default_primary_font() {
        PrimaryFont::BundledDejaVu => BUNDLED_FONT_FAMILY.to_owned(),
        PrimaryFont::Family(faces) => faces.label,
        PrimaryFont::File(path) => path.display().to_string(),
    }
}

/// Maps a stored `font_family` string to a [`PrimaryFont`].
pub(crate) fn primary_font_from_name(name: &str) -> PrimaryFont {
    let name = name.trim();
    if name.is_empty() {
        return default_primary_font();
    }
    if name.eq_ignore_ascii_case(BUNDLED_FONT_FAMILY) {
        return PrimaryFont::BundledDejaVu;
    }
    if let Some(faces) = system_mono_presets()
        .into_iter()
        .find(|faces| faces.label.eq_ignore_ascii_case(name))
    {
        return PrimaryFont::Family(faces);
    }
    PrimaryFont::File(PathBuf::from(name))
}

/// Whether `name` is a known preset label (bundled or system), not a file path.
pub(crate) fn is_font_preset_label(name: &str) -> bool {
    let name = name.trim();
    name.eq_ignore_ascii_case(BUNDLED_FONT_FAMILY)
        || system_mono_presets()
            .iter()
            .any(|faces| faces.label.eq_ignore_ascii_case(name))
}

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
pub(crate) fn terminal_appearance(
    profile: &TerminalProfile,
    catalog: &ThemeCatalog,
) -> TerminalAppearance {
    let (font, rejected_fonts) = resolve_font(profile);
    TerminalAppearance {
        palette: resolve_palette(profile, catalog),
        font,
        font_size: resolve_font_size(profile.font_size),
        rejected_fonts,
    }
}

impl Default for TerminalAppearance {
    fn default() -> Self {
        terminal_appearance(&TerminalProfile::default(), &ThemeCatalog::default())
    }
}

/// Looks up a named scheme: user catalog first, then built-ins, then default.
#[must_use]
pub(crate) fn lookup_scheme_palette(id: Option<&str>, catalog: &ThemeCatalog) -> TerminalPalette {
    let Some(id) = id.filter(|value| !value.is_empty()) else {
        return TerminalPalette::DEFAULT;
    };
    if let Some(theme) = catalog.get_user(id) {
        return palette_from_color_theme(theme);
    }
    color_scheme(id).map_or(TerminalPalette::DEFAULT, |scheme| scheme.palette)
}

/// Whether `id` resolves to a built-in or user theme.
#[must_use]
pub(crate) fn scheme_id_known(id: &str, catalog: &ThemeCatalog) -> bool {
    catalog.contains_user(id) || color_scheme(id).is_some()
}

/// Converts a persisted [`ColorTheme`] into a renderer palette.
#[must_use]
pub(crate) fn palette_from_color_theme(theme: &ColorTheme) -> TerminalPalette {
    let parse = |value: &str, fallback: TerminalColor| {
        TerminalColor::from_hex(value).unwrap_or(fallback)
    };
    let default = TerminalPalette::DEFAULT;
    let ansi = std::array::from_fn(|index| {
        parse(&theme.ansi[index], default.ansi[index])
    });
    TerminalPalette {
        foreground: parse(&theme.foreground, default.foreground),
        background: parse(&theme.background, default.background),
        cursor: parse(&theme.cursor, default.cursor),
        selection: parse(&theme.selection, default.selection),
        ansi,
    }
}

/// Builds a user-theme document from a palette (clone / migration).
#[must_use]
pub(crate) fn color_theme_from_palette(
    id: impl Into<String>,
    name: impl Into<String>,
    palette: TerminalPalette,
) -> ColorTheme {
    ColorTheme {
        id: id.into(),
        name: name.into(),
        foreground: palette.foreground.to_hex(),
        background: palette.background.to_hex(),
        cursor: palette.cursor.to_hex(),
        selection: palette.selection.to_hex(),
        ansi: std::array::from_fn(|index| palette.ansi[index].to_hex()),
    }
}

/// Built-in scheme as a [`ColorTheme`] document (for clone).
#[must_use]
pub(crate) fn builtin_as_color_theme(id: &str) -> Option<ColorTheme> {
    let scheme = color_scheme(id)?;
    Some(color_theme_from_palette(
        scheme.id,
        scheme.name,
        scheme.palette,
    ))
}

/// Palette resolved from a profile: named theme (user or built-in) plus any
/// legacy inline color overrides (kept until migration clears them).
pub(crate) fn resolve_palette(profile: &TerminalProfile, catalog: &ThemeCatalog) -> TerminalPalette {
    let mut palette = lookup_scheme_palette(profile.color_scheme.as_deref(), catalog);
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

/// `true` when a profile still carries legacy inline color overrides.
fn profile_has_color_overrides(profile: &TerminalProfile) -> bool {
    profile.foreground.is_some()
        || profile.background.is_some()
        || profile.cursor.is_some()
        || profile.selection.is_some()
        || profile.ansi.is_some()
}

fn clear_color_overrides(profile: &mut TerminalProfile) {
    profile.foreground = None;
    profile.background = None;
    profile.cursor = None;
    profile.selection = None;
    profile.ansi = None;
}

/// Promotes every legacy inline color override into a named user theme.
pub(crate) fn migrate_inline_color_overrides(
    document: &mut ConfigDocument,
    catalog: &mut ThemeCatalog,
) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();
    migrate_profile_overrides(
        &mut document.terminal,
        "terminal",
        catalog,
        &mut warnings,
    );
    for folder in &mut document.folders {
        migrate_folder_overrides(folder, "folders", catalog, &mut warnings);
    }
    warnings
}

fn migrate_folder_overrides(
    folder: &mut FolderProfile,
    prefix: &str,
    catalog: &mut ThemeCatalog,
    warnings: &mut Vec<ConfigWarning>,
) {
    let location = format!("{prefix}.{}", folder.id);
    if let Some(terminal) = folder.terminal.as_mut() {
        migrate_profile_overrides(
            terminal,
            &format!("{location}.terminal"),
            catalog,
            warnings,
        );
    }
    for session in &mut folder.sessions {
        if let Some(terminal) = session.terminal.as_mut() {
            migrate_profile_overrides(
                terminal,
                &format!("{location}.sessions.{}.terminal", session.id),
                catalog,
                warnings,
            );
        }
    }
    for child in &mut folder.folders {
        migrate_folder_overrides(child, &location, catalog, warnings);
    }
}

fn migrate_profile_overrides(
    profile: &mut TerminalProfile,
    location: &str,
    catalog: &mut ThemeCatalog,
    warnings: &mut Vec<ConfigWarning>,
) {
    if !profile_has_color_overrides(profile) {
        return;
    }
    let palette = resolve_palette(profile, catalog);
    let base_name = profile
        .color_scheme
        .as_deref()
        .and_then(|id| {
            catalog
                .get_user(id)
                .map(|theme| theme.name.clone())
                .or_else(|| color_scheme(id).map(|scheme| scheme.name.to_owned()))
        })
        .unwrap_or_else(|| "Custom".to_owned());
    let theme_name = format!("Custom from {base_name}");
    let source = color_theme_from_palette("pending", &theme_name, palette);
    match catalog.clone_theme(&source, &theme_name) {
        Ok(theme) => {
            warnings.push(ConfigWarning::MigratedColorOverrides {
                location: location.to_owned(),
                theme_id: theme.id.clone(),
                theme_name: theme.name.clone(),
            });
            profile.color_scheme = Some(theme.id);
            clear_color_overrides(profile);
        }
        Err(_) => {
            // Keep overrides if the catalog cannot accept the clone; next load retries.
        }
    }
}

/// Font selection plus the paths that could not be loaded.
pub(crate) fn resolve_font(profile: &TerminalProfile) -> (FontSelection, Vec<String>) {
    let primary = match profile.font_family.as_deref().map(str::trim) {
        None | Some("") => default_primary_font(),
        Some(name) => primary_font_from_name(name),
    };
    let fallback_files = profile
        .backup_font
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .into_iter()
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

/// Normalizes user/legacy font text into the single backup font value.
///
/// The Settings field is single-line, but pasted or programmatic text may still
/// carry several lines; only the first non-empty line is kept (TERM-5).
pub(crate) fn normalize_backup_font(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
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

/// 预览表面的底色（跟随预览配色；见 `AppRuntime::*_preview_background`）。
pub(crate) fn preview_background(palette: TerminalPalette) -> [u8; 3] {
    let color = palette.background;
    [color.red, color.green, color.blue]
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
    /// Built-in schemes are immutable; user themes can be edited/deleted.
    pub(crate) builtin: bool,
}

/// Built-in + user schemes as projection data. Scheme chips carry the four role
/// colors (foreground/background/cursor/selection) so the name stays readable.
pub(crate) fn scheme_options(catalog: &ThemeCatalog) -> Vec<ThemeSchemeOption> {
    let mut options: Vec<ThemeSchemeOption> = COLOR_SCHEMES
        .iter()
        .map(|scheme| {
            let mut swatches = palette_swatches(scheme.palette);
            swatches.truncate(4);
            ThemeSchemeOption {
                id: scheme.id.to_owned(),
                name: scheme.name.to_owned(),
                swatches,
                builtin: true,
            }
        })
        .collect();
    for theme in catalog.user_themes() {
        let palette = palette_from_color_theme(theme);
        let mut swatches = palette_swatches(palette);
        swatches.truncate(4);
        options.push(ThemeSchemeOption {
            id: theme.id.clone(),
            name: theme.name.clone(),
            swatches,
            builtin: false,
        });
    }
    options
}

/// Scheme display names for the folder/session choice control.
pub(crate) fn color_scheme_option_names(catalog: &ThemeCatalog) -> Vec<String> {
    scheme_options(catalog)
        .into_iter()
        .map(|option| option.name)
        .collect()
}

/// Index of a scheme id inside [`scheme_options`].
pub(crate) fn color_scheme_index(catalog: &ThemeCatalog, id: &str) -> i32 {
    scheme_options(catalog)
        .iter()
        .position(|scheme| scheme.id == id)
        .map_or(0, |index| index as i32)
}

/// Scheme id at `index` inside [`scheme_options`].
pub(crate) fn color_scheme_id_at(catalog: &ThemeCatalog, index: i32) -> Option<String> {
    let options = scheme_options(catalog);
    usize::try_from(index)
        .ok()
        .and_then(|index| options.get(index))
        .map(|scheme| scheme.id.clone())
}

/// Display name for a scheme id (user or built-in).
pub(crate) fn scheme_display_name(id: &str, catalog: &ThemeCatalog) -> String {
    if let Some(theme) = catalog.get_user(id) {
        return theme.name.clone();
    }
    color_scheme(id)
        .map(|scheme| scheme.name.to_owned())
        .unwrap_or_else(|| id.to_owned())
}

/// Effective global theme id: the persisted known id, else the built-in default.
pub(crate) fn resolved_global_scheme_id(
    profile: &TerminalProfile,
    catalog: &ThemeCatalog,
) -> String {
    profile
        .color_scheme
        .clone()
        .filter(|id| scheme_id_known(id, catalog))
        .unwrap_or_else(|| DEFAULT_COLOR_SCHEME_ID.to_owned())
}

/// One row of the independent Theme Manager list (Slint `ThemeManagerRow`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeManagerRowData {
    pub id: String,
    pub name: String,
    /// Role colors + ANSI accents (8 chips, never truncated in the list).
    pub swatches: Vec<[u8; 3]>,
    /// Built-ins are immutable: Edit/Delete are disabled, Clone stays enabled.
    pub builtin: bool,
    /// Row is selected for the live preview.
    pub selected: bool,
    /// Row is the current global default (`terminal.color_scheme`).
    pub current: bool,
}

/// Built-in + user themes as Theme Manager rows.
///
/// Order matches the rest of the app: built-ins (compile-time order) first,
/// then user themes sorted by id (the catalog is a `BTreeMap`).
pub(crate) fn theme_manager_rows(
    catalog: &ThemeCatalog,
    selected_id: &str,
    default_id: &str,
) -> Vec<ThemeManagerRowData> {
    let mut rows: Vec<ThemeManagerRowData> = COLOR_SCHEMES
        .iter()
        .map(|scheme| ThemeManagerRowData {
            id: scheme.id.to_owned(),
            name: scheme.name.to_owned(),
            swatches: palette_swatches(scheme.palette),
            builtin: true,
            selected: scheme.id == selected_id,
            current: scheme.id == default_id,
        })
        .collect();
    for theme in catalog.user_themes() {
        rows.push(ThemeManagerRowData {
            id: theme.id.clone(),
            name: theme.name.clone(),
            swatches: palette_swatches(palette_from_color_theme(theme)),
            builtin: false,
            selected: theme.id == selected_id,
            current: theme.id == default_id,
        });
    }
    rows
}

/// Number of `color_scheme` references to `id` across the global profile,
/// folders and sessions (including nested folders).
///
/// Used by the delete confirmation: the existing [`retarget_theme_references`]
/// path rewrites every hit to the built-in default, and the count tells the
/// user how many references will move.
pub(crate) fn theme_reference_count(document: &ConfigDocument, id: &str) -> usize {
    fn profile_matches(profile: Option<&TerminalProfile>, id: &str) -> bool {
        profile.is_some_and(|profile| profile.color_scheme.as_deref() == Some(id))
    }
    fn walk(folders: &[FolderProfile], id: &str) -> usize {
        let mut count = 0;
        for folder in folders {
            if profile_matches(folder.terminal.as_ref(), id) {
                count += 1;
            }
            for session in &folder.sessions {
                if profile_matches(session.terminal.as_ref(), id) {
                    count += 1;
                }
            }
            count += walk(&folder.folders, id);
        }
        count
    }
    let global = usize::from(profile_matches(Some(&document.terminal), id));
    global + walk(&document.folders, id)
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

/// Terminal appearance fields shared by every level (theme reference + fonts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeDraft {
    pub(crate) color_scheme: ThemeFieldDraft,
    pub(crate) font_family: ThemeFieldDraft,
    pub(crate) font_size: ThemeFieldDraft,
    pub(crate) backup_font: ThemeFieldDraft,
}

impl ThemeDraft {
    /// Merges a level's own profile over the resolved parent profile: explicit
    /// fields show the stored value, inherited fields show the parent value
    /// (display-only until the user flips the field to explicit).
    pub(crate) fn from_resolved(
        resolved: &TerminalProfile,
        own: Option<&TerminalProfile>,
        catalog: &ThemeCatalog,
    ) -> Self {
        let merge = |own: Option<String>, resolved: String| -> ThemeFieldDraft {
            match own {
                Some(value) => ThemeFieldDraft::value(value, true),
                None => ThemeFieldDraft::value(resolved, false),
            }
        };
        let own = own.cloned().unwrap_or_default();
        Self {
            color_scheme: merge(
                own.color_scheme,
                resolved
                    .color_scheme
                    .clone()
                    .filter(|id| scheme_id_known(id, catalog))
                    .unwrap_or_else(|| DEFAULT_COLOR_SCHEME_ID.to_owned()),
            ),
            font_family: merge(
                own.font_family,
                resolved
                    .font_family
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(default_font_family_label),
            ),
            font_size: merge(
                own.font_size.map(|size| size.to_string()),
                resolve_font_size(resolved.font_size).round().to_string(),
            ),
            backup_font: merge(
                own.backup_font,
                resolved.backup_font.clone().unwrap_or_default(),
            ),
        }
    }

    /// Refreshes the display text of every inherited field from a new parent
    /// resolution (the editor's target folder changed).
    pub(crate) fn refresh_inherited(
        &mut self,
        resolved: &TerminalProfile,
        catalog: &ThemeCatalog,
    ) {
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
                .filter(|id| scheme_id_known(id, catalog))
                .unwrap_or_else(|| DEFAULT_COLOR_SCHEME_ID.to_owned()),
        );
        update(
            &mut self.font_family,
            resolved
                .font_family
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(default_font_family_label),
        );
        update(
            &mut self.font_size,
            resolve_font_size(resolved.font_size).round().to_string(),
        );
        update(
            &mut self.backup_font,
            resolved.backup_font.clone().unwrap_or_default(),
        );
    }

    /// `true` when at least one theme field is overridden at this level.
    pub(crate) fn has_explicit_field(&self) -> bool {
        [
            &self.color_scheme,
            &self.font_family,
            &self.font_size,
            &self.backup_font,
        ]
        .into_iter()
        .any(|field| field.explicit)
    }

    /// Mutable access to one theme field by key.
    pub(crate) fn field_mut(&mut self, key: &str) -> Option<&mut ThemeFieldDraft> {
        match key {
            KEY_COLOR_SCHEME => Some(&mut self.color_scheme),
            KEY_FONT_FAMILY => Some(&mut self.font_family),
            KEY_FONT_SIZE => Some(&mut self.font_size),
            KEY_BACKUP_FONT => Some(&mut self.backup_font),
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
        catalog: &ThemeCatalog,
    ) -> Result<(), ThemeFieldError> {
        profile.color_scheme = if self.color_scheme.explicit {
            let id = self.color_scheme.text.trim();
            if !scheme_id_known(id, catalog) {
                return Err(ThemeFieldError::with_reason(
                    "appearance.color_scheme",
                    "unknown-scheme",
                    String::new(),
                    "unknown color scheme id",
                ));
            }
            Some(id.to_owned())
        } else {
            inherited.color_scheme.clone()
        };
        // Usage sites only reference a theme; never persist inline color overrides.
        clear_color_overrides(profile);
        profile.font_family = if self.font_family.explicit {
            let text = self.font_family.text.trim().to_owned();
            let default_label = default_font_family_label();
            if text.is_empty() || text.eq_ignore_ascii_case(&default_label) {
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
        profile.backup_font = if self.backup_font.explicit {
            normalize_backup_font(&self.backup_font.text)
        } else {
            inherited.backup_font.clone()
        };
        Ok(())
    }

    pub(crate) fn parse_font_size(&self) -> Result<u16, ThemeFieldError> {
        let text = self.font_size.text.trim();
        let value = text.parse::<u16>().map_err(|_| {
            ThemeFieldError::with_reason(
                "appearance.font_size",
                "not-number",
                String::new(),
                "font size must be a number",
            )
        })?;
        if !(THEME_FONT_SIZE_MIN..=THEME_FONT_SIZE_MAX).contains(&value) {
            return Err(ThemeFieldError::with_reason(
                "appearance.font_size",
                "out-of-range",
                format!("{THEME_FONT_SIZE_MIN} and {THEME_FONT_SIZE_MAX}"),
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
    /// I18N-1：稳定原因 id（`unknown-scheme` / `not-number` / `out-of-range` /
    /// `invalid`），供 Folder Editor 选择状态模板。
    pub(crate) reason: &'static str,
    /// 原因相关的技术参数（当前为数值范围，如 `100 and 1000000`）。
    pub(crate) param: String,
    pub(crate) message: String,
}

impl ThemeFieldError {
    pub(crate) fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self::with_reason(field, "invalid", String::new(), message)
    }

    pub(crate) fn with_reason(
        field: &'static str,
        reason: &'static str,
        param: String,
        message: impl Into<String>,
    ) -> Self {
        Self {
            field,
            reason,
            param,
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
pub(crate) const KEY_FONT_FAMILY: &str = "appearance.font_family";
pub(crate) const KEY_FONT_SIZE: &str = "appearance.font_size";
pub(crate) const KEY_BACKUP_FONT: &str = "appearance.backup_font";
pub(crate) const KEY_SCROLLBACK_LINES: &str = "terminal.scrollback_lines";
pub(crate) const KEY_SCROLLBACK_MAX_CELLS: &str = "terminal.scrollback_max_cells";
pub(crate) const KEY_LOGGING_ENABLED: &str = "logging.enabled";
pub(crate) const KEY_LOGGING_FORMAT: &str = "logging.format";
pub(crate) const KEY_LOGGING_DIRECTORY: &str = "logging.directory";

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
    pub(crate) font_family: ThemeSource,
    pub(crate) font_size: ThemeSource,
    pub(crate) backup_font: ThemeSource,
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
            font_family: terminal(&|profile| profile.font_family.is_some()),
            font_size: terminal(&|profile| profile.font_size.is_some()),
            backup_font: terminal(&|profile| profile.backup_font.is_some()),
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
            KEY_FONT_FAMILY => &self.font_family,
            KEY_FONT_SIZE => &self.font_size,
            KEY_BACKUP_FONT => &self.backup_font,
            KEY_SCROLLBACK_LINES => &self.scrollback_lines,
            KEY_SCROLLBACK_MAX_CELLS => &self.scrollback_max_cells,
            KEY_LOGGING_ENABLED => &self.logging_enabled,
            KEY_LOGGING_FORMAT => &self.logging_format,
            KEY_LOGGING_DIRECTORY => &self.logging_directory,
            _ => &self.color_scheme,
        }
    }
}

/// Builds the appearance rows for a folder/session editor (theme ref + fonts).
pub(crate) fn theme_field_rows(
    draft: &ThemeDraft,
    sources: &ThemeSources,
    local_kind: &'static str,
    palette: TerminalPalette,
    scheme_label: String,
    catalog: &ThemeCatalog,
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
    let scheme_id = if scheme_field.explicit {
        scheme_field.text.clone()
    } else {
        draft.color_scheme.text.clone()
    };
    let font_labels = appearance_font_preset_labels();
    let font_index = font_labels
        .iter()
        .position(|label| label.eq_ignore_ascii_case(draft.font_family.text.trim()))
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0);
    vec![
        row(
            KEY_COLOR_SCHEME,
            "Color scheme",
            "Named theme (built-in or user)",
            "choice",
            &scheme_field,
        )
        .with_choice(
            color_scheme_option_names(catalog),
            color_scheme_index(catalog, &scheme_id),
        )
        .with_swatches(swatches),
        row(
            KEY_FONT_FAMILY,
            "Font family",
            "System mono preset, bundled DejaVu, or an imported file",
            "choice",
            &draft.font_family,
        )
        .with_choice(font_labels, font_index),
        row(
            KEY_FONT_SIZE,
            "Font size",
            "Logical pixels",
            "number",
            &draft.font_size,
        ),
        row(
            KEY_BACKUP_FONT,
            "Fallback font",
            "One optional font file tried before the bundled CJK face",
            "text",
            &draft.backup_font,
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
    let value = text.parse::<usize>().map_err(|_| {
        ThemeFieldError::with_reason(key, "not-number", String::new(), "value must be a number")
    })?;
    if !(minimum..=maximum).contains(&value) {
        return Err(ThemeFieldError::with_reason(
            key,
            "out-of-range",
            format!("{minimum} and {maximum}"),
            format!("value must be between {minimum} and {maximum}"),
        ));
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Session appearance capture (D17)
// ---------------------------------------------------------------------------

impl AppRuntime {
    /// Resolves the appearance a *new* terminal for `runtime` would use.
    pub(crate) fn appearance_for_runtime(&self, runtime: &SessionRuntime) -> TerminalAppearance {
        let profile = self.resolved_terminal_profile_for_runtime(runtime);
        terminal_appearance(&profile, &self.theme_catalog)
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
// Settings → Appearance (inline form)
// ---------------------------------------------------------------------------

/// Editable state of the Settings Appearance section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsAppearanceDraft {
    pub(crate) color_scheme: String,
    pub(crate) font_family: String,
    pub(crate) font_size_text: String,
    /// Single backup font path (TERM-5); the form offers exactly one value.
    pub(crate) backup_font_text: String,
}

impl Default for SettingsAppearanceDraft {
    fn default() -> Self {
        Self::from_terminal_profile(&TerminalProfile::default(), &ThemeCatalog::default())
    }
}

impl SettingsAppearanceDraft {
    /// D15 font presets for the Settings ComboBox: system monospace presets,
    /// bundled DejaVu, plus the current imported file path when needed.
    pub(crate) fn font_options(&self) -> Vec<String> {
        let mut options = appearance_font_preset_labels();
        let family = self.font_family.trim();
        if !family.is_empty()
            && !options
                .iter()
                .any(|option| option.eq_ignore_ascii_case(family))
        {
            options.push(family.to_owned());
        }
        options
    }

    /// Index into [`Self::font_options`] for the current `font_family`.
    pub(crate) fn font_index(&self) -> i32 {
        let options = self.font_options();
        let family = self.font_family.trim();
        options
            .iter()
            .position(|option| option.eq_ignore_ascii_case(family))
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(0)
    }

    pub(crate) fn from_terminal_profile(
        profile: &TerminalProfile,
        catalog: &ThemeCatalog,
    ) -> Self {
        let (font, _) = resolve_font(profile);
        Self {
            color_scheme: profile
                .color_scheme
                .clone()
                .filter(|id| scheme_id_known(id, catalog))
                .unwrap_or_else(|| DEFAULT_COLOR_SCHEME_ID.to_owned()),
            font_family: match font.primary {
                PrimaryFont::BundledDejaVu => BUNDLED_FONT_FAMILY.to_owned(),
                PrimaryFont::Family(faces) => faces.label,
                PrimaryFont::File(path) => path.display().to_string(),
            },
            font_size_text: resolve_font_size(profile.font_size).round().to_string(),
            backup_font_text: profile.backup_font.clone().unwrap_or_default(),
        }
    }

    /// Effective palette for the selected theme id.
    pub(crate) fn palette(&self, catalog: &ThemeCatalog) -> TerminalPalette {
        lookup_scheme_palette(Some(&self.color_scheme), catalog)
    }

    pub(crate) fn font_selection(&self) -> FontSelection {
        FontSelection {
            primary: primary_font_from_name(&self.font_family),
            fallback_files: normalize_backup_font(&self.backup_font_text)
                .map(PathBuf::from)
                .into_iter()
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
    pub(crate) fn theme_fields(
        &self,
        catalog: &ThemeCatalog,
    ) -> Result<SettingsAppearanceTheme, ThemeFieldError> {
        if !scheme_id_known(&self.color_scheme, catalog) {
            return Err(ThemeFieldError::new(
                "appearance.color_scheme",
                "unknown color scheme id",
            ));
        }
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
            let default_label = default_font_family_label();
            if text.is_empty() || text.eq_ignore_ascii_case(&default_label) {
                None
            } else {
                Some(text.to_owned())
            }
        };
        if let Some(name) = font_family.as_deref() {
            if !is_font_preset_label(name) && !Path::new(name).is_file() {
                return Err(ThemeFieldError::new(
                    "appearance.font_family",
                    format!("font file `{name}` was not found"),
                ));
            }
        }
        let backup_font = normalize_backup_font(&self.backup_font_text);
        if let Some(missing) = backup_font
            .as_deref()
            .filter(|path| !Path::new(path).is_file())
        {
            return Err(ThemeFieldError::new(
                "appearance.backup_font",
                format!("font file `{missing}` was not found"),
            ));
        }
        Ok(SettingsAppearanceTheme {
            color_scheme: self.color_scheme.clone(),
            font_family,
            font_size: Some(font_size),
            backup_font,
        })
    }
}

/// Validated theme fields produced by the Settings appearance form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsAppearanceTheme {
    pub(crate) color_scheme: String,
    pub(crate) font_family: Option<String>,
    pub(crate) font_size: Option<u16>,
    pub(crate) backup_font: Option<String>,
}

/// Draft state while editing a user color theme (or previewing a built-in).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ThemeEditorDraft {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) builtin: bool,
    pub(crate) foreground: String,
    pub(crate) background: String,
    pub(crate) cursor: String,
    pub(crate) selection: String,
    pub(crate) ansi: [String; 16],
    pub(crate) dirty: bool,
}

impl ThemeEditorDraft {
    pub(crate) fn from_color_theme(theme: &ColorTheme, builtin: bool) -> Self {
        Self {
            id: theme.id.clone(),
            name: theme.name.clone(),
            builtin,
            foreground: theme.foreground.clone(),
            background: theme.background.clone(),
            cursor: theme.cursor.clone(),
            selection: theme.selection.clone(),
            ansi: theme.ansi.clone(),
            dirty: false,
        }
    }

    pub(crate) fn to_color_theme(&self) -> ColorTheme {
        ColorTheme {
            id: self.id.clone(),
            name: self.name.clone(),
            foreground: self.foreground.clone(),
            background: self.background.clone(),
            cursor: self.cursor.clone(),
            selection: self.selection.clone(),
            ansi: self.ansi.clone(),
        }
    }

    pub(crate) fn palette(&self) -> TerminalPalette {
        palette_from_color_theme(&self.to_color_theme())
    }

    pub(crate) fn set_role_color(&mut self, role: &str, hex: &str) {
        let slot = match role {
            "foreground" => Some(&mut self.foreground),
            "background" => Some(&mut self.background),
            "cursor" => Some(&mut self.cursor),
            "selection" => Some(&mut self.selection),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = hex.to_owned();
            self.dirty = true;
        }
    }

    pub(crate) fn set_ansi_color(&mut self, index: usize, hex: &str) {
        if let Some(slot) = self.ansi.get_mut(index) {
            *slot = hex.to_owned();
            self.dirty = true;
        }
    }
}

/// Status line of the Settings Appearance section.
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
    /// Loads the Appearance draft when Settings opens (inline form + preview).
    pub fn open_settings_appearance(&mut self) -> AppProjection {
        self.settings_appearance = SettingsAppearanceDraft::from_terminal_profile(
            &self.config_document.terminal,
            &self.theme_catalog,
        );
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.settings_appearance_visible = true;
        self.projection()
    }

    pub fn close_settings_appearance(&mut self) -> AppProjection {
        self.settings_appearance_visible = false;
        self.theme_editor = None;
        self.projection()
    }

    /// Picks a named theme by id (built-in or user).
    pub fn select_settings_appearance_scheme(&mut self, id: &str) -> AppProjection {
        if scheme_id_known(id, &self.theme_catalog) {
            self.settings_appearance.color_scheme = id.to_owned();
            self.settings_appearance_status = SettingsAppearanceStatus::Hint;
            if self.theme_manager_visible {
                self.theme_manager_selected_id = Some(id.to_owned());
                self.theme_manager_pending_delete = None;
            }
        }
        self.projection()
    }

    // --- TERM-1: independent Theme Manager (D6 modal page) -----------------

    /// Opens the Theme Manager with the current global default selected.
    pub fn open_theme_manager(&mut self) -> AppProjection {
        self.theme_manager_visible = true;
        self.theme_manager_pending_delete = None;
        self.theme_editor = None;
        self.theme_manager_selected_id = Some(resolved_global_scheme_id(
            &self.config_document.terminal,
            &self.theme_catalog,
        ));
        self.projection()
    }

    /// Closes the manager (discards the editor draft and pending delete).
    pub fn close_theme_manager(&mut self) -> AppProjection {
        self.theme_manager_visible = false;
        self.theme_manager_pending_delete = None;
        self.theme_editor = None;
        self.theme_manager_selected_id = None;
        self.projection()
    }

    /// Row click: select a theme **for the preview only**. The global default
    /// changes only through [`Self::apply_theme_manager_selection`].
    pub fn select_theme_manager_row(&mut self, index: i32) -> AppProjection {
        let default_id = resolved_global_scheme_id(
            &self.config_document.terminal,
            &self.theme_catalog,
        );
        let selected_id = self.theme_manager_selection_id();
        let rows = theme_manager_rows(&self.theme_catalog, &selected_id, &default_id);
        if let Some(row) = usize::try_from(index)
            .ok()
            .and_then(|index| rows.get(index))
        {
            self.theme_manager_selected_id = Some(row.id.clone());
            self.theme_manager_pending_delete = None;
        }
        self.projection()
    }

    /// Applies the selected theme as the global default (writes config).
    ///
    /// Existing sessions keep their captured appearance (D17); only terminals
    /// opened afterwards use the new default.
    pub fn apply_theme_manager_selection(&mut self) -> AppResult<AppProjection> {
        let selected = self.theme_manager_selection_id();
        if !scheme_id_known(&selected, &self.theme_catalog) {
            return Err(AppError::new("selected theme is no longer available"));
        }
        self.config_document.terminal.color_scheme = Some(selected.clone());
        clear_color_overrides(&mut self.config_document.terminal);
        self.settings_appearance.color_scheme = selected.clone();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        let name = scheme_display_name(&selected, &self.theme_catalog);
        self.set_status_kind(
            "terminal-appearance-saved",
            format!("Set `{name}` as the default terminal theme."),
            selected,
            String::new(),
        );
        Ok(self.projection())
    }

    /// Asks for confirmation before deleting a user theme.
    pub fn request_delete_color_theme(&mut self, id: &str) -> AppProjection {
        if self.theme_catalog.contains_user(id) {
            self.theme_manager_pending_delete = Some(id.to_owned());
            self.theme_manager_selected_id = Some(id.to_owned());
        }
        self.projection()
    }

    /// Cancels the pending delete confirmation.
    pub fn cancel_delete_color_theme(&mut self) -> AppProjection {
        self.theme_manager_pending_delete = None;
        self.projection()
    }

    /// Confirms the pending delete (retargets references to the default).
    pub fn confirm_delete_color_theme(&mut self) -> AppResult<AppProjection> {
        let Some(id) = self.theme_manager_pending_delete.clone() else {
            return Ok(self.projection());
        };
        self.theme_manager_pending_delete = None;
        self.delete_color_theme(&id)
    }

    /// Theme selected in the manager (falls back to the global default).
    pub(crate) fn theme_manager_selection_id(&self) -> String {
        self.theme_manager_selected_id
            .clone()
            .filter(|id| scheme_id_known(id, &self.theme_catalog))
            .unwrap_or_else(|| {
                resolved_global_scheme_id(&self.config_document.terminal, &self.theme_catalog)
            })
    }

    /// Palette under the manager preview: editor draft when editing, else the
    /// selected theme (row click preview, no save required).
    pub(crate) fn theme_manager_palette(&self) -> TerminalPalette {
        if let Some(editor) = self.theme_editor.as_ref() {
            return editor.palette();
        }
        lookup_scheme_palette(
            Some(&self.theme_manager_selection_id()),
            &self.theme_catalog,
        )
    }

    /// Clones the selected (or given) theme into an editable user theme.
    pub fn clone_color_theme(&mut self, source_id: &str) -> AppResult<AppProjection> {
        let source = if let Some(user) = self.theme_catalog.get_user(source_id).cloned() {
            user
        } else if let Some(builtin) = builtin_as_color_theme(source_id) {
            builtin
        } else {
            return Err(AppError::new(format!("unknown theme '{source_id}'")));
        };
        let name = format!("{} Copy", source.name);
        let cloned = self
            .theme_catalog
            .clone_theme(&source, &name)
            .map_err(AppError::from_error)?;
        // The manager selects the clone for the editor preview; the Settings
        // draft (and the global default) must stay untouched until the user
        // explicitly saves the theme and clicks "Set as Default".
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.theme_manager_selected_id = Some(cloned.id.clone());
        self.theme_manager_pending_delete = None;
        self.theme_editor = Some(ThemeEditorDraft::from_color_theme(&cloned, false));
        Ok(self.projection())
    }

    /// Opens the theme editor for a user theme (built-ins open read-only).
    pub fn open_theme_editor(&mut self, id: &str) -> AppProjection {
        if let Some(user) = self.theme_catalog.get_user(id).cloned() {
            self.theme_editor = Some(ThemeEditorDraft::from_color_theme(&user, false));
            self.theme_manager_selected_id = Some(id.to_owned());
            self.theme_manager_pending_delete = None;
        } else if let Some(builtin) = builtin_as_color_theme(id) {
            self.theme_editor = Some(ThemeEditorDraft::from_color_theme(&builtin, true));
            self.theme_manager_selected_id = Some(id.to_owned());
            self.theme_manager_pending_delete = None;
        }
        self.projection()
    }

    pub fn close_theme_editor(&mut self) -> AppProjection {
        self.theme_editor = None;
        self.projection()
    }

    pub fn update_theme_editor_name(&mut self, name: &str) -> AppProjection {
        if let Some(editor) = self.theme_editor.as_mut() {
            if !editor.builtin {
                editor.name = name.to_owned();
                editor.dirty = true;
            }
        }
        self.projection()
    }

    pub fn update_theme_editor_role_color(&mut self, role: &str, hex: &str) -> AppProjection {
        if let Some(editor) = self.theme_editor.as_mut() {
            if !editor.builtin {
                editor.set_role_color(role, hex);
            }
        }
        self.projection()
    }

    pub fn update_theme_editor_ansi_color(&mut self, index: i32, hex: &str) -> AppProjection {
        if let Some(editor) = self.theme_editor.as_mut() {
            if !editor.builtin {
                if let Ok(index) = usize::try_from(index) {
                    editor.set_ansi_color(index, hex);
                }
            }
        }
        self.projection()
    }

    /// Saves the theme editor draft (user themes only).
    /// On success, closes the editor (Save = save and close).
    pub fn save_theme_editor(&mut self) -> AppResult<AppProjection> {
        let Some(editor) = self.theme_editor.clone() else {
            return Ok(self.projection());
        };
        if editor.builtin {
            return Err(AppError::new("built-in themes cannot be modified; clone first"));
        }
        let theme = editor.to_color_theme();
        if !theme.colors_valid() {
            return Err(AppError::new("theme has invalid color values"));
        }
        self.theme_catalog
            .save_user(theme.clone())
            .map_err(AppError::from_error)?;
        self.theme_editor = None;
        self.theme_manager_selected_id = Some(theme.id.clone());
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        Ok(self.projection())
    }

    /// Renames a user theme's display name.
    #[allow(dead_code)]
    pub fn rename_color_theme(&mut self, id: &str, new_name: &str) -> AppResult<AppProjection> {
        self.theme_catalog
            .rename_user(id, new_name)
            .map_err(AppError::from_error)?;
        if self.settings_appearance.color_scheme == id {
            // Name-only change; id stays the same.
        }
        if let Some(editor) = self.theme_editor.as_mut() {
            if editor.id == id {
                editor.name = new_name.trim().to_owned();
            }
        }
        Ok(self.projection())
    }

    /// Deletes a user theme and retargets any references to the default.
    pub fn delete_color_theme(&mut self, id: &str) -> AppResult<AppProjection> {
        self.theme_catalog
            .delete_user(id)
            .map_err(AppError::from_error)?;
        retarget_theme_references(&mut self.config_document, id, DEFAULT_COLOR_SCHEME_ID);
        if self.settings_appearance.color_scheme == id {
            self.settings_appearance.color_scheme = DEFAULT_COLOR_SCHEME_ID.to_owned();
        }
        if self.theme_editor.as_ref().is_some_and(|editor| editor.id == id) {
            self.theme_editor = None;
        }
        if self.theme_manager_pending_delete.as_deref() == Some(id) {
            self.theme_manager_pending_delete = None;
        }
        if self.theme_manager_selected_id.as_deref() == Some(id) {
            self.theme_manager_selected_id = Some(resolved_global_scheme_id(
                &self.config_document.terminal,
                &self.theme_catalog,
            ));
        }
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        Ok(self.projection())
    }

    pub fn update_settings_appearance_font_family(&mut self, value: &str) -> AppProjection {
        self.settings_appearance.font_family = value.to_owned();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    /// Selects a font preset from the Settings ComboBox (D15).
    pub fn select_settings_appearance_font(&mut self, index: i32) -> AppProjection {
        let options = self.settings_appearance.font_options();
        if let Ok(index) = usize::try_from(index) {
            if let Some(family) = options.get(index) {
                self.settings_appearance.font_family = family.clone();
                self.settings_appearance_status = SettingsAppearanceStatus::Hint;
            }
        }
        self.projection()
    }

    pub fn update_settings_appearance_font_size(&mut self, value: &str) -> AppProjection {
        self.settings_appearance.font_size_text = value.to_owned();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    /// Steps the Settings font size by `delta` (clamped to the accepted range).
    pub fn bump_settings_appearance_font_size(&mut self, delta: i32) -> AppProjection {
        let current = self
            .settings_appearance
            .font_size()
            .unwrap_or(DEFAULT_FONT_SIZE as u16);
        let next = i32::from(current)
            .saturating_add(delta)
            .clamp(i32::from(THEME_FONT_SIZE_MIN), i32::from(THEME_FONT_SIZE_MAX));
        self.settings_appearance.font_size_text = next.to_string();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    /// Updates the single backup font field (free text; one value only).
    pub fn update_settings_appearance_backup_font(&mut self, value: &str) -> AppProjection {
        self.settings_appearance.backup_font_text = value.to_owned();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    /// Replaces the backup font with one picked file (rfd result). The picker
    /// always overwrites the previous value: exactly one font is allowed.
    pub fn set_settings_appearance_backup_font(&mut self, path: &str) -> AppProjection {
        let path = path.trim();
        if !path.is_empty() {
            self.settings_appearance.backup_font_text = path.to_owned();
        }
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
        self.projection()
    }

    /// Clears the backup font (the bundled CJK face still serves missing glyphs).
    pub fn clear_settings_appearance_backup_font(&mut self) -> AppProjection {
        self.settings_appearance.backup_font_text.clear();
        self.settings_appearance_status = SettingsAppearanceStatus::Hint;
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
        let theme = match self
            .settings_appearance
            .theme_fields(&self.theme_catalog)
        {
            Ok(theme) => theme,
            Err(error) => {
                self.settings_appearance_status = SettingsAppearanceStatus::Rejected(error);
                return Ok(self.projection());
            }
        };
        let terminal = &mut self.config_document.terminal;
        terminal.color_scheme = Some(theme.color_scheme);
        clear_color_overrides(terminal);
        terminal.font_family = theme.font_family;
        terminal.font_size = theme.font_size;
        terminal.backup_font = theme.backup_font;
        // TERM-5: drop the legacy list if it is still around, so a save always
        // writes the single-value field only.
        terminal.legacy_fallback_fonts = None;
        self.config_store
            .save(&self.config_document)
            .map_err(AppError::from_error)?;
        self.settings_appearance_status = SettingsAppearanceStatus::Saved;
        self.settings_appearance = SettingsAppearanceDraft::from_terminal_profile(
            &self.config_document.terminal,
            &self.theme_catalog,
        );
        self.set_status_kind(
            "terminal-appearance-saved",
            "Saved the global terminal appearance defaults.".to_owned(),
            self.settings_appearance.color_scheme.clone(),
            String::new(),
        );
        // Save = save and close Settings.
        self.settings_appearance_visible = false;
        self.theme_editor = None;
        Ok(self.projection())
    }

    /// Palette of the current draft (settings preview / swatches).
    pub(crate) fn settings_appearance_palette(&self) -> TerminalPalette {
        if let Some(editor) = self.theme_editor.as_ref() {
            return editor.palette();
        }
        self.settings_appearance.palette(&self.theme_catalog)
    }

    /// 预览表面的底色（跟随预览配色）。
    ///
    /// 预览位图按该配色的背景渲染；表面用同一个颜色，位图放不下的余量与
    /// 内边距才不会露出主题黑底（浅色配色下的"黑边"）。
    pub(crate) fn settings_appearance_preview_background(&self) -> [u8; 3] {
        preview_background(self.settings_appearance_palette())
    }

    /// Session Editor 外观预览的表面底色（跟随该预览的配色）。
    pub(crate) fn session_editor_preview_background(&self) -> [u8; 3] {
        let resolved = self.editor_theme_resolved_terminal();
        preview_background(resolve_palette(&resolved, &self.theme_catalog))
    }

    /// 主题管理器预览的表面底色（跟随该预览的配色）。
    pub(crate) fn theme_manager_preview_background(&self) -> [u8; 3] {
        preview_background(self.theme_manager_palette())
    }

    /// Device pixel ratio the Settings live preview must rasterize at.
    ///
    /// This is the focused window's `window.scale_factor()` (updated by
    /// `windows.rs` when a window is shown, activated or re-scaled), so the
    /// preview glyph metrics match the live terminal on the same display.
    pub(crate) fn ui_scale_factor(&self) -> f32 {
        preview_scale_factor(self.ui_scale_factor)
    }

    /// Records the focused window's device pixel ratio (TERM-2/TERM-4).
    ///
    /// Invalid values fall back to [`DEFAULT_SCALE_FACTOR`]. A change while a
    /// live preview is visible (Settings Appearance or the Session Editor
    /// Appearance section) re-renders that preview on the next UI refresh.
    pub(crate) fn note_ui_scale_factor(&mut self, scale_factor: f32) {
        let scale_factor = preview_scale_factor(scale_factor);
        if (self.ui_scale_factor - scale_factor).abs() <= f32::EPSILON {
            return;
        }
        self.ui_scale_factor = scale_factor;
        if self.settings_appearance_visible
            || self.theme_manager_visible
            || (self.editor_modal_visible
                && matches!(self.editor_section, EditorSection::Appearance))
        {
            self.mark_ui_dirty();
        }
    }

    /// Renders the live preview frame for the Settings Appearance section.
    ///
    /// Returns `None` while the draft is inactive. The dedicated preview
    /// renderer keeps the live terminal's glyph cache untouched.
    ///
    /// Parity with the live terminal (TERM-2):
    /// * the window's real scale factor is applied, so cell metrics and glyphs
    ///   are rasterized at `font_size * scale_factor` physical pixels;
    /// * the profile's resolved font size is used as-is (no preview-only clamp);
    /// * the grid is floored into [`SETTINGS_PREVIEW_AREA_PX`], so a font that
    ///   does not fit yields fewer rows/columns instead of a stretched bitmap.
    pub(crate) fn settings_appearance_preview_frame(&self) -> Option<TerminalFrame> {
        if !self.settings_appearance_visible {
            return None;
        }
        let palette = self.settings_appearance_palette();
        let font = self.settings_appearance.font_selection();
        let font_size = resolve_font_size(self.settings_appearance.font_size());
        Some(self.settings_preview_renderer.render(
            palette,
            font,
            font_size,
            self.ui_scale_factor(),
            SETTINGS_PREVIEW_AREA_PX,
            self.ui_language(),
        ))
    }

    /// Renders the live preview frame for the Session Editor Appearance section
    /// (TERM-4).
    ///
    /// The frame is produced only while the editor modal is open **and** the
    /// Appearance section is active; closing the editor or switching sections
    /// yields `None`, so the host drops the bitmap and rasterization stops.
    ///
    /// The render path is deliberately the Settings one (and reusable by the
    /// future theme manager, TERM-1):
    /// * the current editor draft is resolved exactly like saving would resolve
    ///   it (inherit → parent, explicit → draft value), so the preview follows
    ///   the combo/font choices immediately;
    /// * the real window DPR is applied and the profile font size is used
    ///   as-is (no preview-only 8–20 clamp);
    /// * the grid is floored into [`SESSION_EDITOR_PREVIEW_AREA_PX`], so a
    ///   large font yields fewer rows/columns instead of a stretched bitmap;
    /// * the localized + CJK sample, block cursor and selection decorations
    ///   come from the same helpers as Settings and the live terminal.
    ///
    /// Cancel/discard never writes the draft, so reopening the editor renders
    /// the persisted theme again.
    pub(crate) fn session_editor_preview_frame(&self) -> Option<TerminalFrame> {
        if !self.editor_modal_visible
            || !matches!(self.editor_section, EditorSection::Appearance)
        {
            return None;
        }
        let resolved = self.editor_theme_resolved_terminal();
        let palette = resolve_palette(&resolved, &self.theme_catalog);
        let (font, _rejected) = resolve_font(&resolved);
        Some(self.editor_preview_renderer.render(
            palette,
            font,
            resolve_font_size(resolved.font_size),
            self.ui_scale_factor(),
            SESSION_EDITOR_PREVIEW_AREA_PX,
            self.ui_language(),
        ))
    }

    /// Renders the Theme Manager live preview (TERM-1).
    ///
    /// Returns `None` while the modal page is closed. The palette comes from the
    /// editor draft while a theme is being edited (so color-picker changes show
    /// immediately) and from the selected row otherwise. The font stack/font
    /// size come from the persisted global terminal profile, because fonts stay
    /// in Settings while the manager only owns color themes.
    pub(crate) fn theme_manager_preview_frame(&self) -> Option<TerminalFrame> {
        if !self.theme_manager_visible {
            return None;
        }
        let profile = &self.config_document.terminal;
        let (font, _rejected) = resolve_font(profile);
        Some(self.theme_manager_preview_renderer.render(
            self.theme_manager_palette(),
            font,
            resolve_font_size(profile.font_size),
            self.ui_scale_factor(),
            THEME_MANAGER_PREVIEW_AREA_PX,
            self.ui_language(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Settings live preview geometry (TERM-2)
// ---------------------------------------------------------------------------

/// Logical-pixel bitmap area of the Settings live preview surface.
///
/// Mirrors `ui/pages/settings.slint`: the preview surface spans the Settings
/// card content width (434 px = 760 dialog − 40 body padding − 240 navigation −
/// 12 gap − 2 card border − 32 card padding) and is 200 px tall; subtracting
/// the surface border (1 px) and the terminal horizontal padding
/// (`Theme.terminal-padding-x`, 6 px per side) leaves 420 × 198 logical pixels.
///
/// The renderer **floors** the grid into this area exactly like the live
/// terminal floors its PTY grid: when a font/DPI combination does not fit, the
/// preview drops columns/rows and keeps the bitmap at 1:1 physical pixels; the
/// remainder stays as surface background (never a stretched image).
pub(crate) const SETTINGS_PREVIEW_AREA_PX: (f32, f32) = (420.0, 198.0);

/// Logical-pixel bitmap area of the Session Editor live preview surface.
///
/// Mirrors `ui/pages/session_editor.slint`: the Session Editor card content is
/// 760 dialog − 40 body padding − 180 navigation − 12 gap − 2 card border −
/// 32 card padding = 494 px wide, and the fixed preview surface is 100 px tall.
/// The surface border (1 px) and `Theme.terminal-padding-x` (6 px per side)
/// leave this 480 × 98 px bitmap area.
///
/// The renderer **floors** the grid into this area exactly like Settings and
/// the live terminal: a font/DPI combination that does not fit drops
/// columns/rows and keeps the bitmap at 1:1 physical pixels (never a stretched
/// image).
pub(crate) const SESSION_EDITOR_PREVIEW_AREA_PX: (f32, f32) = (480.0, 98.0);

/// Logical-pixel bitmap area of the Theme Manager live preview surface.
///
/// Mirrors `ui/pages/theme_manager.slint`: the dialog is 800 px wide with 20 px
/// body padding, so the surface is 760 px; the surface border (1 px) and
/// `Theme.terminal-padding-x` (6 px per side) leave 746 px. The fixed surface
/// height is 120 px, leaving 118 px for the bitmap (`terminal-padding-y` = 0).
///
/// Like the other previews the grid is **floored** into this area: a font/DPI
/// combination that does not fit drops columns/rows and keeps the bitmap at
/// 1:1 physical pixels (never a stretched image).
pub(crate) const THEME_MANAGER_PREVIEW_AREA_PX: (f32, f32) = (746.0, 118.0);

/// Dedicated renderer for one live-preview surface.
///
/// Settings, the Session Editor (TERM-4) and the upcoming theme manager
/// (TERM-1) each own one instance, so preview rasterization never touches the
/// live terminal's glyph cache or renderer state. The inner renderer is
/// created lazily and only re-measures cells when the DPR actually changed.
pub(crate) struct LivePreviewRenderer {
    renderer: RefCell<Option<TerminalRenderer>>,
}

impl LivePreviewRenderer {
    pub(crate) fn new() -> Self {
        Self {
            renderer: RefCell::new(None),
        }
    }

    /// Renders the shared live-preview sample into `area_logical` at 1:1
    /// physical pixels for `scale_factor`.
    ///
    /// `area_logical` is the bitmap area in Slint logical pixels; the grid is
    /// floored after converting to physical pixels with the renderer's cell
    /// metrics (which already include `font_size * scale_factor`).
    pub(crate) fn render(
        &self,
        palette: TerminalPalette,
        font: FontSelection,
        font_size: f32,
        scale_factor: f32,
        area_logical: (f32, f32),
        language: &str,
    ) -> TerminalFrame {
        let scale_factor = preview_scale_factor(scale_factor);
        let mut renderer = self.renderer.borrow_mut();
        let renderer = renderer.get_or_insert_with(TerminalRenderer::new);
        // `set_scale_factor` always re-measures; only pay for it when the DPR
        // actually changed (moving the window between monitors).
        if (renderer.scale_factor() - scale_factor).abs() > f32::EPSILON {
            renderer.set_scale_factor(scale_factor);
        }
        renderer.set_font_size(font_size);
        renderer.apply_appearance(palette, font);
        let (columns, rows) = preview_grid_size(area_logical, scale_factor, renderer.cell_size());
        let content = preview_content(palette, language, columns, rows);
        let snapshot = TerminalSnapshot {
            lines: content
                .lines
                .iter()
                .map(|line| std::borrow::Cow::Borrowed(line.as_slice()))
                .collect(),
            cursor: content.cursor,
            selection: content.selection,
        };
        renderer.render(&snapshot)
    }
}

impl Default for LivePreviewRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Sanitizes a device pixel ratio for preview rasterization.
fn preview_scale_factor(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        DEFAULT_SCALE_FACTOR
    }
}

/// Columns/rows of the preview bitmap that fit `area_logical` at `scale_factor`.
///
/// `cell_size` is already in physical pixels (the renderer measures it at
/// `font_size * scale_factor`), so the area is converted to physical pixels
/// first and the quotient is floored. Always returns at least one cell so the
/// bitmap stays non-empty (the surface clips any overflow).
pub(crate) fn preview_grid_size(
    area_logical: (f32, f32),
    scale_factor: f32,
    cell_size: (u32, u32),
) -> (u16, u16) {
    let scale_factor = preview_scale_factor(scale_factor);
    let physical_width = area_logical.0.max(0.0) * scale_factor;
    let physical_height = area_logical.1.max(0.0) * scale_factor;
    let cell_width = cell_size.0.max(1);
    let cell_height = cell_size.1.max(1);
    let columns = (physical_width.floor() as u32 / cell_width).max(1);
    let rows = (physical_height.floor() as u32 / cell_height).max(1);
    (
        u16::try_from(columns).unwrap_or(u16::MAX),
        u16::try_from(rows).unwrap_or(u16::MAX),
    )
}

/// One fitted preview: viewport rows plus the cursor/selection decorations the
/// real terminal renderer paints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewContent {
    pub(crate) lines: Vec<Vec<TerminalCell>>,
    /// Block cursor at the end of the sample prompt (viewport coordinates).
    pub(crate) cursor: Option<(u16, u16)>,
    /// Highlight over the sample's “selection row” (viewport coordinates).
    pub(crate) selection: Option<SelectionRange>,
}

/// Rewrites every `color_scheme` reference from `from_id` to `to_id`.
fn retarget_theme_references(document: &mut ConfigDocument, from_id: &str, to_id: &str) {
    let retarget = |profile: &mut TerminalProfile| {
        if profile.color_scheme.as_deref() == Some(from_id) {
            profile.color_scheme = Some(to_id.to_owned());
        }
    };
    retarget(&mut document.terminal);
    fn walk(folders: &mut [FolderProfile], from_id: &str, to_id: &str) {
        for folder in folders {
            if let Some(terminal) = folder.terminal.as_mut() {
                if terminal.color_scheme.as_deref() == Some(from_id) {
                    terminal.color_scheme = Some(to_id.to_owned());
                }
            }
            for session in &mut folder.sessions {
                if let Some(terminal) = session.terminal.as_mut() {
                    if terminal.color_scheme.as_deref() == Some(from_id) {
                        terminal.color_scheme = Some(to_id.to_owned());
                    }
                }
            }
            walk(&mut folder.folders, from_id, to_id);
        }
    }
    walk(&mut document.folders, from_id, to_id);
}

/// Resolves the live-preview sample language from the configured UI language
/// (`ui.language`): explicit `zh-CN`/`en-US` win, `system` follows the OS
/// locale, and anything unknown falls back to `en-US`.
///
/// Explicit preferences are resolved without touching any global state so the
/// preview samples stay deterministic in tests; only `system` consults
/// `sys-locale`, mirroring [`crate::bootstrap::apply_language`].
fn preview_sample_language(preference: &str) -> &'static str {
    match preference.trim().to_ascii_lowercase().as_str() {
        "zh-cn" | "zh" | "zh-hans" => "zh-CN",
        "en-us" | "en" => "en-US",
        _ => {
            let locale = sys_locale::get_locale().unwrap_or_default();
            if locale.to_ascii_lowercase().starts_with("zh") {
                "zh-CN"
            } else {
                "en-US"
            }
        }
    }
}

/// Localized sample line plus the fixed CJK fallback line.
///
/// The CJK line is always present (also in English UI) so the primary/backup
/// font rendering can be inspected without changing the UI language.
fn preview_language_lines(language: &str) -> [&'static str; 2] {
    match language {
        "zh-CN" => ["你好，世界 · 终端主题预览", "CJK fallback: 你好，世界"],
        _ => ["Hello, world · theme preview", "CJK fallback: 你好，世界"],
    }
}

/// Terminal display width of a preview character.
///
/// The sample is fixed, so this only needs to cover the CJK ideographs and
/// full-width punctuation used below. Wide characters are expanded to a lead
/// cell followed by a `wide_continuation` cell, matching the terminal grid
/// convention the renderer expects.
pub(crate) fn preview_char_width(character: char) -> usize {
    match character as u32 {
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// Sample text of the live preview window (colors taken from the palette).
///
/// `language` is the configured UI language preference (`system` / `zh-CN` /
/// `en-US`). The localized line and the fixed CJK fallback line are appended
/// after the existing rows so the cursor row stays on the prompt.
///
/// The prompt ends with a trailing blank cell: the renderer paints the block
/// cursor on that cell, mirroring a real prompt whose command has been typed
/// (the cursor sits after the text, never past the line's last cell).
pub(crate) fn preview_lines(palette: TerminalPalette, language: &str) -> Vec<Vec<TerminalCell>> {
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
        let mut cells = Vec::with_capacity(text.chars().count());
        for character in text.chars() {
            cells.push(cell(&character.to_string(), foreground, false));
            if preview_char_width(character) >= 2 {
                let mut continuation = cell(" ", foreground, false);
                continuation.wide_continuation = true;
                cells.push(continuation);
            }
        }
        cells
    };
    let mut prompt = line("user@host:~$ ", palette.ansi(2));
    prompt.extend(line("ls --color", palette.foreground));
    // The blank cell under the block cursor (column = prompt.len() - 1).
    prompt.extend(line(" ", palette.foreground));
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
    let [language_line, cjk_line] = preview_language_lines(preview_sample_language(language));
    vec![
        line("YShell terminal theme preview", palette.foreground),
        prompt,
        listing,
        ansi,
        selection,
        status,
        line(language_line, palette.foreground),
        line(cjk_line, palette.foreground),
    ]
}

/// Row of the sample prompt (and its block cursor) inside [`preview_lines`].
const PREVIEW_PROMPT_ROW: usize = 1;
/// Row of the sample selection inside [`preview_lines`].
const PREVIEW_SELECTION_ROW: usize = 4;

/// Fits the preview sample into a `columns × rows` terminal viewport.
///
/// The grid comes from [`preview_grid_size`]: when the preview surface cannot
/// hold the whole sample (large font / high DPI), rows are dropped from the
/// middle while the localized line and the fixed CJK fallback line (TERM-3)
/// stay visible, and each line is truncated at the last full cell (a wide CJK
/// pair is never split). The result is padded to the full viewport, exactly
/// like a live terminal frame; the bitmap is never scaled.
pub(crate) fn preview_content(
    palette: TerminalPalette,
    language: &str,
    columns: u16,
    rows: u16,
) -> PreviewContent {
    let columns = usize::from(columns.max(1));
    let rows = usize::from(rows.max(1));
    let sample = preview_lines(palette, language);
    let total = sample.len();
    let kept: Vec<usize> = if total <= rows {
        (0..total).collect()
    } else if rows <= 1 {
        // Degenerate box: keep one informative row (the CJK fallback sample).
        vec![total.saturating_sub(1)]
    } else {
        let mut kept: Vec<usize> = (0..rows - 2).collect();
        kept.extend(total.saturating_sub(2)..total);
        kept
    };
    let mut lines: Vec<Vec<TerminalCell>> = kept
        .iter()
        .map(|index| fit_preview_line(&sample[*index], columns))
        .collect();
    // Pad the viewport (missing rows are terminal background).
    while lines.len() < rows {
        lines.push(vec![TerminalCell::default(); columns]);
    }
    let fitted_row = |sample_row: usize| -> Option<u16> {
        kept.iter()
            .position(|index| *index == sample_row)
            .and_then(|row| u16::try_from(row).ok())
    };
    // Block cursor on the prompt's trailing blank cell (same cell model as the
    // live terminal cursor: never beyond the last cell of the row).
    let cursor = fitted_row(PREVIEW_PROMPT_ROW).and_then(|row| {
        let column = sample
            .get(PREVIEW_PROMPT_ROW)
            .and_then(|line| line.len().checked_sub(1))
            .and_then(|column| u16::try_from(column).ok())?;
        (usize::from(column) < columns).then_some((column, row))
    });
    // Selection highlight on the sample's “selection row”, using the renderer's
    // `SelectionRange` (the same type/painting path as the live terminal).
    let selection = fitted_row(PREVIEW_SELECTION_ROW).and_then(|row| {
        let width = sample
            .get(PREVIEW_SELECTION_ROW)
            .map(|line| line.len().min(columns))
            .unwrap_or(0);
        let end = u16::try_from(width.checked_sub(1)?).ok()?;
        Some(SelectionRange {
            start: GridPoint { column: 0, row },
            end: GridPoint { column: end, row },
        })
    });
    PreviewContent {
        lines,
        cursor,
        selection,
    }
}

/// Truncates/pads one sample line to `columns` cells without splitting a wide
/// (two-cell) glyph at the right edge.
fn fit_preview_line(line: &[TerminalCell], columns: usize) -> Vec<TerminalCell> {
    let end = line.len().min(columns);
    let mut cells = line[..end].to_vec();
    // A wide lead cell needs its continuation cell; when the truncation cut the
    // pair in half, drop the lead instead of painting half a glyph.
    if end < line.len()
        && cells.last().is_some_and(|cell| {
            !cell.wide_continuation
                && preview_char_width(cell.grapheme.chars().next().unwrap_or(' ')) >= 2
        })
    {
        cells.pop();
    }
    cells.resize(columns, TerminalCell::default());
    cells
}
