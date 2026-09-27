//! UI 颜色纪律守护测试。
//!
//! 规范依据 `docs/product/ui-winui3-design-language.md`：
//! - §3：所有界面颜色/尺寸/字号/动效必须引用语义 token，禁止写死字面量；
//! - §11 验收第 1 条：`grep '#[0-9a-fA-F]{6}' ui/` 只允许命中 `ui/theme.slint`。
//!
//! 本文件只用 std 实现规则，不引入任何新依赖。三个测试分别守护：
//! 1. `no_hardcoded_colors_outside_theme`：`ui/theme.slint` 之外不得出现
//!    `#RRGGBB` / `#RRGGBBAA` 颜色字面量，颜色必须走 `Theme.*` token；
//! 2. `theme_tokens_are_defined_before_use`：其它 ui 文件引用的 `Theme.<token>`
//!    必须在 `ui/theme.slint` 中声明过，防止改名/手滑造成引用静默失效；
//! 3. `theme_file_exists`：主题入口文件存在且导出 `export global Theme`。
//!
//! 注意：扫描有意保持"逐行文本扫描"的简单规则（不解析字符串/注释），
//! 注释里写死的颜色同样会被判失败——这是规范要求的保守策略。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- 路径与 IO

/// 定位仓库根目录：`CARGO_MANIFEST_DIR` 指向 `crates/yshell-app`，上溯两级即仓库根。
///
/// 用 `parent()` 串联而非拼接 `../../ui`，是为了让失败信息里的路径干净可读。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("定位仓库根目录失败：CARGO_MANIFEST_DIR 不在 crates/yshell-app 之下")
        .to_path_buf()
}

/// `ui/` 目录：设计 token 与所有 Slint 界面的所在地。
fn ui_dir() -> PathBuf {
    workspace_root().join("ui")
}

/// 打开目录；失败时带上路径 panic，替代无上下文的裸 `unwrap()`。
fn read_dir_expected(dir: &Path) -> fs::ReadDir {
    fs::read_dir(dir).unwrap_or_else(|err| panic!("读取目录失败: {} ({err})", dir.display()))
}

/// 读取 UTF-8 文本；失败时带上路径 panic，替代无上下文的裸 `unwrap()`。
fn read_to_string_expected(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("读取文件失败: {} ({err})", path.display()))
}

// ---------------------------------------------------------------- 文件收集

/// 递归收集 `root` 下所有 `*.slint` 文件。
///
/// 返回 `(实际路径, 相对 root 的展示路径)`。目录项先按文件名排序再遍历，
/// 保证失败信息顺序稳定、可复现（不依赖文件系统枚举顺序）。
fn collect_slint_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    visit_dir(root, root, &mut files);
    files
}

fn visit_dir(root: &Path, dir: &Path, files: &mut Vec<(PathBuf, String)>) {
    let mut entries: Vec<fs::DirEntry> = read_dir_expected(dir)
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|err| panic!("读取目录项失败: {} ({err})", dir.display()));
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            visit_dir(root, &path, files);
        } else if is_slint_file(&path) {
            // 展示路径统一用 `/`：Windows 的 `\` 在失败信息里容易看花
            let display = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            files.push((path, display));
        }
    }
}

/// 扩展名是否为 `.slint`。
fn is_slint_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "slint")
}

// ------------------------------------------------------------ 颜色字面量扫描

/// 判断一行中是否出现颜色字面量：`#` 之后恰好 6 或 8 个十六进制字符。
///
/// 判定方式：统计 `#` 之后最长的连续十六进制串长度，等于 6（`#RRGGBB`）或
/// 8（`#RRGGBBAA`）才算命中。于是：
/// - `#005FB8`、`#FFFFFF0F` 命中；
/// - `#12345`（5 位）、`#1234567`（7 位）、`#123456789`（9 位）不命中，
///   因为它们第 7/9 个字符仍然是十六进制字符；
/// - `#` 出现在普通文本且后面不是 6/8 位十六进制时不命中。
///
/// 有意不做字符串/注释解析：注释里的颜色字面量同样按失败处理，规则简单且保守。
fn has_color_literal(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'#' {
            index += 1;
            continue;
        }
        let hex_start = index + 1;
        let mut hex_end = hex_start;
        while hex_end < bytes.len() && bytes[hex_end].is_ascii_hexdigit() {
            hex_end += 1;
        }
        let hex_len = hex_end - hex_start;
        if hex_len == 6 || hex_len == 8 {
            return true;
        }
        index = hex_end; // hex_end >= hex_start > index，必定前进，不会死循环
    }
    false
}

