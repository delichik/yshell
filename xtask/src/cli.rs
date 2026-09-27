//! Command line parsing for `cargo xtask`.
//!
//! The parser is intentionally tiny: commands take a handful of long options
//! and `cargo xtask run` forwards everything after `--` to the app.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::contrast::{ContrastOptions, ThemeSelection};
use crate::dist::{ChecksumOptions, DistOptions};
use crate::tasks::{BuildOptions, CleanOptions, RunOptions, TestOptions};

pub const USAGE: &str = "\
YShell build tool.

Usage: cargo xtask <command> [options]

Commands:
  fmt         Check formatting with rustfmt (CI gate).
  lint        Run clippy with -D warnings (CI gate).
  check       Run fmt + lint.
  build       Build the workspace.
  test        Run the workspace test suite, or the live SSH/SFTP smoke tests.
  run         Run the native app from source.
  dist        Build release artifacts: portable archive and installers.
  checksums   Regenerate SHA256SUMS.txt for the artifacts in dist/.
  contrast    Check WCAG contrast of the color tokens in ui/theme.slint
              (light + dark) and fail when a combination is below its ratio.
  deny        Run cargo-deny license and advisory checks (CI gate).
  doctor      Print toolchain and packaging-tool diagnostics.
  clean       Remove artifacts produced by `cargo xtask dist` (with --all also
              the cargo target directory).
  ci          Run the CI gates locally: fmt, lint and test.
  help        Show this message.

Options:
  build     --release               Optimized build instead of the debug build.
            --package <name>        Build a single workspace package.

  test      --live                  Run the live SSH/SFTP smoke tests against a
                                      real server instead of the unit suite.
            --ssh-target <user@h:p>  Live SSH target (or YSHELL_LIVE_SSH_TARGET).
            --sftp-target <user@h:p> Live SFTP target (or YSHELL_LIVE_SFTP_TARGET,
                                      defaults to the SSH target).

  run       --release               Run the optimized build.
            -- <args>...            Arguments forwarded to the app.

  dist      --target <triple>       Rust target triple (default: host).
            --platform <slug>       Override the platform slug derived from the
                                      target (e.g. linux-x86_64).
            --version <label>       Version label used in file names (default:
                                      tag, YSHELL_RELEASE_VERSION or git SHA).
            --formats <list>        Comma separated: portable, msi, dmg, deb,
                                      rpm (default: portable). Installers are
                                      built with the platform tooling:
                                      WiX (msi), hdiutil (dmg), cargo-deb (deb)
                                      and rpmbuild (rpm).
            --out <dir>             Output directory (default: dist/).
            --no-build              Reuse target/<triple>/release/<binary>.
            --no-smoke              Skip running the binary with --version.

  checksums --dir <dir>             Directory to scan (default: dist/).
            --output <file>         Output file (default: <dir>/SHA256SUMS.txt).

  contrast  --theme <light|dark|both>
                                      Theme(s) to check (default: both).
            --theme-file <path>       Theme file to read (default:
                                      ui/theme.slint, relative to the repo root).
            --verbose                 Also list passing icon / large-text rows.

  clean     --all                   Also remove the cargo target directory.

Examples:
  cargo xtask check
  cargo xtask contrast
  cargo xtask test --live --ssh-target root@127.0.0.1:2222
  cargo xtask dist --formats portable,deb,rpm
  cargo xtask dist --target x86_64-pc-windows-msvc --formats portable,msi
  cargo xtask checksums --dir dist
";

/// A parsed command line.
pub enum Command {
    Help,
    Version,
    Fmt,
    Lint,
    Check,
    Build(BuildOptions),
    Test(TestOptions),
    Run(RunOptions),
    Deny,
    Doctor,
    Clean(CleanOptions),
    Ci,
    Dist(DistOptions),
    Checksums(ChecksumOptions),
    Contrast(ContrastOptions),
}

/// Options collected for one command.
#[derive(Default)]
pub struct Parsed {
    values: BTreeMap<String, String>,
    flags: Vec<String>,
    positional: Vec<String>,
    passthrough: Vec<String>,
}

impl Parsed {
    pub fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn passthrough(&self) -> &[String] {
        &self.passthrough
    }

    fn ensure_no_positional(&self, command: &str) -> Result<()> {
        if self.positional.is_empty() {
            Ok(())
        } else {
            bail!("`cargo xtask {command}` does not take positional arguments");
        }
    }
}

