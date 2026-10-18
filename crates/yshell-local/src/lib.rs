//! Local PTY shell backend for `YShell`.
//!
//! This crate owns the process/PTY half of local terminal tabs. It provides
//! [`LocalPty`], an implementation of [`ShellSession`], so the application can
//! attach a local shell through exactly the same session plumbing it already
//! uses for SSH. It contains no tab, session or UI state.
//!
//! # Backend choice
//!
//! The implementation delegates to the [`portable_pty`] crate, which is the
//! battle-tested PTY layer extracted from WezTerm. On Unix it uses
//! `openpty`/`fork`/`exec`; on Windows it uses ConPTY
//! (`CreatePseudoConsole`, `ResizePseudoConsole` and
//! `ClosePseudoConsole`). Using it keeps the platform-specific unsafe code out
//! of this workspace, avoids maintaining two separate FFI backends, and gives
//! Windows the ConPTY path required by TAB-7 instead of a Win32 console
//! emulation.
//!
//! # Process and output model
//!
//! A dedicated reader thread drains the PTY into a bounded channel, so
//! [`ShellSession::poll_output`] is non-blocking and backpressure reaches the
//! child process naturally. [`ShellSession::disconnect`] (and `Drop`) kills the
//! child, closes the PTY and joins the reader with a bounded wait so closing a
//! tab cannot hang the event loop.
//!
//! When the shell exits on its own, the session reports
//! [`ShellSession::is_connected`] as `false`; the numeric status is available
//! through [`LocalPty::exit_status`] after the next poll or disconnect.
//!
//! # Platform notes
//!
//! On Unix the default shell is `$SHELL` when executable, otherwise `/bin/sh`.
//! On Windows the default is probed in the order `pwsh.exe` ->
//! `powershell.exe` -> `cmd.exe`. [`SHELL_OVERRIDE_ENV`] takes precedence on
//! both platforms, and the working directory defaults to the user's home.
//!
//! ```no_run
//! use yshell_local::{LocalPty, LocalPtyConfig, PtySize, ShellSession};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = LocalPtyConfig::for_default_shell().with_size(120, 40);
//! let mut shell = LocalPty::spawn(config)?;
//! shell.write_input(b"echo hello\r")?;
//! // Poll from the UI loop; empty means "nothing available right now".
//! let output = shell.poll_output()?;
//! shell.resize_pty(PtySize { columns: 100, rows: 30, ..PtySize::default() })?;
//! shell.disconnect()?;
//! # Ok(())
//! # }
//! ```
//!
//! [`portable_pty`]: https://crates.io/crates/portable-pty

mod config;
mod pty;
mod shell;

pub use config::LocalPtyConfig;
pub use pty::{LocalExitStatus, LocalPty};
pub use shell::{default_shell, home_dir, ResolvedShell, ShellSource, SHELL_OVERRIDE_ENV};
pub use yshell_ssh::{PtySize, ShellSession};
