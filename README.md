# YShell

YShell is an open-source, cross-platform SSH terminal and SFTP client planned for Windows, Linux, and macOS.

Current product focus: move toward an app-owned SSH/SFTP implementation. Any path that delegates live shell or file-transfer work to local system `ssh` / `sftp` should be treated as a temporary validation bridge, not release progress.

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

Build, check and packaging tasks go through the workspace build tool
(`cargo xtask`, defined in `xtask/`). It is plain Rust, so Windows, macOS and
Linux/WSL behave the same way:

```bash
cargo xtask check                 # cargo fmt --check + clippy -D warnings
cargo xtask test                  # cargo test --workspace --all-features
cargo xtask ci                    # the gates CI runs: check + test
cargo xtask test --live --ssh-target root@127.0.0.1:2222   # live SSH/SFTP smoke tests
cargo xtask doctor                # toolchain, packaging tools and environment hints
cargo xtask help                  # all commands and options
```

## Release artifacts

`cargo xtask dist` builds the same artifacts locally that CI publishes:

| Artifact | Selection | Built with |
|---|---|---|
| `yshell-<version>-<platform>-portable.zip` / `.tar.gz` | `--formats portable` (default) | built in (zip / tar.gz) |
| `yshell-<version>-windows-x86_64.msi` | `--formats portable,msi` | WiX Toolset v3 |
| `yshell-<version>-macos-<arch>.dmg` | `--formats portable,dmg` | `hdiutil` |
| `yshell-<version>-linux-x86_64.deb` | `--formats portable,deb` | cargo-deb |
| `yshell-<version>-linux-x86_64.rpm` | `--formats portable,rpm` | `rpmbuild` |

Every artifact gets a `.sha256` sidecar and `cargo xtask checksums --dir dist`
aggregates them into `SHA256SUMS.txt`. Installers are built natively, so each
format is produced on (or for) its own platform; `cargo xtask doctor` reports
which packaging tools are available. See [packaging/README.md](packaging/README.md).

The GitHub Actions pipeline does the same work on GitHub-hosted runners, so no
local toolchain is required:

- **CI** (`ci.yml`): rustfmt, clippy, tests on Windows/macOS/Linux, live
  SSH/SFTP smoke tests against a disposable sshd container, cargo-deny, and a
  full packaging run on pushes to `main` (so installers cannot rot between
  releases).
- **Release** (`release.yml`): push a tag to publish. `vX.Y.Z` creates a
  release, `portable-*` a prerelease, both with portable archives, installers
  and `SHA256SUMS.txt`; `workflow_dispatch` builds the artifacts without
  publishing.

See [docs/product/github-actions-release.md](docs/product/github-actions-release.md) for details.

## Linux/WSL development

Linux is the current development focus; Windows and macOS use the same
commands. The supported setup is WSL2 (Debian 13) with a current Rust
toolchain. Note that Debian's packaged `rustc` 1.85 is too old for the locked
dependencies (MSRV is 1.89), so install Rust with rustup:

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c rustfmt -c clippy
cargo xtask doctor   # prints the apt command for the Linux build/runtime libraries
cargo xtask test     # cargo test --workspace --all-features
cargo xtask run      # run the native app from source
cargo xtask dist     # build the release artifacts into dist/
```

When the repository lives on a Windows mount (`/mnt/*`), set a target directory
on the WSL filesystem to speed up builds - `cargo xtask doctor` reminds you:

```bash
export CARGO_TARGET_DIR=$HOME/yshell-target
```

Run the dev build from Windows (needs WSLg, i.e. Windows 11 or a recent WSL):

```powershell
wsl.exe -e bash -lc 'cd /mnt/d/NewSpace/yshell && cargo xtask run'
```

Live SSH/SFTP smoke tests run against a disposable Docker container:

```bash
bash scripts/test-ssh/run.sh test   # build/start container + ssh-agent + live tests
```

See [scripts/README.md](scripts/README.md) for details. The legacy
`scripts/sync-and-test-remote.ps1` helper still works for arbitrary remote hosts, but the old
Debian validation VM has been retired.
