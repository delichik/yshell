# 构建产物与目录约定

本文件是**构建目录、产物落点、命令、清理规则**的唯一约定。

## 1. 构建目录就在仓库里：`<repo>/target`

按 Cargo 标准布局，构建产物统一在**工程目录内**的 `target/`（已 `.gitignore`）：

```text
D:\NewSpace\yshell\target\
  debug\                  # cargo build / cargo test
  release\                # cargo build --release
  <triple>\release\       # cargo xtask dist 的打包中间目录
```

- **不要**设置 `CARGO_TARGET_DIR` 指向仓库外（历史遗留的 `D:\SDK\cargo-target` 已删除，不再使用）。
- 不要按任务另建 `target-*`；一个仓库只有一个 `target/`。
- `CARGO_HOME`（依赖下载/注册表缓存，不是构建目录）可以继续用机器上的 `D:\SDK\cargo-home`，也可用默认 `%USERPROFILE%\.cargo`；它不影响产物位置。

## 2. 产物地图

| 产物 | 规范路径 | 说明 |
|---|---|---|
| debug 二进制 | `target\debug\` | 本地开发 / 冒烟；`gterm`、`gterm-ui`、`gterm-session-worker`、`gterm-mcp-daemon` |
| release 二进制 | `target\release\` | 正式 4 个 exe（LTO + strip） |
| 打包 staging | `target\<triple>\release\` | `cargo xtask dist` 中间目录，打包完成后可随 `cargo clean` 清掉 |
| 可分发产物 | `dist\` | 只允许最新 `gterm-<version>-<platform>-<formats>.zip` + `.sha256` |
| 测试临时配置 | `%TEMP%\yshell-*` | 用完即删，不要放进仓库 |
| 诊断日志 | `%TEMP%\yshell-logs\` 或配置目录 `logs\` | 不要写到仓库根目录 |

发布包固定 **4 个 exe**（`gterm` Host 入口 + `gterm-ui` / `gterm-session-worker` / `gterm-mcp-daemon`），**没有 `gterm-host.exe`**。

## 3. 命令

```powershell
cargo build -p yshell-app --bins                 # debug
cargo build -p yshell-app --bins --release       # release
cargo xtask check                                # fmt + clippy -D warnings
cargo xtask test                                 # workspace tests
cargo xtask ci                                   # check + test
cargo xtask doctor                               # 工具链/打包工具检查
cargo xtask dist --target x86_64-pc-windows-gnu --platform windows-x86_64 --formats portable
```

## 4. 清理规则（每次交付前）

1. 仓库里只允许源码 + `.gitignore` 掉的 `target/`、`dist/`；不要提交任何构建产物。
2. `dist/` 只保留**最新** zip + `.sha256`；删除解包目录、`.work/`、evidence、散落 exe。
3. 回收空间用仓库内的 `cargo clean`（会删整个 `target/`），不要手工去搬 `target/`。
4. 提交前：

```powershell
git status --short
git status --ignored --short | Select-String 'target|dist'
```

## 5. 常见错误

- **把 `CARGO_TARGET_DIR` 指到仓库外**（例如 `D:\SDK\cargo-target`）→ 产物散落在工程外，禁止。
- **在仓库里建 `target-msvc/`、`target-gnu-fp/`** → 重复缓存，发现即删。
- **手工复制 exe 进 `dist/`** → 只通过 `cargo xtask dist` 产出。
- **拿旧 `dist` 包验证新代码** → 必须记录包名与 SHA256，或先重新打包。
