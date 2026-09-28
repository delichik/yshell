//! Versioned configuration schema, profile types, migration, and inheritance.

use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::quick_connect::{parse_quick_connect, QuickConnectError};

/// Current configuration schema version for newly-created stores.
///
/// * `1` — original profile-based schema.
/// * `2` — adds `[ui]`, `[quick_connect]`, `quick_links`, managed `keys`, folder
///   profile overrides, and optional terminal theme fields.
pub const SCHEMA_VERSION: u32 = 2;

/// Root TOML document persisted by the configuration store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDocument {
    /// Schema version used to decide whether migrations are needed.
    pub schema_version: u32,
    /// Saved session folders and sessions.
    #[serde(default)]
    pub folders: Vec<FolderProfile>,
    /// Saved auth profiles keyed by id.
    #[serde(default)]
    pub auth_profiles: BTreeMap<String, AuthProfile>,
    /// Saved proxy profiles keyed by id.
    #[serde(default)]
    pub proxy_profiles: BTreeMap<String, ProxyProfile>,
    /// Default SFTP behavior inherited by sessions.
    #[serde(default)]
    pub sftp: SftpProfile,
    /// Default tunnel behavior inherited by sessions.
    #[serde(default)]
    pub tunnel: TunnelProfile,
    /// Default appearance behavior inherited by sessions.
    #[serde(default)]
    pub appearance: AppearanceProfile,
    /// Default logging behavior inherited by sessions.
    #[serde(default)]
    pub logging: LoggingProfile,
    /// Default terminal scrollback/theme behavior inherited by sessions.
    #[serde(default)]
    pub terminal: TerminalProfile,
    /// UI preferences: new-tab behavior, dock layout, multi-window.
    #[serde(default)]
    pub ui: UiProfile,
    /// Quick Connect availability, history limit, and history.
    #[serde(default)]
    pub quick_connect: QuickConnectProfile,
    /// User-pinned quick links.
    #[serde(default)]
    pub quick_links: Vec<QuickLink>,
    /// Managed private key metadata; key material lives in the secret store.
    #[serde(default)]
    pub keys: Vec<KeyProfile>,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            folders: Vec::new(),
            auth_profiles: BTreeMap::new(),
            proxy_profiles: BTreeMap::new(),
            sftp: SftpProfile::default(),
            tunnel: TunnelProfile::default(),
            appearance: AppearanceProfile::default(),
            logging: LoggingProfile::default(),
            terminal: TerminalProfile::default(),
            ui: UiProfile::default(),
            quick_connect: QuickConnectProfile::default(),
            quick_links: Vec::new(),
            keys: Vec::new(),
        }
    }
}

/// A folder node in the saved-session tree.
///
/// Folders may override the inheritable profiles for every session below them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sessions: Vec<SessionProfile>,
    #[serde(default)]
    pub folders: Vec<FolderProfile>,
    /// Terminal override for this folder and its descendants.
    #[serde(default)]
    pub terminal: Option<TerminalProfile>,
    /// Logging override for this folder and its descendants.
    #[serde(default)]
    pub logging: Option<LoggingProfile>,
    /// Appearance override for this folder and its descendants.
    #[serde(default)]
    pub appearance: Option<AppearanceProfile>,
}

impl FolderProfile {
    /// Creates an empty folder node.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            sessions: Vec::new(),
            folders: Vec::new(),
            terminal: None,
            logging: None,
            appearance: None,
        }
    }

    /// Recursively finds an immutable session by id.
    #[must_use]
    pub fn find_session(&self, id: &str) -> Option<&SessionProfile> {
        self.sessions
            .iter()
            .find(|session| session.id == id)
            .or_else(|| {
                self.folders
                    .iter()
                    .find_map(|folder| folder.find_session(id))
            })
    }

    /// Recursively finds a mutable session by id.
    pub fn find_session_mut(&mut self, id: &str) -> Option<&mut SessionProfile> {
        if let Some(index) = self.sessions.iter().position(|session| session.id == id) {
            return self.sessions.get_mut(index);
        }
        self.folders
            .iter_mut()
            .find_map(|folder| folder.find_session_mut(id))
    }

    /// Recursively finds an immutable folder by id.
    #[must_use]
    pub fn find_folder(&self, id: &str) -> Option<&FolderProfile> {
        if self.id == id {
            return Some(self);
        }
        self.folders
            .iter()
            .find_map(|folder| folder.find_folder(id))
    }

    /// Recursively finds a mutable folder by id.
    pub fn find_folder_mut(&mut self, id: &str) -> Option<&mut FolderProfile> {
        if self.id == id {
            return Some(self);
        }
        self.folders
            .iter_mut()
            .find_map(|folder| folder.find_folder_mut(id))
    }

    /// Recursively removes a session by id and returns it when found.
    pub fn remove_session(&mut self, id: &str) -> Option<SessionProfile> {
        if let Some(index) = self.sessions.iter().position(|session| session.id == id) {
            return Some(self.sessions.remove(index));
        }
        self.folders
            .iter_mut()
            .find_map(|folder| folder.remove_session(id))
    }
}

/// Saved SSH/SFTP session profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub auth_profile_id: Option<String>,
    #[serde(default)]
    pub proxy_profile_id: Option<String>,
    #[serde(default)]
    pub host_key_policy: Option<HostKeyPolicy>,
    #[serde(default)]
    pub sftp: Option<SftpProfile>,
    #[serde(default)]
    pub tunnel: Option<TunnelProfile>,
    #[serde(default)]
    pub appearance: Option<AppearanceProfile>,
    #[serde(default)]
    pub logging: Option<LoggingProfile>,
    #[serde(default)]
    pub terminal: Option<TerminalProfile>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl SessionProfile {
    /// Creates a session with SSH defaults.
    pub fn new(id: impl Into<String>, name: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            host: host.into(),
            port: default_ssh_port(),
            username: None,
            auth_profile_id: None,
            proxy_profile_id: None,
            host_key_policy: None,
            sftp: None,
            tunnel: None,
            appearance: None,
            logging: None,
            terminal: None,
            tags: Vec::new(),
        }
    }

    /// Applies a partial batch edit to basic user-facing fields.
    pub fn apply_basic_edit(&mut self, edit: &SessionBasicEdit) {
        if let Some(name) = &edit.name {
            self.name.clone_from(name);
        }
        if let Some(host) = &edit.host {
            self.host.clone_from(host);
        }
        if let Some(port) = edit.port {
            self.port = port;
        }
        if edit.username_set {
            self.username.clone_from(&edit.username);
        }
        if let Some(tags) = &edit.tags {
            self.tags.clone_from(tags);
        }
    }
}

/// Basic fields supported by batch edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionBasicEdit {
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub username_set: bool,
    pub tags: Option<Vec<String>>,
}

/// Auth material reference. Secrets are addressed by key rather than persisted inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthProfile {
    pub id: String,
    pub name: String,
    pub method: AuthMethod,
}

/// Supported auth methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    Password {
        secret_key: String,
    },
    KeyboardInteractive {
        secret_key: String,
    },
    PrivateKey {
        /// Reference to a managed [`KeyProfile`]; takes precedence over `path`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_id: Option<String>,
        /// Legacy private key file path, kept for backwards compatibility.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        path: String,
        #[serde(default)]
        passphrase_secret_key: Option<String>,
    },
    Agent,
}

impl AuthMethod {
    /// Returns the managed key profile id when this is a `PrivateKey` method.
    #[must_use]
    pub fn private_key_id(&self) -> Option<&str> {
        match self {
            Self::PrivateKey { key_id, .. } => key_id.as_deref(),
            _ => None,
        }
    }

