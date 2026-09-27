# rfd 后端选型 spike 报告

状态：**已完成（2026-09-27）**；类型：调研 spike（仓库代码零改动，仅新增本报告）
对应设计：`docs/product/yshell-next-rfd-spike.md`；关联决策：D2 / D4 / D31；供 **D0b** 一次性入库
实验工程：`/root/rfd-spike/`（临时，不入库）；证据：`/root/rfd-spike/evidence/`

---

## 0. 结论摘要

| 项 | 结论 |
| --- | --- |
| Linux 后端 | **`gtk3`**（`default-features = false, features = ["gtk3"]`），不依赖 xdg-portal |
| 版本 | **`rfd = "0.17.2"`**（2026-01-12 发布，最新稳定） |
| 前端调用方式 | **必须从 worker 线程调用**，结果用 `slint::invoke_from_event_loop` 或 mpsc + 现有 120ms UI Timer 回填；UI 线程直调会冻结渲染（实测 tick delta = 0） |
| 构建依赖（Linux） | `libgtk-3-dev`（Debian 13：3.24.49-3，含依赖共 118 包；pkg-config `gtk+-3.0`） |
| 运行依赖（Linux） | `libgtk-3-0`（trixie 上由 `libgtk-3-0t64` Provides）；deb/rpm 元数据需补 |
| 降级 | 保留内置路径输入；**gtk3 的不可用判定不能依赖 `None`**（用户取消也返回 `None`，且无可用 display 时会静默永久阻塞），需调用前预检 |
| 打包切换 | gtk3 与 xdg-portal 在 rfd 里是**编译期互斥**（同时启用直接 build.rs panic）→ 以 cargo feature 透传（`dialog-gtk3` 默认 / `dialog-portal`）支持 Flatpak 等场景 |
| cargo-deny | gtk3 路线新增 `target-lexicon 0.12.16`（`Apache-2.0 WITH LLVM-exception`）→ `deny.toml` 的 `allow` 必须加该标识符；portal 路线不新增 |

---

## 1. 环境与前置

- Debian 13 (trixie)、WSL2（内核 6.18.33.2）、WSLg：`DISPLAY=:0` + `WAYLAND_DISPLAY=wayland-0`（XWayland + Weston/WSLg RAIL），`/tmp/.X11-unix/X0` 可用，`xdpyinfo` 正常。
- Rust：rustup stable `cargo/rustc 1.98.1`（系统 `/usr/bin/cargo` 为 1.85.1，未使用）；仓库 MSRV 1.92 满足。cargo 源替换为 rsproxy-sparse 镜像（不影响 lock 中的 crates.io 源 URL）。
- DBus：`/run/user/0/bus` **存在**（session bus 可用），但 `org.freedesktop.portal.Desktop` **未提供**（无 xdg-desktop-portal）；无 `zenity`；`libdbus-1.so.3` 存在。
- 测试工具：`Xvfb 2:21.1.16-1.3`、`xdotool`、`xwininfo`、`scrot`、ImageMagick `import`（均可用于无头回归）。
- 安装（本次唯一系统改动，已记录）：`apt-get install -y --no-install-recommends libgtk-3-dev`
  - 结果：`libgtk-3-dev 3.24.49-3` + `libgtk-3-0t64 3.24.49-3` 等共 **118 个包**；顺带 **升级 5 个既有包**（libglib2.0-0t64、libglib2.0-data、libcap2、libcap2-bin、libpcre2-8-0）。
  - 证据：`evidence/apt-gtk3-dev.txt`。

---

## 2. 路线 A：`rfd` + GTK3

### 2.1 构建前置与失败路径

- **缺 dev 包时**：`gtk-sys` / `gdk-pixbuf-sys` 等经 `system-deps + pkg-config` 直接失败，错误信息明确：
  `The system library 'gdk-pixbuf-2.0' required by crate 'gdk-pixbuf-sys' was not found. … HINT: you may need to install a package such as gdk-pixbuf-2.0-dev`
  证据：`evidence/build-rfd-gtk3.txt`、脚本 `missing-gtk-dev.sh`。
