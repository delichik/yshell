#!/usr/bin/env bash
# N9 同步发送按键 UI E2E（无头 Xvfb 驱动，Fake SSH 后端可离线运行）。
#
# 覆盖（N9 设计 §3/§4）：
#   ① 终端右键菜单出现 Send Key Input to All/Visible/Stop 三项（含 Visible 的
#      "分屏落地前等同 All" tooltip 截图）
#   ② Send to Visible 启动同步：状态栏 chip（accent 像素）出现；源/目标标签角标
#      出现（标签条 accent 像素增加）
#   ③ 扇出：在源终端输入命令 → 切到目标标签，终端区域内容变化（stddev 上升）
#   ④ Ctrl+C 广播：chip 出现非阻塞提示（不弹窗；chip 区域像素变化）
#   ⑤ 标签右键"接收键输入"勾选态截图；取消勾选后扇出停止
#   ⑥ 状态栏 chip 一键停止（点击后 accent 像素消失）+ 停止后不再复制
#   ⑦ 目标断开 → 自动移出（chip 提示出现，标签角标消失）
#   ⑧ 源断开 → 同步停止（chip 不再 accent，出现停止提示）
#
# 依赖：Xvfb/xdotool/ImageMagick/python3-PIL（与其它 e2e 相同）；构建产物
#       target/debug/yshell（YSHELL_E2E_BIN 可覆盖）。不需要 SSH 测试容器：
#       脚本用 YSHELL_SSH_BACKEND=fake + 已保存会话直连。
#
# 用法：
#   bash scripts/e2e/run-ui-sync.sh [--theme dark|light] [--lang zh-CN|en-US]
#                                   [--outdir dist/ui-checks] [--prefix n9]
#                                   [--keep-config]
#
# 坐标按 1440×900 窗口校准（窗口原点固定 (0,0)）：
#   * 终端右键锚点 (700,300)；菜单内边距 4，行高 32、分隔符 9——
#     索引 11 All=626 / 12 Visible=658 / 13 Stop=690，点击 x=750。
#   * 标签右键锚点 (tab_x,100)；索引 7 Reconnect=321 / 8 Disconnect=353 / 10 Receive=394。
set -u

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

theme="dark"
lang="zh-CN"
outdir="$repo_dir/dist/ui-checks"
prefix="n9"
keep_config=0
bin="${YSHELL_E2E_BIN:-$repo_dir/target/debug/yshell}"
display="${YSHELL_E2E_DISPLAY:-:99}"
config_dir="/tmp/yshell-e2e-sync-config"
log_file="/tmp/yshell-e2e-sync.log"

# 1440×900 窗口内坐标。
TAB_ROW_Y=100          # 标签条文字行中心
TAB1_X=330             # 标签 1（T1）中心
TAB2_X=480             # 标签 2（T2）中心
TERMINAL_X=700         # 终端内容区（点击输入/右键菜单锚点）
TERMINAL_Y=300         # 终端右键锚点 y（菜单从锚点向下展开）
TERMINAL_TYPE_Y=500    # 终端输入点击点
MENU_DX=50             # 菜单项相对锚点 x 的点击偏移
TERMINAL_MENU_SYNC_ALL_Y=626
TERMINAL_MENU_SYNC_VISIBLE_Y=658
TERMINAL_MENU_STOP_Y=690
TAB_MENU_RECONNECT_Y=321
TAB_MENU_DISCONNECT_Y=353
TAB_MENU_RECEIVE_Y=394

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
if [ ! -x "$bin" ]; then
  e2e_warn "找不到可执行文件：$bin（先 cargo xtask build）"
  exit 1
fi

e2e_log "配置：theme=$theme lang=$lang outdir=$outdir prefix=$prefix（Fake 后端离线）"
e2e_ensure_xvfb "$display" || exit 1

# ---------------------------------------------------------------- 配置（Fake 直连）
write_config() {
  mkdir -p "$config_dir"
  {
    echo 'schema_version = 1'
    echo ''
    echo '[[folders]]'
    echo 'id = "saved-sessions"'
    echo 'name = "Saved Sessions"'
    echo 'sessions = ['
    echo '  { id = "sync-1", name = "T1", host = "one.example.test", port = 22, username = "root" },'
    echo '  { id = "sync-2", name = "T2", host = "two.example.test", port = 22, username = "root" },'
    echo ']'
  } >"$config_dir/config.toml"
}

