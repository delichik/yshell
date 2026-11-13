//! YShell application entry point.

// Hide the console window for the GUI process on Windows. CLI subcommands
// re-attach to the parent console so `--help` / `--version` still print.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> yshell_app::AppResult<()> {
    yshell_app::run()
}
