//! Shared CLI / launcher entry used by `yshell` and kept for `--host` etc. compat.

use std::env;
use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use crate::host::ProcessRole;

pub(crate) fn print_version() -> AppResult<()> {
    println!("yshell {}", env!("YSHELL_VERSION"));
    Ok(())
}

pub(crate) fn print_check_config(config_dir: Option<PathBuf>) -> AppResult<()> {
    let config_dir = resolve_config_dir(config_dir)?;
    println!("configuration OK: {}", config_dir.display());
    Ok(())
}

pub(crate) fn print_config_dir(config_dir: Option<PathBuf>) -> AppResult<()> {
    let config_dir = resolve_config_dir(config_dir)?;
    println!("{}", config_dir.display());
    Ok(())
}

/// Parses the argv for a role binary, defaulting the command to its role.
pub(crate) fn parse_for_role(role: ProcessRole) -> AppResult<CliInvocation> {
    CliInvocation::parse_with_role(env::args().skip(1), Some(role))
}

/// Handles the metadata commands shared by every role (--version / --help /
/// config-dir inspection). Returns None when the command is role-specific.
pub(crate) fn handle_metadata_command(
    command: &CliCommand,
    config_dir: Option<PathBuf>,
) -> Option<AppResult<()>> {
    match command {
        CliCommand::Version => Some(print_version()),
        CliCommand::Help => {
            print_help();
            Some(Ok(()))
        }
        CliCommand::CheckConfig => Some(print_check_config(config_dir)),
        CliCommand::PrintConfigDir => Some(print_config_dir(config_dir)),
        _ => None,
    }
}

/// Resolves the configuration directory: explicit `--config-dir` or the
/// platform default. Application configuration never comes from environment
/// variables (only the `--config-dir` CLI argument is honoured).
pub(crate) fn resolve_config_dir(override_dir: Option<PathBuf>) -> AppResult<PathBuf> {
    match override_dir {
        Some(dir) => Ok(dir),
        None => yshell_config::discover_config_dir().map_err(AppError::from_error),
    }
}

/// Re-attach stdout/stderr to the launching console for CLI flags.
#[cfg(windows)]
pub(crate) fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // AttachConsole is Win32 FFI; workspace denies unsafe by default.
    #[allow(unsafe_code)]
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
pub(crate) fn attach_parent_console() {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliCommand {
    CheckConfig,
    PrintConfigDir,
    QuickConnect(String),
    Version,
    Help,
    Mcp,
    /// Headless Host process (D38).
    Host,
    /// GUI Shell role (same binary path as default run for now).
    Shell,
    SessionWorker {
        session_id: String,
        generation: u64,
        parent_pid: u32,
        ipc_port: u16,
        ipc_token: String,
    },
    McpDaemon {
        bind_host: String,
        port: u16,
        token: String,
        generation: u64,
        parent_pid: u32,
    },
    /// Default entry: become the unique Host and spawn one GUI Shell window.
    HostEntry,
    /// Attach-only GUI Shell (Host-spawned; fails when no Host is running).
    AttachShell,
}

/// Parsed command line: one optional command plus the `--config-dir` override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliInvocation {
    /// Test / smoke configuration directory (`--config-dir <path>`). This is a
    /// CLI argument on purpose: the product must not read its configuration
    /// from environment variables.
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) command: CliCommand,
}

impl CliCommand {
    /// Command a role binary runs when no explicit command flag is given.
    pub(crate) fn for_role(role: ProcessRole) -> Self {
        match role {
            ProcessRole::Host => Self::Host,
            ProcessRole::Shell => Self::AttachShell,
            // Worker / daemon children are always spawned with their full
            // argv flags; a bare role launch therefore fails the usual
            // argument validation with a clear error instead of guessing.
            ProcessRole::SessionWorker => Self::SessionWorker {
                session_id: String::new(),
                generation: 0,
                parent_pid: 0,
                ipc_port: 0,
                ipc_token: String::new(),
            },
            ProcessRole::McpDaemon => Self::McpDaemon {
                bind_host: String::new(),
                port: 0,
                token: String::new(),
                generation: 0,
                parent_pid: 0,
            },
        }
    }
}

impl CliInvocation {
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> AppResult<Self> {
        Self::parse_with_role(args, None)
    }

