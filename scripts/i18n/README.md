# i18n：文案提取与翻译工作流

`ui/**/*.slint` 里所有用户可见文案统一写 `@tr(...)`；`extract.sh` 把它们提取为
gettext 模板 `translations/yshell-app.pot`，再合并进各语言的
`translations/<lang>/LC_MESSAGES/yshell-app.po`，由 `cargo build` 内嵌进二进制。

相关位置：

| 用途 | 位置 |
| --- | --- |
| 提取脚本 | `scripts/i18n/extract.sh` |
| 翻译目录 | `translations/en-US/LC_MESSAGES/yshell-app.po`、`translations/zh-CN/LC_MESSAGES/yshell-app.po` |
| 构建期内嵌 | `crates/yshell-app/build.rs` 的 `.with_bundled_translations("../../translations")` |
| 运行时切换 | `crates/yshell-app/src/bootstrap.rs::apply_language()`（`YSHELL_LANG` 环境变量） |

## 0. 一次性准备

```bash
cargo install slint-tr-extractor
```

脚本找不到该命令时会给出同样的提示并以退出码 1 结束。Windows 下请在 WSL（或 Git Bash）
里安装并运行；`extract.sh` 是 bash 脚本。

## 1. 在 `.slint` 里标注文案

```slint
text: @tr("Connect");                     // 无上下文：context 默认取所在组件名
text: @tr("Session" => "Open");           // 带上下文：区分 "Open" 的不同含义
text: @tr("Host: {0}", host);             // 占位符（翻译可调整语序）
text: @tr("{} item" | "{} items" % n);    // 复数
```

### ⚠️ `=>` 的参数顺序（最容易写反的地方）

Slint 的实际语义是 **上下文在前、待翻译文案在后**：

> `@tr("Session" => "Open")` 表示 `msgctxt = "Session"`、`msgid = "Open"`
> （界面回退显示 `Open`）；**不是** `@tr("英文" => "上下文")`。

这不是推测，已用本仓库实际工具链核对过：

- `slint-tr-extractor 1.18.1` 对 `@tr("File" => "MenuBar")` 提取出
  `msgctxt "File"` + `msgid "MenuBar"`；
- Slint 1.9.2 编译器源码里，`TrContext`（`=>` 前的字符串）只作为 context 传给
  `translate(original, contextid, ...)`，界面回退显示 `original`（`=>` 后的字符串）。

> **仓库现状提醒（需要单独修复，不属于本脚本职责）**：当前 `ui/**.slint`、
> 两个 `.po` 文件以及 `docs/product/ui-winui3-design-language.md` §7.2/§7.3 都按
> `@tr("英文" => "上下文")` 的相反顺序编写（如 `@tr("File" => "MenuBar")`、
> `@tr("Open" => "Session")`），与工具实际语义不符，提取结果会和现有 `.po` 对不上。
> 本文档以工具实际行为为准；提取脚本不做任何"交换"补偿。修正源码与 `.po` 请另开任务。

## 2. 提取/更新 `.pot`

```bash
bash scripts/i18n/extract.sh
```

脚本行为（可重复运行）：

1. `cd` 到仓库根，用 bash globstar 展开 `ui/**/*.slint`（连同入口
   `ui/main_window.slint`）传给 `slint-tr-extractor`，输出 `translations/yshell-app.pot`。
2. **显式传入所有 `.slint` 文件**：实测 extractor 不会跟进 `import`，只传
   `main_window.slint` 会漏掉其它文件里的 `@tr`。新增 `.slint` 文件不用改脚本，glob 会自动带上。
3. 不加 `--no-location`，保留 `#: ui/xxx.slint:12` 定位注释（改动的行号一眼可见）。
4. 每次全量重新生成，不累积已删除的旧条目；若除头部 `POT-Creation-Date` 外内容不变，
   则保留现有文件，因此连续运行是幂等的（不产生 git diff，方便 CI 校验）。
5. 结束时打印扫描的 `.slint` 文件数与 `grep -c '^msgid ' translations/yshell-app.pot`
   的条目数（该计数含头部空的 `msgid ""`，可翻译条目数 = 该值 - 1）。

输出示例：

```text
内容无变化，保留现有 translations/yshell-app.pot
已扫描 22 个 .slint 文件，提取 74 条 msgid -> translations/yshell-app.pot
下一步：把新条目合并进 translations/<lang>/LC_MESSAGES/yshell-app.po（见 scripts/i18n/README.md）。
```

## 3. 合并进 `.po`

`.pot` 只是模板，不要手改；以它的 `msgctxt`（如有）+ `msgid` 为键，把新条目补进两个语言：

