#!/usr/bin/env bash
# 提取 ui/**.slint 里的 @tr(...) 文案，生成/更新 gettext 模板 translations/yshell-app.pot。
#
# 用法（在仓库任意位置执行）：
#   bash scripts/i18n/extract.sh
#
# 行为：
#   * 显式扫描 ui/**/*.slint（extractor 不会跟进 import，只解析传入的文件；
#     重复传参无害）。
#   * 不加 --no-location，保留 "#: file:line" 定位注释。
#   * 幂等：每次从源码全量重新生成，不累积已删除的旧条目；若除
#     POT-Creation-Date 外内容没有变化，则保留现有文件（连同原时间戳），
#     重复运行不产生 git diff（设计规范 §7.3 的 CI 校验依赖这一点）。
#
# 依赖：slint-tr-extractor（cargo install slint-tr-extractor）。
# 工作流与 .po 头部要求见 scripts/i18n/README.md。
set -euo pipefail
cd "$(dirname "$0")/../.."

POT=translations/yshell-app.pot
ENTRY=ui/main_window.slint

if ! command -v slint-tr-extractor >/dev/null 2>&1; then
    echo "错误：未找到 slint-tr-extractor（Slint 的 @tr 文案提取工具）。" >&2
    echo "安装：cargo install slint-tr-extractor" >&2
    echo "若已安装，请确认 ~/.cargo/bin 在 PATH 中。" >&2
    exit 1
fi

# ui/**/*.slint 需要 globstar（bash >= 4）；"..**" 匹配零层或多层目录。
shopt -s globstar nullglob

if [[ ! -f "$ENTRY" ]]; then
    echo "错误：找不到 $ENTRY，请在 yshell 仓库内运行本脚本。" >&2
    exit 1
fi

# main_window.slint 是编译入口，显式放最前；glob 会带上全部 ui/**.slint。
files=("$ENTRY" ui/**/*.slint)
scanned=$(printf '%s\n' "${files[@]}" | sort -u | wc -l | tr -d '[:space:]')

mkdir -p "$(dirname "$POT")"
tmp=$(mktemp "${POT}.XXXXXX")
trap 'rm -f "$tmp"' EXIT

# extractor 默认行为：全量重新生成 + 写入新的 POT-Creation-Date（分钟粒度）。
slint-tr-extractor "${files[@]}" -o "$tmp"

# 幂等处理：除 POT-Creation-Date 外内容一致时保留旧文件，避免每次运行都产生
# diff；内容变化时用新文件（携带新时间戳）替换。
if [[ -f "$POT" ]] \
    && diff -q <(grep -v '"POT-Creation-Date' "$POT") \
               <(grep -v '"POT-Creation-Date' "$tmp") >/dev/null; then
    echo "内容无变化，保留现有 $POT"
else
    mv -f "$tmp" "$POT"
    echo "已更新 $POT"
fi

# 注意：grep 计数包含头部那条空的 msgid ""（可翻译条目数 = 该值 - 1）。
msgids=$(grep -c '^msgid ' "$POT" || true)
echo "已扫描 ${scanned} 个 .slint 文件，提取 ${msgids} 条 msgid -> $POT"
echo "下一步：把新条目合并进 translations/<lang>/LC_MESSAGES/yshell-app.po（见 scripts/i18n/README.md）。"