- **编译成本**（本机 warm 缓存）：`rfd` + 9 个 `-sys` crate ≈ **1.5 s**（纯 Rust 绑定 + pkg-config 探测，**无 C 代码编译**）。对照 `rfd` + portal 的等价构建 ≈ 3.5 s。Slint 1.18.1 全量冷编译 ≈ 40 s（与 rfd 无关，可复用）。
- **链接面**：`ldd` 出现 `libgtk-3.so.0 / libgdk-3.so.0 / libgdk_pixbuf-2.0.so.0 / libwayland-client.so.0 …`，共 65 个共享库（对照 portal 路线仅 4 个）→ **运行期必须有 GTK3**。
  证据：`evidence/build-rfd-gtk3.txt`。

### 2.2 运行结果（弹窗与返回值）

| 场景 | 结果 |
| --- | --- |
| WSLg，GDK 默认（Wayland） | 对话框**真实弹出**为 WSLg Wayland 窗口：`/mnt/wslg/weston.log` 记录 `appId: rfd-gtk3-spike  WindowId: 0x…`（`associateWindowId: 1`）。调用线程阻塞等待用户输入。 |
| WSLg，`GDK_BACKEND=x11` | `xwininfo` 可见 `0x600004 "yshell rfd spike" 1096x822`；`xdotool`（Ctrl+L → 路径 → Enter）驱动 → `pick_file` 返回 `Some("/root/rfd-spike/evidence/picked-e1b-gtk3-gdk-x11.txt")`，耗时 8.39 s（含等待交互）；`save_file` 8.49 s、`pick_folder` 8.50 s 同样返回正确路径。 |
| Xvfb `:99`（无 WM，无头） | pick/save/folder **全部成功**（8.44 / 8.29 / 8.30 s），截图可见真实 GTK 文件选择器（侧栏 / 列表 / Cancel / Open）。 |
| 仅 unset `WAYLAND_DISPLAY` | 对话框仍走 Wayland（GDK 回落到默认 socket `wayland-0`）→ **强制 X11 必须显式 `GDK_BACKEND=x11`**。 |

证据：`evidence/e1b…e1h-*.log|png`、`evidence/e2a…e2c-xvfb-*.png`（真实对话框截图）、`evidence/e1d…e1f-*.log`、`/mnt/wslg/weston.log` 片段。

### 2.3 对 Slint/winit 事件循环的影响（关键）

- **源码事实**：rfd 0.17 的 gtk3 后端把 GTK 固定在自己的**专用线程**上（`GtkGlobalThread`：懒初始化 `gtk_init_check()` + 常驻 `gtk_main_iteration()` 循环；对话框经 `g_idle_add_full` 投递；同步 API 用 condvar 等待）。**GTK 不侵入调用方的事件循环**，也不要求调用方是主线程。
- **实测**（Slint 1.18.1 `backend-winit-x11` + `renderer-software`，与仓库 `crates/yshell-app` 同配置，Xvfb）：

| 模式 | 对话框打开期间 tick 增量（500ms/tick，观察 4 s） | 结论 |
| --- | --- | --- |
| UI 线程直调（sync） | **0** | 事件循环冻结：窗口不重绘、输入不响应；用户操作后 `rfd returned Some(...)`，tick 恢复 |
| worker 线程 + `invoke_from_event_loop` | **8** | 事件循环存活；结果回填 Slint 属性（截图显示 `status: picked (worker thread)`、`picked: /root/…txt`） |

证据：`evidence/s1-slint-gtk3-sync.log`（`delta_while_dialog_open=0`）、`evidence/s2-slint-gtk3-thread.log`（`delta=8`）、`evidence/s2-slint-gtk3-thread-final-app.png`、`evidence/s1-slint-gtk3-sync-dialog.png`。

### 2.4 错误路径（必须注意）

**无可用显示时不报错、静默永久阻塞**：
- `DISPLAY=:77`（无效）+ 无 Wayland（`WAYLAND_DISPLAY` 未设、`XDG_RUNTIME_DIR` 为空）→ `pick_file()` **30 s 仍未返回**（被 `timeout` 杀掉，exit 124）；watchdog 模式显示 worker 一直 blocked。
- 无 `DISPLAY` 时同样阻塞。
- 根因（源码）：`gtk_init_check()` 失败后 GTK 线程直接 `return`，循环不再运行 → `g_idle_add_full` 投递的回调**永远不会被派发** → 同步 API 在 condvar 上永久等待；且 `GtkGlobalThread` 是 `OnceLock`，GTK 不会二次初始化，进程内该后端此后一直不可用。
- 证据：`evidence/gtk3-nodisplay.log`（脚本 `gtk3-nodisplay.sh` 输出）。

