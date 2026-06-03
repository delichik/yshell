//! Configuration support for `YShell`.
//!
//! Milestone 0 provides deterministic configuration-directory discovery. Later
//! milestones will add schema types, migrations, and TOML persistence.

pub mod paths;
pub mod schema;

pub use paths::{discover_config_dir, ConfigPathError, ConfigPathResolver, OperatingSystem};
pub use schema::SCHEMA_VERSION;
