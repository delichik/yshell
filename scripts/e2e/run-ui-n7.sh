#!/usr/bin/env bash
# N7 多窗口 UI E2E（无头 Xvfb + xdotool + 像素断言）。
#
# 覆盖（N7 设计 §1/§2）：
#   ① 标签右键 `Move to New Window`：原窗口标签消失、其它标签不受影响；新窗口能输入/输出
#   ② 两个窗口并存：各自活动标签/终端/尺寸独立；新窗口关闭按钮（WM_DELETE_WINDOW）
#      在带活动连接时弹出确认（D7），确认后断开并关闭，主窗口不受影响
#   ③ `Move to Main Window`：标签迁回主窗口
#   ④ 最后一个窗口关闭 = 退出（沿用 Quit 确认文案）
#
# 说明：本仓 Xvfb 没有窗口管理器，Debian 的 xdotool 3.20160805 `windowclose`
# 在该环境实测不生效（xterm 亦不响应），脚本改用 `scripts/e2e/send-wm-delete.c`
# 直接 XSendEvent(WM_DELETE_WINDOW)（等价窗口管理器关闭按钮）。
#
# 用法：
#   bash scripts/e2e/run-ui-n7.sh [--theme dark|light] [--lang zh-CN|en-US]
#                                 [--outdir dist/ui-checks] [--prefix n7] [--keep-config]
set -u

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

theme="dark"
lang="zh-CN"
outdir="$repo_dir/dist/ui-checks"
prefix="n7"
keep_config=0
bin="${YSHELL_E2E_BIN:-$repo_dir/target/debug/yshell}"
display="${YSHELL_E2E_DISPLAY:-:99}"
config_dir="/tmp/yshell-e2e-n7-config"
log_file="/tmp/yshell-e2e-n7.log"
ssh_key="${HOME}/.ssh/yshell_test_ed25519"
container="${YSHELL_TEST_SSH_CONTAINER:-yshell-test-ssh}"
ssh_port="${YSHELL_TEST_SSH_PORT:-2222}"
wm_delete_bin="/tmp/yshell-e2e-n7-send-wm-delete"

# 1440×900 窗口内坐标（与 run-ui-tabs.sh 同源校准）。
TAB_ROW_Y=100        # 标签条文字行中心
TAB_ACCENT_Y=110     # 活动标签底部强调条
TAB1_X=330           # 第一个标签中心
TAB2_X=480           # 第二个标签中心
# 标签右键菜单项 y：锚点 y=100、面板 padding 4、行高 32、分隔行 9。
# 索引 12 = Move to New Window、13 = Move to Main Window。
MENU_MOVE_NEW_Y=435
MENU_MOVE_MAIN_Y=467

usage() {
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
  --theme) theme="${2:-}" ; shift 2 ;;
  --lang) lang="${2:-}" ; shift 2 ;;
  --outdir) outdir="${2:-}" ; shift 2 ;;
  --prefix) prefix="${2:-}" ; shift 2 ;;
  --keep-config) keep_config=1 ; shift ;;
  -h | --help) usage; exit 0 ;;
  *) echo "未知参数：$1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$theme" in dark | light) ;; *) echo "--theme 只支持 dark|light" >&2; exit 2 ;; esac
case "$lang" in zh-CN | en-US) ;; *) echo "--lang 只支持 zh-CN|en-US" >&2; exit 2 ;; esac

E2E_SHOT_PREFIX="$prefix-$theme-$lang"
E2E_THEME="$theme"
mkdir -p "$outdir"

cleanup() { e2e_stop_app; }
trap cleanup EXIT

# ---------------------------------------------------------------- 前置检查
e2e_require_tools || exit 1
command -v gcc >/dev/null 2>&1 || {
  e2e_warn "缺少 gcc（编译 send-wm-delete helper）"
  exit 1
}
if [ ! -x "$bin" ]; then
  e2e_warn "找不到可执行文件：$bin（先 cargo xtask build）"
  exit 1
fi
container_status=$(bash "$repo_dir/scripts/test-ssh/run.sh" status 2>&1 || true)
if ! grep -q 'Up ' <<<"$container_status"; then
  e2e_warn "测试容器未运行（$container）：先执行 bash scripts/test-ssh/run.sh up"
  exit 1
fi

gcc -O2 -o "$wm_delete_bin" "$script_dir/send-wm-delete.c" -lX11 || {
  e2e_warn "编译 send-wm-delete.c 失败"
  exit 1
}

