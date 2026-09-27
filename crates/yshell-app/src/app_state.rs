//! Application state owned by the shell process.

use crate::{error::AppResult, runtime::AppRuntime};

#[derive(Debug)]
pub struct AppState {
    pub runtime: AppRuntime,
}

impl AppState {
    pub fn new(config_dir: std::path::PathBuf) -> AppResult<Self> {
        let runtime = AppRuntime::new(config_dir)?;
        Ok(Self { runtime })
    }
}
