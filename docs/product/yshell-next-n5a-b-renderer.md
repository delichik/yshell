# N5a-B 终端字形：swash 光栅 + CJK 回退（设计）

状态：**已完成（验收通过 2026-09-27）**；owner `ses_f1e2e0941ffdGGtVQywUnI1xNF`
偏差追认：§4.1 "swash hinted outline + 整像素对齐 + zeno"（字面 `swash::Render` 受内置 Smooth-LCD hinting 限制、达不到验收线）；结果 ge90 25.9%→**55.7%**、`|` 单列、CJK 43.8%。
依赖：T0 ✅（Slint 1.18）、D0 ✅（`swash 0.2.10` 入库）、N5a-W0 ✅（接口/预算/资产准备）
范围：只改 `crates/yshell-terminal/**`、新增字体资产、新增 `scripts/fonts/make-cjk-subset.sh`
与 N0 的交叉：**零**（N0 改 app/ui，本任务只改 terminal crate；但必须保持 crate 随时可编译，N0 会跑全 workspace 测试）

## 1. swash 替换 fontdue 光栅

- 用 `swash 0.2.10`（`ScaleContext` + `.hint(true)` + `Format::Alpha`）替换 `render.rs::ensure_glyph` 的光栅调用；保持 `GlyphSource` / `FontFace` / `TerminalFontStack` / `TerminalFontSet` 对外接口与注入测试不变。
- 度量（advance / ascent / descent / line gap）改用 swash 结果；**cell 尺寸会因 hinting 轻微变化**，更新受影响的测试与默认值；app 侧网格在 N5a-UI 接线时统一验收。
- 若 fontdue 不再被使用则移除该依赖（报告确认）；保留"字体字节 + 按需构造字形"的数据结构以规避生命周期问题。
- **回归策略调整（有意）**：不再要求"与 fontdue 逐像素一致"；改为：
  1. 确定性（同输入 → 同输出像素）；
  2. 墨迹质量指标：16px 下"覆盖率 ≥90% 的墨点占比" ≥ 32%（W0 spike：fontdue 26.5% → swash 35.2%）；
  3. 竖线 `|` 单列为主（不再 0.64+0.36 拆两列）；
  4. 关键字形（`l/i/H/M/u`）覆盖网格快照打印进测试日志。
- 预期收益（W0 spike 实测）：12–13px 提升最大，16px 合计 26.5%→35.2%。

## 2. CJK 回退资产

- 用系统 `fonts-noto-cjk` + `python3-fonttools(pyftsubset)` 生成 **GB2312 全量子集**（含 ASCII / 全角 / 假名 / GB2312 一级+二级），实测约 **1.8 MB**；输出 `crates/yshell-terminal/assets/NotoSansMonoCJKsc-GB2312-Subset.otf`。
  - 备选档：仅一级 ≈995 KiB、最小符号档 ≈78 KiB；如用户更在意包体可后续一行命令切换（脚本参数化）。
- 新增 `scripts/fonts/make-cjk-subset.sh`：记录源包名/版本、`pyftsubset` 命令与参数，保证可复现；资产许可 OFL-1.1 在 assets 许可说明中注明。
- `TerminalFontSet::bundled_dejavu()` 的常规/粗/斜三条链都补 CJK 回退（缺字形时逐字形落到 CJK 子集）。
- **宽字符**：CJK 为双 cell；在 2-cell 盒内渲染（自然尺寸；左对齐或居中选一并在代码注释/报告记录）。补单测：`你` 有真实字形且非 `.notdef`；宽字符占位与 continuation 行为正确；去掉 CJK 资产时仍退化为 `.notdef` 而非空白。
- **CJK 粗体**：粗链找不到 CJK 粗字面时回退到 CJK regular 字形（可接受；记录）。斜体同理。

## 3. 明确不做

- `bootstrap.rs` 的 `scale_factor` 接入、`terminal_view.slint` 1:1 显示与鼠标坐标换算（**N5a-UI**，等 N0 释放 bootstrap）；
- DPI 截图验收（scale=1.25/2.0）与中文端到端可视验收（N5a-UI 阶段）；
- UI/主题/翻译改动。

## 4. 验收（交付给主 agent）

1. `cargo test -p yshell-terminal --all-features --locked` 全绿；`cargo clippy -p yshell-terminal --all-targets --all-features --locked -- -D warnings` 干净；crate 随时可编译（N0 并行跑全 workspace 测试的前提）。
2. 指标对比表（fontdue 基线 vs swash 现状：ge90% / mean / 竖线列数）来自真实测试或探针输出。
3. CJK：单测 + 一张含中文的渲染 PNG（放 `dist/`，复用现有 png 导出工具）。
4. 资产大小、许可、子集脚本可重复执行（贴命令与输出摘要）。
5. 偏离记录（度量变化、宽字符对齐取舍、粗体回退策略）。

## 5. 后续（N5a-UI，另阶段）

`bootstrap` 传窗口 `scale_factor`、`terminal_view` 物理像素 1:1 显示、网格与鼠标坐标换算、scale=1.25/2.0 截图验收（修 B5）。