    /// Parse `args`, defaulting the command to `default_role` when the caller
    /// gave no explicit command flag.
    pub(crate) fn parse_with_role(
        args: impl IntoIterator<Item = String>,
        default_role: Option<ProcessRole>,
    ) -> AppResult<Self> {
        let mut config_dir = None;
        let mut command = None;
        let mut generation = 0u64;
        let mut parent_pid = 0u32;
        let mut mcp_bind = "127.0.0.1".to_owned();
        let mut mcp_port = yshell_config::default_mcp_port();
        let mut mcp_token = String::new();
        let mut ipc_port = 0u16;
        let mut ipc_token = String::new();
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
                "--generation" => {
                    let value = args
                        .next()
                        .ok_or_else(|| AppError::new("--generation requires an integer"))?;
                    generation = value
                        .parse()
                        .map_err(|_| AppError::new("--generation must be an integer"))?;
                }
                "--parent-pid" => {
                    let value = args
                        .next()
                        .ok_or_else(|| AppError::new("--parent-pid requires an integer"))?;
                    parent_pid = value
                        .parse()
                        .map_err(|_| AppError::new("--parent-pid must be an integer"))?;
                }
                "--mcp-bind" => {
                    mcp_bind = args
                        .next()
                        .ok_or_else(|| AppError::new("--mcp-bind requires a host"))?;
                }
                "--mcp-port" => {
                    let value = args
                        .next()
                        .ok_or_else(|| AppError::new("--mcp-port requires a port"))?;
                    mcp_port = value
                        .parse()
                        .map_err(|_| AppError::new("--mcp-port must be an integer"))?;
                }
                "--mcp-token" => {
                    mcp_token = args
                        .next()
                        .ok_or_else(|| AppError::new("--mcp-token requires a token"))?;
                }
                "--ipc-port" => {
                    let value = args
                        .next()
                        .ok_or_else(|| AppError::new("--ipc-port requires a port"))?;
                    ipc_port = value
                        .parse()
                        .map_err(|_| AppError::new("--ipc-port must be an integer"))?;
                }
                "--ipc-token" => {
                    ipc_token = args
                        .next()
                        .ok_or_else(|| AppError::new("--ipc-token requires a token"))?;
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
                "--host" => set_command(&mut command, CliCommand::Host)?,
                "--shell" => set_command(&mut command, CliCommand::Shell)?,
                "--attach" => set_command(&mut command, CliCommand::AttachShell)?,

                "--session-worker" => {
                    let session_id = args
                        .next()
                        .ok_or_else(|| AppError::new("--session-worker requires a session id"))?;
                    set_command(
                        &mut command,
                        CliCommand::SessionWorker {
                            session_id,
                            generation: 0,
                            parent_pid: 0,
                            ipc_port: 0,
                            ipc_token: String::new(),
                        },
                    )?;
                }
                "--mcp-daemon" => set_command(
                    &mut command,
                    CliCommand::McpDaemon {
                        bind_host: String::new(),
                        port: 0,
                        token: String::new(),
                        generation: 0,
                        parent_pid: 0,
                    },
                )?,
                unknown => return Err(AppError::new(format!("unknown argument: {unknown}"))),
            }
        }

        let mut command = command
            .or_else(|| default_role.map(CliCommand::for_role))
            .unwrap_or(CliCommand::HostEntry);
        match &mut command {
            CliCommand::SessionWorker {
                generation: gen,
                parent_pid: parent,
                ipc_port: port,
                ipc_token: token,
                ..
            } => {
                *gen = generation;
                *parent = parent_pid;
                *port = ipc_port;
                *token = ipc_token;
            }
            CliCommand::McpDaemon {
                bind_host,
                port,
                token,
                generation: gen,
                parent_pid: parent,
            } => {
                *bind_host = mcp_bind;
                *port = mcp_port;
                *token = mcp_token;
                *gen = generation;
                *parent = parent_pid;
                if token.is_empty() {
                    return Err(AppError::new("--mcp-daemon requires --mcp-token"));
                }
            }
            _ => {}
        }

