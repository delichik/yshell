# YShell Milestone 0 实施计划

日期：2026-06-03
验收基线：`docs/product/yshell-detailed-development-plan.md` Milestone 0 与每个 PR 最低要求。

## 任务拆分

1. 创建 Rust workspace 与所有产品规划 crate。
   - 输出：根 `Cargo.toml`、`Cargo.lock`、`crates/yshell-*`。
   - 验收：`cargo test --workspace` 能发现并构建全部 workspace member。
2. 建立应用入口和基础 app shell。
   - 输出：`crates/yshell-app/src/main.rs`、`bootstrap.rs`、`app_state.rs`、`commands.rs`、`error.rs`。
   - 验收：二进制 `yshell` 可编译，启动路径不直接实现 SSH/secret/terminal parser。
3. 建立 UI ViewModel 与 Slint 文件骨架。
   - 输出：`crates/yshell-ui/src/**`、`ui/main_window.slint`、`ui/components/**`、`ui/pages/**`。
   - 验收：ViewModel 仅保存展示状态；Slint 文件仅作为布局/事件占位。
4. 建立 SSH/SFTP/Terminal/Core/Secret/Logging/Test-support 边界。
   - 输出：各 crate 的模块占位与最低单元测试。
   - 验收：模块名与产品文档职责一致，跨 crate 边界清晰。
5. 实现配置目录发现逻辑。
   - 输出：`yshell-config::paths`。
   - 验收：覆盖显式环境变量、Unix XDG、macOS Application Support、Windows APPDATA、HOME fallback。
6. 实现 tracing 日志初始化。
   - 输出：`yshell-logging::init_tracing`。
   - 验收：初始化函数幂等，非法 filter 有错误返回。
7. 落地 GitHub Actions CI/release workflow。
   - 输出：`.github/workflows/ci.yml`、`.github/workflows/release.yml`。
   - 验收：CI 包含 fmt、clippy、test、license/advisory；release 支持 `vX.Y.Z` tag。
8. 本地验收并提交。
   - 命令：`cargo fmt --all --check`。
   - 命令：`cargo clippy --workspace --all-targets -- -D warnings`。
   - 命令：`cargo test --workspace`。
