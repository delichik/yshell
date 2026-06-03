//! Startup and app-shell bootstrap logic.

use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};

mod generated_ui {
    #![allow(dead_code)]
    slint::include_modules!();
}

use generated_ui::MainWindow;

#[derive(Debug)]
pub struct YShellApp {
    pub state: AppState,
}

impl YShellApp {
    pub fn run(self) -> AppResult<()> {
        let window = MainWindow::new().map_err(AppError::from_error)?;
        window.set_config_dir(self.state.config_dir.display().to_string().into());
        window.set_status_text(
            format!(
                "Ready. Config: {}. Use Quick Connect to validate and open SSH targets.",
                self.state.config_dir.display()
            )
            .into(),
        );

        wire_callbacks(&window);
        window.run().map_err(AppError::from_error)
    }
}

fn wire_callbacks(window: &MainWindow) {
    let weak = window.as_weak();
    window.on_quick_connect(move |input| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let input = input.to_string();
        match yshell_config::parse_quick_connect(&input) {
            Ok(target) => {
                window.set_active_session(target.host.clone().into());
                window.set_status_text(
                    format!(
                        "Quick Connect validated: user={} host={} port={}",
                        target.username.as_deref().unwrap_or("<default>"),
                        target.host,
                        target.port
                    )
                    .into(),
                );
            }
            Err(error) => {
                window.set_status_text(format!("Quick Connect error: {error}").into());
            }
        }
    });

    let weak = window.as_weak();
    window.on_new_session(move || {
        if let Some(window) = weak.upgrade() {
            window.set_active_session("New Session".into());
            window
                .set_status_text("New session draft created. Fill connection details next.".into());
        }
    });

    let weak = window.as_weak();
    window.on_toggle_sftp(move || {
        if let Some(window) = weak.upgrade() {
            let visible = !window.get_sftp_visible();
            window.set_sftp_visible(visible);
            window.set_status_text(
                if visible {
                    "SFTP panel shown"
                } else {
                    "SFTP panel hidden"
                }
                .into(),
            );
        }
    });

    let weak = window.as_weak();
    window.on_toggle_tunnels(move || {
        if let Some(window) = weak.upgrade() {
            let visible = !window.get_tunnels_visible();
            window.set_tunnels_visible(visible);
            window.set_status_text(
                if visible {
                    "Tunnels panel shown"
                } else {
                    "Tunnels panel hidden"
                }
                .into(),
            );
        }
    });

    let weak = window.as_weak();
    window.on_toggle_commands(move || {
        if let Some(window) = weak.upgrade() {
            let visible = !window.get_commands_visible();
            window.set_commands_visible(visible);
            window.set_status_text(
                if visible {
                    "Quick Commands panel shown"
                } else {
                    "Quick Commands panel hidden"
                }
                .into(),
            );
        }
    });
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