> 落位到 yshell：应用自身用 `backend-winit-x11`，只要窗口已渲染就说明 X11 可用，因此正常场景下 GTK 也能拿到 display；但**若把 `GDK_BACKEND` 强设为与运行会话不一致的值（例如 Wayland-only 环境强设 x11）**，就会命中该静默阻塞。这是 D0b 文档/代码里必须写明的约束。

---

## 3. 路线 B：`rfd` + XDG Portal

### 3.1 本机 portal 状态

- `dpkg -l` 无 `xdg-desktop-portal*`；`dbus-send --session … org.freedesktop.DBus.Peer.Ping` 到 `org.freedesktop.portal.Desktop` 返回 `ServiceUnknown: … was not provided by any .service files`。
- `libdbus-1.so.3` 存在；`zenity` 未安装。

### 3.2 构建与运行

- **零编译期系统依赖**：把 `PKG_CONFIG_LIBDIR/PKG_CONFIG_PATH` 指向不存在目录，portal 路线照常构建成功；`ldd` 仅 `libc/libgcc/ld-linux`，`readelf -d` NEEDED 只有 3 项 → libdbus 是 **dlopen** 的（源码 `portal/libdbus.rs`：`Libdbus::open_libdbus()`，失败时 `error!("Can't connect to a portal: libdbus-1.so not found")`）。
- **无 portal 的链路（实测，13.5–15.5 ms 返回 `None`，不悬挂）**：
  ```
  ERROR rfd::…::portal  OpenFile failed: Some("org.freedesktop.DBus.Error.ServiceUnknown"):
                        Some("The name org.freedesktop.portal.Desktop was not provided by any .service files")
  WARN  rfd::…::xdg_desktop_portal  Using zenity fallback
  ERROR rfd::…::xdg_desktop_portal  Failed to pick file with zenity: No such file or directory (os error 2)
  ```
  `save_file` 同构；把 `DBUS_SESSION_BUS_ADDRESS` 指向不存在的 socket 时同样快速失败（`FileNotFound`）。
- **与同步 UI 的桥接**：0.17.2 的同步 API 内部就是 `pollster::block_on`，**可从任意线程调用**（实测 worker 线程调用，主线程 tick 保持刷新，无冻结）。
- **Slint 集成实测**：portal 构建点击后 15 ms 返回 `None`，UI 未冻结，状态显示 `picker returned None -> fall back to in-app path input (D4 fallback)`（截图 `evidence/portal-s1-slint-gtk3-sync-final-app.png`）。

证据：`evidence/e3a…e3d-*.log`、`evidence/build-rfd-portal.txt`、`evidence/portal-*.log|png`。

### 3.3 未验证（风险，见 §7）

- 有 portal 的**正路径**（原生对话框、父窗口/窗口标识、Wayland 变体）本机无法验证（不安装 portal 以免改动环境）。

---

## 4. 依赖解析、许可证与编译互斥性

### 4.1 Cargo.lock 预览（对仓库**副本**解析，仓库未改动）

基线（原样）598 个 package；解析耗时 0.4 s（lock 已最新，无其它变化）。

| 路线 | 新增 lock 项 | 列表 |
| --- | --- | --- |
| `features=["gtk3"]` | **+17** → 615 | rfd 0.17.2, gtk-sys 0.18.2, glib-sys 0.18.1, gobject-sys 0.18.0, gdk-sys 0.18.2, gdk-pixbuf-sys 0.18.0, pango-sys 0.18.0, atk-sys 0.18.2, cairo-sys-rs 0.18.2, gio-sys 0.18.1, system-deps 6.2.2, cfg-expr 0.15.8, target-lexicon 0.12.16, version-compare 0.2.1, winapi 0.3.9 + winapi-i686/x86_64-pc-windows-gnu 0.4.0 |
| `features=["xdg-portal"]` | **+2** → 600 | rfd 0.17.2, pollster 0.4.0 |

