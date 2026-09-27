#!/usr/bin/env bash
# N0 多标签 + 批量关闭 UI E2E（无头 Xvfb 驱动）。
#
# 覆盖（N0 设计 §8）：
#   ① 打开两个标签并切换（标签活动强调条像素断言）
#   ② 后台标签持续输出（清屏基线 → 输入 ticker → 切走 → 切回，区域内容断言）
#   ③ 标签右键菜单全项 + disabled 项（含 hover tooltip 截图）
#   ④ 批量关闭确认弹窗（数量/活动连接数；Enter 不触发 = 安全默认焦点）
#   ⑤ 关闭活动标签后的相邻选择（右侧→左侧）
#   ⑥ 8+ 标签横向滚动（滚轮；标签条像素差断言）
#
# 依赖：与 scripts/e2e/run-ui-ssh.sh 相同（Xvfb/xdotool/ImageMagick + 测试容器 +
#       cargo xtask build 产生的 target/debug/yshell）。
#
# 用法：
#   bash scripts/e2e/run-ui-tabs.sh [--theme dark|light] [--lang zh-CN|en-US]
#                                   [--outdir dist/ui-checks] [--prefix n0] [--keep-config]
#
# 坐标按 1440×900 窗口校准（标签条 y≈100，标签宽 156 + 4 间距；见截图证据）。
set -u

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

theme="dark"
lang="zh-CN"
outdir="$repo_dir/dist/ui-checks"
prefix="n0"
keep_config=0
bin="${YSHELL_E2E_BIN:-$repo_dir/target/debug/yshell}"
display="${YSHELL_E2E_DISPLAY:-:99}"
config_dir="/tmp/yshell-e2e-tabs-config"
log_file="/tmp/yshell-e2e-tabs.log"
ssh_key="${HOME}/.ssh/yshell_test_ed25519"
container="${YSHELL_TEST_SSH_CONTAINER:-yshell-test-ssh}"
ssh_port="${YSHELL_TEST_SSH_PORT:-2222}"

# 1440×900 窗口内坐标（标签条/菜单/弹窗）。
TAB_ROW_Y=100          # 标签条文字行中心
TAB_ACCENT_Y=112       # 活动标签底部强调条
TAB1_X=330             # 标签 1 中心（宽 156：248..404）
TAB2_X=480             # 标签 2 中心（408..564）
PLUS_X=1042            # “+”按钮中心
TERMINAL_X=700         # 终端内容区
TERMINAL_Y=500
MENU_ITEM_DY=32        # 菜单行高
MENU_ITEM0_Y=120       # 锚点 y=100、内边距 4 后第一行中心
MENU_ANCHOR_DX=50      # 菜单项相对锚点 x 的点击偏移

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
container_status=$(bash "$repo_dir/scripts/test-ssh/run.sh" status 2>&1 || true)
if ! grep -q 'Up ' <<<"$container_status"; then
  e2e_warn "测试容器未运行（$container）：先执行 bash scripts/test-ssh/run.sh up"
  exit 1
fi

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
    for i in 1 2 3 4 5 6 7 8 9 10; do
      echo "  { id = \"tabs-$i\", name = \"T$i\", host = \"127.0.0.1\", port = $ssh_port, username = \"root\", auth_profile_id = \"auth-key\", host_key_policy = \"accept_any_for_testing\" },"
    done
    echo ']'
  } >"$config_dir/config.toml"
}

[ "$keep_config" = 0 ] && rm -rf "$config_dir"
write_config

# ---------------------------------------------------------------- 像素/交互 helper
# 活动标签：底部强调条（accent 蓝）。$1 = 标签中心 x。
e2e_tab_active_at() {
  local x="$1" r g b
  read -r r g b <<<"$(e2e_pixel_rgb "$x" "$TAB_ACCENT_Y")" || return 1
  [ "${b:-0}" -gt 120 ] && [ "${b:-0}" -gt $(( ${r:-0} + 40 )) ]
}

# 打开标签菜单：$1 = 标签中心 x（右键，菜单锚点在点击处）。
e2e_open_tab_menu() { # $1=x
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$1" "$TAB_ROW_Y" click 3
  sleep 1.0
}

