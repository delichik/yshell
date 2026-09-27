#!/usr/bin/env bash
# 由手写 SVG 源生成 PNG 图标资源（单一方向：SVG -> PNG）。
#
# 唯一手写源：ui/icons/svg/*.svg（20×20 viewBox，stroke #ffffff，1.5px 描边，round cap/join）
# 产物：      ui/icons/<name>.png（40×40，显示 20px 的 2x 资源）
#
# 背景：Slint 的软件渲染器不实现 Path（draw_path 为空），所以图标必须用位图 +
# colorize 着色；SVG 仅作为可编辑源文件，不参与编译。
#
# 之后用 scripts/ui-icons/generate-icons-slint.sh 扫描 PNG 重建 icons.slint。
#
# 用法（在仓库根目录）：bash scripts/ui-icons/generate.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

SVG_DIR=ui/icons/svg
PNG_DIR=ui/icons

if ! command -v rsvg-convert >/dev/null 2>&1; then
    echo "error: 未找到 rsvg-convert（librsvg），无法把 SVG 转为 PNG。" >&2
    echo "       安装：Debian/Ubuntu  sudo apt-get install librsvg2-bin" >&2
    echo "              macOS          brew install librsvg" >&2
    echo "              MSYS2          pacman -S mingw-w64-x86_64-librsvg" >&2
    exit 1
fi

shopt -s nullglob
svgs=("$SVG_DIR"/*.svg)
if [ "${#svgs[@]}" -eq 0 ]; then
    echo "error: $SVG_DIR 下没有 .svg 源文件。" >&2
    exit 1
fi

mkdir -p "$PNG_DIR"
for svg in "${svgs[@]}"; do
    name="$(basename "$svg" .svg)"
    rsvg-convert -w 40 -h 40 -o "$PNG_DIR/$name.png" "$svg"
done

echo "generated ${#svgs[@]} PNG: $SVG_DIR/*.svg -> $PNG_DIR/*.png"
