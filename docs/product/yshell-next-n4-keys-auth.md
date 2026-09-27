# N4 密钥管理 + 认证弹窗（设计）

状态：**待确认**；依赖：A0（认证层）✅、C0（keys schema）✅、R2（runtime 模块）与 rfd spike
关联决策：D18–D21、需求 §5
文件落位：新 `ui/pages/private_keys.slint`、`ui/pages/host_keys.slint`（或升级现有 known_hosts 页）、新 `ui/components/auth_prompt_dialog.slint`；`runtime/auth.rs` + `runtime/mod.rs`；`ui/main_window.slint`（最小接线 + 菜单入口）；`bootstrap.rs`；`rfd`（spike 后入库）；secret store 既有 API

## 1. 密钥管理（两个同级、独立入口）
- **私钥管理**（新页面）：列表（名称/算法/指纹/口令状态/被哪些会话使用）；导入（rfd 或路径输入）→ 内容与口令进 secret store；删除（被引用告警）；"测试"（解析 + 可选连接验证）；详情内**派生公钥**：查看/复制/导出。
- **主机密钥**（独立页面，与私钥同级）：升级现有 known_hosts：按 `host:port` 分组、SHA256 指纹、导出/导入、清空确认文案。
- 入口：Session 菜单并列两项 `Private Keys…` 与 `Host Keys…`（同级、不同界面）；Settings 亦可跳转。
- **不生成密钥**（D20）；公钥不做独立清单（D18）。

## 2. 认证弹窗（按服务端能力）
- 数据来自 A0：`AuthMethods` 与 `AuthProblemKind`（错误时）。规则：默认选中会话配置的方式（若允许），否则第一个可用；一次弹窗内可多方式重试，不清空已填内容。
- 选项 UI：
  - 密码：密码框 + "记住密码"（写 secret store）；
  - 公钥：下拉（私钥清单）+ 浏览文件 + passphrase + "使用 ssh-agent"；
  - keyboard-interactive：按 `KeyboardInteractiveChallenge` 动态渲染（name/instruction/多 prompt/echo；多轮）；
  - "其它可用方式"列表随 `AuthMethods` 显隐（服务端仅 publickey 时不显示密码）。
- 运行时：`PendingAuthPrompt` 状态机（从 `PendingPasswordPrompt` 扩展）；host key 弹窗仍在认证之前。
- 敏感值：关闭/取消清空；不落日志。

## 3. 验收
1. 服务端仅 publickey → 无密码输入；仅密码 → 无密钥选择；keyboard-interactive 多 prompt 如实呈现（用 A0 Fake 场景驱动）。
2. 导入私钥 + 记忆口令 → 后续零输入连接；公钥可复制/部署（追加 `authorized_keys` 幂等 + 权限修正，带确认）。
3. 私钥页/主机密钥页为两个独立入口；删除/导出/部署均有确认与错误反馈。
4. `cargo xtask lint/test` 全绿；e2e 双主题；截图 `dist/ui-checks/n4-*`（私钥页/主机密钥页/认证弹窗三态/部署确认）。

## 4. 依赖落地记录（D0c，2026-09-28）

工作区依赖已入库（根 `Cargo.toml`；**未新增使用方**，未引用前不进 `Cargo.lock`）：

```toml
ssh-key = { version = "0.6.7", default-features = false, features = ["std", "encryption", "ecdsa"] }
```

- **版本**：crates.io 当前 stable 最高版 `0.6.7`（`0.7.0-rc.*` 仍是预发布，不选）；MSRV 1.65，兼容工作区 1.92；许可 `Apache-2.0 OR MIT`。
- **features 依据**（官方 docs.rs、仓库 `Cargo.toml`，并以真实 `ssh-keygen` 密钥在 `/root/ssh-key-spike/` 逐项验证）：
  - `std`（隐含 `alloc`）：`PrivateKey/PublicKey::to_openssh()` 需要 alloc；`Error: std::error::Error` 便于 anyhow；另有 `read_openssh_file` 路径导入。
  - `encryption`：启用 `PrivateKey::decrypt()`（bcrypt-pbkdf + aes256-ctr），带口令私钥必需。
  - `ecdsa`：**唯一必须的算法特性**——启用 sec1 与 ECDSA 密钥数据结构；去掉它 `from_openssh` 对 ECDSA 报 `Error::AlgorithmUnknown`。
  - `ed25519`/`rsa` **不需要**：这两个特性只开启签名/验签/生成所需的 `ed25519-dalek`/`rsa` 依赖；实测不开启时 ed25519、RSA（含 2048 位与带口令）的解析、公钥派生、`to_openssh()`、`decrypt()`、重新编码全部正常，且与 `ssh-keygen -y` 逐字节一致。curve 特性 `p256/p384/p521` 同理不需要（仅签名/验签/生成用），仅 `ecdsa` 即可覆盖 P-256/384/521。
  - 若 N4 后续确需签名/验签/生成（当前设计无此需求；D20 不生成密钥），一行加回 `ed25519`/`rsa` 即可。
  - 未选：`rand_core/getrandom`（不生成密钥）、`dsa`（过时/弱）、`serde`、`tdes`。
- **行为实测**（输出 `/root/d0c-spike-output-variantD.txt`）：
  - ed25519 / RSA / ECDSA(P-256/384/521) 无口令与带口令：解析 → 派生公钥 → `to_openssh()` 全部成功；无口令公钥与 `ssh-keygen -y` 逐字节一致，带口令的经 `decrypt(口令)` 后与 `ssh-keygen -y -P` 完全一致。
  - `PublicKey::from_openssh`（主机密钥页用）对 ed25519 / RSA / ECDSA 公钥同样支持。
  - 带口令私钥解析后 `comment()` 为空、派生公钥无 comment（comment 在加密段内）；`decrypt` 成功后 comment 恢复。
  - **错误口令**：`decrypt` 返回 `Error::Crypto`（"cryptographic error"）→ N4 映射为"口令错误"。OpenSSH 格式无 MAC，误判概率 2^-32，解析成功后的公钥即最终公钥。
  - **旧版 PEM 不支持**（PKCS#1，`ssh-keygen -m PEM`）：`Error::Encoding(Pem(Base64(InvalidEncoding)))` → 映射为"仅支持 OpenSSH 格式（BEGIN OPENSSH PRIVATE KEY）"。
- **许可**：新增/复用依赖均在 `deny.toml` allowlist（MIT/Apache-2.0/BSD-2/BSD-3/ISC）；`MIT/Apache-2.0` 非严格写法 cargo-deny 以 lax 模式接受（仓库已有 18 个同类 crate 先例）。
- **公告（首次消费后需确认/处理）**：`ssh-key` 的弱引用可选依赖会把 `rsa 0.9.10` 带进 `Cargo.lock`（即使未启用 `rsa` 特性），命中 **RUSTSEC-2023-0071**（Marvin 计时侧信道，`patched=[]`，截至 2026-09-12 无修复）。本特性集下 `rsa` 不参与编译、不执行私钥运算，风险可控；首次引用 `ssh-key` 并更新 lock 后请跑 `cargo xtask deny`，如命中则在 `deny.toml` `[advisories] ignore` 加该条并注明理由（deny.toml 不在 D0c 范围）。
- **lock 预览**：独立解析 77 个包（其中 `rsa`/`p256`/`p384`/`p521` 等因 `std` 的弱引用进入 lock 但不编译）；与仓库现有 lock 同版本复用 14 个，净新增约 62 个，实际消费时 cargo 优先沿用现值，以届时解析为准。


