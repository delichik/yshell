# Packaging

Templates and launchers used by `cargo xtask dist` (see `xtask/src/dist.rs`
and `xtask/src/installers.rs`). Everything here is consumed by the build tool,
not by the app at runtime.

| Path | Used for |
|---|---|
| `PORTABLE.txt`, `gterm-portable.cmd`, `gterm-portable.sh` | Portable archives: documentation and launchers that keep data in `./data` |
| `windows/gterm.wxs` | WiX v3 source for the MSI installer (`--formats msi`) |
| `macos/Info.plist` | `Info.plist` for the `gTerm.app` bundle inside the DMG (`--formats dmg`) |
| `macos/README-macOS.txt` | First-launch guide bundled into ad-hoc macOS DMGs |
| `linux/gterm.desktop` | Desktop entry installed by the deb/rpm packages |
| `linux/gterm.spec.in` | RPM spec template (`--formats rpm`) |

The deb metadata lives in `[package.metadata.deb]` in
`crates/yshell-app/Cargo.toml`.

## Building artifacts locally

```bash
cargo xtask doctor                          # which packaging tools are available
cargo xtask dist                            # portable archive only
cargo xtask dist --formats portable,deb,rpm # plus Linux installers
cargo xtask dist --formats portable,msi     # Windows installer (WiX)
cargo xtask dist --formats portable,dmg     # macOS disk image (hdiutil)
cargo xtask checksums --dir dist            # regenerate SHA256SUMS.txt
```

Installers are built with the platform tooling, so each format can only be
produced on (or for) its own platform:

| Format | Tool | Notes |
|---|---|---|
| `msi` | WiX Toolset v3 | `choco install wixtoolset`; preinstalled on the Windows runners |
| `dmg` | `hdiutil` | macOS only; preinstalled on the macOS runners |
| `deb` | `cargo-deb` | `cargo install cargo-deb`; CI installs it with `taiki-e/install-action` |
| `rpm` | `rpmbuild` | `sudo apt-get install rpm` on Debian/Ubuntu |

`cargo xtask dist` keeps the unpacked payload in `dist/<package>/` next to the
archive and uses `dist/.work/` as scratch space for the packaging tools; both
can be deleted at any time (`cargo xtask clean` removes what the dist command
produced without touching other content in `dist/`).

## Current limitations

- **Code signing & notarization.**
  - **Windows / Linux:** MSI, deb and rpm packages are unsigned; Windows
    SmartScreen will warn on first launch.
  - **macOS:** `gTerm.app` places fonts/themes under `Contents/Resources/` and
    is **ad-hoc signed** (`codesign --sign -`) by default so Apple Silicon /
    strict bundle checks do not fail, with `README-macOS.txt` included in the
    DMG (`xattr -dr com.apple.quarantine /Applications/gTerm.app`). Because it
    is not notarized without an Apple Developer ID, Gatekeeper still blocks
    downloaded apps until quarantine is cleared.
  - **Optional Apple Developer ID + notarization:** set
    `YSHELL_MACOS_SIGN_IDENTITY` to switch from ad-hoc signing to Developer ID +
    Hardened Runtime + timestamp, and set either `YSHELL_MACOS_NOTARY_PROFILE`
    or `YSHELL_MACOS_API_KEY_PATH` / `YSHELL_MACOS_API_KEY_ID` /
    `YSHELL_MACOS_API_ISSUER_ID` (or `YSHELL_MACOS_APPLE_ID` /
    `YSHELL_MACOS_APPLE_TEAM_ID` / `YSHELL_MACOS_APPLE_APP_PASSWORD`) to submit
    and staple the DMG automatically.
- **AppImage is not built.** It needs FUSE, and the deb/rpm packages already
  cover Linux desktops. Application icons live under `assets/icons/` (generated
  via `python scripts/app-icons/generate.py`) and are embedded in Windows PE
  resources, MSI, DMG, and deb/rpm packages.