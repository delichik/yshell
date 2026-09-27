# YShell GitHub Actions 与发布流程

本文档说明仓库的 CI、打包和发布流程。实际配置以这些文件为准：

- `.github/workflows/ci.yml`：CI 门禁 + 打包冒烟
- `.github/workflows/_build-artifacts.yml`：可复用的产物构建流程
- `.github/workflows/release.yml`：打 tag 后发布 Release
- `.github/actions/setup-build-env/action.yml`：统一的运行器环境准备
- `xtask/`：构建工具（`cargo xtask ...`），本地与 CI 共用同一套逻辑
- `deny.toml`、`packaging/`：依赖检查配置与安装包模板

## 1. 设计原则：逻辑在 xtask，workflow 只做编排

所有构建、检查、打包命令都实现在 `xtask` crate 里，workflow 只负责：

1. 准备运行器环境（Rust、Linux X11/XCB 依赖、Windows 的 Perl/NASM、缓存）；
2. 调用 `cargo xtask <command>`；
3. 上传/发布产物。

因此本地可以完整复现 CI：

```bash
cargo xtask ci        # fmt + clippy + 全量测试
cargo xtask dist      # 与发布完全一致的产物构建
cargo xtask doctor    # 当前机器有哪些工具、能出哪些格式
```

## 2. 产物清单

每个平台产出便携包 + 原生安装包：

| 平台 | 便携包 | 安装包 | 所需工具 |
|---|---|---|---|
| Windows x86_64 | `.zip` | `.msi` | WiX Toolset v3（runner 预装，本地 `choco install wixtoolset`） |
| macOS x86_64 / aarch64 | `.tar.gz` | `.dmg`（内含 `YShell.app`） | `hdiutil`（macOS 自带） |
| Linux x86_64 | `.tar.gz` | `.deb`、`.rpm` | `cargo-deb`、`rpmbuild` |

- 命名：`yshell-<版本>-<平台>[-portable].<扩展名>`，例如
  `yshell-v0.1.0-linux-x86_64.deb`、`yshell-v0.1.0-windows-x86_64-portable.zip`。
- 每个产物都有同名 `.sha256` 校验文件；`checksums` 任务把它们汇总成
  `SHA256SUMS.txt` 一并发布。
- 便携包内含 `yshell`/`yshell.exe`、`yshell-portable.sh`/`.cmd` 启动器、
  `PORTABLE.txt`、`README.md`、`LICENSE`、`BUILD.txt`（版本/目标/rustc/commit）。
- 安装包不做签名（见第 6 节）。

## 3. `ci.yml`

触发：push 到 `main`/`master`/`release/**`、指向 `main`/`master` 的 pull request、
手动 `workflow_dispatch`。同一分支的新推送会取消旧运行（`concurrency`）。

任务：

| 任务 | 内容 |
|---|---|
| `fmt` | 校验 `Cargo.lock` 是否最新，然后 `cargo xtask fmt` |
| `lint` | `cargo xtask lint`（clippy `-D warnings`，Linux 需要 X11/XCB 开发包） |
| `test` | 矩阵 `ubuntu-latest` / `windows-latest` / `macos-latest`，`cargo xtask test` |
| `live-ssh` | Linux 上用 `scripts/test-ssh/run.sh test` 起临时 sshd 容器跑 live SSH/SFTP 冒烟测试 |
| `deny` | `taiki-e/install-action` 安装 cargo-deny，`cargo xtask deny` |
| `package` | 复用 `build-artifacts.yml` 在三种 runner 上完整打包（PR 不跑），确保安装包链路不腐化 |

> 说明：`deny` 目前会因**既有依赖**的许可/公告告警失败（Slint 的
> `GPL-3.0-only OR LicenseRef-Slint-*`、`clipboard-win`/`error-code` 的 BSL-1.0、
> `foldhash` 的 Zlib，以及 `ttf-parser` 的 RUSTSEC-2026-0192 未维护公告）。这些与
> 构建工具重构无关，需要在产品/许可决策后于 `deny.toml` 中显式 `allow`/`ignore`
> 才能让该任务变绿。

## 4. `build-artifacts.yml`（可复用）

输入：`version`（版本标签，留空则用 tag / commit 推导）、`retention-days`。

矩阵（4 个目标）分别执行：

```bash
cargo xtask dist --target <triple> --platform <slug> --formats <list>
```

`--formats`：Windows `portable,msi`；macOS `portable,dmg`；Linux `portable,deb,rpm`。

随后 `checksums` 任务下载所有 artifact，把每个 `.sha256` 汇总为 `SHA256SUMS.txt`
（并校验数量与产物数量一致），单独上传。artifact 名称使用 xtask 输出的包名
（`$GITHUB_OUTPUT` 的 `package`），便于在 Actions 页面直接辨认。

## 5. `release.yml`

| 触发方式 | 行为 |
|---|---|
| push tag `vX.Y.Z` | 构建并发布正式 release（tag 名含 `-` 时自动标记为 prerelease） |
| push tag `portable-*` | 构建并发布 prerelease |
| GitHub UI → Actions → **Release** → **Run workflow** | 只构建，产物作为 Actions artifact（可选填 version），不发布 |

发布任务会把便携包、安装包和 `SHA256SUMS.txt` 一起附加到 Release。

## 6. 本地构建与已知限制

```bash
cargo xtask doctor                                   # 先看工具是否齐全
cargo xtask dist                                     # 仅便携包
cargo xtask dist --formats portable,deb,rpm          # Linux：便携包 + deb + rpm
cargo xtask dist --formats portable,msi              # Windows：便携包 + MSI
cargo xtask dist --formats portable,dmg              # macOS：便携包 + DMG
cargo xtask checksums --dir dist                     # 汇总 SHA256SUMS.txt
```

- 安装包只能在其目标平台（或装有对应工具的平台）上构建：`msi` 需要 WiX、`dmg`
  需要 macOS 的 `hdiutil`、`deb` 需要 `cargo-deb`、`rpm` 需要 `rpmbuild`。
  `--formats` 中请求了当前环境无法构建的格式会直接报错并给出安装提示。
- 未做代码签名：Windows SmartScreen、macOS Gatekeeper 首次运行会提示，属于预期行为。
- 暂无应用图标（`assets/icons/` 仍是占位目录）：安装包使用系统默认图标；
  `packaging/macos/Info.plist` 已支持 `assets/icons/yshell.icns`，补上图标即可生效。
- AppImage 暂不提供（需要图标与 FUSE），Linux 用 deb/rpm 覆盖。

## 7. 后续计划

- Windows：代码签名（EV 证书）、可选 Inno Setup `setup.exe`。
- macOS：Developer ID 签名与 notarization，让 DMG 免 Gatekeeper 提示。
- Linux：AppImage（依赖应用图标）与仓库托管（PPA/COPR）。
- 所有平台：为 release 产物生成 SBOM 与可复现构建校验。
