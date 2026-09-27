# YShell 字体清晰度调研报告（只读调查）

- 任务：用户反馈“感觉字体非常不清晰”的根因调研（终端文字 / UI 文字分开分析；覆盖 WSLg/X11 与 Windows 高 DPI 两种场景）。
- 调查人：只读调研 subagent（不为本任务修改任何产品代码/既有文档）。
- 调查时间：2026-09-27 14:20–15:00（+08:00）。
- 工作树版本确认：`crates/yshell-app/Cargo.toml` 与 `Cargo.lock` 仍为 **Slint `=1.9.2`**（T0 的 1.18.1 升级当时尚未落盘，但 registry 里已看到 `i-slint-common/compiler-1.18.1` 被拉取，说明 T0 正在执行）。本报告的 Slint 侧结论均以 1.9.2 为准；1.18.1 的差异单独用下载的 crate 源码核对（见 §6）。
- 使用的应用二进制：`target/debug/yshell`（md5 `3acfd1f86e416347d27a9b529d83ba95`，2026-09-27 13:29 构建）。未重新编译产品代码；独立探针使用 `CARGO_TARGET_DIR=/root/yshell-font-probe-target*`。

---

## 0. 结论摘要（TL;DR）

| 排名 | 根因假设 | 把握 | 主要症状面 | 一句话证据 |
|---|---|---|---|---|
| 1 | **WSLg 下 UI 字体命中 Windows 安装的 `NotoSansSC-VF.ttf`，fontdue 不支持 OpenType 变体 → 全部 UI 文字用默认实例 `wght=100`（Thin）渲染**，笔画呈发丝状、明显发灰 | **高** | UI 文字（中文/拉丁均受影响） | fontdb 返回该 face `weight=Weight(100)`；fvar `wght default=100`；fontdue 0.9.3 无变体支持；`YSHELL_UI_FONT` A/B 中“会话”墨迹 186px vs 320–344px |
| 2 | 终端自绘字形质量：fontdue 16px 无 hinting、仅灰度 AA、字形按整数像素落点 → 竖笔被分摊到两列（0.4+0.9 / 0.7+0.6），发灰发虚 | **高** | 终端正文 | 探针 `i/l/M` 覆盖网格；截图中 `u/i` 竖笔两列分摊；≥90% 覆盖的墨迹像素只占 24% |
| 3 | 终端无 CJK 字形且无字体回退：`DejaVuSansMono.ttf` 无中文 → 中文显示为 **灰色空心 .notdef 方框**（且取的是 1 个 cell 宽，宽字符右半空白）；粗体只是提亮 25% 颜色、斜体完全不生效 | **高** | 终端中文/粗体/斜体 | 探针 `lookup_glyph_index('你')==0`、`rasterize('你')` 与 .notdef 同形且 0 个全覆盖像素；`render.rs` 字形缓存只按 char 键、`cell.italic` 从未被读取 |
| 4 | 终端位图按 1x 逻辑像素光栅后被 Slint 在绘制时缩放（1.9.2 软件渲染器最近邻采样）：非整数缩放 → 12/13px 交替；2x → 像素块；**这是 B5 遗留的完整机制** | 高（机制） / **不适用于用户当前会话** | HiDPI（Windows 125%/150% 或 WSLg 配置缩放时）的终端 | 本次实测 S=1.25 字距 13/12 交替、S=2.0 20px + 像素块；而用户实际 WSLg 为 scale 1.0，故不是本次症状来源 |
| 5 | Slint 1.9.2 软件渲染器字形推进是整数物理像素（`PhysicalLength = Length<i16, PhysicalPx>`）→ UI 字距不匀；1.18.0 才修（#12356） | 中 | UI 文字间距 | 源码 + Slint changelog + issue #12356 |
| 6 | 字号偏小：caption 12px / body 14px @ 1x（2560×1440 27"），主观“看不清” | 中（主观） | UI/终端 | `theme.slint` 字阶 token；无 UI 缩放设置 |