`libc / log / percent-encoding / raw-window-handle / objc2* / windows-sys / wasm-bindgen / web-sys` 等仓库 lock **已存在**，无重复版本引入。
证据：`evidence/lock-diff.txt`、`/root/rfd-spike/lock-0{0,1,2}-*.lock`。

### 4.2 许可证（cargo-deny 0.20.2，使用仓库 `deny.toml` 原样）

- **基线即 FAILED**（与 rfd 无关，需 owner 另行确认）：`clipboard-win`/`error-code`（BSL-1.0）、`foldhash`/`slotmap`（Zlib）、`i-slint-*`/`slint*`（`GPL-3.0-only OR LicenseRef-Slint-*`）。CI 的 `cargo xtask deny` 版本/配置可能与此不同，请复核。
- **gtk3 路线新增 1 项未许可**：`target-lexicon 0.12.16  license = "Apache-2.0 WITH LLVM-exception"`（链：`gtk-sys → system-deps → cfg-expr → target-lexicon`，Linux 构建脚本用）。
  → `deny.toml` 的 `[licenses] allow` **需加 `"Apache-2.0 WITH LLVM-exception"`**。
- **portal 路线不新增未许可项**。
- advisories：`crossbeam-epoch 0.9.18`（RUSTSEC-2026-0204）为**既有**问题（Slint 依赖链），与 rfd 无关。
证据：`evidence/deny.txt`（含拒绝原因原文）。

### 4.3 编译期互斥（不可运行时二选一）

`rfd 0.17.2/build.rs` 在同时启用两个 feature 时直接 panic：

```
error: failed to run custom build command for `rfd v0.17.2`
  thread 'main' panicked at rfd-0.17.2/build.rs:14:17:
  You can't enable both `gtk3` and `xdg-portal` features at once
```

同时 `src/backend.rs` 把 gtk3/portal 模块互相 `cfg` 排除 → **必须编译期选一个**，打包变体要靠上层 cargo feature 透传。

---

## 5. D0b 入库建议

### 5.1 依赖与 features（Linux 默认 gtk3，可切换 portal）

```toml
# Cargo.toml（workspace）
[workspace.dependencies]
rfd = { version = "0.17.2", default-features = false }

# crates/yshell-app/Cargo.toml
[dependencies]
rfd.workspace = true

[features]
# 默认走 GTK3（桌面发行版 / WSLg / 便携包）
default = ["dialog-gtk3"]
dialog-gtk3 = ["rfd/gtk3"]
# 供 Flatpak / 无 GTK3 的环境切换（含 Wayland 窗口标识）
dialog-portal = ["rfd/xdg-portal", "rfd/wayland"]
```

要点：
- **不要**用 rfd 的默认 features（0.17 默认是 `xdg-portal` + `wayland`）；本机 WSLg 无 portal，默认会在 15 ms 内静默返回 `None` 并触发 zenity 兜底缺失错误。
- `rfd/gtk3` 在 Windows/macOS 上是 no-op（官方文档确认），无需按 target 拆分 features。
- 代码侧不要显式引用具体后端（`FileDialog::pick_file()` 即可）；只有「不可用探测」需要按 feature 分支（见 §6）。

### 5.2 系统依赖 / CI / 打包

| 位置 | 需要补充 |
| --- | --- |
| CI `setup-build-env` | Linux job 增加 `libgtk-3-dev`（Ubuntu runner；Debian 同名）；建议仅在需要构建 Linux 产物的 job 加，或用 input 开关 |
| 无头 e2e | `xvfb` + `xdotool`（验证弹窗/自动化用例；本次已用 Xvfb+Xdotool 跑通全流程） |
| deb（`[package.metadata.deb].depends`） | 追加 `libgtk-3-0`（trixie 的 `libgtk-3-0t64` `Provides: libgtk-3-0`）；因 GTK3 是**直接链接**（非 dlopen），`$auto` 也能带出，显式声明更稳 |
| rpm（`packaging/linux/yshell.spec.in`） | `Requires: gtk3`（以及可能的 `gtk3-libs`） |
| portable tar | 文档写明宿主机需 GTK3 运行库（≥3.24） |
| 文档 | 构建环境文档 + Linux 构建依赖说明（`libgtk-3-dev`）、`GDK_BACKEND` 约束（§6.3） |

