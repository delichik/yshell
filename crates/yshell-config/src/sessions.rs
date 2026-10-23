//! Xshell-style session tree: folders are directories, sessions are TOML files.
//!
//! Layout under `{config_dir}/sessions/`:
//! ```text
//! Prod/
//!   .folder.toml
//!   web.toml
//!   Nested/
//!     .folder.toml
//!     api.toml
//! ```

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    names::{
        name_collides, names_equal_ignore_case, sanitize_entry_name, validate_entry_name, NameError,
    },
    schema::{AppearanceProfile, FolderProfile, LoggingProfile, SessionProfile, TerminalProfile},
};

/// Directory name under the config root that holds the session tree.
pub const SESSIONS_DIR_NAME: &str = "sessions";

/// Sidecar file storing a folder's stable id and optional profile overrides.
pub const FOLDER_META_FILE_NAME: &str = ".folder.toml";

/// File-backed session tree rooted at `{config_dir}/sessions`.
#[derive(Debug, Clone)]
pub struct SessionTreeStore {
    root: PathBuf,
}

/// Errors from loading or mutating the on-disk session tree.
#[derive(Debug)]
pub enum SessionTreeError {
    Io(io::Error),
    Name(NameError),
    Parse { path: PathBuf, message: String },
    Serialize(toml::ser::Error),
    NotFound(String),
    Ambiguous(String),
}

