# Scripts

构建、检查、测试和打包统一走工作区构建工具 `cargo xtask`（源码在 `xtask/`），
Windows / macOS / Linux / WSL 行为一致。本目录只保留容器化的 live 测试、辅助脚本和
遗留的远程同步工具。

## WSL 构建环境

当前开发环境是 WSL2（Debian 13），仓库位于 `/root/yshell`；Windows 侧通过
`\\wsl.localhost\debian\root\yshell` 访问（VS Code、资源管理器可直接打开）。每个新 shell
先 source 环境脚本，再跑构建/检查/测试：

```bash
cd /root/yshell
source scripts/wsl-env.sh
cargo xtask lint
```

`scripts/wsl-env.sh` 做两件事：

- **PATH**：把 `$HOME/.cargo/bin`（rustup 工具链）放到最前。WSL 非登录 shell 不读
  `~/.profile`，默认拿到 Debian 打包的 rustc 1.85，不满足本仓 MSRV 1.92，`cargo xtask`
  会直接失败。
- **共享 target**：导出 `CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/root/yshell-target}`。后续
  所有任务的构建/检查/测试统一复用 `/root/yshell-target`，**不要再按任务自建 target**
  （各自建 target 等于重复全量编译并浪费磁盘）。

约束与提示：

- **不要在 `/mnt/*` 下构建**：9p 文件系统让 cargo 构建极慢；仓库与 target 都在 WSL 原生
  ext4 上。`cargo xtask doctor` 也会给出这样的提示。
- 旧副本 `D:\NewSpace\yshell` 只是历史备份（已冻结）：不要在其中改代码或构建，避免两份
  仓库状态漂移。

## 构建工具：`cargo xtask`

```bash
cargo xtask check      # cargo fmt --check + clippy -D warnings
cargo xtask test       # cargo test --workspace --all-features
cargo xtask ci         # CI 同款门禁：check + test
cargo xtask run        # 从源码运行原生 app（-- --help 透传给 app）
cargo xtask dist       # 产出 release 产物（便携包 + 安装包）
cargo xtask checksums  # 重新生成 dist/SHA256SUMS.txt
cargo xtask deny       # cargo-deny 许可证/安全公告检查
cargo xtask doctor     # 工具链、打包工具、Linux 依赖诊断
cargo xtask clean      # 清理 dist 下由 dist 命令产出的文件（--all 连 target/ 一起清理）
cargo xtask help       # 全部命令与参数
```

常用组合：

```bash
cargo xtask test --live --ssh-target root@127.0.0.1:2222   # 对真实服务器跑 live 测试
cargo xtask dist --formats portable,deb,rpm                # Linux 产物
cargo xtask dist --target x86_64-pc-windows-msvc --formats portable,msi
```

`dist` 的产物命名、安装包格式和所需工具见 [packaging/README.md](../packaging/README.md)；
CI/发布流程见 [docs/product/github-actions-release.md](../docs/product/github-actions-release.md)。

## `test-ssh/`

给 live SSH/SFTP 冒烟测试与 UI E2E 用的临时 sshd 容器（Debian 13 + OpenSSH，公钥 + 密码双认证，
`Subsystem sftp internal-sftp`）。账号与密码覆盖见 [test-ssh/README.md](test-ssh/README.md)。

```bash
bash scripts/test-ssh/run.sh up      # 构建镜像并在 127.0.0.1:2222 启动容器
bash scripts/test-ssh/run.sh test    # up + ssh-agent + 运行 live 测试（cargo xtask test --live）
bash scripts/test-ssh/run.sh live    # 只跑 live 测试（容器需已启动）
bash scripts/test-ssh/run.sh status  # 查看容器状态
bash scripts/test-ssh/run.sh env     # 打印 live 测试环境变量
bash scripts/test-ssh/run.sh down    # 停止并删除容器
```

说明：

- 首次运行会生成一次性测试密钥 `~/.ssh/yshell_test_ed25519`，公钥在容器启动时挂载进
  root 与 tester 账号；容器删除即失效。
- live 测试使用 `AuthMethod::Agent`，脚本会自动启动 ssh-agent 并加载测试密钥。
- 密码流验收用 `tester@127.0.0.1:2222`（默认密码 `yshell-test-pass`，见
  `scripts/test-ssh/README.md`）；root 保持 `prohibit-password`（仅密钥）。
- 可用 `YSHELL_TEST_SSH_PORT` / `YSHELL_TEST_SSH_KEY` / `YSHELL_TEST_SSH_CONTAINER` /
  `YSHELL_TEST_SSH_USER` / `YSHELL_TEST_SSH_PASSWORD` 覆盖默认值。
- 测试目标通过 `YSHELL_LIVE_SSH_TARGET` / `YSHELL_LIVE_SFTP_TARGET`
  传给测试（格式 `user@host:port`）。

## `e2e/`

无头 UI E2E（Xvfb + xdotool + ImageMagick）：驱动真实应用对 Docker 容器做 SSH/SFTP 验证，
主打主机密钥信任与密码弹窗两条流程。结束时打印断言汇总表，失败退出码 1。

```bash
bash scripts/e2e/run-ui-ssh.sh                       # 全流程（dark/zh-CN）
bash scripts/e2e/run-ui-ssh.sh --stage pass          # 只跑密码成功路径
bash scripts/e2e/run-ui-ssh.sh --theme light --lang en-US
bash scripts/e2e/run-ui-ssh.sh --help                # 参数/环境变量/已知限制
```

- 依赖：Xvfb、xdotool、ImageMagick；`scripts/test-ssh/run.sh up` 的容器；
  `cargo xtask build` 产出的 `target/debug/yshell`。
- 截图输出到 `dist/ui-checks/e2e-<theme>-<lang>-NN-<step>.png`。
- 细节与 stage 说明见 `docs/product/ui-winui3-verification.md` §3.1。

## `sync-and-test-remote.ps1`（legacy）

把工作区同步到远程主机并执行测试命令。原 Debian 验证 VM 已退役；如仍有可用的远程
Linux 主机，可以继续使用：

```powershell
.\scripts\sync-and-test-remote.ps1 -RemoteHost <host> -RemoteUser <user>
.\scripts\sync-and-test-remote.ps1 -RemoteHost <host> -TestCommand "cargo xtask test"
```

新开发流程请优先使用 `cargo xtask` 和 `scripts/test-ssh/run.sh`。

## 其它

- `i18n/extract.sh`：从源码提取翻译字符串到 `translations/`。
- `ui-icons/`：Slint 图标资源生成脚本。
- `measure-scrollback.sh`：终端回滚性能测量辅助脚本。
