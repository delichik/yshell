#!/usr/bin/env bash
# WSL 构建环境（source 使用，不要直接执行）：
#
#   cd /root/yshell
#   source scripts/wsl-env.sh
#   cargo xtask lint
#
# 两件事：
# 1) PATH：把 rustup 工具链目录（$HOME/.cargo/bin）放到最前。WSL 非登录 shell
#    不读 ~/.profile，默认拿到 Debian 打包的 rustc 1.85（不满足本仓 MSRV 1.92）；
#    T0 验收第一次失败就是这个原因。
# 2) CARGO_TARGET_DIR：默认指向共享的 /root/yshell-target（调用方已显式设置则
#    尊重现有值）。此前各任务自建 target，等于重复全量编译并浪费磁盘；后续所有
#    构建/检查/测试统一复用这一个 target，不要再自建。
#
# 注意：只在 WSL 原生文件系统（ext4，如 /root）上构建；9p（/mnt/*）下 cargo
# 极慢。Windows 侧访问仓库：\\wsl.localhost\debian\root\yshell

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/root/yshell-target}"