    /// Returns the legacy key file path when set on a `PrivateKey` method.
    #[must_use]
    pub fn private_key_path(&self) -> Option<&str> {
        match self {
            Self::PrivateKey { path, .. } if !path.is_empty() => Some(path),
            _ => None,
        }
    }
}

/// Where the private key material of a [`AuthMethod::PrivateKey`] comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrivateKeySource<'a> {
    /// Managed key metadata resolved through `key_id`.
    Managed(&'a KeyProfile),
    /// Legacy private key file path.
    Path(&'a str),
}

/// Optional SSH proxy/jump profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub protocol: ProxyProtocol,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default = "default_true")]
    pub resolve_dns_by_proxy: bool,
    #[serde(default)]
    pub password_secret_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyProtocol {
    Socks4,
    Socks4a,
    #[default]
    Socks5,
    HttpConnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyPolicy {
    Strict,
    TrustOnFirstUse,
    AcceptAnyForTesting,
}

/// SFTP defaults or session override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpProfile {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub initial_directory: Option<String>,
}

impl Default for SftpProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_directory: None,
        }
    }
}

/// Local/remote tunnel defaults or session override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TunnelProfile {
    #[serde(default)]
    pub forwards: Vec<TunnelForward>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelForward {
    pub kind: TunnelForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelForwardKind {
    Local,
    Remote,
    Dynamic,
}

/// Visual defaults or session override.
///
/// Every field carries a serde default so that a folder/session can override a
/// single attribute; attributes omitted at a level fall back to that level's
/// built-in default rather than to a farther level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppearanceProfile {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_terminal_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u16,
}

impl Default for AppearanceProfile {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            font_family: default_terminal_font_family(),
            font_size: default_font_size(),
        }
    }
}

/// Logging defaults or session override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingProfile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingProfile {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: None,
            format: default_log_format(),
        }
    }
}

/// Terminal scrollback/theme defaults, folder override, or session override.
///
/// Scrollback fields are concrete (missing keys fall back to the built-in
/// defaults); every theme field is optional so that a nearer level can set a
/// single attribute without resetting the others. Use
/// [`TerminalProfile::inherit_from`] to merge a nearer profile over a farther
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProfile {
    /// Maximum number of scrollback lines kept per session.
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: usize,
    /// Maximum number of cells (`columns` summed over lines) kept in scrollback.
    #[serde(default = "default_scrollback_max_cells")]
    pub scrollback_max_cells: usize,
    /// Named color scheme resolved by the terminal/UI layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
    /// Default foreground color (`#RRGGBB`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    /// Default background color (`#RRGGBB`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Cursor color (`#RRGGBB`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Selection highlight color (`#RRGGBB`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
    /// The 16 ANSI palette entries as `#RRGGBB` strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ansi: Option<[String; 16]>,
    /// Primary font family for the terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    /// Primary font size in logical pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u16>,
    /// Fallback font families tried in order for missing glyphs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_fonts: Option<Vec<String>>,
}

impl TerminalProfile {
    /// Returns `self` with unset optional fields filled from `fallback`.
    ///
    /// Scrollback fields are concrete and always come from `self`, whether or
    /// not the profile was explicitly configured.
    #[must_use]
    pub fn inherit_from(mut self, fallback: &Self) -> Self {
        self.color_scheme = self.color_scheme.or_else(|| fallback.color_scheme.clone());
        self.foreground = self.foreground.or_else(|| fallback.foreground.clone());
        self.background = self.background.or_else(|| fallback.background.clone());
        self.cursor = self.cursor.or_else(|| fallback.cursor.clone());
        self.selection = self.selection.or_else(|| fallback.selection.clone());
        self.ansi = self.ansi.or_else(|| fallback.ansi.clone());
        self.font_family = self.font_family.or_else(|| fallback.font_family.clone());
        self.font_size = self.font_size.or(fallback.font_size);
        self.fallback_fonts = self
            .fallback_fonts
            .or_else(|| fallback.fallback_fonts.clone());
        self
    }
}

impl Default for TerminalProfile {
    fn default() -> Self {
        Self {
            scrollback_lines: default_scrollback_lines(),
            scrollback_max_cells: default_scrollback_max_cells(),
            color_scheme: None,
            foreground: None,
            background: None,
            cursor: None,
            selection: None,
            ansi: None,
            font_family: None,
            font_size: None,
            fallback_fonts: None,
        }
    }
}

/// UI preferences (`[ui]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiProfile {
    /// What the new-tab action (`+`) opens.
    #[serde(default)]
    pub new_tab_mode: NewTabMode,
    /// Docked panel layout shared by every window.
    #[serde(default)]
    pub layout: LayoutProfile,
    /// Whether opening additional windows is allowed.
    #[serde(default = "default_true")]
    pub multi_window_enabled: bool,
}

impl Default for UiProfile {
    fn default() -> Self {
        Self {
            new_tab_mode: NewTabMode::default(),
            layout: LayoutProfile::default(),
            multi_window_enabled: true,
        }
    }
}

/// Destination of the new-tab action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NewTabMode {
    /// Open the Quick Connect page.
    #[default]
    QuickConnect,
    /// Open the session editor.
    SessionEditor,
}

/// Tool panels that can be docked into the left/right panel stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelId {
    Sessions,
    Sftp,
    Tunnels,
    QuickCommands,
    Transfers,
}

impl PanelId {
    /// Returns the stable snake_case name used in configuration files.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Sftp => "sftp",
            Self::Tunnels => "tunnels",
            Self::QuickCommands => "quick_commands",
            Self::Transfers => "transfers",
        }
    }
}

impl fmt::Display for PanelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Side of the main window a panel stack lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelSide {
    Left,
    Right,
}

/// A panel placed in a left/right stack along with its collapsed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelSlot {
    /// Which panel occupies this slot.
    pub panel: PanelId,
    /// Whether the panel is collapsed to its title bar.
    #[serde(default)]
    pub collapsed: bool,
}

/// Persisted dock layout (`[ui.layout]`).
///
/// `left_ratios`/`right_ratios` hold stack split fractions (one per boundary,
/// i.e. `panels.len() - 1` entries); an empty list means "split evenly".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutProfile {
    /// Left stack, top to bottom.
    #[serde(default = "default_left_panels")]
    pub left: Vec<PanelSlot>,
    /// Right stack, top to bottom.
    #[serde(default = "default_right_panels")]
    pub right: Vec<PanelSlot>,
    /// Vertical split ratios for the left stack.
    #[serde(default)]
    pub left_ratios: Vec<f32>,
    /// Vertical split ratios for the right stack.
    #[serde(default)]
    pub right_ratios: Vec<f32>,
    /// Persisted width of the left stack in logical pixels.
    #[serde(default = "default_left_width")]
    pub left_width: u32,
    /// Persisted width of the right stack in logical pixels.
    #[serde(default = "default_right_width")]
    pub right_width: u32,
    /// Side collapsed automatically in narrow windows, when the user pinned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrow_collapsed_side: Option<PanelSide>,
}

impl Default for LayoutProfile {
    fn default() -> Self {
        Self {
            left: default_left_panels(),
            right: default_right_panels(),
            left_ratios: Vec::new(),
            right_ratios: Vec::new(),
            left_width: default_left_width(),
            right_width: default_right_width(),
            narrow_collapsed_side: None,
        }
    }
}

/// Quick Connect preferences and history (`[quick_connect]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickConnectProfile {
    /// Whether Quick Connect targets are recorded in `history`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum number of history entries kept.
    #[serde(default = "default_quick_connect_limit")]
    pub limit: usize,
    /// Recently used targets, newest first. Targets are normalized on load.
    #[serde(default)]
    pub history: Vec<QuickConnectEntry>,
}