> 修复归属一句话：**UI 文字的三条根因（Thin 变体、字距、光栅器）都随 T0 升级到 1.18.1 一起修掉/大幅改善；终端文字的三条（CJK 回退、粗斜体、按物理像素光栅）T0 修不了，需要独立任务（可由 N5 终端字体主题承接）。**
> 用户当前环境（Windows 显示缩放 100%、WSLg weston scale=1、X11 96dpi）**没有**任何环境级缩放模糊，B5 不属于本次症状，但它是高 DPI 场景下确定会出现的风险。

---

## 1. 显示环境事实（先排除“环境缩放模糊”）

命令与证据：

```bash
# WSLg / weston
grep -iE 'scale|dpi' /mnt/wslg/weston.log | tail
#   rdpMonitor[0]: desktopScaleFactor:100, deviceScaleFactor:100
#   rdpMonitor[0]: scale:1, clientScale:1.00

# 真实 X11 会话
DISPLAY=:0 xdpyinfo | grep -E 'dimensions|resolution'
#   dimensions: 2560x1440 pixels (677x381 millimeters)
#   resolution: 96x96 dots per inch
DISPLAY=:0 xrdb -query | grep -i dpi      # 无 Xft.dpi 资源
```

```powershell
# Windows 主机
Get-ItemProperty 'HKCU:\Control Panel\Desktop\WindowMetrics' -Name AppliedDPI
#   AppliedDPI = 96   → 100% 缩放（2560x1440）
```

winit 0.30.13 的 X11 缩放规则（`randr.rs`）：有 `Xft.dpi` 用 `Xft.dpi/96`；否则用 RandR 物理尺寸计算并 **量化到 1/12 步进**。对本机 :0（2560×1440/677×381mm）结果恰好 = **1.0**；对 Xvfb 1600×1000（406×254mm，现值 `:99`）= 13/12 ≈ **1.0833**；对 e2e 脚本启动的 1920×1200（488×305mm）= 12.49/12 → **1.0**。

已有截图的实测复核（像素测量，非推断）：

| 截图 | 终端单元格/字距实测 | 结论 |
|---|---|---|
| `dist/linux-x86_64/terminal-live-ssh.png`（1124×696） | 字距众数 10px（28/38 处）；游标块 10×19 | 1:1 显示，cell=10×19 |
| `dist/ui-checks/before-dark-zh-CN-11-tofu-connected.png`（1440×900） | 字距众数 10px；行距 19；终端位图框 780×692 @ (280,152) | 1:1 显示（e2e Xvfb 1920×1200） |
| 本次探针 `content-scale1.png`（1440×900，`WINIT_X11_SCALE_FACTOR=1.0`） | 字距众数 10px（38/47）；游标块 10 宽 | 与 e2e 完全一致（同机可复现） |

**结论：用户会话与验收环境都是 1x；不存在“缩放导致的模糊”。** 唯一提醒：e2e 若复用了当前正在跑的 `Xvfb :99`（1600×1000），winit 会得到 1.0833 缩放，截图将带轻微亚像素缩放——这不是用户问题，但会污染验收对比（见 §7 建议）。

---

## 2. 终端图像链路（TerminalRenderer → Slint Image → 屏幕）

### 2.1 代码链路（只读）

1. `crates/yshell-terminal/src/render.rs`
   - 内置 `DejaVuSansMono.ttf`，`DEFAULT_FONT_SIZE = 16.0`；
   - `cell_width = round(advance)` = `round(9.633)` = **10px**；`cell_height = ceil(ascent-descent+gap)` = **19px**；baseline = `ascent.round()` = 15；
   - `font.rasterize(char, 16.0)`（fontdue 0.9.3，灰度 AA，**无 hinting、无子像素**）；
   - 字形缓存 `glyph_cache: HashMap<char, CachedGlyph>` **只按字符键**，与 bold/italic 无关；
   - 粗体：`cell_colors()` 里 `foreground = lighten(fg, 0.25)`（颜色提亮，不改字形）；
   - 输出 `TerminalFrame { width = columns*10, height = rows*19, rgba }`。
