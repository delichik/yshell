//! Named terminal color themes (Xshell-style color schemes).
//!
//! Built-in schemes are shipped as TOML next to the binary (`themes/`) or, in
//! development, under the repo `assets/themes/` directory. User themes are
//! TOML files under `{config_dir}/themes/<id>.toml`. Sessions, folders, and
//! Settings reference a theme by [`ColorTheme::id`] only.

use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Directory name for both shipped and user theme folders.
pub const THEMES_DIR_NAME: &str = "themes";

/// Stable id of the synthetic fallback theme injected when no shipped themes load.
pub const DEFAULT_THEME_ID: &str = "yshell-default";

/// One color theme document (user-persisted or a clone candidate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorTheme {
    /// Stable id stored in `TerminalProfile::color_scheme`.
    pub id: String,
    /// Display name for the theme picker / editor.
    pub name: String,
    /// Default foreground (`#RRGGBB`).
    pub foreground: String,
    /// Default background (`#RRGGBB`).
    pub background: String,
    /// Cursor color (`#RRGGBB`).
    pub cursor: String,
    /// Selection highlight (`#RRGGBB`).
    pub selection: String,
    /// The 16 ANSI palette entries as `#RRGGBB` strings.
    pub ansi: [String; 16],
}

impl ColorTheme {
    /// Whether every color field is a valid `#RRGGBB` (or `RRGGBB`) hex string.
    #[must_use]
    pub fn colors_valid(&self) -> bool {
        is_hex_color(&self.foreground)
            && is_hex_color(&self.background)
            && is_hex_color(&self.cursor)
            && is_hex_color(&self.selection)
            && self.ansi.iter().all(|value| is_hex_color(value))
    }

    /// Colors matching `yshell-terminal`'s `TerminalPalette::DEFAULT`.
    #[must_use]
    pub fn yshell_default() -> Self {
        Self {
            id: DEFAULT_THEME_ID.to_owned(),
            name: "YShell Default".to_owned(),
            foreground: "#d8dee9".to_owned(),
            background: "#05070b".to_owned(),
            cursor: "#d8dee9".to_owned(),
            selection: "#264f78".to_owned(),
            ansi: [
                "#000000".to_owned(),
                "#cd3131".to_owned(),
                "#0dbc79".to_owned(),
                "#e5e510".to_owned(),
                "#2472c8".to_owned(),
                "#bc3fbc".to_owned(),
                "#11a8cd".to_owned(),
                "#e5e5e5".to_owned(),
                "#666666".to_owned(),
                "#f14c4c".to_owned(),
                "#23d18b".to_owned(),
                "#f5f543".to_owned(),
                "#3b8eea".to_owned(),
                "#d670d6".to_owned(),
                "#29b8db".to_owned(),
                "#ffffff".to_owned(),
            ],
        }
    }
}

/// In-memory catalog of shipped (read-only) and user themes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeCatalog {
    themes_dir: PathBuf,
    shipped_dir: PathBuf,
    builtin: BTreeMap<String, ColorTheme>,
    user: BTreeMap<String, ColorTheme>,
}

impl Default for ThemeCatalog {
    fn default() -> Self {
        let (catalog, _) = Self::load_shipped(resolve_shipped_themes_dir());
        catalog
    }
}

/// Errors from loading or saving user themes.
#[derive(Debug)]
pub enum ThemeCatalogError {
    Io(io::Error),
    Parse { path: PathBuf, message: String },
    Serialize(toml::ser::Error),
    ReservedId(String),
    NotFound(String),
    InvalidColors(String),
}

impl std::fmt::Display for ThemeCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "theme catalog I/O error: {error}"),
            Self::Parse { path, message } => {
                write!(f, "failed to parse theme {}: {message}", path.display())
            }
            Self::Serialize(error) => write!(f, "theme serialization error: {error}"),
            Self::ReservedId(id) => write!(f, "cannot mutate built-in theme '{id}'"),
            Self::NotFound(id) => write!(f, "theme '{id}' not found"),
            Self::InvalidColors(id) => write!(f, "theme '{id}' has invalid color values"),
        }
    }
}

impl std::error::Error for ThemeCatalogError {}

impl From<io::Error> for ThemeCatalogError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl ThemeCatalog {
    /// Empty catalog rooted at `themes_dir` (no shipped themes yet).
    #[must_use]
    pub fn new(themes_dir: impl Into<PathBuf>) -> Self {
        Self {
            themes_dir: themes_dir.into(),
            shipped_dir: PathBuf::new(),
            builtin: BTreeMap::new(),
            user: BTreeMap::new(),
        }
    }