### 5.3 版本选择理由

- `0.17.2` 是最新稳定；其 portal 后端**已移除 ashpd/zbus/tokio/async-std**（改用 dlopen libdbus + pollster）→ 设计文档中「portal 需要执行器 feature（tokio/async-std）」的问题在 0.17 已不存在；`gtk3` 路线的 `-sys` crate 为纯 Rust 绑定，MSRV 1.70，兼容仓库 1.92。
- 不建议停留在 0.15/0.16：那两个版本默认 `xdg-portal + async-std`（隐式执行器），且 portal 依赖 ashpd（更大的依赖面、许可证与 lock 增量都更大）。

---

## 6. 降级策略（代码侧）

### 6.1 统一调用封装

```
UI 回调 → DialogRequest{kind, default_path} ——(线程) 一次性 worker——→ rfd::FileDialog → 结果
        ←—— slint::invoke_from_event_loop / mpsc + 现有 120ms UI Timer drain ——
```

- 必须**离开 UI 线程**：sync 直调会让 winit 事件循环冻结（实测 tick delta = 0；软件渲染下窗口不重绘）。
- 需要一个 `dialog_in_flight: bool` 守卫：rfd 的 GTK 线程是**单个全局线程**，并发/重复调用会在同一 GTK 主循环上嵌套对话框。

### 6.2 不可用判定与切换（两后端不同！）

- **portal（`dialog-portal` 变体）**：`pick_file()` 在无 portal 时 **<300 ms 返回 `None`**（实测 13.5–15.5 ms，且日志有 `OpenFile failed … ServiceUnknown`）。可用「首次调用若在 300 ms 内返回 `None`」→ 置会话级 `native_picker_unavailable = true`，之后改用内置路径输入。（可选：临时挂一个 `log` 捕获器识别 rfd 的 ERROR target，更精确。）
- **gtk3（默认变体）**：**不能**用返回值判定（用户取消也返回 `None`；无 display 时永久阻塞）。建议：
  1. 启动/首次调用前做 display 预检：应用自身窗口已渲染 ⇒ 该 display 可用（本项目的 Slint `backend-winit-x11` 天然满足）；
  2. 若确实要探测，检查 `DISPLAY`/`WAYLAND_DISPLAY` 与 GDK 将使用的后端一致（`GDK_BACKEND` 未设时 GDK 优先 Wayland，即使 `WAYLAND_DISPLAY` 未设也会回落默认 socket `wayland-0`——实测）；
  3. 兜底：把 pick 调用放在 worker 线程 + 同时提供内置路径输入入口（双轨，D4），即便对话框卡住，应用其余部分与路径输入仍可用（GTK 线程不可恢复，但主进程不冻结）。
- 两条路线都保留「内置路径输入弹窗」（现有组件模式），并在不可用时自动作为默认入口。

### 6.3 `GDK_BACKEND` 约束（重要）

- 不设置时：WSLg/Wayland 会话下 GTK 走 Wayland（**实测可正常弹出**，为独立 Wayland/RAIL 窗口）。
- 设置 `GDK_BACKEND=x11`：与 Slint X11 后端一致、可被 `xdotool` 观测与驱动（e2e 首选）；但**必须保证 X11 真的可用**，否则命中 §2.4 的静默阻塞。
- 建议：生产**不强制** `GDK_BACKEND`（让 GDK 跟随会话）；**e2e/CI 显式 `GDK_BACKEND=x11` + Xvfb**；若将来强制，只在「应用自身为 X11 后端且 `DISPLAY` 有效」时设置。

---

## 7. 风险与未确认点