impl Default for QuickConnectProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            limit: default_quick_connect_limit(),
            history: Vec::new(),
        }
    }
}

/// One recorded Quick Connect target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickConnectEntry {
    /// Normalized target (`user@host:port` form).
    pub target: String,
    /// Unix epoch seconds of the last use; `0` means unknown.
    #[serde(default)]
    pub last_used_at: i64,
    /// Number of times this target has been used.
    #[serde(default)]
    pub use_count: u32,
}

/// A user-pinned connection shortcut (`quick_links`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickLink {
    /// Stable identifier used by the UI for rename/reorder/delete.
    pub id: String,
    /// User-visible label.
    #[serde(default)]
    pub label: String,
    /// Normalized target (`user@host:port` form).
    pub target: String,
    /// Ascending sort key used to order the list.
    #[serde(default)]
    pub sort_order: i32,
}

/// Managed private key metadata (`keys`).
///
/// The private key material and its passphrase live in the secret store and are
/// only referenced by [`KeyProfile::secret_ref`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyProfile {
    /// Stable identifier referenced by `AuthMethod::PrivateKey::key_id`.
    pub id: String,
    /// User-visible label.
    #[serde(default)]
    pub label: String,
    /// Key algorithm, e.g. `ssh-ed25519`.
    #[serde(default)]
    pub algorithm: String,
    /// OpenSSH-style fingerprint of the public key.
    #[serde(default)]
    pub fingerprint: String,
    /// OpenSSH public key line derived from the private key.
    #[serde(default)]
    pub public_key: String,
    /// Secret store reference holding the private key material.
    #[serde(default)]
    pub secret_ref: String,
    /// Optional user comment (defaults to the key's own comment when absent).
    #[serde(default)]
    pub comment: Option<String>,
}

/// A session with all inheritable defaults resolved.
///
/// Resolution walks the session, its nearest ancestor folder, the remaining
/// ancestors up to the root folder, the document-level defaults, and finally
/// the built-in defaults. Optional fields are taken from the nearest level that
/// sets them; non-optional fields are taken from the nearest level that has the
/// corresponding section at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSessionProfile {
    pub session: SessionProfile,
    pub auth: Option<AuthProfile>,
    pub proxy: Option<ProxyProfile>,
    pub sftp: SftpProfile,
    pub tunnel: TunnelProfile,
    pub appearance: AppearanceProfile,
    pub logging: LoggingProfile,
    pub terminal: TerminalProfile,
}

impl ConfigDocument {
    /// Serializes the document as TOML.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Parses a document from TOML, runs migrations, and sanitizes it.
    ///
    /// Non-fatal problems found while sanitizing (invalid Quick Connect targets,
    /// invalid colors) are reported by [`Self::from_toml_str_with_warnings`].
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigSchemaError> {
        Self::from_toml_str_with_warnings(input).map(|(document, _)| document)
    }

    /// Parses a document from TOML and returns the non-fatal warnings it dropped.
    pub fn from_toml_str_with_warnings(
        input: &str,
    ) -> Result<(Self, Vec<ConfigWarning>), ConfigSchemaError> {
        let value = toml::Value::from_str(input)?;
        migrate_value_with_warnings(value)
    }

    /// Finds a session anywhere in the folder tree.
    #[must_use]
    pub fn find_session(&self, id: &str) -> Option<&SessionProfile> {
        self.folders
            .iter()
            .find_map(|folder| folder.find_session(id))
    }

    /// Finds a mutable session anywhere in the folder tree.
    pub fn find_session_mut(&mut self, id: &str) -> Option<&mut SessionProfile> {
        self.folders
            .iter_mut()
            .find_map(|folder| folder.find_session_mut(id))
    }

    /// Finds a folder anywhere in the folder tree.
    #[must_use]
    pub fn find_folder(&self, id: &str) -> Option<&FolderProfile> {
        self.folders
            .iter()
            .find_map(|folder| folder.find_folder(id))
    }

    /// Finds a mutable folder anywhere in the folder tree.
    pub fn find_folder_mut(&mut self, id: &str) -> Option<&mut FolderProfile> {
        self.folders
            .iter_mut()
            .find_map(|folder| folder.find_folder_mut(id))
    }

    /// Finds a managed private key profile by id.
    #[must_use]
    pub fn find_key(&self, id: &str) -> Option<&KeyProfile> {
        self.keys.iter().find(|key| key.id == id)
    }

