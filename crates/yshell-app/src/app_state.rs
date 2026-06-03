//! Application state owned by the shell process.

use std::path::PathBuf;

use crate::{error::AppResult, runtime::AppRuntime};

#[derive(Debug)]
pub struct AppState {
    pub config_dir: PathBuf,
    pub runtime: AppRuntime,
}

impl AppState {
    pub fn new(config_dir: PathBuf) -> AppResult<Self> {
        let runtime = AppRuntime::new(config_dir.clone())?;
        Ok(Self {
            config_dir,
            runtime,
        })
    }
}
