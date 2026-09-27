# test-ssh：YShell 真连验证容器

给 SSH / SFTP 真连测试（`cargo xtask test --live`、UI E2E、手工排查）用的可丢弃
sshd 容器。镜像基于 `debian:13-slim` + `openssh-server`，入口脚本
`entrypoint.sh` 在启动时准备账号与 `sshd_config`。

## 账号与端口

| 用途 | 地址 | 认证 | 说明 |
| --- | --- | --- | --- |
| 密钥/ssh-agent（live tests 默认） | `root@127.0.0.1:2222` | 公钥（`~/.ssh/yshell_test_ed25519`） | `PermitRootLogin prohibit-password`，root 不允许密码 |
| 密码认证（密码弹窗 / SFTP） | `tester@127.0.0.1:2222` | 密码（默认 `yshell-test-pass`） | home 可写（`/home/tester`），同时装了同一把测试公钥方便 `ssh -i` 连通性检查 |

`PasswordAuthentication yes` 与 `KbdInteractiveAuthentication yes`（`UsePAM yes`）
只在这个测试容器里开启；没有被真实部署配置引用。

## 用法

```bash
# 构建镜像 + 启动容器（幂等：已在运行则直接返回）
bash scripts/test-ssh/run.sh up
# 或在仓库根：
bash scripts/wsl-dev.sh test-ssh up

bash scripts/test-ssh/run.sh status   # 容器状态 + 两组目标/密码提示
bash scripts/test-ssh/run.sh env      # 打印 live tests / E2E 用的环境变量
bash scripts/test-ssh/run.sh live     # ssh-agent + cargo xtask test --live
bash scripts/test-ssh/run.sh test     # up + live
bash scripts/test-ssh/run.sh shell    # 进容器交互 shell
bash scripts/test-ssh/run.sh down     # 停止并删除容器
```

改了 `entrypoint.sh` / `Dockerfile` 后必须重建，否则改动不生效：

```bash
bash scripts/test-ssh/run.sh down && bash scripts/test-ssh/run.sh up
```

## 环境变量覆盖

| 变量 | 默认 | 用途 |
| --- | --- | --- |
| `YSHELL_TEST_SSH_PORT` | `2222` | 宿主机端口 |
| `YSHELL_TEST_SSH_KEY` | `~/.ssh/yshell_test_ed25519` | 测试密钥（缺失时自动生成，公钥 bake 进镜像） |
| `YSHELL_TEST_SSH_CONTAINER` | `yshell-test-ssh` | 容器名 |
| `YSHELL_TEST_SSH_USER` | `tester` | 密码认证用户名 |
| `YSHELL_TEST_SSH_PASSWORD` | `yshell-test-pass` | 密码认证密码（`run.sh up` 会透传进容器） |
| `YSHELL_TEST_SSH_IMAGE` | `yshell-test-ssh` | 镜像名 |

`entrypoint.sh` 直接跑在容器里时读同名的 `YSHELL_TEST_SSH_USER` /
`YSHELL_TEST_SSH_PASSWORD`；两者都不允许为空，也不允许包含 `:`（`chpasswd`
的行格式限制）。

## UI E2E（密码弹窗）

仓库根的 `.tmp-e2e-ssh.sh` 各 stage 依赖：

- key 会话：`root@127.0.0.1:2222` + `host_key_policy = "strict"`；
- 密码会话：`tester@127.0.0.1:2222` + `host_key_policy = "trust_on_first_use"`
  （首次连接同样必须弹信任弹窗；见 `docs/product/ui-winui3-design-language.md` §5.9）；
- 密码值取自本文件默认 `yshell-test-pass`（或启动容器时的覆盖值）。

注意：`.tmp-e2e-ssh.sh` 的 `pass-ok` stage 目前输入的是错误密码（用于覆盖
"密码错 → 状态栏错误"这条失败路径）；成功路径需要输入 `tester` 的真实密码。