fn parse_arguments(
    args: &[String],
    value_options: &'static [&'static str],
    flag_options: &'static [&'static str],
) -> Result<Parsed> {
    let mut parsed = Parsed::default();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "--" {
            parsed.passthrough.extend(args[index + 1..].iter().cloned());
            break;
        }
        if let Some(name) = argument.strip_prefix("--") {
            let (name, inline) = match name.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (name, None),
            };
            if value_options.contains(&name) {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        index += 1;
                        match args.get(index) {
                            Some(value) => value.clone(),
                            None => bail!("option `--{name}` needs a value"),
                        }
                    }
                };
                parsed.values.insert(name.to_string(), value);
            } else if flag_options.contains(&name) {
                if inline.is_some() {
                    bail!("option `--{name}` does not take a value");
                }
                parsed.flags.push(name.to_string());
            } else {
                bail!("unknown option `--{name}`");
            }
            index += 1;
            continue;
        }
        if argument.starts_with('-') {
            bail!("unknown option `{argument}`");
        }
        parsed.positional.push(argument.clone());
        index += 1;
    }
    Ok(parsed)
}

/// Parse the full argument list (without the program name).
pub fn parse(argv: &[String]) -> Result<Command> {
    if argv.is_empty() {
        return Ok(Command::Help);
    }
    if argv
        .iter()
        .take_while(|argument| argument.as_str() != "--")
        .any(|argument| argument == "-h" || argument == "--help" || argument == "help")
    {
        return Ok(Command::Help);
    }

    let command = argv[0].as_str();
    let rest = &argv[1..];

    let parsed = match command {
        "fmt" | "lint" | "check" | "deny" | "doctor" | "ci" => parse_arguments(rest, &[], &[])?,
        "build" => parse_arguments(rest, &["package"], &["release"])?,
        "test" => parse_arguments(rest, &["ssh-target", "sftp-target"], &["live"])?,
        "run" => parse_arguments(rest, &[], &["release"])?,
        "clean" => parse_arguments(rest, &[], &["all"])?,
        "checksums" => parse_arguments(rest, &["dir", "output"], &[])?,
        "contrast" => parse_arguments(rest, &["theme", "theme-file"], &["verbose"])?,
        "dist" => parse_arguments(
            rest,
            &["target", "platform", "version", "formats", "out"],
            &["no-build", "no-smoke"],
        )?,
        "-V" | "--version" | "version" => return Ok(Command::Version),
        other => bail!("unknown command `{other}` (run `cargo xtask help`)"),
    };

    match command {
        "fmt" => {
            parsed.ensure_no_positional("fmt")?;
            Ok(Command::Fmt)
        }
        "lint" => {
            parsed.ensure_no_positional("lint")?;
            Ok(Command::Lint)
        }
        "check" => {
            parsed.ensure_no_positional("check")?;
            Ok(Command::Check)
        }
        "deny" => {
            parsed.ensure_no_positional("deny")?;
            Ok(Command::Deny)
        }
        "doctor" => {
            parsed.ensure_no_positional("doctor")?;
            Ok(Command::Doctor)
        }
        "ci" => {
            parsed.ensure_no_positional("ci")?;
            Ok(Command::Ci)
        }
        "build" => {
            parsed.ensure_no_positional("build")?;
            Ok(Command::Build(BuildOptions {
                release: parsed.flag("release"),
                package: parsed.value("package").map(str::to_string),
            }))
        }
        "test" => {
            parsed.ensure_no_positional("test")?;
            Ok(Command::Test(TestOptions {
                live: parsed.flag("live"),
                ssh_target: parsed.value("ssh-target").map(str::to_string),
                sftp_target: parsed.value("sftp-target").map(str::to_string),
            }))
        }
        "run" => {
            parsed.ensure_no_positional("run")?;
            Ok(Command::Run(RunOptions {
                release: parsed.flag("release"),
                args: parsed.passthrough().to_vec(),
            }))
        }
        "clean" => {
            parsed.ensure_no_positional("clean")?;
            Ok(Command::Clean(CleanOptions {
                all: parsed.flag("all"),
            }))
        }
        "checksums" => {
            parsed.ensure_no_positional("checksums")?;
            Ok(Command::Checksums(ChecksumOptions {
                dir: parsed.value("dir").map(str::to_string),
                output: parsed.value("output").map(str::to_string),
            }))
        }
        "contrast" => {
            parsed.ensure_no_positional("contrast")?;
            Ok(Command::Contrast(ContrastOptions {
                theme: match parsed.value("theme") {
                    Some(value) => ThemeSelection::parse(value)?,
                    None => ThemeSelection::Both,
                },
                theme_file: parsed.value("theme-file").map(PathBuf::from),
                verbose: parsed.flag("verbose"),
            }))
        }
        "dist" => Ok(Command::Dist(DistOptions::parse(&parsed)?)),
        _ => unreachable!("command was validated above"),
    }
}