    /// Loads shipped themes from `shipped_dir`, injecting `yshell-default` if needed.
    pub fn load_shipped(shipped_dir: impl Into<PathBuf>) -> (Self, Vec<String>) {
        let shipped_dir = shipped_dir.into();
        let mut catalog = Self {
            themes_dir: PathBuf::new(),
            shipped_dir: shipped_dir.clone(),
            builtin: BTreeMap::new(),
            user: BTreeMap::new(),
        };
        let mut warnings = Vec::new();
        load_theme_dir_into(&shipped_dir, &mut catalog.builtin, &mut warnings, None);
        ensure_default_builtin(&mut catalog.builtin, &mut warnings);
        (catalog, warnings)
    }

    /// Loads shipped themes plus every `*.toml` under `{config_root}/themes/`.
    ///
    /// Invalid user files are skipped (returned in the warning list) so a single
    /// corrupt theme does not block startup. User files whose id collides with a
    /// shipped theme are skipped.
    pub fn load(config_root: &Path) -> (Self, Vec<String>) {
        let (mut catalog, mut warnings) = Self::load_shipped(resolve_shipped_themes_dir());
        catalog.themes_dir = config_root.join(THEMES_DIR_NAME);
        let Ok(entries) = fs::read_dir(&catalog.themes_dir) else {
            return (catalog, warnings);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            match load_theme_file(&path) {
                Ok(theme) => {
                    if catalog.builtin.contains_key(&theme.id) {
                        warnings.push(format!(
                            "skipped user theme file {} with reserved id '{}'",
                            path.display(),
                            theme.id
                        ));
                        continue;
                    }
                    if !theme.colors_valid() {
                        warnings.push(format!(
                            "skipped user theme file {} with invalid colors",
                            path.display()
                        ));
                        continue;
                    }
                    catalog.user.insert(theme.id.clone(), theme);
                }
                Err(message) => warnings.push(message),
            }
        }
        (catalog, warnings)
    }

    /// Directory that holds user theme TOML files.
    #[must_use]
    pub fn themes_dir(&self) -> &Path {
        &self.themes_dir
    }

    /// Directory that held the shipped theme files for this catalog.
    #[must_use]
    pub fn shipped_dir(&self) -> &Path {
        &self.shipped_dir
    }

    /// All shipped themes, sorted by id.
    pub fn builtin_themes(&self) -> impl Iterator<Item = &ColorTheme> {
        self.builtin.values()
    }

    /// Number of shipped themes.
    #[must_use]
    pub fn builtin_count(&self) -> usize {
        self.builtin.len()
    }

    /// All user themes, sorted by id.
    pub fn user_themes(&self) -> impl Iterator<Item = &ColorTheme> {
        self.user.values()
    }

    /// Looks up a user theme by id.
    #[must_use]
    pub fn get_user(&self, id: &str) -> Option<&ColorTheme> {
        self.user.get(id)
    }

    /// Looks up a shipped theme by id.
    #[must_use]
    pub fn get_builtin(&self, id: &str) -> Option<&ColorTheme> {
        self.builtin.get(id)
    }

    /// Looks up user first, then shipped.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ColorTheme> {
        self.user.get(id).or_else(|| self.builtin.get(id))
    }

    /// Whether `id` is a shipped (immutable) scheme id in this catalog.
    #[must_use]
    pub fn is_builtin(&self, id: &str) -> bool {
        self.builtin.contains_key(id)
    }

    /// Whether a user theme with this id exists.
    #[must_use]
    pub fn contains_user(&self, id: &str) -> bool {
        self.user.contains_key(id)
    }

    /// Whether any theme (user or shipped) has this id.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.contains_user(id) || self.is_builtin(id)
    }

    /// Inserts or updates a user theme and writes it to disk.
    pub fn save_user(&mut self, theme: ColorTheme) -> Result<(), ThemeCatalogError> {
        if self.builtin.contains_key(&theme.id) {
            return Err(ThemeCatalogError::ReservedId(theme.id));
        }
        if !theme.colors_valid() {
            return Err(ThemeCatalogError::InvalidColors(theme.id));
        }
        fs::create_dir_all(&self.themes_dir)?;
        let path = self.theme_path(&theme.id);
        let tmp = path.with_extension("toml.tmp");
        let body = toml::to_string_pretty(&theme).map_err(ThemeCatalogError::Serialize)?;
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &path)?;
        self.user.insert(theme.id.clone(), theme);
        Ok(())
    }

    /// Renames a user theme's display name (id stays stable).
    pub fn rename_user(&mut self, id: &str, new_name: &str) -> Result<(), ThemeCatalogError> {
        if self.builtin.contains_key(id) {
            return Err(ThemeCatalogError::ReservedId(id.to_owned()));
        }
        let Some(mut theme) = self.user.get(id).cloned() else {
            return Err(ThemeCatalogError::NotFound(id.to_owned()));
        };
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(ThemeCatalogError::InvalidColors(id.to_owned()));
        }
        theme.name = trimmed.to_owned();
        self.save_user(theme)
    }

    /// Deletes a user theme file and removes it from the catalog.
    pub fn delete_user(&mut self, id: &str) -> Result<(), ThemeCatalogError> {
        if self.builtin.contains_key(id) {
            return Err(ThemeCatalogError::ReservedId(id.to_owned()));
        }
        if !self.user.contains_key(id) {
            return Err(ThemeCatalogError::NotFound(id.to_owned()));
        }
        let path = self.theme_path(id);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        self.user.remove(id);
        Ok(())
    }

    /// Clones any theme into a new user theme with a fresh id.
    pub fn clone_theme(
        &mut self,
        source: &ColorTheme,
        new_name: &str,
    ) -> Result<ColorTheme, ThemeCatalogError> {
        let name = {
            let trimmed = new_name.trim();
            if trimmed.is_empty() {
                format!("{} Copy", source.name)
            } else {
                trimmed.to_owned()
            }
        };
        let theme = ColorTheme {
            id: allocate_user_theme_id(&self.user, &self.builtin),
            name,
            foreground: source.foreground.clone(),
            background: source.background.clone(),
            cursor: source.cursor.clone(),
            selection: source.selection.clone(),
            ansi: source.ansi.clone(),
        };
        self.save_user(theme.clone())?;
        Ok(theme)
    }

    fn theme_path(&self, id: &str) -> PathBuf {
        self.themes_dir.join(format!("{id}.toml"))
    }
}