// ------------------------------------------------------------ Slint 名字工具

/// Slint 标识符里允许出现的字符（名字允许 kebab-case，如 `text-primary`）。
fn is_slint_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

/// 名字是否以合法标识符字符开头（字母或下划线）。
fn starts_like_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
}

// -------------------------------------------------------- 主题 token 定义解析

/// 收集 `ui/theme.slint` 中 `in property` / `out property` / `in-out property`
/// 声明的 token 名。私有 `property` 不收集：它们不应被外部 `Theme.*` 引用。
fn collect_theme_tokens(source: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    for line in source.lines() {
        for name in public_property_declarations(line) {
            tokens.insert(name.to_owned());
        }
    }
    tokens
}

/// 提取一行中所有 `[in|out|in-out] property <类型> 名字:` 声明的名字。
///
/// 一行里可能有多条声明；允许 `property <color> x:` 与 `property<color> x:` 两种写法。
fn public_property_declarations(line: &str) -> Vec<&str> {
    const KEYWORD: &str = "property";
    let mut names = Vec::new();
    let mut search_from = 0;

    while let Some(offset) = line[search_from..].find(KEYWORD) {
        let keyword_start = search_from + offset;
        let keyword_end = keyword_start + KEYWORD.len();
        search_from = keyword_end;

        // 词边界：`has-property` 这种名字里出现的 "property" 不算声明关键字
        let before_is_boundary = line[..keyword_start]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_slint_ident_char(ch));
        // 关键字后允许空白，或直接 `<`（`property<color> x` 也是合法 Slint 语法）
        let after_is_valid = line[keyword_end..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace() || ch == '<');
        if !(before_is_boundary && after_is_valid) {
            continue;
        }
        // 只收对外可见的 token；无修饰符/private 的声明不在守护范围
        if !has_public_visibility(&line[..keyword_start]) {
            continue;
        }
        if let Some(name) = parse_declared_name(&line[keyword_end..]) {
            names.push(name);
        }
    }

    names
}

/// `property` 关键字之前是否带 `in` / `out` / `in-out` 可见性修饰符。
fn has_public_visibility(before_keyword: &str) -> bool {
    let trimmed = before_keyword.trim_end();
    for modifier in ["in-out", "in", "out"] {
        if let Some(rest) = trimmed.strip_suffix(modifier) {
            let boundary_ok = rest
                .chars()
                .next_back()
                .is_none_or(|ch| !is_slint_ident_char(ch));
            if boundary_ok {
                return true;
            }
        }
    }
    false
}

/// 从 `property` 关键字之后解析声明名字：跳过 `<类型>`，读到 `名字:` 为止。
fn parse_declared_name(after_keyword: &str) -> Option<&str> {
    let rest = after_keyword.trim_start();
    let type_and_rest = rest.strip_prefix('<')?;
    let type_end = type_and_rest.find('>')?;
    let after_type = type_and_rest[type_end + 1..].trim_start();

    let name_end = after_type
        .char_indices()
        .find(|(_, ch)| !is_slint_ident_char(*ch))
        .map_or(after_type.len(), |(index, _)| index);
    let name = &after_type[..name_end];

    if starts_like_identifier(name) && after_type[name_end..].trim_start().starts_with(':') {
        Some(name)
    } else {
        None
    }
}

// -------------------------------------------------------- 主题 token 引用扫描