| 语言 | 文件 | `msgstr` 规则 |
| --- | --- | --- |
| en-US | `translations/en-US/LC_MESSAGES/yshell-app.po` | 与 `msgid` 相同（源语言基线） |
| zh-CN | `translations/zh-CN/LC_MESSAGES/yshell-app.po` | 中文译文 |

> **`msgctxt` 必须照抄 `.pot`，一条都不能省。** Slint 1.9.2 编译时会给没有显式上下文的
> `@tr("X")` 自动填**所在组件名**作为 context（如 `MainWindow`）；`.po` 里少写 `msgctxt`
> 的条目在编译期按 `(context, msgid)` 查找不到，译文不会进二进制，运行时静默回退英文。
> 实测踩坑：`msgid "New Session"` 缺 `msgctxt "MainWindow"` → 生成的翻译表里该条目为 `None`。

- 改动 `msgid` 视为破坏性变更；旧翻译建议用 `msgmerge` 迁移（或手工）。
- 已从源码删除的 msgid，也要从 `.po` 里删掉。
- 复数条目保留 `msgid_plural` 与对应数量的 `msgstr[n]`。
- `.pot` 里尚未翻译的条目可以先不加进 `.po`（运行时回退 msgid）；但一旦加了，`msgctxt` 必须一致。

### `.po` 头部必须完整（否则 `cargo build` panic）

`slint-build` 用 **polib** 解析 `.po`；头部缺字段会直接 panic（已踩过坑）。
每个 `.po` 的头部必须包含完整 gettext 字段（顺序不限，日期格式随意，示例）：

```po
msgid ""
msgstr ""
"Project-Id-Version: yshell\n"
"POT-Creation-Date: 2026-09-27 00:00+0000\n"
"PO-Revision-Date: 2026-09-27 00:00+0000\n"
"Last-Translator: YShell\n"
"Language-Team: YShell\n"
"Language: zh-CN\n"
"MIME-Version: 1.0\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Content-Transfer-Encoding: 8bit\n"
"Plural-Forms: nplurals=1; plural=0;\n"
```

必需的九个字段：`Project-Id-Version` / `POT-Creation-Date` / `PO-Revision-Date` /
`Language-Team` / `Language` / `MIME-Version` / `Content-Type` /
`Content-Transfer-Encoding` / `Plural-Forms`（`Last-Translator` 也一并保留）。
`Language` 写 `zh-CN` 或 `en-US`；复数规则 zh-CN 用 `nplurals=1; plural=0;`，
en-US 用 `nplurals=2; plural=(n != 1);`。

## 4. 构建期内嵌

`crates/yshell-app/build.rs`：

```rust
let config = slint_build::CompilerConfiguration::new()
    .with_style("fluent".into())
    .with_bundled_translations("../../translations");
slint_build::compile_with_config("../../ui/main_window.slint", config)
```

`cargo build` 时，`translations/<lang>/LC_MESSAGES/yshell-app.po` 会被打包进二进制
（domain 名取 build 脚本所在 crate 名 `yshell-app`，与文件名对应），运行期不需要 gettext。

`build.rs` 里额外声明了 `cargo:rerun-if-changed=../../translations`：slint-build 只会为
`.slint` 文件发 rerun-if-changed，不会跟踪它内嵌的 `.po`；没有这一行时改翻译不会重新内嵌，
二进制会一直用旧的翻译表。

## 5. 运行时切换

```rust
slint::select_bundled_translation("zh-CN")?; // 或 "en-US"
```

- 应用侧入口：`YSHELL_LANG=system|zh-CN|en-US` 环境变量
  （`bootstrap.rs::apply_language()`）；未设置时读 `LC_ALL`/`LANG`，判断不了则回退 `en-US`。
- 后续设置页语言下拉会接同一个函数，切换后立即生效，无需重启。
- 查不到的 msgid 回退显示 msgid 本身（英文源文），不会显示 key。

## 常见问题

| 现象 | 处理 |
| --- | --- |
| `slint-tr-extractor: command not found` | `cargo install slint-tr-extractor`；确认 `~/.cargo/bin` 在 PATH 中 |
| 新文案没被提取 | 确认写的是 `@tr("...")` 而非普通字符串；extractor 不解析注释，且**不会跟进 import**（脚本已全量 glob） |
| `cargo build` 时 polib panic | `.po` 头部缺字段，按第 3 节补全 |
| 界面仍是英文 | 检查 `YSHELL_LANG`；确认 `.po` 里的 `msgid`/`msgctxt` 与 `.pot` 完全一致 |
| 切换语言后布局截断 | 按设计规范 §7.4 保持文本容器弹性，不写死宽度 |
