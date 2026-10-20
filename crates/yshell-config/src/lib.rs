//! Configuration support for `YShell`.
//!
//! The crate owns versioned TOML configuration, profile inheritance, saved
//! session-tree editing, Quick Connect parsing, and file-store recovery.

pub mod paths;
pub mod quick_connect;
pub mod schema;
pub mod store;
pub mod themes;

pub use paths::{discover_config_dir, ConfigPathError, ConfigPathResolver, OperatingSystem};
pub use quick_connect::{parse_quick_connect, QuickConnectError, QuickConnectTarget};
pub use schema::{
    clamp_sftp_max_parallel_transfers, migrate_value, migrate_value_with_warnings,
    AppearanceProfile, AuthMethod, AuthProfile, ConfigDocument, ConfigSchemaError, ConfigWarning,
    FolderProfile, HostKeyPolicy, KeyProfile, LayoutProfile, LoggingProfile, NewTabMode, PanelId,
    PanelSide, PanelSlot, PrivateKeySource, ProxyProfile, ProxyProtocol, QuickConnectEntry,
    QuickConnectProfile, ResolvedSessionProfile, SessionBasicEdit, SessionProfile, SftpProfile,
    TerminalProfile, TunnelForward, TunnelForwardKind, TunnelProfile, UiProfile,
    SFTP_MAX_PARALLEL_TRANSFERS_MAX, SFTP_MAX_PARALLEL_TRANSFERS_MIN, SCHEMA_VERSION,
};
pub use store::{ConfigStore, ConfigStoreError, LoadOutcome};
pub use themes::{
    allocate_user_theme_id, is_builtin_scheme_id, ColorTheme, ThemeCatalog, ThemeCatalogError,
    BUILTIN_SCHEME_IDS, THEMES_DIR_NAME,
};
