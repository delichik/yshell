# Packaging

Templates and launchers used by `cargo xtask dist` (see `xtask/src/dist.rs`
and `xtask/src/installers.rs`). Everything here is consumed by the build tool,
not by the app at runtime.

| Path | Used for |
|---|---|
| `PORTABLE.txt`, `yshell-portable.cmd`, `yshell-portable.sh` | Portable archives: documentation and launchers that keep data in `./data` |
| `windows/yshell.wxs` | WiX v3 source for the MSI installer (`--formats msi`) |
| `macos/Info.plist` | `Info.plist` for the `YShell.app` bundle inside the DMG (`--formats dmg`) |
| `linux/yshell.desktop` | Desktop entry installed by the deb/rpm packages |
| `linux/yshell.spec.in` | RPM spec template (`--formats rpm`) |

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

- **No icons yet.** `assets/icons/` is still a placeholder, so installers use
  the platform default icon. `packaging/macos/Info.plist` already looks for
  `assets/icons/yshell.icns`; add the icon files and the bundle picks it up.
- **Unsigned.** The MSI, DMG, deb and rpm artifacts are not signed; Windows
  SmartScreen and macOS Gatekeeper will warn on first launch. Code signing and
  notarization are tracked as follow-up work.
- **AppImage is not built.** It needs an icon and FUSE, and the deb/rpm
  packages already cover Linux desktops.