#!/usr/bin/env bash
# 由 PNG 资源重新生成 icons.slint 的 Icon 组件（位图 + colorize 着色）。
# 背景：软件渲染器不实现 Path，图标必须是位图。
# 前置：先运行 scripts/ui-icons/generate.sh 从 SVG 源生成 ui/icons/*.png。
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
names = sorted(p.stem for p in icon_dir.glob("*.png"))
if not names:
    sys.exit("no icons found in " + str(icon_dir) +
             " — 先运行 bash scripts/ui-icons/generate.sh 从 SVG 源生成 PNG")

chain = []
for i, name in enumerate(names):
    prefix = "        " if i == 0 else "        : "
    chain.append(f'{prefix}name == "{name}" ? @image-url("../../icons/{name}.png")\n')
chain.append('        : @image-url("../../icons/plus.png")')
source_expr = "".join(chain)

template = '''// Fluent 风格线性图标（20×20 viewBox，1.5px 描边，PNG 40×40 资源）。
//
// 规范：docs/product/ui-winui3-design-language.md §5.16
// 用法：Icon { name: "plus"; icon-color: Theme.text-primary; }
//
// 为什么是位图而不是矢量 Path：Slint 的软件渲染器不实现 Path（draw_path 为空），
// 矢量图标会完全不可见。资源由 scripts/ui-icons/generate.sh 从 SVG 源生成，
// 通过 Image.colorize 跟随主题着色（悬停/选中/禁用状态都能变色）。

import { Theme } from "../../theme.slint";

export component Icon inherits Image {
    in property <string> name: "plus";
    in property <brush> icon-color: Theme.text-primary;
    in property <length> icon-size: Theme.icon-size;

    width: icon-size;
    height: icon-size;
    image-fit: contain;
    colorize: icon-color;

    source:
__SOURCE__;
}
'''
out.write_text(template.replace("__SOURCE__", source_expr), encoding="utf-8")
print(f"wrote {out} with {len(names)} icons")
PY

head -25 "$OUT"