1. **portal 正路径未验证**：本机无 portal 服务（未安装以免改动环境）。真实 GNOME/KDE 桌面、Flatpak 沙箱内的原生对话框、父窗口/窗口标识、Wayland 变体均未实测 → 若 D0b 只落 gtk3，风险为零；若同时落 `dialog-portal` 变体，建议在 D0b 验收里补一次真实桌面冒烟。
2. **gtk3 无 display 静默阻塞**（§2.4）：必须在文档与调用封装里防住（预检 + 不在 UI 线程直调）。
3. **不测 `set_parent`**：rfd 支持父窗口（raw-window-handle）；在「Slint X11 + GTK Wayland」混后端下行为未验证 → 建议 D0b 不使用 `set_parent`（本 spike 全程未用，弹窗与返回值均正常）；混后端的窗口定位/置顶体验需人工确认。
4. **cargo-deny 基线已 FAILED**（§4.2，与 rfd 无关）：D0b 落地前需 owner 确认 CI 侧 `cargo xtask deny` 的版本/配置，否则「基线红」会掩盖新增项判定。
5. **时间数据受同机负载影响**：本机同时有其它 agent 的 Xvfb/构建进程，耗时仅作相对参考。
6. **自动化伪影**：`save_file` 用例中 GTK 名称框多出一个 `.txt` 后缀（xdotool 键入与预填名交互所致），非 rfd 缺陷。
7. 未验证：HiDPI 缩放、真实 Wayland-only 会话、`pick_files`（多选）与 filter 行为；rfd 0.17 的 `wayland` feature 对本项目（Slint x11）影响未实测。

---

## 8. 设计文档问题 → 结论对照

| `yshell-next-rfd-spike.md` 问题 | 结论 |
| --- | --- |
| GTK3 构建前置 | `libgtk-3-dev`（+pkg-config）；缺则 `-sys` crate 构建期明确报错 |
| 对 winit/Slint 事件循环影响 | 内部有独立 GTK 线程；**UI 线程直调冻结**（实测 delta=0），worker 线程调用不冻结（delta=8） |
| WSLg 无 portal 时是否可用 | **可用**：GDK 默认走 Wayland 可弹窗；`GDK_BACKEND=x11` 时走 XWayland，pick/save/folder 全部返回正确路径；Xvfb 无头同样成功 |
| 打包体积/依赖面 | 运行期新增 GTK3 全家（65 个共享库），deb/rpm 需声明；无静态链接、无体积膨胀（二进制 debug 29.5 MB vs portal 28.5 MB，差异主要是 debug info） |
| portal 需要执行器 | 0.17 不需要（pollster 内部 block_on，已移除 tokio/async-std） |
| 与同步 UI 的桥接 | worker 线程 + `invoke_from_event_loop`（或 mpsc + 现有 Timer drain）；两种后端同构 |
| 无 portal 的错误路径 | 13.5–15.5 ms 内 `None` + ERROR/zenity 兜底失败（不悬挂） |
| fallback | 保留内置路径输入；portal 用「快速 None」判定，gtk3 用 display 预检（**不可用 `None` 判定**） |

---

## 9. 证据索引

工程根：`/root/rfd-spike/`（临时）；证据目录：`/root/rfd-spike/evidence/`

| 内容 | 路径 |
| --- | --- |
| 系统包安装记录（118 包 / 5 升级 / 版本） | `evidence/apt-gtk3-dev.txt` |
| gtk3 构建（tree + ldd + NEEDED） | `evidence/build-rfd-gtk3.txt` |
| portal 构建（tree + ldd + NEEDED） | `evidence/build-rfd-portal.txt` |
| 缺 `libgtk-3-dev` 的构建失败原文 | 脚本 `missing-gtk-dev.sh` 输出 |
| WSLg gtk3：pick/save/folder（Wayland 与 X11） | `evidence/e1b…e1h-*.log`、`evidence/e1d…e1f-*.log`、`/mnt/wslg/weston.log` |
| Xvfb gtk3：pick/save/folder + 对话框截图 | `evidence/e2a…e2c-xvfb-*.log`、`evidence/e2a-xvfb-pick.png` |
| portal：无 portal / 无 dbus / 线程调用 | `evidence/e3a…e3d-*.log` |
| Slint 集成：冻结 vs 存活 + 结果回填截图 | `evidence/s1-slint-gtk3-sync*.{log,png}`、`evidence/s2-slint-gtk3-thread*.{log,png}` |
| Slint + portal 降级截图 | `evidence/portal-s1-slint-gtk3-sync-final-app.png` |
| gtk3 无 display 静默阻塞 | `evidence/gtk3-nodisplay.log`（脚本 `gtk3-nodisplay.sh`） |
| lock 预览（+17 / +2） | `evidence/lock-diff.txt`、`lock-00-baseline.lock`、`lock-01-rfd-gtk3.lock`、`lock-02-rfd-portal.lock` |
| cargo-deny（许可证链与原文） | `evidence/deny.txt` |
| deb/rpm/CI 现状快照 | `evidence/packaging.txt` |
| 实验脚本（可复跑） | `exp1.sh exp1b.sh exp1d.sh exp1g.sh exp2.sh exp3.sh exp4.sh exp6.sh gtk3-nodisplay.sh lock-preview.sh deny-compare.sh both-features.sh` |
| spike 源码工程 | `rfd-gtk3/`、`rfd-portal/`、`slint-app/`（Slint 1.18.1 同仓库配置） |