2. `crates/yshell-app/src/bootstrap.rs`
   - `slint::Image::from_rgba8(...)` → `window.set_terminal_image(image)`；
   - 定时器（120ms）用**逻辑像素**算网格：`columns = floor(viewport_width / 10)`、`rows = floor(viewport_height / 19)`；
   - `viewport_width_px = terminal_surface.width/1px - 24`（`ui/components/terminal_view.slint`，单位是 Slint **逻辑像素**）。
3. `ui/components/terminal_view.slint`
   - `Image { source: root.terminal-image; image-fit: fill; width: parent.width; height: parent.height; }`
   - 即：位图**永远被拉伸铺满视口**，拉伸系数 = 视口逻辑宽 / (columns×10) ≥ 1（1x 时通常在 1.000–1.011，因为 columns 向下取整）。
4. **全仓库 `scale-factor` / `scale_factor` 零命中**：应用从不读取窗口设备缩放，终端光栅尺寸固定 16px/10×19 逻辑像素。

### 2.2 实测：不同窗口缩放下终端字距（本机 Xvfb + `WINIT_X11_SCALE_FACTOR`）

对同一逻辑布局 1440×900、只改设备缩放（物理窗口 = 1440/1800/2880 宽）：

| 设备缩放 | 实测字距众数（rising-edge delta） | 行距 | 游标整格实测 | 现象 |
|---|---|---|---|---|
| 1.0 | **10px**（38/47） | 19 | 10×20* | 与位图 1:1，锐利 |
| 1.25 | **13px（22处）/ 12px（18处）交替**，另有 14/11 | 24 | 12×24 | 最近邻重采样：列被复制/丢弃，字距忽宽忽窄 |
| 2.0 | **20px**（29处） | 38/39 | 20×39 | 位图被像素级放大（不是按 2x 重新光栅），笔画成块 |

\* 游标整格检测会把相邻行同 x 区间的一列全前景像素并进来，故有时 +1 行；宽度 10px 是精确的。

结论：
- **1x 时**，终端显示是 1:1 的，模糊完全来自 §3 的字形质量；
- **任何非 1.0 的缩放**（Windows 125%/150%、或 WSLg 设置缩放时），终端字会以最近邻方式被拉伸——非整数缩放出现 12/13px 交替的“歪/毛”，整数倍缩放是“方块感”。这正是遗留项 B5 的完整机制。当前用户环境 scale=1.0，因此 **B5 不是本次“非常不清晰”的直接原因**，但必须在 HiDPI 任务里修（见 §7）。

---

## 3. 终端自绘字形清晰度（fontdue 16px，实测）

用独立探针（`fontdue 0.9.3` + 应用同一个 `DejaVuSansMono.ttf`）打印覆盖网格（0–9 = 覆盖率 0–0.9+，`.` = 0）：

```
'l' 16px（8x13）：竖笔两列         'i' 16px（8x13）：竖笔两列
 1222....                           ...11...
 6899....                           ...76...
 ..49....   ← 0.4 + 0.9            ...54...
 ...（竖笔 `49`/`39`）              ..7775... / ..3386...  ← 0.7 + 0.6
```

同一批字符在**应用实时截图**（`content-scale1.png`，1:1）也复现：

```
cell 'u': .37...171. / .49...292.    ← 左右竖笔分别 0.3+0.7、0.2+0.9+0.2
cell 'i': ....76... / ....54...      ← 竖笔 0.7+0.6（而不是 1 列全黑）
```

整块终端区域统计：墨迹像素 17146，平均覆盖率 **0.520**，覆盖率 ≥0.9 的仅 **24.3%**，0.3–0.9 的占 37.6%。

含义：16px 等宽字形本应 1px 宽的全黑竖笔，被 fontdue 以“无 hinting + 亚像素位置”拆到相邻两列（≈0.4+0.9 或 0.7+0.6）。屏幕上的直观效果就是**笔画发灰、发毛、对比度下降**；这不是缩放造成的，1:1 也如此。