impl std::fmt::Display for SessionTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "session tree I/O error: {error}"),
            Self::Name(error) => write!(f, "session tree name error: {error}"),
            Self::Parse { path, message } => {
                write!(f, "failed to parse {}: {message}", path.display())
            }
            Self::Serialize(error) => write!(f, "session serialization error: {error}"),
            Self::NotFound(id) => write!(f, "session tree entry `{id}` was not found"),
            Self::Ambiguous(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SessionTreeError {}

impl From<io::Error> for SessionTreeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<NameError> for SessionTreeError {
    fn from(value: NameError) -> Self {
        Self::Name(value)
    }
}

impl From<toml::ser::Error> for SessionTreeError {
    fn from(value: toml::ser::Error) -> Self {
        Self::Serialize(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FolderMetaFile {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<AppearanceProfile>,
}

impl SessionTreeStore {
    /// Creates a store rooted at `{config_root}/sessions`.
    #[must_use]
    pub fn new(config_root: impl AsRef<Path>) -> Self {
        Self {
            root: config_root.as_ref().join(SESSIONS_DIR_NAME),
        }
    }

    /// Directory that holds the session tree.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Loads the folder tree from disk. Missing directory yields an empty tree.
    ///
    /// Corrupt individual files are skipped and reported as warnings.
    pub fn load(&self) -> (Vec<FolderProfile>, Vec<String>) {
        let mut warnings = Vec::new();
        if !self.root.exists() {
            return (Vec::new(), warnings);
        }
        match self.load_folder_children(&self.root, &mut warnings) {
            Ok(folders) => (folders, warnings),
            Err(error) => {
                warnings.push(format!(
                    "failed to read session tree at {}: {error}",
                    self.root.display()
                ));
                (Vec::new(), warnings)
            }
        }
    }

    /// Writes an entire folder tree (used for legacy migration). Replaces the
    /// existing `sessions/` directory contents.
    pub fn write_tree(&self, folders: &[FolderProfile]) -> Result<(), SessionTreeError> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root)?;
        }
        fs::create_dir_all(&self.root)?;
        for folder in folders {
            self.write_folder_recursive(&self.root, folder)?;
        }
        Ok(())
    }

    /// Ensures a folder directory and `.folder.toml` exist on disk.
    pub fn ensure_folder(
        &self,
        folders: &[FolderProfile],
        folder_id: &str,
    ) -> Result<PathBuf, SessionTreeError> {
        let segments = folder_name_segments(folders, folder_id)
            .ok_or_else(|| SessionTreeError::NotFound(folder_id.to_owned()))?;
        let path = self.path_from_segments(&segments)?;
        fs::create_dir_all(&path)?;
        let folder = find_folder(folders, folder_id)
            .ok_or_else(|| SessionTreeError::NotFound(folder_id.to_owned()))?;
        self.write_folder_meta(&path, folder)?;
        Ok(path)
    }

    /// Creates a new folder directory under `parent_id` (or tree root when `None`).
    pub fn create_folder(
        &self,
        folders: &mut Vec<FolderProfile>,
        parent_id: Option<&str>,
        folder: FolderProfile,
    ) -> Result<(), SessionTreeError> {
        let name = validate_entry_name(&folder.name)?;
        let mut folder = folder;
        folder.name = name.clone();

        let sibling_names: Vec<String> = match parent_id {
            None => folders.iter().map(|f| f.name.clone()).collect(),
            Some(parent_id) => {
                let parent = find_folder_mut(folders, parent_id)
                    .ok_or_else(|| SessionTreeError::NotFound(parent_id.to_owned()))?;
                parent
                    .folders
                    .iter()
                    .map(|f| f.name.clone())
                    .chain(parent.sessions.iter().map(|s| s.name.clone()))
                    .collect()
            }
        };
        if name_collides(
            &name,
            sibling_names.iter().map(String::as_str),
        ) {
            return Err(NameError::Collision { name }.into());
        }

        let parent_path = match parent_id {
            None => {
                fs::create_dir_all(&self.root)?;
                self.root.clone()
            }
            Some(parent_id) => self.ensure_folder(folders, parent_id)?,
        };
        let path = parent_path.join(&name);
        if path.exists() {
            return Err(NameError::Collision { name }.into());
        }
        fs::create_dir_all(&path)?;
        self.write_folder_meta(&path, &folder)?;

        match parent_id {
            None => folders.push(folder),
            Some(parent_id) => {
                find_folder_mut(folders, parent_id)
                    .ok_or_else(|| SessionTreeError::NotFound(parent_id.to_owned()))?
                    .folders
                    .push(folder);
            }
        }
        Ok(())
    }

    /// Persists folder metadata (overrides) without renaming.
    pub fn save_folder_meta(
        &self,
        folders: &[FolderProfile],
        folder_id: &str,
    ) -> Result<(), SessionTreeError> {
        let path = self.ensure_folder(folders, folder_id)?;
        let folder = find_folder(folders, folder_id)
            .ok_or_else(|| SessionTreeError::NotFound(folder_id.to_owned()))?;
        self.write_folder_meta(&path, folder)
    }

    /// Writes (or renames) a session file under its parent folder.
    ///
    /// `folders` must already contain the updated session profile. When the
    /// display name changed, the previous `*.toml` is removed.
    pub fn save_session(
        &self,
        folders: &[FolderProfile],
        session_id: &str,
        previous_name: Option<&str>,
    ) -> Result<PathBuf, SessionTreeError> {
        let (folder_id, session) = find_session_with_parent(folders, session_id)
            .ok_or_else(|| SessionTreeError::NotFound(session_id.to_owned()))?;
        let name = validate_entry_name(&session.name)?;
        let folder_path = self.ensure_folder(folders, folder_id)?;
        let sibling_names: Vec<&str> = find_folder(folders, folder_id)
            .map(|folder| {
                folder
                    .sessions
                    .iter()
                    .filter(|s| s.id != session_id)
                    .map(|s| s.name.as_str())
                    .chain(
                        folder
                            .folders
                            .iter()
                            .map(|f| f.name.as_str()),
                    )
                    .collect()
            })
            .unwrap_or_default();
        if name_collides(&name, sibling_names) {
            return Err(NameError::Collision { name }.into());
        }

        if let Some(previous) = previous_name {
            let previous = previous.trim();
            if !previous.is_empty() && !names_equal_ignore_case(previous, &name) {
                let old_path = folder_path.join(format!("{previous}.toml"));
                if old_path.exists() {
                    let _ = fs::remove_file(&old_path);
                }
            }
        }

        let path = folder_path.join(format!("{name}.toml"));
        let mut to_write = session.clone();
        to_write.name = name;
        atomic_write_toml(&path, &to_write)?;
        Ok(path)
    }

    /// Inserts a new session into `folder_id` in memory and on disk.
    pub fn add_session(
        &self,
        folders: &mut Vec<FolderProfile>,
        folder_id: &str,
        mut session: SessionProfile,
    ) -> Result<(), SessionTreeError> {
        let name = validate_entry_name(&session.name)?;
        session.name = name.clone();
        let folder = find_folder_mut(folders, folder_id)
            .ok_or_else(|| SessionTreeError::NotFound(folder_id.to_owned()))?;
        let sibling_names: Vec<&str> = folder
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .chain(folder.folders.iter().map(|f| f.name.as_str()))
            .collect();
        if name_collides(&name, sibling_names) {
            return Err(NameError::Collision { name }.into());
        }
        if folder.sessions.iter().any(|s| s.id == session.id) {
            return Err(SessionTreeError::Ambiguous(format!(
                "session id `{}` already exists",
                session.id
            )));
        }
        folder.sessions.push(session.clone());
        self.save_session(folders, &session.id, None)?;
        Ok(())
    }

    /// Updates an existing session in memory and on disk.
    pub fn update_session(
        &self,
        folders: &mut Vec<FolderProfile>,
        session: SessionProfile,
    ) -> Result<(), SessionTreeError> {
        let previous_name = find_session(folders, &session.id)
            .map(|existing| existing.name.clone())
            .ok_or_else(|| SessionTreeError::NotFound(session.id.clone()))?;
        let name = validate_entry_name(&session.name)?;
        let target = find_session_mut(folders, &session.id)
            .ok_or_else(|| SessionTreeError::NotFound(session.id.clone()))?;
        *target = SessionProfile {
            name: name.clone(),
            ..session
        };
        self.save_session(folders, &target.id, Some(&previous_name))?;
        Ok(())
    }

    /// Removes a session from memory and deletes its file.
    pub fn delete_session(
        &self,
        folders: &mut Vec<FolderProfile>,
        session_id: &str,
    ) -> Result<SessionProfile, SessionTreeError> {
        let (folder_id, session) = find_session_with_parent(folders, session_id)
            .ok_or_else(|| SessionTreeError::NotFound(session_id.to_owned()))?;
        let folder_path = self.path_for_folder(folders, folder_id)?;
        let file_path = folder_path.join(format!("{}.toml", session.name));
        if file_path.exists() {
            fs::remove_file(&file_path)?;
        }
        let removed = remove_session(folders, session_id)
            .ok_or_else(|| SessionTreeError::NotFound(session_id.to_owned()))?;
        Ok(removed)
    }

    /// Removes a folder (and its descendants) from memory and disk.
    pub fn delete_folder(
        &self,
        folders: &mut Vec<FolderProfile>,
        folder_id: &str,
    ) -> Result<FolderProfile, SessionTreeError> {
        let path = self.path_for_folder(folders, folder_id)?;
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        remove_folder(folders, folder_id)
            .ok_or_else(|| SessionTreeError::NotFound(folder_id.to_owned()))
    }

    fn path_for_folder(
        &self,
        folders: &[FolderProfile],
        folder_id: &str,
    ) -> Result<PathBuf, SessionTreeError> {
        let segments = folder_name_segments(folders, folder_id)
            .ok_or_else(|| SessionTreeError::NotFound(folder_id.to_owned()))?;
        self.path_from_segments(&segments)
    }

    fn path_from_segments(&self, segments: &[String]) -> Result<PathBuf, SessionTreeError> {
        let mut path = self.root.clone();
        for segment in segments {
            let name = validate_entry_name(segment)?;
            path.push(name);
        }
        Ok(path)
    }

    fn write_folder_recursive(
        &self,
        parent: &Path,
        folder: &FolderProfile,
    ) -> Result<(), SessionTreeError> {
        let name = sanitize_entry_name(&folder.name, &folder.id);
        let path = parent.join(name);
        fs::create_dir_all(&path)?;
        self.write_folder_meta(&path, folder)?;
        for session in &folder.sessions {
            let session_name = sanitize_entry_name(&session.name, &session.id);
            let mut to_write = session.clone();
            to_write.name = session_name.clone();
            atomic_write_toml(&path.join(format!("{session_name}.toml")), &to_write)?;
        }
        for child in &folder.folders {
            self.write_folder_recursive(&path, child)?;
        }
        Ok(())
    }

    fn write_folder_meta(
        &self,
        folder_path: &Path,
        folder: &FolderProfile,
    ) -> Result<(), SessionTreeError> {
        let meta = FolderMetaFile {
            id: folder.id.clone(),
            terminal: folder.terminal.clone(),
            logging: folder.logging.clone(),
            appearance: folder.appearance.clone(),
        };
        atomic_write_toml(&folder_path.join(FOLDER_META_FILE_NAME), &meta)
    }

    fn load_folder_children(
        &self,
        dir: &Path,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<FolderProfile>, SessionTreeError> {
        let mut folders = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                match self.load_folder(&path, warnings) {
                    Ok(folder) => folders.push(folder),
                    Err(error) => warnings.push(error.to_string()),
                }
            }
        }
        Ok(folders)
    }

    fn load_folder(
        &self,
        path: &Path,
        warnings: &mut Vec<String>,
    ) -> Result<FolderProfile, SessionTreeError> {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| SessionTreeError::Ambiguous(format!(
                "folder path {} has a non-utf8 name",
                path.display()
            )))?
            .to_owned();
        let name = validate_entry_name(&name)?;
        let meta_path = path.join(FOLDER_META_FILE_NAME);
        let meta = if meta_path.exists() {
            match load_toml::<FolderMetaFile>(&meta_path) {
                Ok(meta) => meta,
                Err(message) => {
                    warnings.push(message);
                    FolderMetaFile {
                        id: synthesize_folder_id(&name),
                        terminal: None,
                        logging: None,
                        appearance: None,
                    }
                }
            }
        } else {
            FolderMetaFile {
                id: synthesize_folder_id(&name),
                terminal: None,
                logging: None,
                appearance: None,
            }
        };

        let mut sessions = Vec::new();
        let mut child_folders = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child = entry.path();
            if child.is_dir() {
                match self.load_folder(&child, warnings) {
                    Ok(folder) => child_folders.push(folder),
                    Err(error) => warnings.push(error.to_string()),
                }
                continue;
            }
            let Some(file_name) = child.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if file_name == FOLDER_META_FILE_NAME {
                continue;
            }
            if child.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let stem = child
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            match load_toml::<SessionProfile>(&child) {
                Ok(mut session) => {
                    if let Ok(validated) = validate_entry_name(stem) {
                        session.name = validated;
                    } else {
                        session.name = stem.to_owned();
                    }
                    if session.id.trim().is_empty() {
                        session.id = synthesize_session_id(&session.name);
                    }
                    sessions.push(session);
                }
                Err(message) => warnings.push(message),
            }
        }

        Ok(FolderProfile {
            id: if meta.id.trim().is_empty() {
                synthesize_folder_id(&name)
            } else {
                meta.id
            },
            name,
            sessions,
            folders: child_folders,
            terminal: meta.terminal,
            logging: meta.logging,
            appearance: meta.appearance,
        })
    }
}