复跑提示：WSLg 自动化用 `GDK_BACKEND=x11`；无头回归用 `Xvfb :199 -screen 0 1440x900x24` + `xdotool`（点击坐标 = Slint 窗口 `X+110, Y+260`，见 `slint-app/ui/app.slint` 的固定布局）。

---

## 10. D0b 落地记录

日期：**2026-09-27**；执行者：rfd spike owner（D0b 单 owner）。

### 10.1 改动

根 `Cargo.toml` 的 `[workspace.dependencies]` **仅新增 1 行**（无 features、无使用方、未改任何 crate）：

```toml
rfd = { version = "0.17.2", default-features = false }
```

- **未改 `deny.toml`**：`target-lexicon` 的 `allow`（`Apache-2.0 WITH LLVM-exception`）留到**首次真正引入 gtk3 依赖图**（即 N1 使用方透传 `rfd/gtk3`）时再加，避免现在加入无效 allow 项。
- 未改 `Cargo.lock`、未改任何 `crates/*`、未改 CI/打包。

### 10.2 验证结果

| 验证项 | 命令 | 结果 |
| --- | --- | --- |
| lock 一致性（改动前基线） | `cargo metadata --locked` | **exit=0**（0.4 s） |
| lock 一致性（改动后） | `cargo metadata --locked` | **exit=0**（0.6 s） |
| lock 无净变化 | `md5sum Cargo.lock` 前后对比 | **相同**：`4de3fe36c4f1ceb17d20f1c81057943a`（文件 mtime 保持 `2026-09-27 20:58:35`，D0b 未写 lock） |
| 未进 lock | `grep -c '^name = "rfd"' Cargo.lock` | **0**（与 D0 的 `time`/`swash` 同语义：未被引用的 workspace 依赖不进 lock） |
| 未进依赖图 | `cargo tree --locked --workspace \| grep -c rfd` | **0 匹配** |
| metadata 解析图 | `cargo metadata --locked` + 检索 | rfd **不在 resolve 图**（输出中仅有的 `rfd` 子串来自 `timerfd` / `linux-timerfd`，async-io 的 Linux target） |

证据目录：`/root/rfd-spike/d0b/`（`lock-before.md5`、`metadata-baseline.json`、`metadata-after.json`、`baseline.sh`、`verify.sh`、`details.sh`）。

复现：`cd /root/yshell && cargo metadata --locked && md5sum Cargo.lock && cargo tree --locked --workspace | grep -c rfd`。

### 10.3 后续消费者（N1/N4/N6）待办

1. 在使用方（`crates/yshell-app`）加 feature 透传：`default = ["dialog-gtk3"]`、`dialog-gtk3 = ["rfd/gtk3"]`、`dialog-portal = ["rfd/xdg-portal", "rfd/wayland"]`（两者互斥，rfd 会 build.rs panic）。
2. 调用封装：worker 线程调用 + `slint::invoke_from_event_loop`（或 mpsc + 现有 120ms UI Timer drain），加 `dialog_in_flight` 守卫；**禁止 UI 线程直调**（实测冻结）。
3. 系统依赖/打包：CI 构建机加 `libgtk-3-dev`；deb 加 `libgtk-3-0`、rpm 加 `gtk3`；portable 文档注明宿主机需 GTK3；首次引入 gtk3 时 `deny.toml` 加 `"Apache-2.0 WITH LLVM-exception"`。
4. 降级：保留内置路径输入；gtk3 **不用 `None` 判不可用**（用 display 预检），portal 用「<300 ms 返回 `None`」启发式。