补充（终端主题/粗斜体事实，均来自代码/探针）：

- **粗体不是真粗体**：字形缓存只按 char 键，bold 与 regular 用**完全相同的位图**；只是前景色 `#D8DEE9 → #E2E6EF`（约 +4% 亮度，肉眼几乎无感）。
- **斜体不生效**：`TerminalCell.italic` 由 parser（SGR 3）写入，但 `render.rs` 从未读取它。
- **CJK = 灰色 `.notdef` 框**：探针 `lookup_glyph_index('你') == 0`，`rasterize('你')` 与 `.notdef` 同形（9×15 空心矩形），且 **全覆盖像素为 0**（整框是灰的）；宽字符只占 2 个 cell 中的第 1 个，右半是空的。

---

## 4. UI 文字（本报告最关键的发现）

### 4.1 字体来源

`bootstrap.rs::platform_default_ui_font()`：Windows → `Microsoft YaHei UI`；macOS → `PingFang SC`；其它（WSLg/Linux）→ **`Noto Sans SC`**。

WSLg 的 fontconfig 通过 `/etc/fonts/local.conf` 把 **`/mnt/c/Windows/Fonts`** 挂进字体库（`ls /etc/fonts/local.conf` 确认）。实测：

```bash
fc-match --format '%{file}|%{index}|%{family}|%{weight}|%{style}\n' 'Noto Sans SC'
# /mnt/c/Windows/Fonts/NotoSansSC-VF.ttf|262144|Noto Sans SC|80|Regular
md5sum /mnt/c/Windows/Fonts/NotoSansSC-VF.ttf
# 504abdda545478632820c606a577b4a3
```

用与 Slint 1.9.2 完全相同的 `fontdb 0.22.0` 查询（独立探针）：

```
query "Noto Sans SC" weight=400/500/600/700
  -> face families=["Noto Sans SC"] weight=Weight(100) style=Normal
     file=/mnt/c/Windows/Fonts/NotoSansSC-VF.ttf
```

同一文件用 ttf-parser / fontdue 检查：

```
ttf-parser: units_per_em=1000 weight()=100 is_variable=true
   axis tag=wght min=100 default=100 max=900      ← 默认实例是 Thin(100)！
fontdue 0.9.3: FontSettings 只有 collection_index/scale/load_substitutions，
               源码中 grep -i variation 无任何命中 → 不应用 wght 变体
fontdue rasterize('清', 14px): 14x13, 81 ink px, full_cover_px=0
```

即链路是：`Theme.font-ui = "Noto Sans SC"` → fontdb 命中唯一 face（OS/2 权重 100）→ Slint 1.9.2 软件渲染器用 fontdue 光栅 → **默认变体 Thin**。UI 的 `font-weight: 400`（默认）与 `600`（semibold token）在这个 face 上都拿到**同一个默认实例**，所以粗细也没有区分。

### 4.2 端到端 A/B（同一二进制、只改 `YSHELL_UI_FONT`，1x 截图）

测量区域：“会话”标题区（(10,96)-(210,124)），深色主题、浅色文字：

| `YSHELL_UI_FONT` | 墨迹像素数 | 平均灰度 | 观感 |
|---|---|---|---|
| 未设置（= Noto Sans SC，Thin） | **186** | **119.9** | 发丝、发灰（现存 e2e 截图一致：同样 186/119.9） |
| `DejaVu Sans`（无 CJK） | **9** | 190 | 中文**几乎完全消失** → 证明 1.9.2 软件渲染器**没有逐字形回退** |
| `Microsoft YaHei UI` | **344** | 199.3 | 笔画实、可读（Windows 静态字体，weight 400/600 有真实两级） |
| `Noto Sans CJK SC`（Debian 静态 TTC） | **320** | 190.3 | 同上，实心可读 |