e2e_log "配置：theme=$theme lang=$lang outdir=$outdir prefix=$prefix"
e2e_ensure_xvfb "$display" || exit 1

# ---------------------------------------------------------------- 配置
write_config() {
  mkdir -p "$config_dir"
  {
    echo 'schema_version = 1'
    echo ''
    echo '[auth_profiles.auth-key]'
    echo 'id = "auth-key"'
    echo 'name = "Docker Key"'
    echo "method = { type = \"private_key\", path = \"$ssh_key\" }"
    echo ''
    echo '[[folders]]'
    echo 'id = "saved-sessions"'
    echo 'name = "Saved Sessions"'
    echo 'sessions = ['
    for i in 1 2 3; do
      echo "  { id = \"n7-$i\", name = \"S$i\", host = \"127.0.0.1\", port = $ssh_port, username = \"root\", auth_profile_id = \"auth-key\", host_key_policy = \"accept_any_for_testing\" },"
    done
    echo ']'
  } >"$config_dir/config.toml"
}
[ "$keep_config" = 0 ] && rm -rf "$config_dir"
write_config

# ---------------------------------------------------------------- 局部 helper
# 以 pid 列出窗口 id（不含标题匹配）。
list_windows() { xdotool search --pid "$E2E_APP_PID" 2>/dev/null; }

other_window() { # 返回除主窗口之外的第一个窗口 id
  list_windows | grep -v "^$E2E_APP_WIN$" | head -1
}

# 指定窗口的像素/区域探针（lib.sh 的探针都绑定 E2E_APP_WIN）。
win_pixel_gray() { # $1=win $2=x $3=y
  timeout 10 import -window "$1" png:- 2>/dev/null \
    | convert - -crop 1x1+"$2"+"$3" +repage -colorspace gray -format '%[fx:int(255*r)]' info: 2>/dev/null
}

win_region_stddev() { # $1=win $2=x $3=y $4=w $5=h
  timeout 10 import -window "$1" png:- 2>/dev/null \
    | convert - -crop "$4x$5+$2+$3" +repage -colorspace gray -format '%[fx:standard_deviation]' info: 2>/dev/null
}

win_region_mode_gray() { # $1=win $2=x $3=y $4=w $5=h
  timeout 10 import -window "$1" png:- 2>/dev/null \
    | convert - -crop "$4x$5+$2+$3" +repage -colorspace gray -depth 8 -format %c histogram:info:- 2>/dev/null \
    | sort -rn | head -1 | sed -n 's/.*gray(\([0-9]*\)).*/\1/p'
}

win_shot() { # $1=win $2=文件名 → stdout 路径/空
  local path="$outdir/${E2E_SHOT_PREFIX}-$2.png"
  if timeout 10 import -window "$1" "$path" 2>/dev/null && [ -s "$path" ]; then
    echo "$path"
  fi
}

win_check_shot() { # $1=win $2=文件名 $3=步骤名
  local path
  if path=$(win_shot "$1" "$2"); then
    e2e_check "$3" "截图已生成" 1 "$(basename "$path")"
  else
    e2e_check "$3" "截图已生成" 0 "截图失败"
  fi
}

tab_active_at() { # $1=win $2=x → 活动标签强调条（accent 蓝）
  local r g b
  read -r r g b <<<"$(timeout 10 import -window "$1" png:- 2>/dev/null \
    | convert - -crop 1x1+"$2"+"$TAB_ACCENT_Y" +repage txt:- 2>/dev/null \
    | sed -n 's/.*srgb(\([0-9]*\),\([0-9]*\),\([0-9]*\)).*/\1 \2 \3/p' | tail -1)" || return 1
  [ "${b:-0}" -gt 120 ] && [ "${b:-0}" -gt $(( ${r:-0} + 40 )) ]
}

tab_bar_background_at() { # $1=win $2=x → 该点无标签（标签条底色）
  local gray
  gray=$(win_pixel_gray "$1" "$2" "$TAB_ROW_Y")
  case "$E2E_THEME" in
  light) [ "${gray:-0}" -ge 200 ] ;;
  *) [ "${gray:-0}" -le 90 ] ;;
  esac
}

# 关闭指定窗口（等价 WM 关闭按钮；xdotool windowclose 在无 WM 的 Xvfb 下不生效）。
close_window() { # $1=win
  DISPLAY="$E2E_DISPLAY" "$wm_delete_bin" "$1" >/dev/null 2>&1
}

