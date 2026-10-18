//! Named terminal color themes (Xshell-style color schemes).
//!
//! Built-in schemes live in `yshell-terminal` as compile-time constants. User
//! themes are TOML files under `{config_dir}/themes/<id>.toml`. Sessions,
//! folders, and Settings reference a theme by [`ColorTheme::id`] only.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Directory name under the config root that holds user theme files.
pub const THEMES_DIR_NAME: &str = "themes";

/// Built-in scheme ids that must never be overwritten or deleted as user themes.
pub const BUILTIN_SCHEME_IDS: &[&str] = &[
    "yshell-default",
    "dracula",
    "nord",
    "gruvbox-dark",
    "solarized-dark",
    "solarized-light",
    "one-dark",
    "tokyo-night",
];

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
}

/// In-memory catalog of user themes plus helpers for reserved built-in ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeCatalog {
    themes_dir: PathBuf,
    user: BTreeMap<String, ColorTheme>,
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
    /// Empty catalog rooted at `themes_dir` (directory need not exist yet).
    #[must_use]
    pub fn new(themes_dir: impl Into<PathBuf>) -> Self {
        Self {
            themes_dir: themes_dir.into(),
            user: BTreeMap::new(),
        }
    }

    /// Loads every `*.toml` file from `{config_root}/themes/`.
    ///
    /// Invalid files are skipped (returned in the warning list) so a single
    /// corrupt theme does not block startup.
    pub fn load(config_root: &Path) -> (Self, Vec<String>) {
        let themes_dir = config_root.join(THEMES_DIR_NAME);
        let mut catalog = Self::new(themes_dir.clone());
        let mut warnings = Vec::new();
        let Ok(entries) = fs::read_dir(&themes_dir) else {
            return (catalog, warnings);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            match load_theme_file(&path) {
                Ok(theme) => {
                    if is_builtin_scheme_id(&theme.id) {
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

    /// All user themes, sorted by id.
    pub fn user_themes(&self) -> impl Iterator<Item = &ColorTheme> {
        self.user.values()
    }

    /// Looks up a user theme by id.
    #[must_use]
    pub fn get_user(&self, id: &str) -> Option<&ColorTheme> {
        self.user.get(id)
    }

    /// Whether `id` is a built-in (immutable) scheme id.
    #[must_use]
    pub fn is_builtin(id: &str) -> bool {
        is_builtin_scheme_id(id)
    }

    /// Whether a user theme with this id exists.
    #[must_use]
    pub fn contains_user(&self, id: &str) -> bool {
        self.user.contains_key(id)
    }

    /// Inserts or updates a user theme and writes it to disk.
    pub fn save_user(&mut self, theme: ColorTheme) -> Result<(), ThemeCatalogError> {
        if is_builtin_scheme_id(&theme.id) {
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
        if is_builtin_scheme_id(id) {
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
        if is_builtin_scheme_id(id) {
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

    /// Clones any theme (built-in palette provided by the caller, or an existing
    /// user theme) into a new user theme with a fresh id.
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
            id: allocate_user_theme_id(&self.user),
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

/// Whether `id` collides with a built-in color scheme.
#[must_use]
pub fn is_builtin_scheme_id(id: &str) -> bool {
    BUILTIN_SCHEME_IDS.contains(&id)
}

/// Allocates a unique `user-<millis>-<n>` id that is not reserved and not in use.
#[must_use]
pub fn allocate_user_theme_id(existing: &BTreeMap<String, ColorTheme>) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    for suffix in 0u32..10_000 {
        let id = format!("user-{millis}-{suffix}");
        if !is_builtin_scheme_id(&id) && !existing.contains_key(&id) {
            return id;
        }
    }
    format!("user-{millis}-x")
}

fn load_theme_file(path: &Path) -> Result<ColorTheme, String> {
    let input = fs::read_to_string(path).map_err(|error| {
        format!(
            "failed to read theme {}: {error}",
            path.display()
        )
    })?;
    let theme: ColorTheme = toml::from_str(&input).map_err(|error| {
        format!(
            "failed to parse theme {}: {error}",
            path.display()
        )
    })?;
    if theme.id.trim().is_empty() {
        return Err(format!(
            "theme file {} has an empty id",
            path.display()
        ));
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
        assert!(warnings.is_empty());
        assert_eq!(reloaded.get_user("user-1"), Some(&theme));
    }

    #[test]
    fn cannot_save_or_delete_builtin_id() {
        let temp = TempDir::new().expect("tempdir");
        let mut catalog = ThemeCatalog::new(temp.path().join("themes"));
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
        assert_eq!(catalog.get_user("user-a").map(|t| t.name.as_str()), Some("New"));
        catalog.delete_user("user-a").expect("delete");
        assert!(!catalog.contains_user("user-a"));
        assert!(!temp.path().join("themes/user-a.toml").exists());
    }
}
