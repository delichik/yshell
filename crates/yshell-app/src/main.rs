//! YShell application entry point.

// Hide the console window for the GUI process on Windows. CLI subcommands
// re-attach to the parent console so `--help` / `--version` still print.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app_state;
mod bootstrap;
mod connect_jobs;
mod dialog_windows;
mod error;
mod local_fs;
mod mcp_server;
mod runtime;
mod session_runtime;
mod sftp_jobs;
mod sftp_view;
mod win_chrome;
mod windows;

use std::env;
use std::path::PathBuf;

use bootstrap::bootstrap_app;
use error::{AppError, AppResult};
use runtime::AppRuntime;

fn main() -> AppResult<()> {
    let invocation = CliInvocation::parse(env::args().skip(1))?;
    if !matches!(invocation.command, CliCommand::RunShell) {
        attach_parent_console();
    }
    match invocation.command {
        CliCommand::CheckConfig => {
            let config_dir = resolve_config_dir(invocation.config_dir)?;
            println!("configuration OK: {}", config_dir.display());
            Ok(())
        }
        CliCommand::PrintConfigDir => {
            let config_dir = resolve_config_dir(invocation.config_dir)?;
            println!("{}", config_dir.display());
            Ok(())
        }
        CliCommand::QuickConnect(input) => {
            let config_dir = resolve_config_dir(invocation.config_dir)?;
            let mut runtime = AppRuntime::new(config_dir)?;
            let projection = runtime.handle_quick_connect(&input)?;
            println!(
                "quick connect pipeline initialized: {}",
                projection.status_text
            );
            Ok(())
        }
        CliCommand::Version => {
            println!("yshell {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        CliCommand::Help => {
            print_help();
            Ok(())
        }
        CliCommand::Mcp => {
            let config_dir = resolve_config_dir(invocation.config_dir)?;
            yshell_mcp::run_stdio_server(&config_dir).map_err(AppError::from_error)
        }
        CliCommand::RunShell => {
            let app = bootstrap_app(invocation.config_dir)?;
            app.run()
        }
    }
}

/// Resolves the configuration directory: explicit `--config-dir` or the
/// platform default. Application configuration never comes from environment
/// variables (only the `--config-dir` CLI argument is honoured).
fn resolve_config_dir(override_dir: Option<PathBuf>) -> AppResult<PathBuf> {
    match override_dir {
        Some(dir) => Ok(dir),
        None => yshell_config::discover_config_dir().map_err(AppError::from_error),
    }
}

/// Re-attach stdout/stderr to the launching console for CLI flags.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // AttachConsole is Win32 FFI; workspace denies unsafe by default.
    #[allow(unsafe_code)]
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
fn attach_parent_console() {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    CheckConfig,
    PrintConfigDir,
    QuickConnect(String),
    Version,
    Help,
    Mcp,
    RunShell,
}

/// Parsed command line: one optional command plus the `--config-dir` override.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CliInvocation {
    /// Test / smoke configuration directory (`--config-dir <path>`). This is a
    /// CLI argument on purpose: the product must not read its configuration
    /// from environment variables.
    config_dir: Option<PathBuf>,
    command: CliCommand,
}

impl CliInvocation {
    fn parse(args: impl IntoIterator<Item = String>) -> AppResult<Self> {
        let mut config_dir = None;
        let mut command = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--config-dir" => {
                    let value = args
                        .next()
                        .ok_or_else(|| AppError::new("--config-dir requires a directory path"))?;
                    if value.trim().is_empty() {
                        return Err(AppError::new(
                            "--config-dir requires a non-empty directory path",
                        ));
                    }
                    config_dir = Some(PathBuf::from(value));
                }
                "--check-config" => set_command(&mut command, CliCommand::CheckConfig)?,
                "--print-config-dir" => set_command(&mut command, CliCommand::PrintConfigDir)?,
                "--quick-connect" => {
                    let input = args.next().ok_or_else(|| {
                        AppError::new("--quick-connect requires an input such as user@example.com")
                    })?;
                    set_command(&mut command, CliCommand::QuickConnect(input))?;
                }
                "--version" | "-V" => set_command(&mut command, CliCommand::Version)?,
                "--help" | "-h" => set_command(&mut command, CliCommand::Help)?,
                "--mcp" => set_command(&mut command, CliCommand::Mcp)?,
                unknown => return Err(AppError::new(format!("unknown argument: {unknown}"))),
            }
        }
        Ok(Self {
            config_dir,
            command: command.unwrap_or(CliCommand::RunShell),
        })
    }
}

fn set_command(slot: &mut Option<CliCommand>, command: CliCommand) -> AppResult<()> {
    if slot.is_some() {
        return Err(AppError::new("only one command may be given"));
    }
    *slot = Some(command);
    Ok(())
}

fn print_help() {
    println!(
        "YShell\n\nUSAGE:\n    yshell [OPTIONS]\n\nOPTIONS:\n    --config-dir <path>       Use <path> as the configuration directory (test / smoke isolation)\n    --check-config            Validate configuration discovery\n    --print-config-dir        Print the configuration directory\n    --quick-connect <input>   Run the runtime quick-connect path without launching UI\n    --mcp                     Host-facing MCP stdio server (AI clients only; enable MCP in the GUI Settings first)\n    --version                 Print version\n    --help                    Print help\n\nCONFIGURATION:\n    All application settings (language, appearance, motion, fonts, local shell,\n    secret store) are read from the configuration directory only; YShell does\n    not use environment variables for configuration."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> AppResult<CliInvocation> {
        CliInvocation::parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn parses_quick_connect_input() {
        let invocation = parse(&["--quick-connect", "me@example"]).expect("parse");
        assert_eq!(
            invocation.command,
            CliCommand::QuickConnect("me@example".to_owned())
        );
        assert!(invocation.config_dir.is_none());
    }

    #[test]
    fn defaults_to_the_shell_with_the_platform_config_dir() {
        let invocation = parse(&[]).expect("parse");
        assert_eq!(invocation.command, CliCommand::RunShell);
        assert!(invocation.config_dir.is_none());
    }

    #[test]
    fn config_dir_works_before_or_after_the_command() {
        let before = parse(&["--config-dir", r"D:\tmp\smoke", "--print-config-dir"])
            .expect("parse with leading config dir");
        assert_eq!(before.command, CliCommand::PrintConfigDir);
        assert_eq!(
            before.config_dir.as_deref(),
            Some(std::path::Path::new(r"D:\tmp\smoke"))
        );

        let after = parse(&["--check-config", "--config-dir", "/tmp/smoke"]).expect("parse");
        assert_eq!(after.command, CliCommand::CheckConfig);
        assert_eq!(
            after.config_dir.as_deref(),
            Some(std::path::Path::new("/tmp/smoke"))
        );
    }

    #[test]
    fn config_dir_requires_a_non_empty_value() {
        assert!(parse(&["--config-dir"]).is_err());
        assert!(parse(&["--config-dir", "  "]).is_err());
    }

    #[test]
    fn rejects_unknown_and_duplicate_commands() {
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--version", "--help"]).is_err());
    }
}