fn atomic_write_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), SessionTreeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    let body = toml::to_string_pretty(value)?;
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn load_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let input = fs::read_to_string(path).map_err(|error| {
        format!("failed to read {}: {error}", path.display())
    })?;
    toml::from_str(&input).map_err(|error| {
        format!("failed to parse {}: {error}", path.display())
    })
}

fn synthesize_folder_id(name: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let stem = sanitize_id_stem(name);
    format!("folder-{stem}-{millis}")
}

fn synthesize_session_id(name: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let stem = sanitize_id_stem(name);
    format!("saved-{stem}-{millis}")
}

fn sanitize_id_stem(name: &str) -> String {
    let stem = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    if stem.is_empty() {
        "item".to_owned()
    } else {
        stem
    }
}

fn folder_name_segments(folders: &[FolderProfile], folder_id: &str) -> Option<Vec<String>> {
    fn walk(folder: &FolderProfile, id: &str, chain: &mut Vec<String>) -> bool {
        chain.push(folder.name.clone());
        if folder.id == id {
            return true;
        }
        for child in &folder.folders {
            if walk(child, id, chain) {
                return true;
            }
        }
        chain.pop();
        false
    }

    for folder in folders {
        let mut chain = Vec::new();
        if walk(folder, folder_id, &mut chain) {
            return Some(chain);
        }
    }
    None
}