    /// Resolves the private key source of an auth method, preferring `key_id`.
    ///
    /// Returns `None` for non-`PrivateKey` methods. A `key_id` that does not
    /// resolve to a [`KeyProfile`] falls back to the legacy `path` when set.
    #[must_use]
    pub fn resolve_private_key_source<'a>(
        &'a self,
        method: &'a AuthMethod,
    ) -> Option<PrivateKeySource<'a>> {
        let AuthMethod::PrivateKey { key_id, path, .. } = method else {
            return None;
        };
        if let Some(key) = key_id.as_deref().and_then(|id| self.find_key(id)) {
            return Some(PrivateKeySource::Managed(key));
        }
        if !path.is_empty() {
            return Some(PrivateKeySource::Path(path));
        }
        None
    }

    /// Applies a basic edit to every matching session id and returns the count updated.
    pub fn batch_edit_basic<'a>(
        &mut self,
        ids: impl IntoIterator<Item = &'a str>,
        edit: &SessionBasicEdit,
    ) -> usize {
        ids.into_iter()
            .filter(|id| {
                self.find_session_mut(id)
                    .map(|session| session.apply_basic_edit(edit))
                    .is_some()
            })
            .count()
    }

    /// Removes a session anywhere in the folder tree.
    pub fn remove_session(&mut self, id: &str) -> Option<SessionProfile> {
        self.folders
            .iter_mut()
            .find_map(|folder| folder.remove_session(id))
    }

    /// Resolves a session against its ancestor folders, document defaults, and
    /// built-in defaults.
    #[must_use]
    pub fn resolve_session(&self, id: &str) -> Option<ResolvedSessionProfile> {
        let (session, folder_chain) = self.find_session_with_folder_chain(id)?;
        Some(self.resolve_session_parts(session, &folder_chain))
    }

    /// Resolves a not-yet-saved session that is being edited for `folder_id`
    /// (Session Editor previews: the draft's explicit fields win, then the
    /// target folder chain, then the document defaults and built-in defaults).
    #[must_use]
    pub fn resolve_session_draft(
        &self,
        session: &SessionProfile,
        folder_id: &str,
    ) -> ResolvedSessionProfile {
        let folder_chain = self.folder_chain_to(folder_id);
        self.resolve_session_parts(session, &folder_chain)
    }

    /// Merges a session (or draft) over its nearest-first folder chain and the
    /// document defaults.
    fn resolve_session_parts(
        &self,
        session: &SessionProfile,
        folder_chain: &[&FolderProfile],
    ) -> ResolvedSessionProfile {
        let appearance = nearest_or(
            std::iter::once(session.appearance.as_ref())
                .chain(folder_chain.iter().map(|folder| folder.appearance.as_ref())),
            &self.appearance,
        )
        .clone();
        let logging = nearest_or(
            std::iter::once(session.logging.as_ref())
                .chain(folder_chain.iter().map(|folder| folder.logging.as_ref())),
            &self.logging,
        )
        .clone();
        let terminal = resolve_terminal_profile(session, folder_chain, &self.terminal);

        ResolvedSessionProfile {
            auth: session
                .auth_profile_id
                .as_deref()
                .and_then(|auth_id| self.auth_profiles.get(auth_id))
                .cloned(),
            proxy: session
                .proxy_profile_id
                .as_deref()
                .and_then(|proxy_id| self.proxy_profiles.get(proxy_id))
                .cloned(),
            sftp: session.sftp.clone().unwrap_or_else(|| self.sftp.clone()),
            tunnel: session
                .tunnel
                .clone()
                .unwrap_or_else(|| self.tunnel.clone()),
            appearance,
            logging,
            terminal,
            session: session.clone(),
        }
    }

    /// Folder chain to `folder_id`, nearest ancestor (the folder itself) first.
    ///
    /// Unknown ids resolve to an empty chain, so callers fall back to the
    /// document defaults instead of failing.
    #[must_use]
    pub fn folder_chain_to(&self, folder_id: &str) -> Vec<&FolderProfile> {
        fn path<'a>(
            folder: &'a FolderProfile,
            id: &str,
            chain: &mut Vec<&'a FolderProfile>,
        ) -> bool {
            chain.push(folder);
            if folder.id == id {
                return true;
            }
            for child in &folder.folders {
                if path(child, id, chain) {
                    return true;
                }
            }
            chain.pop();
            false
        }

        let mut chain = Vec::new();
        for folder in &self.folders {
            if path(folder, folder_id, &mut chain) {
                chain.reverse();
                return chain;
            }
        }
        Vec::new()
    }

    /// Drops invalid Quick Connect/quick-link entries and clears invalid theme
    /// values, returning a warning for every change.
    ///
    /// Invalid Quick Connect and quick-link targets are removed; valid targets
    /// are rewritten to their canonical `user@host:port` form. Invalid colors
    /// (`#RRGGBB` expected) are cleared so resolution falls back to the next
    /// level.
    pub fn sanitize(&mut self) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();

        self.quick_connect
            .history
            .retain_mut(|entry| match parse_quick_connect(&entry.target) {
                Ok(target) => {
                    entry.target = target.canonical();
                    true
                }
                Err(error) => {
                    warnings.push(ConfigWarning::InvalidQuickConnectTarget {
                        target: std::mem::take(&mut entry.target),
                        error,
                    });
                    false
                }
            });

        self.quick_links
            .retain_mut(|link| match parse_quick_connect(&link.target) {
                Ok(target) => {
                    link.target = target.canonical();
                    true
                }
                Err(error) => {
                    warnings.push(ConfigWarning::InvalidQuickLinkTarget {
                        id: std::mem::take(&mut link.id),
                        target: std::mem::take(&mut link.target),
                        error,
                    });
                    false
                }
            });

        sanitize_terminal_colors(&mut self.terminal, "terminal", &mut warnings);
        for folder in &mut self.folders {
            let prefix = format!("folders[{}]", folder.id);
            sanitize_folder(folder, &prefix, &mut warnings);
        }

        warnings
    }

    /// Walks the folder tree to a session, returning the session and the chain
    /// of folders that contain it (nearest ancestor first).
    fn find_session_with_folder_chain<'a>(
        &'a self,
        id: &str,
    ) -> Option<(&'a SessionProfile, Vec<&'a FolderProfile>)> {
        fn walk<'a>(
            folder: &'a FolderProfile,
            id: &str,
            chain: &mut Vec<&'a FolderProfile>,
        ) -> Option<&'a SessionProfile> {
            chain.push(folder);
            if let Some(session) = folder.sessions.iter().find(|session| session.id == id) {
                return Some(session);
            }
            for child in &folder.folders {
                if let Some(session) = walk(child, id, chain) {
                    return Some(session);
                }
            }
            chain.pop();
            None
        }

        for folder in &self.folders {
            let mut chain = Vec::new();
            if let Some(session) = walk(folder, id, &mut chain) {
                chain.reverse();
                return Some((session, chain));
            }
        }
        None
    }
}

/// Non-fatal problems detected while loading configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigWarning {
    /// A Quick Connect history entry was dropped because its target is invalid.
    InvalidQuickConnectTarget {
        target: String,
        error: QuickConnectError,
    },
    /// A quick link was dropped because its target is invalid.
    InvalidQuickLinkTarget {
        id: String,
        target: String,
        error: QuickConnectError,
    },
    /// A color value was not `#RRGGBB` and was cleared.
    InvalidColor {
        location: String,
        field: &'static str,
        value: String,
    },
    /// An ANSI palette entry was not `#RRGGBB`; the whole palette was cleared.
    InvalidAnsiColor {
        location: String,
        index: usize,
        value: String,
    },
    /// A font size of `0` was cleared.
    InvalidFontSize { location: String },
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuickConnectTarget { target, error } => write!(
                f,
                "dropped invalid quick connect history target '{target}': {error}"
            ),
            Self::InvalidQuickLinkTarget { id, target, error } => write!(
                f,
                "dropped quick link '{id}' with invalid target '{target}': {error}"
            ),
            Self::InvalidColor {
                location,
                field,
                value,
            } => write!(
                f,
                "cleared invalid color '{value}' at {location}.{field} (expected #RRGGBB)"
            ),
            Self::InvalidAnsiColor {
                location,
                index,
                value,
            } => write!(
                f,
                "cleared ANSI palette at {location} because entry {index} ('{value}') is not #RRGGBB"
            ),
            Self::InvalidFontSize { location } => {
                write!(f, "cleared font size 0 at {location}.font_size")
            }
        }
    }
}

/// Schema/migration parse errors.
#[derive(Debug)]
pub enum ConfigSchemaError {
    TomlDe(toml::de::Error),
    UnsupportedVersion(u32),
    MissingVersion,
}

impl fmt::Display for ConfigSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TomlDe(error) => write!(f, "invalid TOML configuration: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported configuration schema version {version}")
            }
            Self::MissingVersion => f.write_str("missing configuration schema_version"),
        }
    }
}

impl std::error::Error for ConfigSchemaError {}

impl From<toml::de::Error> for ConfigSchemaError {
    fn from(value: toml::de::Error) -> Self {
        Self::TomlDe(value)
    }
}

/// Migrates a parsed TOML value into the current schema.
pub fn migrate_value(value: toml::Value) -> Result<ConfigDocument, ConfigSchemaError> {
    migrate_value_with_warnings(value).map(|(document, _)| document)
}

/// Migrates a parsed TOML value into the current schema and reports warnings.
///
/// Older schema versions are read as-is: every section added since version 1
/// carries `#[serde(default)]`, so missing sections fall back to defaults and
/// only the version marker needs rewriting.
pub fn migrate_value_with_warnings(
    mut value: toml::Value,
) -> Result<(ConfigDocument, Vec<ConfigWarning>), ConfigSchemaError> {
    let version = value
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        .ok_or(ConfigSchemaError::MissingVersion)?;
    if version < 0 {
        return Err(ConfigSchemaError::UnsupportedVersion(version as u32));
    }
    let version = version as u32;
    if version > SCHEMA_VERSION {
        return Err(ConfigSchemaError::UnsupportedVersion(version));
    }

    value["schema_version"] = toml::Value::Integer(i64::from(SCHEMA_VERSION));
    let mut document: ConfigDocument =
        toml::Value::try_into(value).map_err(ConfigSchemaError::TomlDe)?;
    let warnings = document.sanitize();
    Ok((document, warnings))
}

/// Returns the nearest present level, falling back to the document defaults.
fn nearest_or<'a, T>(levels: impl IntoIterator<Item = Option<&'a T>>, fallback: &'a T) -> &'a T {
    levels.into_iter().flatten().next().unwrap_or(fallback)
}

