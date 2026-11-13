//! Headless Host role binary (`yshell-host`).

// Role children are GUI-subsystem processes so they never flash a console;
// the CLI re-attaches to the launching console only when it has to print.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> yshell_app::AppResult<()> {
    yshell_app::cli::run_with_role(yshell_app::host::ProcessRole::Host)
}