# 弹窗判定（单窗口局部）：用该窗口菜单栏亮度做基线（scrim 压暗）。
win_dialog_open() { # $1=win $2=baseline
  local current threshold
  current=$(win_pixel_gray "$1" 700 20)
  threshold=$(( $2 * 80 / 100 ))
  [ "${current:-999}" -lt "$threshold" ]
}

# 确认按钮中心扫描（强调/破坏性色块在弹窗下半部）。
win_confirm_center() { # $1=win → 屏坐标 "x y"
  local shot=/tmp/yshell-n7-dialog.png result
  timeout 10 import -window "$1" "$shot" 2>/dev/null || return 1
  result=$(python3 - "$shot" <<'PY' 2>/dev/null || true
import sys
from PIL import Image
img = Image.open(sys.argv[1]).convert('RGB')
points = []
for y in range(200, 760):
    for x in range(300, 1140):
        r, g, b = img.getpixel((x, y))
        accent = b > 140 and b - r > 50 and g > 80          # 强调色（主按钮）
        destructive = r > 150 and r - b > 40 and r - g > 50  # 破坏性按钮
        if accent or destructive:
            points.append((x, y))
if not points:
    sys.exit(1)
# 取最右侧的按钮簇（Confirm 在 Cancel 右侧），避免扫到两个按钮时中心落空。
max_x = max(p[0] for p in points)
cluster = [p for p in points if p[0] >= max_x - 90]
cx = sum(p[0] for p in cluster) // len(cluster)
cy = sum(p[1] for p in cluster) // len(cluster)
print(cx, cy)
PY
)
  [ -n "$result" ] && echo "$result"
}

# ---------------------------------------------------------------- 场景
e2e_log "== N7 多窗口 e2e =="
e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$E2E_DISPLAY" || exit 1

e2e_log "-- ① 打开两个标签（两个活动连接）"
e2e_open_saved_session 239
sleep 1.0
e2e_open_saved_session 268
sleep 2.0
win_check_shot "$E2E_APP_WIN" "01-two-tabs" "①主窗口双标签截图"

e2e_log "-- ① 第二标签右键 → Move to New Window"
e2e_click "$TAB2_X" "$TAB_ROW_Y" 0.5
xdotool mousemove --sync --window "$E2E_APP_WIN" "$TAB2_X" "$TAB_ROW_Y" click 3
sleep 1.0
win_check_shot "$E2E_APP_WIN" "02-tab-menu" "①标签右键菜单（含迁移项）"
xdotool mousemove --sync --window "$E2E_APP_WIN" "$((TAB2_X + 50))" "$MENU_MOVE_NEW_Y"
sleep 0.3
xdotool click 1
sleep 3.0

SECOND=$(other_window)
window_count=$(list_windows | wc -l | tr -d ' ')
e2e_check "①迁移后窗口数" "pid 下有 2 个窗口" \
  "$([ "$window_count" = "2" ] && echo 1 || echo 0)" \
  "窗口：$(list_windows | tr '\n' ' ')"

if [ -z "${SECOND:-}" ]; then
  e2e_warn "未出现第二个窗口，后续场景跳过"
  e2e_summary
  exit 1
fi

xdotool windowmove --sync "$SECOND" 480 160 >/dev/null 2>&1 || true
sleep 0.5
e2e_check "①原窗口标签消失" "第二个标签位置已是标签条底色（右邻居也不存在）" \
  "$(tab_bar_background_at "$E2E_APP_WIN" "$TAB2_X" && echo 1 || echo 0)" \
  "$(tab_bar_background_at "$E2E_APP_WIN" "$TAB2_X" && echo 是 || echo 否)"
e2e_check "①原窗口保留其它标签" "第一个标签仍是活动标签（强调条）" \
  "$(tab_active_at "$E2E_APP_WIN" "$TAB1_X" && echo 1 || echo 0)" \
  "$(tab_active_at "$E2E_APP_WIN" "$TAB1_X" && echo 是 || echo 否)"
e2e_check "①新窗口显示迁入标签" "新窗口第一个标签为活动标签" \
  "$(tab_active_at "$SECOND" "$TAB1_X" && echo 1 || echo 0)" \
  "$(tab_active_at "$SECOND" "$TAB1_X" && echo 是 || echo 否)"
e2e_check "①新窗口不显示其它标签" "新窗口第二个标签位置为标签条底色" \
  "$(tab_bar_background_at "$SECOND" "$TAB2_X" && echo 1 || echo 0)" \
  "$(tab_bar_background_at "$SECOND" "$TAB2_X" && echo 是 || echo 否)"