/// Resolves the directory that holds shipped theme TOML files.
///
/// Order: `<exe_dir>/themes/`, then Linux `/usr/share/yshell/themes/`, then the
/// repo `assets/themes/` fallback used by `cargo run` / tests.
#[must_use]
pub fn resolve_shipped_themes_dir() -> PathBuf {
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            let beside = parent.join(THEMES_DIR_NAME);
            if beside.is_dir() {
                return beside;
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let system = PathBuf::from("/usr/share/yshell/themes");
        if system.is_dir() {
            return system;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes")
}

/// Allocates a unique `user-<millis>-<n>` id that is not reserved and not in use.
#[must_use]
pub fn allocate_user_theme_id(
    existing: &BTreeMap<String, ColorTheme>,
    builtin: &BTreeMap<String, ColorTheme>,
) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    for suffix in 0u32..10_000 {
        let id = format!("user-{millis}-{suffix}");
        if !builtin.contains_key(&id) && !existing.contains_key(&id) {
            return id;
        }
    }
    format!("user-{millis}-x")
}

fn ensure_default_builtin(builtin: &mut BTreeMap<String, ColorTheme>, warnings: &mut Vec<String>) {
    if builtin.contains_key(DEFAULT_THEME_ID) {
        return;
    }
    if builtin.is_empty() {
        warnings.push(format!(
            "no shipped themes found; injecting synthetic {DEFAULT_THEME_ID}"
        ));
    } else {
        warnings.push(format!(
            "shipped themes missing {DEFAULT_THEME_ID}; injecting synthetic default"
        ));
    }
    builtin.insert(DEFAULT_THEME_ID.to_owned(), ColorTheme::yshell_default());
}

fn load_theme_dir_into(
    dir: &Path,
    into: &mut BTreeMap<String, ColorTheme>,
    warnings: &mut Vec<String>,
    skip_ids: Option<&BTreeMap<String, ColorTheme>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        match load_theme_file(&path) {
            Ok(theme) => {
                if skip_ids.is_some_and(|ids| ids.contains_key(&theme.id)) {
                    warnings.push(format!(
                        "skipped theme file {} with reserved id '{}'",
                        path.display(),
                        theme.id
                    ));
                    continue;
                }
                if !theme.colors_valid() {
                    warnings.push(format!(
                        "skipped theme file {} with invalid colors",
                        path.display()
                    ));
                    continue;
                }
                into.insert(theme.id.clone(), theme);
            }
            Err(message) => warnings.push(message),
        }
    }
}

fn load_theme_file(path: &Path) -> Result<ColorTheme, String> {
    let input = fs::read_to_string(path)
        .map_err(|error| format!("failed to read theme {}: {error}", path.display()))?;
    let theme: ColorTheme = toml::from_str(&input)
        .map_err(|error| format!("failed to parse theme {}: {error}", path.display()))?;
    if theme.id.trim().is_empty() {
        return Err(format!("theme file {} has an empty id", path.display()));
    }
    Ok(theme)
}

