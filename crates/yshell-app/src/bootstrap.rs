//! Startup and app-shell bootstrap logic.

use std::{cell::RefCell, rc::Rc};

use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::runtime::{AppProjection, AppRuntime};

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
        let runtime = Rc::new(RefCell::new(self.state.runtime));
        let initial_projection = runtime.borrow().projection();
        apply_projection(&window, &initial_projection);
        wire_callbacks(&window, runtime);
        window.run().map_err(AppError::from_error)
    }
}

fn wire_callbacks(window: &MainWindow, runtime: Rc<RefCell<AppRuntime>>) {
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_quick_connect(move |input| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().handle_quick_connect(&input.to_string()) {
            Ok(projection) => {
                apply_projection(&window, &projection);
            }
            Err(error) => {
                window.set_status_text(format!("Quick Connect error: {error}").into());
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_new_session(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().handle_new_session();
            apply_projection(&window, &projection);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_save_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().save_active_session() {
            Ok(projection) => apply_projection(&window, &projection),
            Err(error) => window.set_status_text(format!("Save session error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_open_saved_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().open_first_saved_session() {
            Ok(projection) => apply_projection(&window, &projection),
            Err(error) => {
                window.set_status_text(format!("Open saved session error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_send_demo_input(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().send_active_terminal_input("pwd\n") {
            Ok(projection) => apply_projection(&window, &projection),
            Err(error) => window.set_status_text(format!("Terminal input error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_disconnect_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().disconnect_active_session() {
            Ok(projection) => apply_projection(&window, &projection),
            Err(error) => window.set_status_text(format!("Disconnect error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_reconnect_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().reconnect_active_session() {
            Ok(projection) => apply_projection(&window, &projection),
            Err(error) => window.set_status_text(format!("Reconnect error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_select_fake_backend(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_fake_transport_backend();
            apply_projection(&window, &projection);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_select_real_backend(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_real_transport_backend();
            apply_projection(&window, &projection);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_toggle_sftp(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_sftp();
            apply_projection(&window, &projection);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_toggle_tunnels(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_tunnels();
            apply_projection(&window, &projection);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_toggle_commands(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_commands();
            apply_projection(&window, &projection);
        }
    });
}

fn apply_projection(window: &MainWindow, projection: &AppProjection) {
    window.set_config_dir(projection.config_dir_text.clone().into());
    window.set_active_session(projection.active_session_text.clone().into());
    window.set_session_summary_text(projection.session_summary_text.clone().into());
    window.set_tab_text(projection.tab_text.clone().into());
    window.set_terminal_title_text(projection.terminal_title_text.clone().into());
    window.set_terminal_body_text(projection.terminal_body_text.clone().into());
    window.set_status_text(projection.status_text.clone().into());
    window.set_transport_backend_text(projection.transport_backend_text.clone().into());
    window.set_sftp_visible(projection.sftp_visible);
    window.set_tunnels_visible(projection.tunnels_visible);
    window.set_commands_visible(projection.commands_visible);
}

pub fn bootstrap_app() -> AppResult<YShellApp> {
    init_logging()?;
    let config_dir = yshell_config::discover_config_dir().map_err(AppError::from_error)?;
    Ok(YShellApp {
        state: AppState::new(config_dir)?,
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
