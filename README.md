# YShell

YShell is an open-source, cross-platform SSH terminal and SFTP client planned for Windows, Linux, and macOS.

The repository is currently at Milestone 0: Rust workspace, crate boundaries, SSH/SFTP adapter models, UI shell view models, configuration-path discovery, tracing setup, and CI/release automation.

## App shell CLI

```bash
yshell --check-config
yshell --print-config-dir
yshell --quick-connect user@example.com
yshell --version
```

Running `yshell` without arguments reports that the UI runtime is not wired yet instead of pretending to launch a graphical shell.

## Adapter coverage

- `yshell-ssh` defines connection configuration, authentication methods, host-key policy and in-memory known-hosts management, proxy state, PTY configuration, tunnel configuration, error classification, and a deterministic fake SSH adapter for tests.
- `yshell-sftp` defines directory listings, file operation interfaces (`chmod`, `delete`, `rename`, `mkdir`), a transfer queue state machine with progress/retry/cancel, and a deterministic fake backend for tests.
- `yshell-ui` defines view models and command/event types for sessions, tabs, SFTP, tunnels, quick commands, logging, search, and status.

## Development checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