win_check_shot "$E2E_APP_WIN" "03-main-after-move" "①原窗口（迁移后）"
win_check_shot "$SECOND" "04-second-window" "①新窗口（迁入的标签）"

e2e_log "-- ② 两窗口并行：在新窗口输入/输出"
second_baseline=$(win_region_mode_gray "$SECOND" 700 20 8 8)
shot_before=/tmp/yshell-n7-before-typing.png
timeout 10 import -window "$SECOND" "$shot_before" 2>/dev/null || true
xdotool windowfocus --sync "$SECOND" >/dev/null 2>&1 || true
xdotool mousemove --sync --window "$SECOND" 400 400 click 1
sleep 0.5
xdotool type --delay 60 'echo n7-moved-tab-ok'
xdotool key Return
sleep 1.5
shot_after=/tmp/yshell-n7-after-typing.png
timeout 10 import -window "$SECOND" "$shot_after" 2>/dev/null || true
changed=0
if [ -s "$shot_before" ] && [ -s "$shot_after" ]; then
  changed=$(compare -metric AE \
    <(convert "$shot_before" -crop 300x160+250+300 +repage png:-) \
    <(convert "$shot_after" -crop 300x160+250+300 +repage png:-) null: 2>&1 || true)
fi
e2e_check "②新窗口输入输出" "迁入标签的终端响应输入（区域像素变化 > 50）" \
  "$(awk -v d="${changed:-0}" 'BEGIN { exit !(d + 0 > 50) }' && echo 1 || echo 0)" \
  "AE=${changed:-0}"
win_check_shot "$SECOND" "05-second-typed" "②新窗口输入后"

e2e_log "-- ② 主窗口输入独立（不与新窗口串台）"
shot_second_before_main=/tmp/yshell-n7-second-before-main.png
timeout 10 import -window "$SECOND" "$shot_second_before_main" 2>/dev/null || true
xdotool windowfocus --sync "$E2E_APP_WIN" >/dev/null 2>&1 || true
xdotool mousemove --sync --window "$E2E_APP_WIN" 700 500 click 1
sleep 0.5
xdotool type --delay 60 'echo n7-main-window-ok'
xdotool key Return
sleep 1.5
win_check_shot "$E2E_APP_WIN" "06-main-typed" "②主窗口输入后"
shot_second_after_main=/tmp/yshell-n7-second-after-main.png
timeout 10 import -window "$SECOND" "$shot_second_after_main" 2>/dev/null || true
changed_second=0
if [ -s "$shot_second_before_main" ] && [ -s "$shot_second_after_main" ]; then
  # 只比终端输出区（避开失焦时会变色的 accent 边框与光标位置）：
  # x 290..900 / y 145..265 是窗口 2 的静态输出文本区。
  changed_second=$(compare -metric AE \
    <(convert "$shot_second_before_main" -crop 610x120+290+145 +repage png:-) \
    <(convert "$shot_second_after_main" -crop 610x120+290+145 +repage png:-) null: 2>&1 || true)
fi
e2e_check "②主窗口输入不串台" "主窗口打字不影响新窗口终端输出区（AE = 0）" \
  "$(awk -v d="${changed_second:-0}" 'BEGIN { exit !(d + 0 < 10) }' && echo 1 || echo 0)" \
  "AE=${changed_second:-0}"

e2e_log "-- ② 关闭带活动连接的新窗口 → 确认（D7）"
close_window "$SECOND"
sleep 1.5
dialog_open=0
if [ -n "${second_baseline:-}" ]; then
  win_dialog_open "$SECOND" "$second_baseline" && dialog_open=1 || dialog_open=0
fi
e2e_check "②关闭确认弹窗" "带活动连接的窗口关闭前弹确认（scrim 压暗）" "$dialog_open" \
  "$(win_dialog_open "$SECOND" "${second_baseline:-1}" && echo 弹窗已打开 || echo 无弹窗)"
win_check_shot "$SECOND" "07-close-confirm" "②关闭窗口确认弹窗"

center=$(win_confirm_center "$SECOND")
e2e_check "②确认按钮可定位" "弹窗存在强调/破坏性主按钮" \
  "$([ -n "${center:-}" ] && echo 1 || echo 0)" "${center:-未找到}"
if [ -n "${center:-}" ]; then
  set -- $center
  xdotool mousemove --sync --window "$SECOND" "$1" "$2"
  sleep 0.3
  xdotool click 1
  sleep 2.0
