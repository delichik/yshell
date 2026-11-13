//! GUI Shell role binary (`yshell-shell`).

// GUI process: never show a console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> yshell_app::AppResult<()> {
    yshell_app::run_shell_role()
}
