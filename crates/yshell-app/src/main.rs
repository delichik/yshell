//! YShell application entry point.

mod app_state;
mod bootstrap;
mod error;
mod runtime;
mod session_runtime;

use std::env;

use bootstrap::bootstrap_app;
use error::{AppError, AppResult};
use runtime::AppRuntime;

fn main() -> AppResult<()> {
    match CliCommand::parse(env::args().skip(1))? {
        CliCommand::CheckConfig => {
            let config_dir = yshell_config::discover_config_dir().map_err(AppError::from_error)?;
            println!("configuration OK: {}", config_dir.display());
            Ok(())
        }
        CliCommand::PrintConfigDir => {
            let config_dir = yshell_config::discover_config_dir().map_err(AppError::from_error)?;
            println!("{}", config_dir.display());
            Ok(())
        }
        CliCommand::QuickConnect(input) => {
            let config_dir = yshell_config::discover_config_dir().map_err(AppError::from_error)?;
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
        CliCommand::RunShell => {
            let app = bootstrap_app()?;
            app.run()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    CheckConfig,
    PrintConfigDir,
    QuickConnect(String),
    Version,
    Help,
    RunShell,
}

impl CliCommand {
    fn parse(args: impl IntoIterator<Item = String>) -> AppResult<Self> {
        let mut args = args.into_iter();
        let Some(first) = args.next() else {
            return Ok(Self::RunShell);
        };
        match first.as_str() {
            "--check-config" => Ok(Self::CheckConfig),
            "--print-config-dir" => Ok(Self::PrintConfigDir),
            "--quick-connect" => args.next().map(Self::QuickConnect).ok_or_else(|| {
                AppError::new("--quick-connect requires an input such as user@example.com")
            }),
            "--version" | "-V" => Ok(Self::Version),
            "--help" | "-h" => Ok(Self::Help),
            unknown => Err(AppError::new(format!("unknown argument: {unknown}"))),
        }
    }
}

fn print_help() {
    println!(
        "YShell\n\nUSAGE:\n    yshell [OPTIONS]\n\nOPTIONS:\n    --check-config            Validate configuration discovery\n    --print-config-dir        Print the configuration directory\n    --quick-connect <input>   Run the runtime quick-connect path without launching UI\n    --version                 Print version\n    --help                    Print help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quick_connect_input() {
        let command = CliCommand::parse(["--quick-connect".to_owned(), "me@example".to_owned()])
            .expect("parse");
        assert_eq!(command, CliCommand::QuickConnect("me@example".to_owned()));
    }
}
