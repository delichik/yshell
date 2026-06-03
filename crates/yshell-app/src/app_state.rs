//! Application state owned by the shell process.

use std::path::PathBuf;

use yshell_ui::AppViewModel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub config_dir: PathBuf,
    pub view_model: AppViewModel,
}

impl AppState {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir,
            view_model: AppViewModel::default(),
        }
    }
}
