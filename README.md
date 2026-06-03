# YShell

YShell is an open-source, cross-platform SSH terminal and SFTP client planned for Windows, Linux, and macOS.

The repository is currently at Milestone 0: Rust workspace, crate boundaries, UI shell placeholders, configuration-path discovery, tracing setup, and CI/release automation.

## Development checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
