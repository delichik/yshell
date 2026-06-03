# YShell

YShell is an open-source, cross-platform SSH terminal and SFTP client planned for Windows, Linux, and macOS.

The repository is currently at Milestone 0+: Rust workspace, crate boundaries, SSH/SFTP adapter models, a native Slint app shell, UI view models, configuration-path discovery, tracing setup, and CI/release automation.

## Native app shell

Running `yshell` without arguments starts the native Slint main window with the session manager, toolbar, terminal area, SFTP/tunnel/quick-command docks, and status bar wired to Rust callbacks.

The CLI also exposes smoke-friendly commands:

```bash
yshell --check-config
yshell --print-config-dir
yshell --quick-connect user@example.com
yshell --version
```

Quick Connect input is parsed and validated both from the CLI and from the native toolbar.

## Adapter coverage

- `yshell-ssh` defines connection configuration, authentication methods, host-key policy and in-memory known-hosts management, proxy state, PTY configuration, tunnel configuration, error classification, and a deterministic fake SSH adapter for tests.
- `yshell-sftp` defines directory listings, file operation interfaces (`chmod`, `delete`, `rename`, `mkdir`), a transfer queue state machine with progress/retry/cancel, and a deterministic fake backend for tests.
- `yshell-ui` defines view models and command/event types for sessions, tabs, SFTP, tunnels, quick commands, logging, search, and status.

## Development checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p yshell-test-support --test product_smoke -- --nocapture
```