放大截图（临时证据）直观可见：默认字体按钮上的“新建会话”是浅白发丝字；换成 YaHei / Noto Sans CJK SC 后变成实心黑字；换 DejaVu 后中文全部消失。现有 `dist/ui-checks/before-*` 截图与本次默认渲染的像素统计完全相同——**说明历史验收截图里 UI 一直是 Thin 渲染，只是当时没有字体清晰度验收项**。

### 4.3 其它 UI 相关事实

- Windows 场景（按代码+本机字体库推理）：`Microsoft YaHei UI` 在 fontdb 中解析为 `msyh.ttc`（Regular）+ `msyhbd.ttc`（Bold），400/500→Regular，600/700→Bold；**Windows 原生不会被 Thin 变体坑到**，但仍使用 fontdue（无 hinting、灰度 AA）。
- Slint 1.9.2 软件渲染器字形推进是整数物理像素（`software_renderer.rs: type PhysicalLength = euclid::Length<i16, PhysicalPx>`；`shape_text` 把 advance `.cast()` 成 i16），所以 12–14px 比例字体字距会有 1px 级抖动——与 issue #12356“字距忽宽忽窄”同类，1.18.0 才修（见 §6）。
- `Theme.font-mono = "monospace"` 仅是 token（终端不走它）；没有 UI 缩放/字号设置项。

---

## 5. 症状归因（按用户可能看到的对象拆分）

**如果用户看的是 UI 文字**（会话列表、状态栏、菜单、对话框）：
- 主因 = §4 的 Thin 变体渲染（发丝、发灰、对比度低）；
- 次因 = 12/14px 小字号 + 整数推进导致字距不匀 + 灰度 AA 无 subpixel。

**如果用户看的是终端文字**：
- 主因 = §3 的 fontdue 无 hinting 灰度光栅（竖笔被拆成两列灰线）；
- 若终端里有中文 → §3 的 `.notdef` 灰框（“看不清”甚至“看不懂”）；
- 若用了粗体 → 只提亮 4% 亮度、笔画不加粗；斜体缺省直立体；
- 当前环境（scale 1.0）不涉及 §2.2 的位图缩放。

**排除项**：
- Windows/WSLg 环境缩放（100%、weston scale 1、Xft.dpi 未设、winit 量化后 = 1.0）；
- 终端位图 1x 拉伸（实测字距精确 10px、行距 19px；位图 Q 与截图网格相位对齐）；
- “缺少 Noto Sans SC 字体”（字体存在，只是它是默认 Thin 的可变字体）。

---

## 6. 与 T0（Slint 1.9.2 → 1.18.1）的关系（用 1.18.1 源码核对）

下载核对（临时目录，未改仓库）：`i-slint-renderer-software-1.18.1`、`i-slint-core-1.18.1`、`fontique-0.11.1`、`parlance-0.1.0`（经 rsproxy）。

1. **1.16.0** changelog：`FemtoVG & Software Renderer: Use swash for glyph rasterization for better text rendering.` → 1.18.1 软件渲染器用 **swash**（不再是 fontdue），字形光栅与灰度质量应改善。
2. **1.18.0** changelog：`Software renderer: Fixed uneven gaps between glyphs. (#12356)`；1.18.1 源码里有明确注释与实现：`SUBPIXEL_BIN_COUNT = 4`（“keeps inter-glyph spacing even … remove the visible unevenness at UI text sizes”）、字形缓存键包含 `subpixel_bin` 与 `coords_hash`（变体坐标哈希）。
3. **变体权重会被应用**：1.18.1 `graphics.rs::FontRequest::query_fontique` 把 `weight` 传给 fontique；fontique 从 OS/2 读 weight=100，`Synthesis` 构造规则为“若 `self.weight != requested` 且字体有 `wght` 轴 → 产出 `("wght", requested)`”；`parlance::FontWeight::default() = NORMAL = 400`，另有 600（semibold）。
   → 结论（源码级推断，需升级后实测）：**T0 之后 UI 文字会按 `wght=400`（正文）/`600`（semibold）实例渲染，Thin 问题、字距问题、fontdue 光栅问题三条一起解决/大幅缓解。**