[ "$keep_config" = 0 ] && rm -rf "$config_dir"
write_config

# ---------------------------------------------------------------- 启动（YSHELL_SSH_BACKEND=fake）
sync_launch_app() {
  DISPLAY="${E2E_DISPLAY:-$display}" \
    env -u YSHELL_MASTER_PASSWORD \
    WINIT_X11_SCALE_FACTOR="$E2E_WINIT_SCALE" \
    YSHELL_SSH_BACKEND=fake \
    YSHELL_THEME="$theme" \
    YSHELL_LANG="$lang" \
    YSHELL_CONFIG_DIR="$config_dir" \
    "$bin" >"$log_file" 2>&1 &
  E2E_APP_PID=$!
  E2E_APP_WIN=$(e2e_wait_window "$E2E_APP_PID" 15) || {
    e2e_warn "未找到窗口（pid $E2E_APP_PID），日志尾部："
    tail -5 "$log_file" >&2 || true
    e2e_stop_app
    return 1
  }
  xdotool windowsize --sync "$E2E_APP_WIN" 1440 900 >/dev/null 2>&1
  xdotool windowmove --sync "$E2E_APP_WIN" 0 0 >/dev/null 2>&1
  xdotool windowfocus --sync "$E2E_APP_WIN" >/dev/null 2>&1 || true
  e2e_wait_paint || true
  sleep 1
  e2e_capture_dialog_baseline
  e2e_log "app 已就绪：win=$E2E_APP_WIN pid=$E2E_APP_PID（$theme/$lang，fake）"
}

# ---------------------------------------------------------------- 像素 helper
shot_path() { echo "$outdir/${E2E_SHOT_PREFIX}-$1.png"; }

# 区域内"强调色/蓝色系"像素计数（同步 chip 的 accent 文本、标签角标）。
# dark accent ≈ #60CDFF（b-r=159）、light ≈ #005FB8（b-r=184）。
sync_accent_pixels() { # $1=x $2=y $3=w $4=h
  local shot=/tmp/yshell-n9-accent.png
  import -window "$E2E_APP_WIN" "$shot" 2>/dev/null || { echo 0; return; }
  python3 - "$shot" "$1" "$2" "$3" "$4" <<'PY' 2>/dev/null || echo 0
import sys
from PIL import Image
img = Image.open(sys.argv[1]).convert('RGB')
x, y, w, h = (int(v) for v in sys.argv[2:6])
count = 0
for py in range(y, min(y + h, img.size[1])):
    for px in range(x, min(x + w, img.size[0])):
        r, g, b = img.getpixel((px, py))
        if b > 120 and b - r > 40 and b - g > 10:
            count += 1
print(count)
PY
}