e2e_click_tab_menu_item() { # $1=锚点 x $2=index
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$(( $1 + MENU_ANCHOR_DX ))" "$(( MENU_ITEM0_Y + $2 * MENU_ITEM_DY ))"
  sleep 0.3
  xdotool click 1
  sleep 1.2
}

# 标签条空位 = 背景色（dark ≈ 32 / light ≈ 243）；用于确认标签已被关闭。
e2e_tab_bar_background_at() {
  local gray
  gray=$(e2e_pixel_gray "$1" "$TAB_ROW_Y")
  case "$E2E_THEME" in
  light) [ "${gray:-0}" -ge 200 ] ;;
  *) [ "${gray:-0}" -le 90 ] ;;
  esac
}

# 关闭当前菜单（点终端空白处）。
e2e_dismiss_menus() {
  xdotool mousemove --sync --window "$E2E_APP_WIN" 700 600 click 1
  sleep 0.5
}

# 确认弹窗：扫描强调色主按钮中心（dark/light 通用）；失败时回退校准值。
e2e_dialog_confirm_center() {
  local shot=/tmp/yshell-tabs-dialog.png result
  import -window "$E2E_APP_WIN" "$shot" 2>/dev/null || return 1
  result=$(python3 - "$shot" <<'PY' 2>/dev/null || true
import sys
from PIL import Image
img = Image.open(sys.argv[1]).convert('RGB')
minx, miny, maxx, maxy = 10**9, 10**9, -1, -1
for y in range(200, 700):
    for x in range(400, 1040):
        r, g, b = img.getpixel((x, y))
        if b > 140 and b - r > 50 and g > 80:
            minx, miny = min(minx, x), min(miny, y)
            maxx, maxy = max(maxx, x), max(maxy, y)
if maxx < 0:
    sys.exit(1)
print((minx + maxx) // 2, (miny + maxy) // 2)
PY
)
  if [ -n "$result" ]; then
    echo "$result"
  else
    echo "904 482"
  fi
}

e2e_click_dialog_confirm() {
  local center x y
  center=$(e2e_dialog_confirm_center) || return 1
  set -- $center
  x="$1"; y="$2"
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$x" "$y"
  sleep 0.2
  xdotool click 1
  sleep 1.2
}

e2e_click_dialog_cancel() {
  local center x y
  center=$(e2e_dialog_confirm_center) || return 1
  set -- $center
  x=$(( $1 - 70 )); y="$2"
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$x" "$y"
  sleep 0.2
  xdotool click 1
  sleep 1.0
}

# 在活动终端的指定位置输入：$1 = 文本（不自动回车）。
e2e_terminal_type() {
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$TERMINAL_X" "$TERMINAL_Y"
  sleep 0.2
  xdotool click 1
  sleep 0.4
  xdotool type --delay 40 "$1"
}

# 两张截图在给定区域的像素差（AE > $5 视为变化）。
e2e_region_changed_between() { # $1=before.png $2=after.png $3=x $4=y $5=w $6=h $7=min_ae
  local diff
  diff=$(compare -metric AE \
    <(convert "$1" -crop "$5x$6+$3+$4" +repage png:-) \
    <(convert "$2" -crop "$5x$6+$3+$4" +repage png:-) null: 2>&1 || true)
  awk -v d="${diff:-0}" -v m="$7" 'BEGIN { exit !(d + 0 > m) }'
}

shot_path() { echo "$outdir/${E2E_SHOT_PREFIX}-$1.png"; }

# ---------------------------------------------------------------- 场景
e2e_log "== N0 tabs e2e：打开两个标签并切换 =="
e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$E2E_DISPLAY" || exit 1

e2e_open_saved_session 239
sleep 1.0
e2e_open_saved_session 268
sleep 2.0

e2e_check "①标签活动态" "第二个标签为活动（强调条）" \
  "$(e2e_tab_active_at "$TAB2_X" && echo 1 || echo 0)" \
  "$(e2e_tab_active_at "$TAB2_X" && echo 是 || echo 否)"
e2e_check "①标签活动态" "第一个标签非活动" \
  "$(e2e_tab_active_at "$TAB1_X" && echo 0 || echo 1)" \
  "$(e2e_tab_active_at "$TAB1_X" && echo 是 || echo 否)"
e2e_check_shot "$outdir" "01-two-tabs" "①双标签截图"

e2e_click "$TAB1_X" "$TAB_ROW_Y" 1.0
e2e_check "①切换标签" "第一个标签变为活动" \
  "$(e2e_tab_active_at "$TAB1_X" && echo 1 || echo 0)" \
  "$(e2e_tab_active_at "$TAB1_X" && echo 是 || echo 否)"
e2e_check "①切换标签" "第二个标签变为非活动" \
  "$(e2e_tab_active_at "$TAB2_X" && echo 0 || echo 1)" \
  "$(e2e_tab_active_at "$TAB2_X" && echo 是 || echo 否)"
e2e_check_shot "$outdir" "02-tab-switch" "①切换到第一个标签"

e2e_log "== ②后台标签持续输出 =="
e2e_click "$TAB2_X" "$TAB_ROW_Y" 0.8
e2e_terminal_type 'clear'
xdotool key Return
sleep 1.2
baseline_sd=$(e2e_region_stddev 300 190 600 110)
e2e_terminal_type 'for i in 1 2 3 4 5; do echo bg-tick-$i; sleep 0.7; done'
xdotool key Return
sleep 0.8
e2e_click "$TAB1_X" "$TAB_ROW_Y" 0.5   # 切走：输出发生在后台
sleep 5.0
e2e_click "$TAB2_X" "$TAB_ROW_Y" 0.8   # 切回
after_sd=$(e2e_region_stddev 300 190 600 110)
e2e_check "②后台输出" "切回后终端出现后台命令输出（stddev 上升）" \
  "$(awk -v b="${baseline_sd:-0}" -v a="${after_sd:-0}" 'BEGIN { print (a + 0 > 0.05 && a + 0 > b + 0.03) ? 1 : 0 }')" \
  "清屏基线 stddev=${baseline_sd:-?}，切回后 stddev=${after_sd:-?}"
e2e_check_shot "$outdir" "03-background-output" "②后台标签输出截图"

e2e_log "== ③标签右键菜单（disabled/tooltip） =="
e2e_open_tab_menu "$TAB1_X"
e2e_check_shot "$outdir" "04-tab-context-menu" "③标签右键菜单（全项；左侧/断开项 disabled）"
e2e_check "③标签菜单" "右键出现标签菜单（区域覆盖变化）" \
  "$(e2e_region_changed_between "$(shot_path 03-background-output)" "$(shot_path 04-tab-context-menu)" "$TAB1_X" 100 240 208 200 && echo 1 || echo 0)" \
  "$(e2e_region_changed_between "$(shot_path 03-background-output)" "$(shot_path 04-tab-context-menu)" "$TAB1_X" 100 240 208 200 && echo 出现 || echo 未出现)"

before_tip=$(shot_path "04-tab-context-menu")
xdotool mousemove --sync --window "$E2E_APP_WIN" "$(( TAB1_X + MENU_ANCHOR_DX ))" "$(( MENU_ITEM0_Y + 5 * MENU_ITEM_DY ))"
sleep 2.5
e2e_check_shot "$outdir" "05-tab-menu-tooltip" "③disabled 项 hover tooltip"
after_tip=$(shot_path "05-tab-menu-tooltip")
e2e_check "③tooltip" "disabled 项 hover 显示提示（区域像素变化）" \
  "$(e2e_region_changed_between "$before_tip" "$after_tip" "$TAB1_X" 292 300 40 30 && echo 1 || echo 0)" \
  "$(e2e_region_changed_between "$before_tip" "$after_tip" "$TAB1_X" 292 300 40 30 && echo 有变化 || echo 无变化)"
e2e_dismiss_menus

e2e_log "== ④批量关闭确认（数量/活动连接数） =="
e2e_open_tab_menu "$TAB1_X"
e2e_click_tab_menu_item "$TAB1_X" 1   # Close Others
e2e_check "④批量关闭" "出现关闭确认弹窗" \
  "$(e2e_dialog_open && echo 1 || echo 0)" \
  "$(e2e_dialog_open && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "06-batch-close-confirm" "④批量关闭确认弹窗"
xdotool key Return
sleep 0.8
e2e_check "④安全默认" "Enter 不触发关闭（弹窗仍可见）" \
  "$(e2e_dialog_open && echo 1 || echo 0)" \
  "$(e2e_dialog_open && echo 仍可见 || echo 已关闭)"
e2e_click_dialog_cancel
e2e_check "④取消关闭" "取消后弹窗关闭且标签保留" \
  "$([ "$(e2e_dialog_open && echo 1 || echo 0)" = 0 ] && echo 1 || echo 0)" \
  "dialog_open=$(e2e_dialog_open && echo 1 || echo 0)"

e2e_log "== ⑤关闭活动标签后的相邻选择 =="
e2e_open_tab_menu "$TAB2_X"
e2e_click_tab_menu_item "$TAB2_X" 0   # Close（活动标签 = T2）
e2e_check "⑤单关确认" "出现单标签关闭确认弹窗" \
  "$(e2e_dialog_open && echo 1 || echo 0)" \
  "$(e2e_dialog_open && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "07-single-close-confirm" "⑤单关确认（显示会话名）"
e2e_click_dialog_confirm
e2e_check "⑤相邻选择" "活动标签回到左侧相邻标签" \
  "$(e2e_tab_active_at "$TAB1_X" && echo 1 || echo 0)" \
  "$(e2e_tab_active_at "$TAB1_X" && echo 是 || echo 否)"
e2e_check "⑤相邻选择" "原第二个标签已关闭" \
  "$(e2e_tab_bar_background_at "$TAB2_X" && echo 1 || echo 0)" \
  "空位背景=$(e2e_tab_bar_background_at "$TAB2_X" && echo 是 || echo 否)"
e2e_check_shot "$outdir" "08-neighbor-selected" "⑤关闭活动标签后的相邻选择"

e2e_log "== ⑥8+ 标签横向滚动 =="
# N2：`+` 按钮改为"按 ui.new_tab_mode 打开 Quick Connect 页 / Session Editor"，
# 不再新建草稿终端标签；这里改用会话树打开 T3..T9（标签标题各不相同，
# 保证横向滚动的像素差断言有效）补足 8+ 标签。
for row in 297 326 355 384 413 442 471; do
  e2e_open_saved_session "$row"
done
sleep 1.0
e2e_check "⑥多标签" "标签条出现 8+ 标签" \
  "$(awk -v s="$(e2e_region_stddev 260 90 600 30)" 'BEGIN { exit !((s + 0) > 0.05) }' && echo 1 || echo 0)" \
  "tab-strip stddev=$(e2e_region_stddev 260 90 600 30)"
e2e_check_shot "$outdir" "09-many-tabs" "⑥8+ 标签（首屏）"

# 滚轮在无头 Xvfb 下不生效（最小 Flickable 应用复现：AE=0），改用标签条支持的
# "按住拖动"（`mouse-drag-pan-enabled`）验证横向滚动。
xdotool mousemove --sync --window "$E2E_APP_WIN" 950 "$TAB_ROW_Y"
sleep 0.3
xdotool mousedown 1
sleep 0.2
for x in 900 850 800 750 700 650 600 550 500 450 400; do
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$x" "$TAB_ROW_Y"
  sleep 0.08
done
sleep 0.2
xdotool mouseup 1
sleep 1.0
e2e_check_shot "$outdir" "10-tabs-scrolled" "⑥拖动横向滚动后"
e2e_check "⑥横向滚动" "拖动后标签条内容变化" \
  "$(e2e_region_changed_between "$(shot_path 09-many-tabs)" "$(shot_path 10-tabs-scrolled)" 240 82 850 36 500 && echo 1 || echo 0)" \
  "$(e2e_region_changed_between "$(shot_path 09-many-tabs)" "$(shot_path 10-tabs-scrolled)" 240 82 850 36 500 && echo 已滚动 || echo 未变化)"

if [ "$keep_config" = 1 ]; then
  e2e_log "保留配置目录：$config_dir（--keep-config）"
fi

if e2e_summary; then
  exit 0
else
  exit 1
fi