/// Resolves the terminal profile: session > nearest ancestor folder > … > root
/// folder > global > built-in defaults.
fn resolve_terminal_profile(
    session: &SessionProfile,
    folder_chain: &[&FolderProfile],
    global: &TerminalProfile,
) -> TerminalProfile {
    // `folder_chain` is nearest-ancestor-first; folding from the root down lets
    // every nearer level override the resolved farther levels.
    let mut resolved = global.clone();
    for folder in folder_chain.iter().rev() {
        if let Some(terminal) = folder.terminal.as_ref() {
            resolved = terminal.clone().inherit_from(&resolved);
        }
    }
    if let Some(terminal) = session.terminal.as_ref() {
        resolved = terminal.clone().inherit_from(&resolved);
    }
    resolved
}

/// Clears terminal theme values that are not `#RRGGBB`, recording warnings.
fn sanitize_terminal_colors(
    terminal: &mut TerminalProfile,
    location: &str,
    warnings: &mut Vec<ConfigWarning>,
) {
    let fields: [(&'static str, &mut Option<String>); 4] = [
        ("foreground", &mut terminal.foreground),
        ("background", &mut terminal.background),
        ("cursor", &mut terminal.cursor),
        ("selection", &mut terminal.selection),
    ];
    for (field, slot) in fields {
        let invalid = slot.as_deref().filter(|value| !is_hex_color(value));
        if let Some(value) = invalid {
            warnings.push(ConfigWarning::InvalidColor {
                location: location.to_owned(),
                field,
                value: value.to_owned(),
            });
            *slot = None;
        }
    }

    if let Some(ansi) = terminal.ansi.as_ref() {
        if let Some((index, value)) = ansi
            .iter()
            .enumerate()
            .find(|(_, value)| !is_hex_color(value))
        {
            warnings.push(ConfigWarning::InvalidAnsiColor {
                location: location.to_owned(),
                index,
                value: value.clone(),
            });
            terminal.ansi = None;
        }
    }

    if terminal.font_size == Some(0) {
        warnings.push(ConfigWarning::InvalidFontSize {
            location: location.to_owned(),
        });
        terminal.font_size = None;
    }
}

/// Recursively sanitizes a folder and every session/descendant folder.
fn sanitize_folder(folder: &mut FolderProfile, prefix: &str, warnings: &mut Vec<ConfigWarning>) {
    if let Some(section) = folder.terminal.as_mut() {
        let location = format!("{prefix}.terminal");
        sanitize_terminal_colors(section, &location, warnings);
    }
    for session in &mut folder.sessions {
        if let Some(section) = session.terminal.as_mut() {
            let location = format!("{prefix}.sessions[{}].terminal", session.id);
            sanitize_terminal_colors(section, &location, warnings);
        }
    }
    for child in &mut folder.folders {
        let child_prefix = format!("{prefix}.folders[{}]", child.id);
        sanitize_folder(child, &child_prefix, warnings);
    }
}

/// Returns whether `value` is a `#RRGGBB` color string.
fn is_hex_color(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    digits.len() == 6 && digits.chars().all(|ch| ch.is_ascii_hexdigit())
}

const fn default_ssh_port() -> u16 {
    22
}

fn default_theme() -> String {
    "system".to_owned()
}

fn default_terminal_font_family() -> String {
    "monospace".to_owned()
}

const fn default_font_size() -> u16 {
    13
}

const fn default_true() -> bool {
    true
}

fn default_log_format() -> String {
    "text".to_owned()
}

const fn default_scrollback_lines() -> usize {
    10_000
}

const fn default_scrollback_max_cells() -> usize {
    2_000_000
}

const fn default_quick_connect_limit() -> usize {
    20
}

const fn default_left_width() -> u32 {
    240
}

const fn default_right_width() -> u32 {
    340
}

fn default_left_panels() -> Vec<PanelSlot> {
    vec![PanelSlot {
        panel: PanelId::Sessions,
        collapsed: false,
    }]
}

fn default_right_panels() -> Vec<PanelSlot> {
    [PanelId::Sftp, PanelId::Tunnels, PanelId::QuickCommands]
        .into_iter()
        .map(|panel| PanelSlot {
            panel,
            collapsed: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> ConfigDocument {
        let mut document = ConfigDocument {
            sftp: SftpProfile {
                enabled: true,
                initial_directory: Some("/srv".to_owned()),
            },
            ..ConfigDocument::default()
        };
        document.auth_profiles.insert(
            "auth-1".to_owned(),
            AuthProfile {
                id: "auth-1".to_owned(),
                name: "Key".to_owned(),
                method: AuthMethod::Agent,
            },
        );
        let mut session = SessionProfile::new("session-1", "Prod", "example.com");
        session.auth_profile_id = Some("auth-1".to_owned());
        document.folders.push(FolderProfile {
            sessions: vec![session],
            ..FolderProfile::new("folder-1", "Servers")
        });
        document
    }

    fn sample_ui() -> UiProfile {
        UiProfile {
            new_tab_mode: NewTabMode::SessionEditor,
            layout: LayoutProfile {
                left: vec![
                    PanelSlot {
                        panel: PanelId::Sessions,
                        collapsed: true,
                    },
                    PanelSlot {
                        panel: PanelId::Tunnels,
                        collapsed: false,
                    },
                ],
                right: vec![PanelSlot {
                    panel: PanelId::Transfers,
                    collapsed: false,
                }],
                left_ratios: vec![0.4],
                right_ratios: Vec::new(),
                left_width: 260,
                right_width: 380,
                narrow_collapsed_side: Some(PanelSide::Right),
            },
            multi_window_enabled: false,
        }
    }

    #[test]
    fn toml_round_trip_preserves_profiles() {
        let mut document = sample_document();
        document.ui = sample_ui();
        document.quick_connect = QuickConnectProfile {
            enabled: false,
            limit: 5,
            history: vec![QuickConnectEntry {
                target: "alice@example.com:2200".to_owned(),
                last_used_at: 1_725_000_000,
                use_count: 3,
            }],
        };
        document.quick_links = vec![
            QuickLink {
                id: "link-1".to_owned(),
                label: "Prod".to_owned(),
                target: "deploy@prod.example.com:22".to_owned(),
                sort_order: 1,
            },
            QuickLink {
                id: "link-2".to_owned(),
                label: "Lab".to_owned(),
                target: "lab.example.com:2200".to_owned(),
                sort_order: 2,
            },
        ];
        document.keys = vec![KeyProfile {
            id: "key-1".to_owned(),
            label: "Work".to_owned(),
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:abc".to_owned(),
            public_key: "ssh-ed25519 AAAA work".to_owned(),
            secret_ref: "local://yshell/keys/key-1/material".to_owned(),
            comment: Some("work key".to_owned()),
        }];
        document.folders[0].terminal = Some(TerminalProfile {
            font_size: Some(15),
            ..TerminalProfile::default()
        });
        document
            .find_session_mut("session-1")
            .expect("session")
            .terminal = Some(TerminalProfile {
            color_scheme: Some("Ayu Mirage".to_owned()),
            foreground: Some("#c0c0c0".to_owned()),
            background: None,
            cursor: Some("#ffffff".to_owned()),
            selection: None,
            ansi: Some(std::array::from_fn(|_| "#000000".to_owned())),
            font_family: Some("JetBrains Mono".to_owned()),
            font_size: Some(14),
            fallback_fonts: Some(vec!["Noto Sans Mono CJK".to_owned()]),
            ..TerminalProfile::default()
        });

        let toml = document.to_toml_string().expect("serialize");
        let parsed = ConfigDocument::from_toml_str(&toml).expect("parse");

        assert_eq!(parsed, document);
    }

    #[test]
    fn resolves_document_defaults_and_references() {
        let resolved = sample_document()
            .resolve_session("session-1")
            .expect("session should resolve");

        assert_eq!(resolved.sftp.initial_directory.as_deref(), Some("/srv"));
        assert_eq!(resolved.auth.expect("auth").name, "Key");
    }

    #[test]
    fn terminal_profile_defaults_and_overrides_resolve() {
        let mut document = sample_document();
        assert_eq!(document.terminal.scrollback_lines, 10_000);
        assert_eq!(document.terminal.scrollback_max_cells, 2_000_000);
        assert_eq!(
            document
                .resolve_session("session-1")
                .expect("resolve")
                .terminal,
            TerminalProfile::default()
        );

        document.terminal.scrollback_lines = 500;
        document.terminal.scrollback_max_cells = 50_000;
        document.terminal.font_family = Some("Global Mono".to_owned());
        document
            .find_session_mut("session-1")
            .expect("session")
            .terminal = Some(TerminalProfile {
            scrollback_lines: 42,
            scrollback_max_cells: 4_200,
            font_size: Some(16),
            ..TerminalProfile::default()
        });

        let resolved = document.resolve_session("session-1").expect("resolve");
        assert_eq!(resolved.terminal.scrollback_lines, 42);
        assert_eq!(resolved.terminal.scrollback_max_cells, 4_200);
        assert_eq!(resolved.terminal.font_size, Some(16));
        // A nearer level that omits an optional field inherits it.
        assert_eq!(
            resolved.terminal.font_family.as_deref(),
            Some("Global Mono")
        );

        let toml = document.to_toml_string().expect("serialize");
        let reparsed = ConfigDocument::from_toml_str(&toml).expect("parse");
        assert_eq!(reparsed, document);
    }

    #[test]
    fn draft_resolution_uses_target_folder_chain_and_document_defaults() {
        let mut document = sample_document();
        // root folder -> child folder, both with terminal overrides.
        document
            .find_folder_mut("folder-1")
            .expect("folder")
            .terminal = Some(TerminalProfile {
            color_scheme: Some("nord".to_owned()),
            font_size: Some(13),
            ..TerminalProfile::default()
        });
        document
            .find_folder_mut("folder-1")
            .expect("folder")
            .folders
            .push(FolderProfile {
                terminal: Some(TerminalProfile {
                    color_scheme: Some("dracula".to_owned()),
                    ..TerminalProfile::default()
                }),
                ..FolderProfile::new("folder-2", "Nested")
            });
        document.terminal.font_family = Some("Global Mono".to_owned());

        let chain: Vec<&str> = document
            .folder_chain_to("folder-2")
            .iter()
            .map(|folder| folder.id.as_str())
            .collect();
        assert_eq!(chain, ["folder-2", "folder-1"], "nearest ancestor first");

        let draft = SessionProfile::new("draft", "Draft", "example.com");
        let resolved = document.resolve_session_draft(&draft, "folder-2");
        assert_eq!(resolved.terminal.color_scheme.as_deref(), Some("dracula"));
        // The draft inherits the nested chain: folder-2 -> folder-1 -> global.
        assert_eq!(resolved.terminal.font_size, Some(13));
        assert_eq!(
            resolved.terminal.font_family.as_deref(),
            Some("Global Mono"),
            "the draft inherits document defaults"
        );

        // A draft override wins over the whole chain but keeps the other fields.
        let mut draft = draft;
        draft.terminal = Some(TerminalProfile {
            color_scheme: Some("one-dark".to_owned()),
            ..TerminalProfile::default()
        });
        let resolved = document.resolve_session_draft(&draft, "folder-2");
        assert_eq!(resolved.terminal.color_scheme.as_deref(), Some("one-dark"));
        assert_eq!(resolved.terminal.font_size, Some(13));

        // Unknown target folder: the draft still resolves against the document.
        let resolved = document.resolve_session_draft(&draft, "missing-folder");
        assert_eq!(resolved.terminal.color_scheme.as_deref(), Some("one-dark"));
        assert_eq!(
            resolved.terminal.font_family.as_deref(),
            Some("Global Mono")
        );
        assert!(document.folder_chain_to("missing-folder").is_empty());
    }

    #[test]
    fn folder_chain_to_returns_empty_for_unknown_ids() {
        let document = sample_document();
        assert!(document.folder_chain_to("nope").is_empty());
        assert_eq!(document.folder_chain_to("folder-1").len(), 1);
    }

    #[test]
    fn terminal_profile_defaults_fill_missing_toml_keys() {
        let parsed = ConfigDocument::from_toml_str("schema_version = 1\n").expect("parse");
        assert_eq!(parsed.terminal, TerminalProfile::default());
    }

    #[test]
    fn keyboard_interactive_auth_profile_round_trips() {
        let document = ConfigDocument {
            auth_profiles: BTreeMap::from([(
                "auth-kbdint".to_owned(),
                AuthProfile {
                    id: "auth-kbdint".to_owned(),
                    name: "Keyboard Interactive".to_owned(),
                    method: AuthMethod::KeyboardInteractive {
                        secret_key: "local://yshell/auth-kbdint/keyboard-interactive".to_owned(),
                    },
                },
            )]),
            ..ConfigDocument::default()
        };

        let toml = document.to_toml_string().expect("serialize");
        let parsed = ConfigDocument::from_toml_str(&toml).expect("parse");

        assert_eq!(parsed, document);
    }

    #[test]
    fn batch_edits_basic_fields() {
        let mut document = sample_document();
        let edit = SessionBasicEdit {
            port: Some(2200),
            username: Some("deploy".to_owned()),
            username_set: true,
            ..SessionBasicEdit::default()
        };

        assert_eq!(
            document.batch_edit_basic(["session-1"].into_iter(), &edit),
            1
        );
        let session = document.find_session("session-1").expect("edited");
        assert_eq!(session.port, 2200);
        assert_eq!(session.username.as_deref(), Some("deploy"));
    }

    #[test]
    fn removes_session_from_folder_tree() {
        let mut document = sample_document();

        let removed = document
            .remove_session("session-1")
            .expect("removed session");

        assert_eq!(removed.id, "session-1");
        assert!(document.find_session("session-1").is_none());
    }

    #[test]
    fn migrates_schema_zero_to_current() {
        let parsed = ConfigDocument::from_toml_str("schema_version = 0\n").expect("migration");

        assert_eq!(SCHEMA_VERSION, 2);
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert!(parsed.folders.is_empty());
    }

    #[test]
    fn migrates_schema_one_document_preserving_data() {
        let input = r#"
schema_version = 1

[terminal]
scrollback_lines = 2_000

[[folders]]
id = "root"
name = "Root"

[[folders.sessions]]
id = "legacy"
name = "Legacy"
host = "example.test"
auth_profile_id = "auth-key"

[folders.sessions.terminal]
scrollback_lines = 500

[auth_profiles.auth-key]
id = "auth-key"
name = "Legacy key"
[auth_profiles.auth-key.method]
type = "private_key"
path = "/home/me/.ssh/id_ed25519"
"#;

        let parsed = ConfigDocument::from_toml_str(input).expect("v1 migration");

        assert_eq!(parsed.schema_version, 2);
        assert_eq!(parsed.terminal.scrollback_lines, 2_000);
        let session = parsed.find_session("legacy").expect("session survives");
        assert_eq!(session.host, "example.test");
        assert_eq!(
            session
                .terminal
                .as_ref()
                .expect("override")
                .scrollback_lines,
            500
        );
        assert_eq!(parsed.ui, UiProfile::default());
        assert_eq!(parsed.quick_connect, QuickConnectProfile::default());
        assert!(parsed.quick_links.is_empty());
        assert!(parsed.keys.is_empty());

        let auth = parsed.auth_profiles.get("auth-key").expect("auth");
        assert_eq!(
            auth.method.private_key_path(),
            Some("/home/me/.ssh/id_ed25519")
        );
        assert_eq!(auth.method.private_key_id(), None);
    }

    #[test]
    fn rejects_future_schema_version() {
        let error = ConfigDocument::from_toml_str("schema_version = 99\n")
            .expect_err("future version must be rejected");

        assert!(matches!(error, ConfigSchemaError::UnsupportedVersion(99)));
    }

    #[test]
    fn ui_section_defaults_and_round_trips() {
        let defaulted = ConfigDocument::from_toml_str("schema_version = 2\n").expect("parse");
        assert_eq!(defaulted.ui.new_tab_mode, NewTabMode::QuickConnect);
        assert!(defaulted.ui.multi_window_enabled);
        assert_eq!(
            defaulted.ui.layout.left,
            vec![PanelSlot {
                panel: PanelId::Sessions,
                collapsed: false
            }]
        );
        assert_eq!(
            defaulted.ui.layout.right,
            vec![
                PanelSlot {
                    panel: PanelId::Sftp,
                    collapsed: false
                },
                PanelSlot {
                    panel: PanelId::Tunnels,
                    collapsed: false
                },
                PanelSlot {
                    panel: PanelId::QuickCommands,
                    collapsed: false
                },
            ]
        );
        assert_eq!(defaulted.ui.layout.left_width, 240);
        assert_eq!(defaulted.ui.layout.right_width, 340);
        assert_eq!(defaulted.ui.layout.narrow_collapsed_side, None);

        let document = ConfigDocument {
            ui: sample_ui(),
            ..ConfigDocument::default()
        };
        let toml = document.to_toml_string().expect("serialize");
        // `new_tab_mode` uses the documented kebab-case spelling.
        assert!(toml.contains("new_tab_mode = \"session-editor\""));
        assert!(toml.contains("narrow_collapsed_side = \"right\""));
        let reparsed = ConfigDocument::from_toml_str(&toml).expect("parse");
        assert_eq!(reparsed.ui, document.ui);
    }

    #[test]
    fn ui_section_parses_partial_values() {
        let parsed = ConfigDocument::from_toml_str(
            r#"
schema_version = 2

[ui]
new_tab_mode = "session-editor"
multi_window_enabled = false

[ui.layout]
left_width = 300

[[ui.layout.right]]
panel = "quick_commands"
collapsed = true
"#,
        )
        .expect("parse");

        assert_eq!(parsed.ui.new_tab_mode, NewTabMode::SessionEditor);
        assert!(!parsed.ui.multi_window_enabled);
        assert_eq!(parsed.ui.layout.left_width, 300);
        // Unspecified fields keep their defaults.
        assert_eq!(parsed.ui.layout.left.len(), 1);
        assert_eq!(parsed.ui.layout.right_width, 340);
        assert_eq!(
            parsed.ui.layout.right,
            vec![PanelSlot {
                panel: PanelId::QuickCommands,
                collapsed: true
            }]
        );
    }

    #[test]
    fn panel_id_serializes_as_snake_case() {
        let cases = [
            (PanelId::Sessions, "sessions"),
            (PanelId::Sftp, "sftp"),
            (PanelId::Tunnels, "tunnels"),
            (PanelId::QuickCommands, "quick_commands"),
            (PanelId::Transfers, "transfers"),
        ];
        for (panel, expected) in cases {
            let value = toml::Value::try_from(panel).expect("serialize");
            assert_eq!(value.as_str(), Some(expected));
            assert_eq!(PanelId::as_str(panel), expected);
        }
    }

    #[test]
    fn quick_connect_defaults_and_round_trip() {
        let defaulted = QuickConnectProfile::default();
        assert!(defaulted.enabled);
        assert_eq!(defaulted.limit, 20);
        assert!(defaulted.history.is_empty());

        let document = ConfigDocument {
            quick_connect: QuickConnectProfile {
                enabled: true,
                limit: 20,
                history: vec![QuickConnectEntry {
                    target: "root@1.1.1.1:12345".to_owned(),
                    last_used_at: 42,
                    use_count: 2,
                }],
            },
            quick_links: vec![QuickLink {
                id: "link-1".to_owned(),
                label: "Router".to_owned(),
                target: "root@1.1.1.1:12345".to_owned(),
                sort_order: -1,
            }],
            ..ConfigDocument::default()
        };

        let toml = document.to_toml_string().expect("serialize");
        let reparsed = ConfigDocument::from_toml_str(&toml).expect("parse");
        assert_eq!(reparsed, document);
    }

    #[test]
    fn sanitize_normalizes_and_drops_invalid_quick_targets() {
        let input = r#"
schema_version = 2

[quick_connect]
limit = 20

[[quick_connect.history]]
target = "ssh://bob@example.com:2222/ignored"
use_count = 1

[[quick_connect.history]]
target = "not a host"

[[quick_links]]
id = "good"
label = "Good"
target = "example.com"

[[quick_links]]
id = "bad"
label = "Bad"
target = "bad host"
"#;

        let (parsed, warnings) = ConfigDocument::from_toml_str_with_warnings(input).expect("parse");

        assert_eq!(parsed.quick_connect.history.len(), 1);
        assert_eq!(
            parsed.quick_connect.history[0].target,
            "bob@example.com:2222"
        );
        assert_eq!(parsed.quick_links.len(), 1);
        assert_eq!(parsed.quick_links[0].target, "example.com:22");
        assert!(warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::InvalidQuickConnectTarget { target, .. } if target == "not a host"
        )));
        assert!(warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::InvalidQuickLinkTarget { id, .. } if id == "bad"
        )));
    }

    #[test]
    fn folder_inheritance_resolves_nearest_overrides_field_by_field() {
        let input = r##"
schema_version = 2

[terminal]
scrollback_lines = 5_000
scrollback_max_cells = 100_000
font_family = "Global Mono"
color_scheme = "Global Scheme"

[appearance]
theme = "dark"

[logging]
enabled = true
format = "raw"

[[folders]]
id = "root"
name = "Root"

[folders.terminal]
font_size = 16

[[folders.folders]]
id = "child"
name = "Child"

[folders.folders.terminal]
color_scheme = "Child Scheme"

[folders.folders.appearance]
theme = "light"

[folders.folders.logging]
enabled = false
format = "text"

[[folders.folders.folders]]
id = "leaf"
name = "Leaf"

[folders.folders.folders.terminal]
background = "#101010"

[[folders.folders.folders.sessions]]
id = "s1"
name = "Nested"
host = "example.test"

[folders.folders.folders.sessions.terminal]
foreground = "#abcdef"
font_size = 18

[[folders.sessions]]
id = "s2"
name = "Root level"
host = "example.test"
"##;

        let document = ConfigDocument::from_toml_str(input).expect("parse");

        let nested = document.resolve_session("s1").expect("resolve nested");
        assert_eq!(nested.terminal.foreground.as_deref(), Some("#abcdef"));
        // Session level wins over the folder levels.
        assert_eq!(nested.terminal.font_size, Some(18));
        assert_eq!(nested.terminal.background.as_deref(), Some("#101010"));
        assert_eq!(
            nested.terminal.color_scheme.as_deref(),
            Some("Child Scheme")
        );
        // Unset at every nearer level, so the global value survives.
        assert_eq!(nested.terminal.font_family.as_deref(), Some("Global Mono"));
        // Appearance/logging resolve to the nearest ancestor section that sets
        // the profile at all (child), not the global section.
        assert_eq!(nested.appearance.theme, "light");
        assert_eq!(nested.appearance.font_size, 13);
        assert!(!nested.logging.enabled);
        assert_eq!(nested.logging.format, "text");

        let root_level = document.resolve_session("s2").expect("resolve root");
        assert_eq!(root_level.terminal.font_size, Some(16));
        assert_eq!(
            root_level.terminal.color_scheme.as_deref(),
            Some("Global Scheme")
        );
        assert_eq!(root_level.appearance.theme, "dark");
        assert!(root_level.logging.enabled);
        assert_eq!(root_level.logging.format, "raw");
    }

    #[test]
    fn folder_without_overrides_inherits_global_profiles() {
        let input = r##"
schema_version = 2

[appearance]
theme = "dark"
font_family = "Global Mono"
font_size = 15

[[folders]]
id = "root"
name = "Root"

[[folders.sessions]]
id = "s1"
name = "S"
host = "example.test"
"##;

        let document = ConfigDocument::from_toml_str(input).expect("parse");
        let resolved = document.resolve_session("s1").expect("resolve");

        assert_eq!(resolved.appearance.theme, "dark");
        assert_eq!(resolved.appearance.font_family, "Global Mono");
        assert_eq!(resolved.appearance.font_size, 15);
    }

    #[test]
    fn session_without_overrides_uses_global_profiles() {
        let document = sample_document();
        let resolved = document.resolve_session("session-1").expect("resolve");

        assert_eq!(resolved.terminal, document.terminal);
        assert_eq!(resolved.appearance, document.appearance);
        assert_eq!(resolved.logging, document.logging);
    }

    #[test]
    fn sanitize_clears_invalid_colors_and_falls_back() {
        let input = r##"
schema_version = 2

[terminal]
foreground = "red"
font_size = 0

[[folders]]
id = "root"
name = "Root"

[[folders.sessions]]
id = "s1"
name = "S"
host = "example.test"

[folders.sessions.terminal]
background = "#12345"
ansi = ["#000000", "#111111", "#222222", "#333333", "#444444", "#555555", "#666666", "#777777", "#888888", "#999999", "#aaaaaa", "#bbbbbb", "#cccccc", "#dddddd", "#eeeeee", "not-a-color"]
"##;

        let (parsed, warnings) = ConfigDocument::from_toml_str_with_warnings(input).expect("parse");

        assert_eq!(parsed.terminal.foreground, None);
        assert_eq!(parsed.terminal.font_size, None);
        let session = parsed.find_session("s1").expect("session");
        let terminal = session.terminal.as_ref().expect("override");
        assert_eq!(terminal.background, None);
        assert_eq!(terminal.ansi, None);
        assert!(warnings.iter().any(|warning| matches!(
            warning,
            ConfigWarning::InvalidColor {
                field: "foreground",
                ..
            }
        )));
        assert!(warnings
            .iter()
            .any(|warning| matches!(warning, ConfigWarning::InvalidAnsiColor { index: 15, .. })));
        assert!(warnings
            .iter()
            .any(|warning| matches!(warning, ConfigWarning::InvalidFontSize { .. })));

        // The cleared values fall back to the next level during resolution.
        let resolved = parsed.resolve_session("s1").expect("resolve");
        assert_eq!(resolved.terminal.foreground, None);
        assert_eq!(resolved.terminal.ansi, None);
    }

    #[test]
    fn keys_round_trip_and_resolve_by_id() {
        let document = ConfigDocument {
            keys: vec![KeyProfile {
                id: "key-1".to_owned(),
                label: "Ops".to_owned(),
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "SHA256:xyz".to_owned(),
                public_key: "ssh-ed25519 AAAA ops".to_owned(),
                secret_ref: "local://yshell/keys/key-1/material".to_owned(),
                comment: None,
            }],
            auth_profiles: BTreeMap::from([(
                "auth-1".to_owned(),
                AuthProfile {
                    id: "auth-1".to_owned(),
                    name: "Ops".to_owned(),
                    method: AuthMethod::PrivateKey {
                        key_id: Some("key-1".to_owned()),
                        path: String::new(),
                        passphrase_secret_key: Some(
                            "local://yshell/keys/key-1/passphrase".to_owned(),
                        ),
                    },
                },
            )]),
            ..ConfigDocument::default()
        };

        let toml = document.to_toml_string().expect("serialize");
        let reparsed = ConfigDocument::from_toml_str(&toml).expect("parse");

        assert_eq!(reparsed, document);
        assert_eq!(reparsed.find_key("key-1").expect("key").label, "Ops");
        let auth = reparsed.auth_profiles.get("auth-1").expect("auth");
        assert_eq!(auth.method.private_key_id(), Some("key-1"));
        assert_eq!(auth.method.private_key_path(), None);
    }

    #[test]
    fn private_key_without_path_still_parses() {
        let input = r#"
schema_version = 2

[auth_profiles.auth-1]
id = "auth-1"
name = "Key only"
[auth_profiles.auth-1.method]
type = "private_key"
key_id = "key-1"
"#;

        let parsed = ConfigDocument::from_toml_str(input).expect("parse");
        let auth = parsed.auth_profiles.get("auth-1").expect("auth");

        assert_eq!(auth.method.private_key_id(), Some("key-1"));
        assert_eq!(auth.method.private_key_path(), None);
        assert!(matches!(
            &auth.method,
            AuthMethod::PrivateKey { path, .. } if path.is_empty()
        ));
    }

    #[test]
    fn private_key_source_prefers_key_id_and_falls_back_to_path() {
        let document = ConfigDocument {
            keys: vec![KeyProfile {
                id: "key-1".to_owned(),
                label: "Managed".to_owned(),
                algorithm: "ssh-ed25519".to_owned(),
                fingerprint: "SHA256:x".to_owned(),
                public_key: "ssh-ed25519 AAAA".to_owned(),
                secret_ref: "local://yshell/keys/key-1/material".to_owned(),
                comment: None,
            }],
            ..ConfigDocument::default()
        };

        let managed = AuthMethod::PrivateKey {
            key_id: Some("key-1".to_owned()),
            path: "/legacy/key".to_owned(),
            passphrase_secret_key: None,
        };
        assert!(matches!(
            document.resolve_private_key_source(&managed),
            Some(PrivateKeySource::Managed(key)) if key.label == "Managed"
        ));

        let dangling = AuthMethod::PrivateKey {
            key_id: Some("missing".to_owned()),
            path: "/legacy/key".to_owned(),
            passphrase_secret_key: None,
        };
        assert!(matches!(
            document.resolve_private_key_source(&dangling),
            Some(PrivateKeySource::Path("/legacy/key"))
        ));

        let path_only = AuthMethod::PrivateKey {
            key_id: None,
            path: "/legacy/key".to_owned(),
            passphrase_secret_key: None,
        };
        assert!(matches!(
            document.resolve_private_key_source(&path_only),
            Some(PrivateKeySource::Path("/legacy/key"))
        ));

        let empty = AuthMethod::PrivateKey {
            key_id: None,
            path: String::new(),
            passphrase_secret_key: None,
        };
        assert_eq!(document.resolve_private_key_source(&empty), None);
        assert_eq!(
            document.resolve_private_key_source(&AuthMethod::Agent),
            None
        );
    }

    #[test]
    fn missing_schema_version_is_rejected() {
        let error = ConfigDocument::from_toml_str("[terminal]\nscrollback_lines = 5\n")
            .expect_err("missing version");

        assert!(matches!(error, ConfigSchemaError::MissingVersion));
    }

    #[test]
    fn inherit_from_fills_only_unset_fields() {
        let nearer = TerminalProfile {
            font_size: Some(14),
            ..TerminalProfile::default()
        };
        let farther = TerminalProfile {
            font_size: Some(11),
            font_family: Some("Farther".to_owned()),
            scrollback_lines: 777,
            ..TerminalProfile::default()
        };

        let merged = nearer.inherit_from(&farther);

        assert_eq!(merged.font_size, Some(14));
        assert_eq!(merged.font_family.as_deref(), Some("Farther"));
        // Scrollback is concrete and always comes from the nearer profile.
        assert_eq!(merged.scrollback_lines, 10_000);
    }
}
