#!/usr/bin/env bash
# 生成 yshell-terminal 内置的 CJK 回退子集（可复现）。
#
# 用途：终端在 DejaVu Sans Mono 缺字时逐字形回退到该子集（中文/全角/假名）。
#
# 源与许可：
#   * 上游：notofonts/noto-cjk（Debian 包 fonts-noto-cjk；本机 1:20240730+repack1-1）
#   * 文件：/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc
#           —— 取其中的 "Noto Sans Mono CJK SC" face（等宽变体；脚本用 fontTools
#           按家族名查找，不写死 TTC 序号，序号变化时自动适配）。
#   * 许可：SIL Open Font License 1.1（OFL-1.1）。
#           许可正文与来源说明见 crates/yshell-terminal/assets/LICENSE-NotoSansMonoCJK-OFL-1.1.txt。
#
# 工具：python3 + fontTools（pyftsubset）。
#   Debian trixie：apt-get install -y python3-fonttools   # 4.57.0-1
#
# 字符集（与设计 §2 一致，约 1.8 MB）：
#   ASCII 可见字符 + 全角标点/符号 + 平假名/片假名 + GB2312 符号区 + 一级 + 二级汉字。
#   备选小档：把 GB2312-LEVEL2 去掉 ≈995 KiB；只要符号/假名 ≈78 KiB。
#
# 用法：
#   bash scripts/fonts/make-cjk-subset.sh [输出路径]
#   环境变量：NOTO_CJK_TTC（默认 /usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc）
#
# 输出：默认 crates/yshell-terminal/assets/NotoSansMonoCJKsc-GB2312-Subset.otf
#       （结尾打印字节数与 sha256，便于与仓库内资产比对）
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
ttc="${NOTO_CJK_TTC:-/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc}"
output="${1:-$repo_dir/crates/yshell-terminal/assets/NotoSansMonoCJKsc-GB2312-Subset.otf}"

if [ ! -f "$ttc" ]; then
  echo "找不到源字体：$ttc" >&2
  echo "安装：sudo apt-get install -y fonts-noto-cjk（或设置 NOTO_CJK_TTC）" >&2
  exit 1
fi

if ! python3 -c "import fontTools" >/dev/null 2>&1; then
  echo "缺少 fontTools：sudo apt-get install -y python3-fonttools" >&2
  exit 1
fi

if command -v dpkg-query >/dev/null 2>&1; then
  echo "源包：$(dpkg-query -W -f='${Package} ${Version}' fonts-noto-cjk 2>/dev/null || echo 'fonts-noto-cjk (未知版本)')"
fi
echo "源文件：$ttc"
echo "输出：$output"

charset_file=$(mktemp /tmp/yshell-cjk-charset.XXXXXX.txt)
trap 'rm -f "$charset_file"' EXIT

NOTO_CJK_TTC="$ttc" CHARSET_FILE="$charset_file" OUTPUT_FILE="$output" python3 - <<'PY'
import os

from fontTools.ttLib import TTCollection
from fontTools import subset as ftsubset

TTC = os.environ["NOTO_CJK_TTC"]
OUTPUT = os.environ["OUTPUT_FILE"]
CHARSET_FILE = os.environ["CHARSET_FILE"]

ASCII = [chr(code) for code in range(0x20, 0x7F)]
FULLWIDTH_AND_KANA = (
    "　、。，．・：；？！゛゜´｀¨＾￣＿ヽヾゝゞ〃仝々〆〇ー—‐／＼～∥｜…‥‘’“”"
    "（）〔〕［］｛｝〈〉《》「」『』【】＋－±×÷＝≠＜＞≦≧∞∴♂♀°′″℃￥＄￠￡％＃＆＊＠§☆★○●◎◇"
    + "".join(chr(code) for code in range(0x3041, 0x3097))
    + "".join(chr(code) for code in range(0x30A1, 0x30FB))
)


def gb2312_rows(first, last):
    chars = []
    for high in range(first, last + 1):
        for low in range(0xA1, 0xFF):
            try:
                chars.append(bytes([high, low]).decode("gb2312"))
            except UnicodeDecodeError:
                pass
    return chars


GB2312_SYMBOLS = gb2312_rows(0xA1, 0xAF)
GB2312_LEVEL1 = gb2312_rows(0xB0, 0xD7)
GB2312_LEVEL2 = gb2312_rows(0xD8, 0xF7)

chars = list(dict.fromkeys(ASCII + list(FULLWIDTH_AND_KANA) + GB2312_SYMBOLS + GB2312_LEVEL1 + GB2312_LEVEL2))
with open(CHARSET_FILE, "w", encoding="utf-8") as handle:
    handle.write("".join(chars))

collection = TTCollection(TTC, lazy=True)
font_number = None
for index, font in enumerate(collection.fonts):
    family = font["name"].getDebugName(1) or ""
    if "Noto Sans Mono CJK SC" in family:
        font_number = index
        break
collection.close()
if font_number is None:
    raise SystemExit("在 TTC 中找不到 'Noto Sans Mono CJK SC'")

print(f"字符集：{len(chars)} 个码位（ASCII + 全角/假名 + GB2312 符号/一级/二级）")
print(f"TTC face 序号：{font_number}（按家族名查找）")
print("pyftsubset 参数：--layout-features= --no-subset-tables+=DSIG")
ftsubset.main(
    [
        TTC,
        f"--font-number={font_number}",
        f"--text-file={CHARSET_FILE}",
        f"--output-file={OUTPUT}",
        "--layout-features=",
        "--no-subset-tables+=DSIG",
    ]
)
PY

bytes=$(wc -c <"$output")
echo "输出大小：${bytes} bytes（$((bytes / 1024)) KiB）"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$output"
fi
