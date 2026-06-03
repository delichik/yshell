//! Startup and app-shell bootstrap logic.

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};

#[derive(Debug)]
pub struct YShellApp {
    pub state: AppState,
}

impl YShellApp {
    pub fn run(self) -> AppResult<()> {
        println!(
            "YShell UI runtime is not wired yet. Config directory: {}",
            self.state.config_dir.display()
        );
        Ok(())
    }
}

pub fn bootstrap_app() -> AppResult<YShellApp> {
    init_logging()?;
    let config_dir = yshell_config::discover_config_dir().map_err(AppError::from_error)?;
    Ok(YShellApp {
        state: AppState::new(config_dir),
    })
}

pub fn init_logging() -> AppResult<()> {
    yshell_logging::init_tracing(&yshell_logging::LoggingConfig::default())
        .map_err(AppError::from_error)
}

#[cfg(test)]
mod tests {
    use yshell_config::discover_config_dir;

    #[test]
    fn discovers_a_config_dir_in_this_environment() {
        let path = discover_config_dir().expect("config directory should be discoverable");
        assert!(!path.as_os_str().is_empty());
    }
}