fn find_folder<'a>(folders: &'a [FolderProfile], id: &str) -> Option<&'a FolderProfile> {
    folders.iter().find_map(|folder| folder.find_folder(id))
}

fn find_folder_mut<'a>(
    folders: &'a mut [FolderProfile],
    id: &str,
) -> Option<&'a mut FolderProfile> {
    for folder in folders.iter_mut() {
        if let Some(found) = folder.find_folder_mut(id) {
            return Some(found);
        }
    }
    None
}

fn find_session<'a>(folders: &'a [FolderProfile], id: &str) -> Option<&'a SessionProfile> {
    folders.iter().find_map(|folder| folder.find_session(id))
}

fn find_session_mut<'a>(
    folders: &'a mut [FolderProfile],
    id: &str,
) -> Option<&'a mut SessionProfile> {
    for folder in folders.iter_mut() {
        if let Some(found) = folder.find_session_mut(id) {
            return Some(found);
        }
    }
    None
}

fn find_session_with_parent<'a>(
    folders: &'a [FolderProfile],
    session_id: &str,
) -> Option<(&'a str, &'a SessionProfile)> {
    fn walk<'a>(
        folder: &'a FolderProfile,
        session_id: &str,
    ) -> Option<(&'a str, &'a SessionProfile)> {
        if let Some(session) = folder.sessions.iter().find(|s| s.id == session_id) {
            return Some((folder.id.as_str(), session));
        }
        folder
            .folders
            .iter()
            .find_map(|child| walk(child, session_id))
    }
    folders.iter().find_map(|folder| walk(folder, session_id))
}

