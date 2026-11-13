//! SessionWorker role binary (`yshell-session-worker`).

// Worker children are spawned with stdio null; keep them console-less.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> yshell_app::AppResult<()> {
    yshell_app::cli::run_with_role(yshell_app::host::ProcessRole::SessionWorker)
}