fn is_hex_color(value: &str) -> bool {
    let text = value.trim();
    let digits = text.strip_prefix('#').unwrap_or(text);
    digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_theme(id: &str, name: &str) -> ColorTheme {
        ColorTheme {
            id: id.to_owned(),
            name: name.to_owned(),
            foreground: "#f8f8f2".to_owned(),
            background: "#282a36".to_owned(),
            cursor: "#f8f8f2".to_owned(),
            selection: "#44475a".to_owned(),
            ansi: std::array::from_fn(|index| format!("#{index:02x}{index:02x}{index:02x}")),
        }
    }

    #[test]
    fn save_load_round_trip() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = ThemeCatalog::new(temp.path().join("themes"));
        let theme = sample_theme("user-1", "Mine");
        catalog.save_user(theme.clone()).expect("save");
        let (reloaded, warnings) = ThemeCatalog::load(temp.path());
        assert!(warnings.is_empty() || warnings.iter().all(|w| w.contains("injecting")));
        assert_eq!(reloaded.get_user("user-1"), Some(&theme));
        assert!(reloaded.is_builtin(DEFAULT_THEME_ID));
    }

    #[test]
    fn cannot_save_or_delete_builtin_id() {
        let (mut catalog, _) = ThemeCatalog::load_shipped(resolve_shipped_themes_dir());
        catalog.themes_dir = TempDir::new().expect("tempdir").path().join("themes");
        assert!(catalog.is_builtin("dracula"));
        let err = catalog
            .save_user(sample_theme("dracula", "Nope"))
            .expect_err("reserved");
        assert!(matches!(err, ThemeCatalogError::ReservedId(_)));
        let err = catalog.delete_user("nord").expect_err("reserved");
        assert!(matches!(err, ThemeCatalogError::ReservedId(_)));
    }

    #[test]
    fn clone_allocates_fresh_user_id() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = ThemeCatalog::new(temp.path().join("themes"));
        let source = sample_theme("dracula", "Dracula");
        let cloned = catalog.clone_theme(&source, "My Dracula").expect("clone");
        assert!(cloned.id.starts_with("user-"));
        assert_eq!(cloned.name, "My Dracula");
        assert_eq!(cloned.foreground, source.foreground);
        assert!(catalog.contains_user(&cloned.id));
    }

    #[test]
    fn rename_and_delete_user_theme() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = ThemeCatalog::new(temp.path().join("themes"));
        catalog
            .save_user(sample_theme("user-a", "Old"))
            .expect("save");
        catalog.rename_user("user-a", "New").expect("rename");
        assert_eq!(
            catalog.get_user("user-a").map(|t| t.name.as_str()),
            Some("New")
        );
        catalog.delete_user("user-a").expect("delete");
        assert!(!catalog.contains_user("user-a"));
        assert!(!temp.path().join("themes/user-a.toml").exists());
    }

    #[test]
    fn shipped_assets_include_legacy_and_xshell_schemes() {
        let (catalog, warnings) = ThemeCatalog::load_shipped(resolve_shipped_themes_dir());
        assert!(warnings.is_empty(), "shipped theme warnings: {warnings:?}");
        assert!(
            catalog.builtin_count() >= 28,
            "expected >= 28 shipped themes, got {}",
            catalog.builtin_count()
        );
        let afterglow = catalog.get_builtin("afterglow").expect("afterglow");
        assert_eq!(afterglow.background, "#212121");
        assert_eq!(afterglow.foreground, "#d0d0d0");
        let dracula = catalog.get_builtin("dracula").expect("dracula");
        assert_eq!(dracula.name, "Dracula");
        assert!(catalog.is_builtin(DEFAULT_THEME_ID));
    }

    #[test]
    fn user_file_with_builtin_id_is_skipped() {
        let temp = TempDir::new().expect("tempdir");
        let themes = temp.path().join("themes");
        fs::create_dir_all(&themes).expect("mkdir");
        let body = toml::to_string_pretty(&sample_theme("dracula", "Hijack")).expect("ser");
        fs::write(themes.join("dracula.toml"), body).expect("write");
        let (catalog, warnings) = ThemeCatalog::load(temp.path());
        assert!(catalog.get_user("dracula").is_none());
        assert!(catalog.is_builtin("dracula"));
        assert!(warnings.iter().any(|w| w.contains("reserved id")));
    }

    #[test]
    fn empty_shipped_dir_injects_default() {
        let temp = TempDir::new().expect("tempdir");
        let (catalog, warnings) = ThemeCatalog::load_shipped(temp.path());
        assert_eq!(catalog.builtin_count(), 1);
        assert_eq!(
            catalog.get_builtin(DEFAULT_THEME_ID),
            Some(&ColorTheme::yshell_default())
        );
        assert!(warnings.iter().any(|w| w.contains("injecting")));
    }
}