4. **终端不受 T0 影响**：终端位图由应用自绘（fontdue 仍在 `crates/yshell-terminal`）；1.18.1 软件渲染器的图片采样逻辑与 1.9.2 同类（`fetch_blend_pixel` 对 RGBA 仍忽略小数采样偏移，即最近邻），因此 §2.2 的 B5 行为在 T0 后**不变**；T0 文档 H7 的判断（“终端位图来自自绘 TerminalRenderer，应不受影响”）经本次源码核对成立。
5. 升级后需要复核的**新风险**：1.18 用 fontique 选择系统字体，解析结果可能与 fontdb 不同（是否仍命中同一个 NotoSansSC-VF、是否命中别的 face）；1.17 起默认字号会读系统设置（T0 H6，窗口已显式设置字体族/字号，理论上被覆盖，仍需截图核对行高）。

---

## 7. 建议修复方向与归属

### 7.1 直接缓解（无需代码，给用户/验收）
- 在 WSLg 下临时设置 `YSHELL_UI_FONT="Microsoft YaHei UI"` 或 `"Noto Sans CJK SC"`（本机都实测有效，中文立刻变实心）；前提是系统有该字体（Debian 需 `fonts-noto-cjk`）。
- 终端方面无环境开关可救：CJK 会一直是方框，直到引入回退字体/换字体。

### 7.2 归 T0（升级任务内解决/验证，**不要另开任务抢文件**）
- UI 三条根因随 1.18.1 一起修（swash + `wght` 变体 + 4 档亚像素推进）。
- 建议在 T0 验收清单补一项“**UI 文字字形/字重回归**”：
  1. 升级前后同机同主题截图对比“会话/新建会话/SFTP”等标签的墨迹（升级前参考值：会话区 ink_px=186、mean_gray≈120；按钮区 ink_px≈9504）；
  2. 确认 UI 文字不再是发丝细字（可用临时 `YSHELL_UI_FONT` 对照，或放大截图看笔画宽度）；
  3. 确认 `font-weight:600`（如“SFTP/终端标题”）与正文出现可见字重差。
- 工作量：≈0.5h（截图对比），风险低；若升级后仍发细，再排查 fontique 命中 face（可能命中 Debian 的别的 Noto，或仍取默认实例）。

### 7.3 新任务（终端字体与 DPI；可并入 N5 终端字体主题）
建议拆 3 个小项，每项都可独立验收：
1. **终端 DPI 感知（B5 修复）**：从 `slint::Window::scale_factor()` 取缩放，按物理像素光栅（`font_size*scale`、cell×scale），并让列/行数与视口逻辑尺寸对齐；同时处理选择/鼠标命中的坐标换算。
   - 验收：`WINIT_X11_SCALE_FACTOR=1.25/2` 截图；修后应为 12.5/25px 物理级清晰字形，而不是 12/13 交替或像素块（现状证据已备）。
   - 工作量 1–2 天（含单测：cell 尺寸/帧尺寸）。
2. **CJK 回退字体**：用 fontconfig/系统字体发现一个 CJK 等宽或可接受的无衬线（如 `Noto Sans CJK SC` / `Noto Sans Mono CJK SC`），优先 monospace、失败再 sans；无字体时明确绘制“空框 + 日志提示”，并把它纳入终端主题配置。
   - 验收：终端输出“你好，世界”显示为汉字（现在一定是灰框）。
   - 工作量 1–2 天；风险：宽字符 2-cell 对齐与选中/复制语义。
3. **真粗体/斜体**：加载 `DejaVuSansMono-Bold.ttf`（现有资源可补），斜体可先用 `fontdue` 的倾斜合成或明确不做但文档标注。
   - 验收：`printf '\033[1mB\033[0m'` 的笔画宽度明显大于普通体（当前只亮 4%）。
   - 工作量 0.5–1 天。
