# E0 构建环境与路径收口（设计）

状态：**待用户确认**（确认后分配唯一 owner）
设计/验收：主 agent
关联：`yshell-next-features-requirements.md`（D32）、`yshell-next-t0-slint-upgrade.md`

## 背景

- 仓库已迁到 WSL ext4：`/root/yshell`（Windows 访问：`\\wsl.localhost\debian\root\yshell`）。旧 `D:\NewSpace\yshell` 保留为备份，不再使用。
- 9p（`/mnt/d`）文件系统让 cargo 构建极慢；此前多条并行线各自建 target，等于重复全量编译。
- 新环境事实：WSL 非登录 shell 默认拿到 Debian 的 `rustc 1.85.1`（不满足 MSRV 1.92）；rustup 工具链在 `$HOME/.cargo/bin`（当前 1.98.x），脚本必须显式加 PATH（T0 验收第一次失败就是这个原因）。

## 范围

1. **新增 `scripts/wsl-env.sh`（可 source）**
   - `export PATH="$HOME/.cargo/bin:$PATH"`
   - `export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/root/yshell-target}"`
   - 注释说明：登录 shell 与脚本环境的差异、共享 target 的原因。
2. **`scripts/README.md` 新增"WSL 构建环境"节**
   - source `wsl-env.sh`、共享 target 规范、"不要在 `/mnt/*` 下构建"、仓库位置与 UNC 访问、旧副本冻存提示。
3. **`README.md`**
   - 第 105 行命令 `/mnt/d/NewSpace/yshell` → `/root/yshell`。
   - WSL 开发节补充：新路径、`\\wsl.localhost\debian\root\yshell` 访问方式、旧副本（`D:\NewSpace\yshell`）为历史备份、不要再在其中改代码/构建。
4. **清理旧 target 目录**
   - 删除 `/root/yt-a0`、`/root/yt-c0`、`/root/yt-c0-scratch`、`/root/yt-f0`、`/root/yt-g0`、`/root/yt-l3`、`/root/yt-n5a`、`/root/yt-n5a-probe-target`；保留 `/root/yt-n5a-evidence`（N5a 证据）与 `/root/yshell-target`（共享 target）。
   - 执行前提：T0 验收通过、各任务证据已归档；输出清理前后 `df`/`du` 对比作为证据。
5. **任务规范落文案**
   - 在 `scripts/README.md` 写明：后续所有任务构建统一 `CARGO_TARGET_DIR=/root/yshell-target`；不再按任务自建 target。

## 不做

- 不改 CI、cargo 配置或代码逻辑（仅脚本/文档/清理）。
- 不迁移 git remote、不动 `dist/` 证据、不删除旧 `D:` 副本（是否重命名/删除由用户决定）。

## 验收

1. 业务面（`README.md`、`scripts/`、代码）grep：`/mnt/d/NewSpace` 零命中；`docs/product/` 任务书内的历史路径自述除外（验收实测：业务面 0 命中）。
2. **非登录 shell** 下 `source scripts/wsl-env.sh && cargo xtask lint` 通过（验证 PATH 与共享 target 生效）。
3. README 的 WSL 运行命令按新路径可直接执行（复核命令文本即可，实际运行由主 agent 抽查）。
4. `yt-*` 目录清理完成，附清理前后磁盘占用对比。
5. 证据（命令输出/目录列表）随交付报告提交，主 agent 验收。

## Owner

（待分配；预计为 W1 之前的独立小任务）
