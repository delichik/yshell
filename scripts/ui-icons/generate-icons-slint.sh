#!/usr/bin/env bash
# 由 PNG 资源重新生成 icons.slint 的 Icon 组件（位图 + colorize 着色）。
# 背景：软件渲染器不实现 Path，图标必须是位图。
# 前置：先运行 scripts/ui-icons/generate.sh 从 SVG 源生成 ui/icons/{24,32,40}/*.png。
#
# 生成逻辑（E14/E15）：
#   * `icon-size` ≤12px 取 24px 档、≤16px 取 32px 档、其余取 40px 档（显示尺寸的 2 倍）；
#   * 未知图标名回退到该档的透明占位 `blank.png`，并在 debug 构建打印一次告警。
#
# 用法（在仓库根目录）：bash scripts/ui-icons/generate-icons-slint.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

ICON_DIR=ui/icons
OUT=ui/components/basic/icons.slint

if ! command -v python3 >/dev/null 2>&1; then
    echo "error: 未找到 python3，无法生成 $OUT。" >&2
    echo "       安装：Debian/Ubuntu  sudo apt-get install python3" >&2
    exit 1
fi

python3 - "$ICON_DIR" "$OUT" <<'PY'
import pathlib, sys

icon_dir, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
sizes = (24, 32, 40)
buckets = {
    size: sorted(p.stem for p in (icon_dir / str(size)).glob("*.png"))
    for size in sizes
}
names = buckets[40]
if not names:
    sys.exit("no icons found in " + str(icon_dir) +
             " — 先运行 bash scripts/ui-icons/generate.sh 从 SVG 源生成 PNG")

def chain(size, indent):
    """生成某一档的 name 选择链，末尾回退到该档的 blank 占位。"""
    lines = []
    for i, name in enumerate(buckets[size]):
        prefix = indent if i == 0 else indent + ": "
        lines.append(f'{prefix}name == "{name}" ? @image-url("../../icons/{size}/{name}.png")\n')
    lines.append(f'{indent}: @image-url("../../icons/{size}/blank.png")')
    return "".join(lines)

source_expr = (
    f"        icon-size <= 12px ? (\n{chain(24, '            ')})\n"
    f"        : icon-size <= 16px ? (\n{chain(32, '            ')})\n"
    f"        : (\n{chain(40, '            ')});"
)
known_expr = " || ".join(f'name == "{name}"' for name in names)

template = '''// Fluent 风格线性图标（20×20 viewBox，1.5px 描边，PNG 分档：24/32/40）。
//
// 规范：docs/product/ui-winui3-design-language.md §5.16
// 用法：Icon { name: "plus"; icon-color: Theme.text-primary; }
//
// 为什么是位图而不是矢量 Path：Slint 的软件渲染器不实现 Path（draw_path 为空），
// 矢量图标会完全不可见。资源由 scripts/ui-icons/generate.sh 从 SVG 源生成，
// 通过 Image.colorize 跟随主题着色（悬停/选中/禁用状态都能变色）。
//
// 本文件由 scripts/ui-icons/generate-icons-slint.sh 生成，请改生成脚本后重跑。

import { Theme } from "../../theme.slint";

export component Icon inherits Image {
    in property <string> name: "plus";
    in property <brush> icon-color: Theme.text-primary;
    in property <length> icon-size: Theme.icon-size;

    width: icon-size;
    height: icon-size;
    image-fit: contain;
    colorize: icon-color;

    // E14：按显示尺寸选最近的 2× 资源（12→24、16→32、20→40），
    // 避免最近邻采样在非整数比（40/12≈3.33）下发虚。
    source:
__SOURCE__

    // E15：未知图标名回退到透明占位（不再凭空显示一个"+"），debug 构建打告警。
    private property <bool> name-known: __KNOWN__;

    changed name => {
        if (!root.name-known) {
            debug("Icon: unknown icon name '" + root.name + "'");
        }
    }
}
'''
out.write_text(template.replace("__SOURCE__", source_expr)
                       .replace("__KNOWN__", known_expr), encoding="utf-8")
print(f"wrote {out} with {len(names)} icons x {len(sizes)} sizes")
PY

head -20 "$OUT"