4. （可选，建议先只调研）**光栅质量追平系统终端**：fontdue 无 hinting；若要锐利，需要带 hinting 的 FreeType 栈或预渲染位图字体。依赖体积/许可/跨平台风险大，建议在 N5 里单独立项评估，不阻塞 1–3。

### 7.4 环境/工作流（主 agent/验收）
- e2e 脚本固定窗口缩放：在 `scripts/e2e/lib.sh` 的 `e2e_launch_app` 里加 `WINIT_X11_SCALE_FACTOR=1`（或让 Xvfb 用 96dpi），避免复用 1600×1000 的 `:99` 时悄悄以 1.0833 渲染，污染“字体/布局像素级对比”。
- 注意：本报告建议**只覆盖**“调查与拆分”，不动产品代码；T0 与终端任务的落地由主 agent 定夺。

---

## 8. 未确认点 / 需要用户提供的最小信息

1. 用户说的“不清晰”是**终端**还是**UI**（或两者），最好附一张 2–3 倍放大的截图并圈出区域。
2. 运行平台与显示设置：Windows 原生还是 WSLg；Windows“缩放”百分比；窗口是否最大化/当前分辨率。
3. 是否设置过 `YSHELL_UI_FONT`；终端里是否出现过中文（如 `ls` 中文文件名、程序中文输出）。
4. Windows 原生构建上 UI/终端的实际观感（本次无法在 Windows 图形栈上实测，只能按 fontdb/字体表 + 代码推理；例如本机 WSL 里 `Microsoft YaHei UI` 有 Regular/Bold 两级，Windows 上应一致）。
5. 升级 1.18.1 后 UI 是否确实变为 `wght=400`（源码推断高把握，但需实测确认）。

---

## 9. 证据清单与复现命令（临时目录，均为只读调查产物）

调查脚本与截图（会话结束后可能被清理）：
- `/tmp/yshell-font-probe/`（WSL）：`content-scale1.png`、`content-scale125.png`、`content-scale2.png`、`zoom2-*.png`、`uifont-{default,dejavu,yahei,cjksc}.png`、`uifont-compare.png`、`crop-ui-*.png`
- Windows 侧同步副本：`C:\Users\iiii_\AppData\Local\Temp\opencode\font-probe\`
- 复现脚本：`run_scale_probe.sh`（不同缩放截图）、`analyze2.py`（字距/行距/游标整格测量）、`run_font_variant.sh` + `compare_uifonts.py`（UI 字体 A/B）、`rustprobe/`、`rustprobe2/`（fontdb + fontdue + ttf-parser 探针）

关键命令摘要：

```bash
# 1) 真实环境缩放
DISPLAY=:0 xdpyinfo | grep -E 'dimensions|resolution'
grep -iE 'scale' /mnt/wslg/weston.log | tail -3

# 2) UI 字体解析
fc-match --format '%{file}|%{index}|%{family}|%{weight}|%{style}\n' 'Noto Sans SC'

# 3) 变体/权重与 fontdue 能力（独立探针，CARGO_TARGET_DIR 隔离）
bash /mnt/c/Users/iiii_/AppData/Local/Temp/opencode/font-probe/run_rustprobe2.sh
#   → weight()=100, fvar wght default=100, fontdue rasterize('清') 无全覆盖像素

# 4) 终端 1x/1.25/2x 对比
/tmp/yshell-font-probe/run_scale_probe.sh 1.0   ...
/tmp/yshell-font-probe/run_scale_probe.sh 1.25  ...
/tmp/yshell-font-probe/run_scale_probe.sh 2.0   ...
python3 analyze2.py content-scale125.png scale125

# 5) UI 字体 A/B
/tmp/yshell-font-probe/run_font_variant.sh default ''
/tmp/yshell-font-probe/run_font_variant.sh dejavu  'DejaVu Sans'
/tmp/yshell-font-probe/run_font_variant.sh yahei   'Microsoft YaHei UI'
python3 compare_uifonts.py
```

本次调查未修改任何产品代码或既有设计文档；未运行 `cargo fmt`。
