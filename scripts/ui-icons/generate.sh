#!/usr/bin/env bash
# 由手写 SVG 源生成多档 PNG 图标资源（单一方向：SVG -> PNG）。
#
# 唯一手写源：ui/icons/svg/*.svg（20×20 viewBox，stroke #ffffff，1.5px 描边，round cap/join）
# 产物：      ui/icons/<px>/<name>.png，px ∈ {24, 32, 40}
#
# 为什么分档（E14）：Slint 1.18 软件渲染器对位图是最近邻采样，40px 资源缩到 16px
#（2.5:1）或 12px（3.33:1）会发虚；改为"显示尺寸 × 2"分档（12→24、16→32、20→40）
# 后，DPR 1（2:1 缩小）与 DPR 2（1:1）都是整数比。
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
# 档位 = 各显示尺寸的 2 倍：Theme.icon-size 20 / Theme.icon-size-small 16 / 关闭图标 12
SIZES=(24 32 40)

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

# 清掉历史版本平铺在 ui/icons/ 下的 40px 资源，避免与新分档目录混淆
rm -f "$PNG_DIR"/*.png

for svg in "${svgs[@]}"; do
    name="$(basename "$svg" .svg)"
    for size in "${SIZES[@]}"; do
        mkdir -p "$PNG_DIR/$size"
        rsvg-convert -w "$size" -h "$size" -o "$PNG_DIR/$size/$name.png" "$svg"
    done
done

echo "generated ${#svgs[@]} SVG x ${#SIZES[@]} sizes: $SVG_DIR/*.svg -> $PNG_DIR/{24,32,40}/*.png"