/// 提取一行中所有 `Theme.<名字>` 引用的名字（名字允许 kebab-case）。
///
/// 会跳过前面还有标识符字符的伪引用（例如 `MyTheme.foo`），
/// 以及 `Theme.` 后不是合法标识符起始的片段。
fn theme_references(line: &str) -> Vec<&str> {
    const PREFIX: &str = "Theme.";
    let mut names = Vec::new();
    let mut search_from = 0;

    while let Some(offset) = line[search_from..].find(PREFIX) {
        let prefix_start = search_from + offset;
        let name_start = prefix_start + PREFIX.len();
        search_from = name_start; // 严格前进，避免死循环

        let before_is_boundary = line[..prefix_start]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_slint_ident_char(ch));
        if !before_is_boundary {
            continue;
        }

        let rest = &line[name_start..];
        let name_end = rest
            .char_indices()
            .find(|(_, ch)| !is_slint_ident_char(*ch))
            .map_or(rest.len(), |(index, _)| index);
        // 末尾的 `-` 视为减号运算符而不是名字的一部分（合法名字不会以 `-` 结尾）
        let name = rest[..name_end].trim_end_matches('-');
        if starts_like_identifier(name) {
            names.push(name);
        }
    }

    names
}

// -------------------------------------------------------------------- 测试

/// 前提：`ui/theme.slint` 存在且导出 `export global Theme`。
#[test]
fn theme_file_exists() {
    let path = ui_dir().join("theme.slint");
    assert!(
        path.is_file(),
        "ui/theme.slint 不存在（{}）：设计 token 必须集中在该文件，见 docs/product/ui-winui3-design-language.md §3",
        path.display()
    );
    let source = read_to_string_expected(&path);
    assert!(
        source.contains("export global Theme"),
        "ui/theme.slint 缺少 `export global Theme`；其它 ui 文件都通过 `Theme.*` 引用 token"
    );
}

/// `ui/theme.slint` 之外不得出现 `#RRGGBB` / `#RRGGBBAA` 硬编码颜色。
#[test]
fn no_hardcoded_colors_outside_theme() {
    let ui = ui_dir();
    let files = collect_slint_files(&ui);
    assert!(
        !files.is_empty(),
        "在 {} 下没有找到任何 *.slint 文件，测试定位可能失效",
        ui.display()
    );

    let mut violations: Vec<String> = Vec::new();
    for (path, display) in &files {
        if display == "theme.slint" {
            continue; // 唯一允许出现颜色字面量的文件
        }
        let source = read_to_string_expected(path);
        for (index, line) in source.lines().enumerate() {
            if has_color_literal(line) {
                let line_no = index + 1;
                violations.push(format!("ui/{display}:{line_no}: {line}"));
            }
        }
    }

    let count = violations.len();
    let details = violations.join("\n");
    assert!(
        violations.is_empty(),
        "发现 {count} 处 ui/theme.slint 之外的硬编码颜色；颜色必须走 Theme token，见 ui/theme.slint:\n{details}"
    );
}

/// 其它 ui 文件引用的 `Theme.<token>` 必须在 `ui/theme.slint` 里声明过。
#[test]
fn theme_tokens_are_defined_before_use() {
    let ui = ui_dir();
    let theme_source = read_to_string_expected(&ui.join("theme.slint"));
    let defined = collect_theme_tokens(&theme_source);
    assert!(
        !defined.is_empty(),
        "未能从 ui/theme.slint 解析出任何 token 声明，token 解析逻辑可能已失效"
    );

    let files = collect_slint_files(&ui);
    let mut undefined: BTreeMap<String, String> = BTreeMap::new();
    for (path, display) in &files {
        if display == "theme.slint" {
            continue; // 定义文件自身不算引用
        }
        let source = read_to_string_expected(path);
        for (index, line) in source.lines().enumerate() {
            for name in theme_references(line) {
                if !defined.contains(name) {
                    let line_no = index + 1;
                    undefined
                        .entry(name.to_owned())
                        .or_insert_with(|| format!("ui/{display}:{line_no}"));
                }
            }
        }
    }

    let details = undefined
        .iter()
        .map(|(name, first_use)| {
            format!("Theme.{name} 未在 ui/theme.slint 声明（首次引用 {first_use}）")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let count = undefined.len();
    assert!(
        undefined.is_empty(),
        "发现 {count} 个未定义就被引用的 Theme token（改名/手滑？）:\n{details}"
    );
}
