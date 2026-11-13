//! MCP daemon role binary (`yshell-mcp-daemon`).

// Daemon children are spawned with stdio null; keep them console-less.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> yshell_app::AppResult<()> {
    yshell_app::run_mcp_daemon_role()
}