        Ok(Self {
            config_dir,
            command,
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

pub(crate) fn print_help() {
    println!(
        "YShell\n\nUSAGE:\n    yshell [OPTIONS]\n\nOPTIONS:\n    --config-dir <path>       Use <path> as the configuration directory (test / smoke isolation)\n    --check-config            Validate configuration discovery\n    --print-config-dir        Print the configuration directory\n    --quick-connect <input>   Run the runtime quick-connect path without launching UI\n    --mcp                     Host-facing MCP stdio bridge (AI clients; enable MCP in Settings first)\n    --host                    Run the Host only, without spawning a GUI Shell (debug/headless)\n    --shell                   Run/attach a GUI Shell, ensuring a Host exists\n    --attach                  Attach this GUI Shell to a running Host (internal; no Host spawn)\n    --session-worker <id>     Run a SessionWorker child (Host-spawned)\n    --mcp-daemon              Run standalone MCP TCP listener (Host-spawned)\n    --generation <n>          Host generation for worker/daemon children\n    --parent-pid <pid>        Parent Host pid for worker/daemon children\n    --ipc-port <port>         SessionWorker Host IPC port\n    --ipc-token <token>       SessionWorker Host IPC token\n    --mcp-bind <host>         MCP daemon bind host\n    --mcp-port <port>         MCP daemon port\n    --mcp-token <token>       MCP daemon endpoint token\n    --version                 Print version\n    --help                    Print help\n\nDEFAULT: become the unique Host for this configuration directory and spawn one\nGUI Shell window; a second launch only asks the running Host to open a window.\n\nCONFIGURATION:\n    All application settings (language, appearance, motion, fonts, local shell,\n    secret store) are read from the configuration directory only; YShell does\n    not use environment variables for configuration.\n\nSee docs/prototypes/process-architecture.md for the Host/Session/MCP/Shell model."
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
    fn defaults_to_the_host_entry_with_the_platform_config_dir() {
        let invocation = parse(&[]).expect("parse");
        assert_eq!(invocation.command, CliCommand::HostEntry);
        assert!(invocation.config_dir.is_none());
    }

    #[test]
    fn role_binaries_default_to_their_role() {
        let parse_role =
            |role: ProcessRole| CliInvocation::parse_with_role(Vec::<String>::new(), Some(role));

        assert_eq!(
            parse_role(ProcessRole::Host).expect("host").command,
            CliCommand::Host
        );
        assert_eq!(
            parse_role(ProcessRole::Shell).expect("shell").command,
            CliCommand::AttachShell
        );
        assert!(matches!(
            parse_role(ProcessRole::SessionWorker)
                .expect("worker")
                .command,
            CliCommand::SessionWorker { .. }
        ));
        // A bare mcp-daemon has no token; the binary must fail clearly.
        assert!(parse_role(ProcessRole::McpDaemon).is_err());

        // An explicit command flag still wins over the role default.
        let explicit =
            CliInvocation::parse_with_role(["--version".to_owned()], Some(ProcessRole::Host))
                .expect("explicit");
        assert_eq!(explicit.command, CliCommand::Version);
    }

    #[test]
    fn role_binaries_keep_shared_options() {
        let invocation = CliInvocation::parse_with_role(
            ["--config-dir".to_owned(), "/tmp/smoke".to_owned()],
            Some(ProcessRole::Host),
        )
        .expect("parse");
        assert_eq!(invocation.command, CliCommand::Host);
        assert_eq!(
            invocation.config_dir.as_deref(),
            Some(std::path::Path::new("/tmp/smoke"))
        );
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

    #[test]
    fn parses_host_and_session_worker_roles() {
        assert_eq!(parse(&["--host"]).expect("host").command, CliCommand::Host);
        let worker = parse(&[
            "--session-worker",
            "ses_1",
            "--generation",
            "3",
            "--parent-pid",
            "42",
        ])
        .expect("worker");
        assert_eq!(
            worker.command,
            CliCommand::SessionWorker {
                session_id: "ses_1".to_owned(),
                generation: 3,
                parent_pid: 42,
                ipc_port: 0,
                ipc_token: String::new(),
            }
        );
        let worker_ipc = parse(&[
            "--session-worker",
            "ses_2",
            "--ipc-port",
            "5550",
            "--ipc-token",
            "tok",
        ])
        .expect("worker ipc");
        assert_eq!(
            worker_ipc.command,
            CliCommand::SessionWorker {
                session_id: "ses_2".to_owned(),
                generation: 0,
                parent_pid: 0,
                ipc_port: 5550,
                ipc_token: "tok".to_owned(),
            }
        );
    }

    #[test]
    fn mcp_daemon_requires_token() {
        assert!(parse(&["--mcp-daemon", "--mcp-port", "9"]).is_err());
        let daemon = parse(&[
            "--mcp-daemon",
            "--mcp-bind",
            "127.0.0.1",
            "--mcp-port",
            "9",
            "--mcp-token",
            "secret",
            "--generation",
            "1",
        ])
        .expect("daemon");
        assert_eq!(
            daemon.command,
            CliCommand::McpDaemon {
                bind_host: "127.0.0.1".to_owned(),
                port: 9,
                token: "secret".to_owned(),
                generation: 1,
                parent_pid: 0,
            }
        );
    }
}
