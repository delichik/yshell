//! Configuration support for `YShell`.
//!
//! The crate owns versioned TOML configuration, profile inheritance, saved
//! session-tree editing, Quick Connect parsing, and file-store recovery.

pub mod paths;
pub mod quick_connect;
pub mod schema;
pub mod store;

pub use paths::{discover_config_dir, ConfigPathError, ConfigPathResolver, OperatingSystem};
pub use quick_connect::{parse_quick_connect, QuickConnectError, QuickConnectTarget};
pub use schema::{
    migrate_value, AppearanceProfile, AuthMethod, AuthProfile, ConfigDocument, ConfigSchemaError,
    FolderProfile, LoggingProfile, ProxyProfile, ResolvedSessionProfile, SessionBasicEdit,
    SessionProfile, SftpProfile, TunnelForward, TunnelForwardKind, TunnelProfile, SCHEMA_VERSION,
};
pub use store::{ConfigStore, ConfigStoreError, LoadOutcome};