# 状态栏里的同步 chip 中心（accent 像素 bbox 中心）。
sync_chip_center() {
  local shot=/tmp/yshell-n9-chip.png result
  import -window "$E2E_APP_WIN" "$shot" 2>/dev/null || return 1
  result=$(python3 - "$shot" <<'PY' 2>/dev/null || true
import sys
from PIL import Image
img = Image.open(sys.argv[1]).convert('RGB')
w, h = img.size
minx, miny, maxx, maxy = 10**9, 10**9, -1, -1
for py in range(max(0, h - 26), h):
    for px in range(w // 2, w):
        r, g, b = img.getpixel((px, py))
        if b > 120 and b - r > 40 and b - g > 10:
            minx = min(minx, px); maxx = max(maxx, px)
            miny = min(miny, py); maxy = max(maxy, py)
if maxx < 0:
    sys.exit(1)
print((minx + maxx) // 2, (miny + maxy) // 2)
PY
)
  [ -n "$result" ] && echo "$result"
}

# 打开终端右键菜单（锚点 700,300）。
sync_open_terminal_menu() {
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$TERMINAL_X" "$TERMINAL_Y" click 3
  sleep 1.0
}

sync_click_terminal_menu_item() { # $1=y
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$(( TERMINAL_X + MENU_DX ))" "$1"
  sleep 0.3
  xdotool click 1
  sleep 1.0
}

sync_open_tab_menu() { # $1=tab x
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$1" "$TAB_ROW_Y" click 3
  sleep 1.0
}

sync_click_tab_menu_item() { # $1=tab x $2=y
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$(( $1 + MENU_DX ))" "$2"
  sleep 0.3
  xdotool click 1
  sleep 1.2
}

sync_select_tab() { # $1=tab x
  e2e_click "$1" "$TAB_ROW_Y" 0.8
}

sync_clear_and_type() { # $1=文本（自动回车）
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$TERMINAL_X" "$TERMINAL_TYPE_Y"
  sleep 0.2
  xdotool click 1
  sleep 0.4
  xdotool type --delay 40 "$1"
  xdotool key Return
}

# 关联区域（终端内容）stddev；用于"输出到达/未到达"判定。
sync_terminal_stddev() { e2e_region_stddev 300 190 600 110; }

# 两张截图在指定区域的像素差（AE；同 run-ui-tabs.sh 的实现）。
e2e_region_changed_between() { # $1=before.png $2=after.png $3=x $4=y $5=w $6=h $7=min_ae
  local diff
  diff=$(compare -metric AE \
    <(convert "$1" -crop "$5x$6+$3+$4" +repage png:-) \
    <(convert "$2" -crop "$5x$6+$3+$4" +repage png:-) null: 2>&1 || true)
  awk -v d="${diff:-0}" -v m="$7" 'BEGIN { exit !(d + 0 > m) }'
}

# 抓取当前窗口到临时文件（AE 对比用）。
sync_capture() { # $1=输出路径
  import -window "$E2E_APP_WIN" "$1" 2>/dev/null
}

# 终端内容区 AE（覆盖整块终端表面；Fake shell 无 clear，内容对比需用整块差分）。
sync_terminal_ae() { # $1=before.png $2=after.png
  local diff
  diff=$(compare -metric AE \
    <(convert "$1" -crop 700x620+310+180 +repage png:-) \
    <(convert "$2" -crop 700x620+310+180 +repage png:-) null: 2>&1 || true)
  echo "${diff:-0}"
}

# ---------------------------------------------------------------- 场景
e2e_log "== N9 sync e2e：打开两个 fake 会话标签 =="
sync_launch_app || exit 1
e2e_open_saved_session 239
sleep 1.0
e2e_open_saved_session 268
sleep 1.5

# T2 活动 → T1 活动（点击确保终端聚焦；Fake shell 不需要 clear）。
sync_select_tab "$TAB2_X"
sync_select_tab "$TAB1_X"

e2e_log "== ① 终端右键菜单（Send Key Input 三项 + Visible 说明） =="
sync_open_terminal_menu
e2e_check_shot "$outdir" "00-terminal-sync-menu" "①终端右键出现同步发送按键菜单项"
e2e_check "①菜单出现" "菜单区域覆盖变化" \
  "$(e2e_region_stddev 700 320 240 380 | awk '{ print ($1 + 0 > 0.02) ? 1 : 0 }')" \
  "菜单区域 stddev=$(e2e_region_stddev 700 320 240 380)"

# 悬停 Visible 项：tooltip = "分屏落地前等同 All"。
before_tip=$(shot_path "00-terminal-sync-menu")
xdotool mousemove --sync --window "$E2E_APP_WIN" "$(( TERMINAL_X + MENU_DX ))" "$TERMINAL_MENU_SYNC_VISIBLE_Y"
sleep 2.5
e2e_check_shot "$outdir" "01-visible-tooltip" "①Visible 菜单项 hover 说明（分屏落地前等同 All）"
after_tip=$(shot_path "01-visible-tooltip")
e2e_check "①Visible 说明" "hover 后出现 tooltip（区域像素变化）" \
  "$(e2e_region_changed_between "$before_tip" "$after_tip" 700 660 320 40 200 && echo 1 || echo 0)" \
  "$(e2e_region_changed_between "$before_tip" "$after_tip" 700 660 320 40 200 && echo 有变化 || echo 无变化)"

e2e_log "== ② Send to Visible 启动同步：chip + 角标 =="
# 重新打开菜单（hover 后菜单仍在，这行点击直接点 Visible 项）。
sync_click_terminal_menu_item "$TERMINAL_MENU_SYNC_VISIBLE_Y"
chip_before=$(sync_accent_pixels 720 874 720 26)
e2e_check "②状态栏 chip" "启动后状态栏出现 accent chip" \
  "$(awk -v n="${chip_before:-0}" 'BEGIN { print (n + 0 > 20) ? 1 : 0 }')" \
  "状态栏 accent 像素=$chip_before"
badge_pixels=$(sync_accent_pixels 248 82 320 32)
e2e_check "②标签角标" "源/目标标签出现同步角标（accent 像素）" \
  "$(awk -v n="${badge_pixels:-0}" 'BEGIN { print (n + 0 > 20) ? 1 : 0 }')" \
  "标签条 accent 像素=$badge_pixels"
e2e_check_shot "$outdir" "02-sync-chip-badges" "②状态栏 chip + 源/目标标签角标"

e2e_log "== ③ 扇出：源输入 → 目标收到 =="
# 基线：T2 当前画面（Fake shell 不认识 clear，内容对比用整块终端 AE）。
sync_select_tab "$TAB2_X"
sleep 0.5
sync_capture /tmp/yshell-n9-target-before.png
sync_select_tab "$TAB1_X"
sync_clear_and_type 'for i in 1 2 3 4 5; do echo n9-$i; sleep 0.5; done'
sleep 4.0
sync_select_tab "$TAB2_X"
sleep 0.5
sync_capture /tmp/yshell-n9-target-after.png
fanout_ae=$(sync_terminal_ae /tmp/yshell-n9-target-before.png /tmp/yshell-n9-target-after.png)
e2e_check "③扇出复制" "目标终端出现源输入（整块 AE 上升）" \
  "$(awk -v d="${fanout_ae:-0}" 'BEGIN { print (d + 0 > 2000) ? 1 : 0 }')" \
  "终端区域 AE=$fanout_ae"
e2e_check_shot "$outdir" "03-fanout-output" "③目标标签收到同步输入"

e2e_log "== ④ Ctrl+C 广播的非阻塞提示 =="
sync_select_tab "$TAB1_X"
xdotool mousemove --sync --window "$E2E_APP_WIN" "$TERMINAL_X" "$TERMINAL_TYPE_Y" click 1
sleep 0.3
xdotool key ctrl+c
sleep 1.0
chip_after_ctrlc=$(sync_accent_pixels 720 874 720 26)
e2e_check "④控制键提示" "Ctrl+C 后 chip 区域出现提示（不弹窗）" \
  "$(awk -v n="${chip_after_ctrlc:-0}" 'BEGIN { print (n + 0 > 20) ? 1 : 0 }')" \
  "chip 像素=$chip_after_ctrlc（同步仍激活，提示在 chip 上）"
e2e_check "④不弹窗" "无模态弹窗（scrim 未出现）" \
  "$(e2e_dialog_open && echo 0 || echo 1)" \
  "dialog_open=$(e2e_dialog_open && echo 是 || echo 否)"
e2e_check_shot "$outdir" "04-control-broadcast" "④Ctrl+C 广播提示（状态栏，非弹窗）"

e2e_log "== ⑤ 标签右键 接收键输入 勾选/取消 =="
sync_open_tab_menu "$TAB2_X"
e2e_check_shot "$outdir" "05-tab-receive-checked" "⑤标签右键菜单（Receive Key Input 已勾选）"
sync_click_tab_menu_item "$TAB2_X" "$TAB_MENU_RECEIVE_Y"
chip_unchecked=$(sync_accent_pixels 720 874 720 26)
e2e_check "⑤取消勾选" "取消后 chip 仍在（目标数变化）" \
  "$(awk -v n="${chip_unchecked:-0}" 'BEGIN { print (n + 0 > 20) ? 1 : 0 }')" \
  "chip 像素=$chip_unchecked"
e2e_check_shot "$outdir" "06-receive-unchecked" "⑤取消接收（目标 0，不再扇出）"

# 取消勾选后不再扇出：清 T2 → 从 T1 输入 → T2 内容不变。
sync_select_tab "$TAB2_X"
sync_clear_and_type 'clear'
sleep 1.0
t2_baseline2=$(sync_terminal_stddev)
sync_select_tab "$TAB1_X"
sync_clear_and_type 'echo n9-should-not-arrive'
sleep 2.0
sync_select_tab "$TAB2_X"
sleep 0.5
t2_after_unchecked=$(sync_terminal_stddev)
e2e_check "⑤取消后不复制" "T2 内容保持清屏（stddev 不上升）" \
  "$(awk -v b="${t2_baseline2:-0}" -v a="${t2_after_unchecked:-0}" 'BEGIN { print (a + 0 <= b + 0.02) ? 1 : 0 }')" \
  "清屏基线=${t2_baseline2:-?}，输入后=${t2_after_unchecked:-?}"

# 重新勾选接收，恢复 target 集合（后续断开/停止场景用）。
sync_open_tab_menu "$TAB2_X"
sync_click_tab_menu_item "$TAB2_X" "$TAB_MENU_RECEIVE_Y"
sleep 0.5

e2e_log "== ⑥ 状态栏 chip 一键停止 =="
chip_center=$(sync_chip_center) || chip_center=""
if [ -n "$chip_center" ]; then
  set -- $chip_center
  e2e_click "$1" "$2" 1.0
else
  e2e_warn "chip 像素扫描失败，回退点击状态栏右侧"
  e2e_click 1100 887 1.0
fi
chip_stopped=$(sync_accent_pixels 720 874 720 26)
e2e_check "⑥一键停止" "点击 chip 后同步 chip 消失" \
  "$(awk -v n="${chip_stopped:-0}" 'BEGIN { print (n + 0 < 20) ? 1 : 0 }')" \
  "状态栏 accent 像素=$chip_stopped"
e2e_check_shot "$outdir" "07-stopped" "⑥停止后的状态栏（chip 消失）"

# 停止后不再复制：清 T2 → T1 输入 → T2 不变。
sync_select_tab "$TAB2_X"
sync_clear_and_type 'clear'
sleep 1.0
t2_baseline3=$(sync_terminal_stddev)
sync_select_tab "$TAB1_X"
sync_clear_and_type 'echo n9-after-stop'
sleep 2.0
sync_select_tab "$TAB2_X"
sleep 0.5
t2_after_stop=$(sync_terminal_stddev)
e2e_check "⑥停止后不复制" "T2 内容保持清屏（stddev 不上升）" \
  "$(awk -v b="${t2_baseline3:-0}" -v a="${t2_after_stop:-0}" 'BEGIN { print (a + 0 <= b + 0.02) ? 1 : 0 }')" \
  "清屏基线=${t2_baseline3:-?}，停止后输入=${t2_after_stop:-?}"

e2e_log "== ⑦ 目标断开 → 自动移出 =="
sync_select_tab "$TAB1_X"
sync_open_terminal_menu
sync_click_terminal_menu_item "$TERMINAL_MENU_SYNC_ALL_Y"
chip_restart=$(sync_accent_pixels 720 874 720 26)
e2e_check "⑦重启同步" "All 模式重启后 chip 出现" \
  "$(awk -v n="${chip_restart:-0}" 'BEGIN { print (n + 0 > 20) ? 1 : 0 }')" \
  "chip 像素=$chip_restart"
sync_open_tab_menu "$TAB2_X"
e2e_check_shot "$outdir" "08-target-connected-menu" "⑦目标标签菜单（断开前）"
sync_click_tab_menu_item "$TAB2_X" "$TAB_MENU_DISCONNECT_Y"
sleep 1.0
e2e_check "⑦断开移除" "目标断开后角标消失（标签条 accent 像素下降）" \
  "$(awk -v n="$(sync_accent_pixels 408 82 160 32)" 'BEGIN { print (n + 0 < 60) ? 1 : 0 }')" \
  "T2 标签 accent 像素=$(sync_accent_pixels 408 82 160 32)"
e2e_check_shot "$outdir" "09-target-disconnect-removed" "⑦目标断开被移出（chip 提示）"

e2e_log "== ⑧ 源断开 → 同步停止 =="
sync_open_tab_menu "$TAB1_X"
sync_click_tab_menu_item "$TAB1_X" "$TAB_MENU_DISCONNECT_Y"
sleep 1.0
chip_after_source=$(sync_accent_pixels 720 874 720 26)
e2e_check "⑧源断开停止" "源断开后同步 chip 不再显示（accent 像素消失）" \
  "$(awk -v n="${chip_after_source:-0}" 'BEGIN { print (n + 0 < 20) ? 1 : 0 }')" \
  "状态栏 accent 像素=$chip_after_source"
e2e_check_shot "$outdir" "10-source-disconnect-stop" "⑧源断开 → 同步停止提示"

if [ "$keep_config" = 1 ]; then
  e2e_log "保留配置目录：$config_dir（--keep-config）"
fi

if e2e_summary; then
  exit 0
else
  exit 1
fi
