# gTerm

gTerm (formerly YShell) is an open-source, cross-platform multi-protocol terminal and SFTP client planned for Windows, Linux, and macOS.

Current product focus: move toward an app-owned SSH/SFTP implementation. Any path that delegates live shell or file-transfer work to local system `ssh` / `sftp` should be treated as a temporary validation bridge, not release progress.

Product docs live under [`docs/product/`](docs/product/README.md). Start with [`docs/product/yshell-current-status.md`](docs/product/yshell-current-status.md) for what this build ships, intentionally omits, and still leaves open.

The repository is a Rust workspace with crate boundaries, a native Slint app shell, SSH/SFTP stacks, configuration, tracing, and CI/release automation.

## Native app shell

Running `gterm` without arguments starts the headless Host entry and opens the native Slint main window (`gterm-ui`) with the session manager, toolbar, terminal area, SFTP/tunnel/quick-command docks, and status bar wired to Rust callbacks.

The CLI also exposes smoke-friendly commands:

```bash
gterm --config-dir /tmp/gterm-config --check-config
gterm --config-dir /tmp/gterm-config --print-config-dir
gterm --config-dir /tmp/gterm-config
gterm --quick-connect user@example.com
gterm --version
```

`--config-dir <path>` selects the configuration directory (test / smoke isolation and portable runs) and is the **only** configuration override: app settings (language, appearance, motion, fonts, local shell, secret store) are read from that directory, never from `YSHELL_*` environment variables.

Quick Connect input is parsed and validated both from the CLI and from the native toolbar.

### MCP (AI control of terminal sessions)

MCP is an opt-in product feature: open gTerm → **Settings → AI / MCP → Enable MCP**. The Host then starts `gterm-mcp-daemon`. AI hosts (Cursor, Claude Desktop, or any stdio MCP client) start `gterm --mcp` as their MCP server process — that flag is not a user workflow.

```json
{
  "mcpServers": {
    "gterm": {
      "command": "gterm",
      "args": ["--mcp"]
    }
  }
}
```

If `gterm` is not on `PATH`, set `command` to the absolute path of the binary. Prerequisites: GUI running and MCP enabled.

**In scope:** list saved profiles and terminal tabs; after user approval, `session_run` (wait for output), input/output, keys, connect/disconnect/duplicate. Sessions report blockers (`password`, `host_key`, …) when the GUI still needs you.

**Out of scope:** SFTP/tunnels; agent-filled passwords or host-key acceptance; headless mode without the GUI.

The agent should call `get_mcp_info` / `request_control` first. Prefer `session_run` over raw input/output. Status bar shows **AI controlling** with Exit.

## Adapter coverage

- `yshell-ssh` defines connection configuration, authentication methods, host-key policy and in-memory known-hosts management, proxy state, PTY configuration, tunnel configuration, and error classification for real SSH sessions.
- `yshell-sftp` defines directory listings, file operation interfaces (`chmod`, `delete`, `rename`, `mkdir`), a transfer queue state machine with progress/retry/cancel, and an in-memory backend for tests.
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
| `gterm-<version>-<platform>-portable.zip` / `.tar.gz` | `--formats portable` (default) | built in (zip / tar.gz) |
| `gterm-<version>-windows-x86_64.msi` | `--formats portable,msi` | WiX Toolset v3 |
| `gterm-<version>-macos-<arch>.dmg` | `--formats portable,dmg` | `hdiutil` |
| `gterm-<version>-linux-x86_64.deb` | `--formats portable,deb` | cargo-deb |
| `gterm-<version>-linux-x86_64.rpm` | `--formats portable,rpm` | `rpmbuild` |

Every artifact gets a `.sha256` sidecar and `cargo xtask checksums --dir dist`
aggregates them into `SHA256SUMS.txt`. Installers are built natively, so each
format is produced on (or for) its own platform; `cargo xtask doctor` reports
which packaging tools are available. See [packaging/README.md](packaging/README.md).

The GitHub Actions pipeline does the same work on GitHub-hosted runners, so no
local toolchain is required:

- **CI** (`ci.yml`): rustfmt, clippy, the test suite on Linux with compile
  checks on Windows and macOS, live SSH/SFTP smoke tests against a disposable
  sshd container, cargo-deny, and a full packaging run on pushes to `main` (so
  installers cannot rot between releases).
- **Release** (`release.yml`): push a tag to publish. `vX.Y.Z` creates a
  release, `portable-*` a prerelease, both with portable archives, installers
  and `SHA256SUMS.txt`; `workflow_dispatch` builds the artifacts without
  publishing.
- **Dev build** (`dev-build.yml`): pushes to `dev` compile the same artifacts
  but only refresh a draft release (no tag, nothing published), so `dev`
  installers can be tested before tagging.

See [docs/product/github-actions-release.md](docs/product/github-actions-release.md) for details.

## Linux development

Linux is the current development focus; Windows and macOS use the same
commands. Use a current Rust toolchain (MSRV 1.92); Debian's packaged `rustc`
1.85 is too old, so install Rust with rustup:

```bash
export RUSTUP_DIST_SERVER=https://rsproxy.cn RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c rustfmt -c clippy
```

Builds use the standard in-repo `target/` directory (see
[docs/agents/build-artifacts.md](docs/agents/build-artifacts.md)); do not point
`CARGO_TARGET_DIR` outside the repository:

```bash
cargo xtask doctor   # prints the apt command for the Linux build/runtime libraries
cargo xtask test     # cargo test --workspace --all-features
cargo xtask run      # run the native app from source
cargo xtask dist     # build the release artifacts into dist/
```

If you build under WSL, keep the repository on the native ext4 filesystem
(`/root/...`); 9p mounts such as `/mnt/*` make cargo builds very slow.

Live SSH/SFTP smoke tests run against a disposable Docker container:

```bash
bash scripts/test-ssh/run.sh test   # build/start container + ssh-agent + live tests
```

See [scripts/README.md](scripts/README.md) for details. The legacy
`scripts/sync-and-test-remote.ps1` helper still works for arbitrary remote hosts, but the old
Debian validation VM has been retired.
