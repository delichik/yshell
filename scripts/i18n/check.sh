#!/usr/bin/env bash
# 重新从 ui/**/*.slint 提取 .pot 到临时文件，并校验它和两个 .po 目录是否一致：
#   * 模板与当前源码同步（无缺失/过期条目）；
#   * en-US / zh-CN 的 (msgctxt, msgid, msgid_plural) 与模板一一对应；
#   * 无缺失、无过期、无空 msgstr，en-US msgstr 必须等于 msgid；
#   * 头部字段完整、Language / Plural-Forms 正确。
#
# 用法（在仓库任意位置执行）：
#   bash scripts/i18n/check.sh
# Windows 下在 WSL 里运行（与 extract.sh 相同）：
#   wsl.exe -e bash /mnt/d/NewSpace/yshell/scripts/i18n/check.sh
#
# 依赖：slint-tr-extractor、python3；gettext 的 msgfmt 可选（有则额外做语法检查）。
# 不需要网络；只读取源码与翻译文件，不会改写它们。
set -euo pipefail
cd "$(dirname "$0")/../.."

for tool in slint-tr-extractor python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "错误：未找到 $tool。" >&2
        if [[ "$tool" == "slint-tr-extractor" ]]; then
            echo "安装：cargo install slint-tr-extractor" >&2
        fi
        exit 1
    fi
done

shopt -s globstar nullglob
files=(ui/main_window.slint ui/**/*.slint)

tmp=$(mktemp "translations/yshell-app.pot.check.XXXXXX")
trap 'rm -f "$tmp"' EXIT

# 与 extract.sh 完全相同的提取 + 规范化，保证比较的是同一份内容。
slint-tr-extractor "${files[@]}" -o "$tmp" >/dev/null
python3 scripts/i18n/catalogs.py normalize "$tmp" >/dev/null

if command -v msgfmt >/dev/null 2>&1; then
    for po in translations/en-US/LC_MESSAGES/yshell-app.po \
              translations/zh-CN/LC_MESSAGES/yshell-app.po; do
        if ! msgfmt --check -o /dev/null "$po"; then
            echo "错误：msgfmt --check 失败：$po" >&2
            exit 1
        fi
    done
    echo "msgfmt --check: OK (en-US, zh-CN)"
else
    echo "提示：未找到 msgfmt，跳过 gettext 语法检查（catalogs.py 仍会解析两个 .po）。"
fi

python3 scripts/i18n/catalogs.py check --pot "$tmp"