fn remove_session(folders: &mut Vec<FolderProfile>, id: &str) -> Option<SessionProfile> {
    for folder in folders.iter_mut() {
        if let Some(removed) = folder.remove_session(id) {
            return Some(removed);
        }
    }
    None
}

fn remove_folder(folders: &mut Vec<FolderProfile>, id: &str) -> Option<FolderProfile> {
    if let Some(index) = folders.iter().position(|folder| folder.id == id) {
        return Some(folders.remove(index));
    }
    for folder in folders.iter_mut() {
        if let Some(removed) = remove_folder(&mut folder.folders, id) {
            return Some(removed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_tree() -> Vec<FolderProfile> {
        let mut root = FolderProfile::new("folder-prod", "Prod");
        root.sessions
            .push(SessionProfile::new("saved-web", "web", "web.example.test"));
        let mut nested = FolderProfile::new("folder-nested", "Nested");
        nested
            .sessions
            .push(SessionProfile::new("saved-api", "api", "api.example.test"));
        root.folders.push(nested);
        vec![root]
    }

    #[test]
    fn write_and_load_round_trip() {
        let temp = TempDir::new().expect("tempdir");
        let store = SessionTreeStore::new(temp.path());
        store.write_tree(&sample_tree()).expect("write");
        assert!(temp
            .path()
            .join("sessions/Prod/.folder.toml")
            .exists());
        assert!(temp.path().join("sessions/Prod/web.toml").exists());
        assert!(temp
            .path()
            .join("sessions/Prod/Nested/api.toml")
            .exists());

        let (loaded, warnings) = store.load();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Prod");
        assert_eq!(loaded[0].sessions[0].name, "web");
        assert_eq!(loaded[0].folders[0].sessions[0].host, "api.example.test");
    }

    #[test]
    fn rejects_sibling_name_collision() {
        let temp = TempDir::new().expect("tempdir");
        let store = SessionTreeStore::new(temp.path());
        let mut folders = sample_tree();
        let err = store
            .add_session(
                &mut folders,
                "folder-prod",
                SessionProfile::new("saved-other", "WEB", "other.test"),
            )
            .expect_err("collision");
        assert!(matches!(err, SessionTreeError::Name(NameError::Collision { .. })));
    }

    #[test]
    fn rename_session_replaces_file() {
        let temp = TempDir::new().expect("tempdir");
        let store = SessionTreeStore::new(temp.path());
        let mut folders = sample_tree();
        store.write_tree(&folders).expect("write");
        let mut updated = folders[0].sessions[0].clone();
        updated.name = "portal".to_owned();
        store.update_session(&mut folders, updated).expect("rename");
        assert!(!temp.path().join("sessions/Prod/web.toml").exists());
        assert!(temp.path().join("sessions/Prod/portal.toml").exists());
    }
}