fi
e2e_check "②确认后窗口关闭" "pid 下只剩主窗口" \
  "$([ "$(list_windows | wc -l | tr -d ' ')" = "1" ] && echo 1 || echo 0)" \
  "窗口：$(list_windows | tr '\n' ' ')"
e2e_check "②主窗口不受影响" "主窗口活动标签仍是 S1 且终端有内容" \
  "$(tab_active_at "$E2E_APP_WIN" "$TAB1_X" && awk -v s="$(win_region_stddev "$E2E_APP_WIN" 300 190 600 110)" 'BEGIN { exit !(s + 0 > 0.02) }' && echo 1 || echo 0)" \
  "stddev=$(win_region_stddev "$E2E_APP_WIN" 300 190 600 110)"
win_check_shot "$E2E_APP_WIN" "08-main-after-close" "②新窗口关闭后的主窗口"

e2e_log "-- ③ Move to Main Window（再迁移一次）"
e2e_open_saved_session 297 >/dev/null 2>&1 || true
sleep 2.0
# 右键当前第二个标签（S1 之后的新标签）→ Move to New Window，再迁回主窗口。
xdotool mousemove --sync --window "$E2E_APP_WIN" "$TAB2_X" "$TAB_ROW_Y" click 3
sleep 1.0
xdotool mousemove --sync --window "$E2E_APP_WIN" "$((TAB2_X + 50))" "$MENU_MOVE_NEW_Y"
sleep 0.3
xdotool click 1
sleep 3.0
SECOND=$(other_window)
e2e_check "③迁出到新窗口" "再次出现第二个窗口" \
  "$([ -n "${SECOND:-}" ] && echo 1 || echo 0)" "${SECOND:-none}"
if [ -n "${SECOND:-}" ]; then
  xdotool windowmove --sync "$SECOND" 480 160 >/dev/null 2>&1 || true
  sleep 0.5
  xdotool mousemove --sync --window "$SECOND" "$TAB1_X" "$TAB_ROW_Y" click 3
  sleep 1.0
  xdotool mousemove --sync --window "$SECOND" "$((TAB1_X + 50))" "$MENU_MOVE_MAIN_Y"
  sleep 0.3
  xdotool click 1
  sleep 2.5
  e2e_check "③迁回主窗口" "第二个窗口已关闭（只剩主窗口）" \
    "$([ "$(list_windows | wc -l | tr -d ' ')" = "1" ] && echo 1 || echo 0)" \
    "窗口：$(list_windows | tr '\n' ' ')"
  # 迁回的标签成为主窗口的活动标签（迁移语义：迁入窗口把该标签设为活动）——
  # 用第二个标签位置的强调条断言，比"是否有标签"更精确（非活动标签底色透明，
  # 在深色主题下与标签条底色无法区分）。
  e2e_check "③主窗口标签恢复" "第二个标签位置出现活动标签强调条" \
    "$(tab_active_at "$E2E_APP_WIN" "$TAB2_X" && echo 1 || echo 0)" \
    "$(tab_active_at "$E2E_APP_WIN" "$TAB2_X" && echo 活动标签 || echo 无活动标签)"
fi
win_check_shot "$E2E_APP_WIN" "09-after-move-back" "③迁回主窗口后"

e2e_log "-- ④ 最后一个窗口关闭 = 退出（沿用 Quit 确认）"
close_window "$E2E_APP_WIN"
sleep 1.5
e2e_check "④最后窗口关闭弹退出确认" "窗口仍在且弹出确认（不是直接退出）" \
  "$(win_dialog_open "$E2E_APP_WIN" "$E2E_DIALOG_BASELINE" && echo 1 || echo 0)" \
  "$(win_dialog_open "$E2E_APP_WIN" "$E2E_DIALOG_BASELINE" && echo 弹窗已打开 || echo 无弹窗)"
win_check_shot "$E2E_APP_WIN" "10-quit-confirm" "④退出确认弹窗"
center=$(win_confirm_center "$E2E_APP_WIN")
if [ -n "${center:-}" ]; then
  set -- $center
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$1" "$2"
  sleep 0.3
  xdotool click 1
fi
sleep 2.5
if kill -0 "$E2E_APP_PID" 2>/dev/null; then app_alive=1; else app_alive=0; fi
e2e_check "④确认后应用退出" "进程已结束" "$([ "$app_alive" = 0 ] && echo 1 || echo 0)" \
  "$([ "$app_alive" = 0 ] && echo 已退出 || echo 仍在运行)"

e2e_log "-- 应用日志（yshell::app）"
grep -a 'yshell::app' "$log_file" | tail -8 || true

e2e_summary
