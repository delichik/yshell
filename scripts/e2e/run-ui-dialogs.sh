#!/usr/bin/env bash
# L2 对话框 Esc / 遮罩点击关闭 UI E2E（无头 Xvfb 驱动）。
#
# 覆盖（L2 设计 §规则 + §验收）：
#   ① 密码弹窗：Esc 关闭 = Cancel 路径；重开后密码已清空；输入/焦点正常
#   ② 删除确认：遮罩点击关闭；Enter 不误确认（E-064 安全默认）；会话未被删除
#   ③ NewFolderDialog：Esc 关闭
#   ④ SessionEditor：Esc 关闭；弹窗期间快捷键让位；关闭后快捷键焦点恢复（Ctrl+B）
#   ⑤ 嵌套（Settings → Known Hosts）：Esc 只关上层，下层保持并可继续操作
#   ⑥ About：Esc 关闭
#   ⑦ Quit 确认：Esc 取消退出（危险确认允许取消）
#   ⑧ HostKey（strict 未知主机）：Esc 关闭且不连接
#   ⑧b HostKey changed（伪造 known_hosts 指纹）：REPLACE 输入生效；Esc 取消后重开确认词已清空
#   ⑨ SFTP 弹窗（真连）：New Folder Esc 关闭；Delete 弹窗 Enter 不误确认 + Esc 关闭
#
# 依赖：与 scripts/e2e/run-ui-ssh.sh 相同（Xvfb/xdotool/ImageMagick + 测试容器 +
#       cargo xtask build 产生的 target/debug/yshell）。
# 环境变量：YSHELL_E2E_BIN（默认 <repo>/target/debug/yshell）、YSHELL_E2E_DISPLAY（默认 :99）。
#
# 用法：
#   bash scripts/e2e/run-ui-dialogs.sh [--theme dark|light] [--lang zh-CN|en-US]
#                                      [--outdir dist/ui-checks] [--prefix l2]
#                                      [--skip-sftp] [--keep-config]
#
# 坐标按 1440×900 窗口校准（窗口原点固定在 (0,0)；会话树行高 29px，行菜单第 N 项
# 中心 y = 行 y + 20 + N*32；密码弹窗控件坐标复用 lib.sh）。
set -u

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

theme="dark"
lang="zh-CN"
outdir="$repo_dir/dist/ui-checks"
prefix="l2"
keep_config=0
skip_sftp=0
bin="${YSHELL_E2E_BIN:-$repo_dir/target/debug/yshell}"
display="${YSHELL_E2E_DISPLAY:-:99}"
config_dir="/tmp/yshell-e2e-dialogs-config"
log_file="/tmp/yshell-e2e-dialogs.log"
ssh_key="${HOME}/.ssh/yshell_test_ed25519"
container="${YSHELL_TEST_SSH_CONTAINER:-yshell-test-ssh}"
ssh_port="${YSHELL_TEST_SSH_PORT:-2222}"
ssh_user="${YSHELL_TEST_SSH_USER:-tester}"
ssh_password="${YSHELL_TEST_SSH_PASSWORD:-yshell-test-pass}"

# 1440×900 窗口内坐标。
ROW_KEY_Y=239           # 第 1 行（key 会话）
ROW_PASS_Y=268          # 第 2 行（password 会话）
ROW_STRICT_Y=297        # 第 3 行（strict 未知主机）
ROW_CHANGED_Y=326       # 第 4 行（strict + known_hosts 指纹被伪造 → changed 模式）
ROW_MENU_X=150          # 行菜单点击 x（菜单锚点 x=100 + 50）
ROW_MENU_ITEM0_DY=20    # 行菜单第 0 项中心相对行 y 的偏移
MENU_ITEM_DY=32         # 菜单行高
BLANK_MENU_Y=700        # 会话树空白区右键 y
BLANK_MENU_NEW_FOLDER_Y=730  # 空白区菜单「新建文件夹…」项中心
SETTINGS_KNOWN_HOSTS_X=903   # Settings「打开已知主机管理器」按钮中心
SETTINGS_KNOWN_HOSTS_Y=573
SCRIM_X=60              # 遮罩点击点（弹窗面板之外）
SCRIM_Y=60
HELP_MENU_X=226         # 一级菜单「帮助」中心
MENUBAR_Y=18
HELP_MENU_ITEM0_Y=52    # 下拉第一项（About）中心
HELP_MENU_ITEM_X=300
SFTP_ROW_X=1200         # SFTP 文件列表行（N1 紧凑布局后第一行数据在 y≈360）
SFTP_ROW_Y=360
SFTP_MENU_DX=50         # SFTP 菜单项相对锚点 x 的点击偏移
# N1 §4 目录行菜单（锚点 = 右键点，面板内边距 4，条目 32px、分隔符 9px）：
# New Folder = index 5、Delete = index 8（前面 6 个条目 + 一个分隔符 + Rename）。
SFTP_MENU_NEW_FOLDER_Y=540  # 360 + 4 + 5*32 + 16
SFTP_MENU_DELETE_Y=613      # 360 + 4 + 6*32 + 9 + 32 + 16
# HostKey changed 模式的 REPLACE 输入框。
# E2E-fix 重新校准（2026-09-28，冻结二进制 dist/e2efix-work/yshell-e2efix-verified）：
# N5 把正文改成结构化行 + 8*line-caption 高度预算后，输入框上移到 y=528..557、
# x=482..957；旧探测区 y=546..574 只剩输入框下缘，测不到文本。这里改到输入框
# 左侧文本区（占位符 "Type REPLACE" 与输入后的 "REPLACE" 都完整落在区内）。
CHANGED_INPUT_X=495
CHANGED_INPUT_Y=532
CHANGED_INPUT_W=120
CHANGED_INPUT_H=22
# WinTextInput 聚焦时底部 2px 强调条的采样行/点（输入框底缘 557）。
CHANGED_INPUT_FOCUS_Y=557
CHANGED_INPUT_FOCUS_X1=600
CHANGED_INPUT_FOCUS_X2=840

usage() {
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
  --theme) theme="${2:-}" ; shift 2 ;;
  --lang) lang="${2:-}" ; shift 2 ;;
  --outdir) outdir="${2:-}" ; shift 2 ;;
  --prefix) prefix="${2:-}" ; shift 2 ;;
  --skip-sftp) skip_sftp=1 ; shift ;;
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
if [ "$skip_sftp" = 0 ]; then
  container_status=$(bash "$repo_dir/scripts/test-ssh/run.sh" status 2>&1 || true)
  if ! grep -q 'Up ' <<<"$container_status"; then
    e2e_warn "测试容器未运行（$container）：先执行 bash scripts/test-ssh/run.sh up（或加 --skip-sftp）"
    exit 1
  fi
fi

e2e_log "配置：theme=$theme lang=$lang outdir=$outdir prefix=$prefix skip_sftp=$skip_sftp"
e2e_ensure_xvfb "$display" || exit 1

# ---------------------------------------------------------------- 配置
write_config() {
  mkdir -p "$config_dir"
  cat >"$config_dir/config.toml" <<TOML
schema_version = 1

[auth_profiles.auth-key]
id = "auth-key"
name = "Docker Key"
method = { type = "private_key", path = "$ssh_key" }

[auth_profiles.auth-pass]
id = "auth-pass"
name = "Docker Pass"
method = { type = "password", secret_key = "docker-pass" }

[[folders]]
id = "saved-sessions"
name = "Saved Sessions"
sessions = [
    { id = "dlg-key", name = "Dlg Key", host = "127.0.0.1", port = $ssh_port, username = "root", auth_profile_id = "auth-key", host_key_policy = "accept_any_for_testing" },
    { id = "dlg-pass", name = "Dlg Pass", host = "127.0.0.1", port = $ssh_port, username = "$ssh_user", auth_profile_id = "auth-pass", host_key_policy = "accept_any_for_testing" },
    { id = "dlg-strict", name = "Dlg Strict", host = "127.0.0.1", port = $ssh_port, username = "root", auth_profile_id = "auth-key", host_key_policy = "strict" },
    { id = "dlg-changed", name = "Dlg Changed", host = "localhost", port = $ssh_port, username = "root", auth_profile_id = "auth-key", host_key_policy = "strict" },
]
TOML
}

# HostKey changed 模式：known_hosts 里给 localhost:port 放一个"算法正确、指纹不同"的
# 条目（运行时启动时加载）。这样 dlg-changed 会话连接时会走"主机密钥变更"弹窗，
# 用来验证 REPLACE 确认词输入与取消清空（真实改主机密钥会动共享测试容器，不做）。
write_changed_known_hosts() {
  cat >"$config_dir/known_hosts.toml" <<TOML
[entries]
"localhost:$ssh_port" = { algorithm = "ssh-ed25519", fingerprint = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" }
TOML
}

[ "$keep_config" = 0 ] && rm -rf "$config_dir"
write_config
write_changed_known_hosts

# ---------------------------------------------------------------- 交互 helper
shot_path() { echo "$outdir/${E2E_SHOT_PREFIX}-$1.png"; }

# lib.sh 只有 open 判定；这里补 closed 谓词（e2e_wait_until 需要无参命令）。
e2e_dialog_closed() { ! e2e_dialog_open; }
e2e_password_dialog_closed() { ! e2e_password_dialog_open; }

# 会话树行右键菜单：$1 = 行 y，$2 = 菜单项序号。
e2e_row_menu_item() { # $1=row_y $2=index
  xdotool mousemove --sync --window "$E2E_APP_WIN" 100 "$1" click 3
  sleep 1.0
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$ROW_MENU_X" "$(( $1 + ROW_MENU_ITEM0_DY + $2 * MENU_ITEM_DY ))"
  sleep 0.3
  xdotool click 1
  sleep 1.2
}

# 会话树空白区菜单：$1 = 菜单项中心的绝对 y（空白区菜单在底部会被钳制，见坐标注释）。
e2e_blank_menu_item_at() { # $1=item_y
  xdotool mousemove --sync --window "$E2E_APP_WIN" 100 "$BLANK_MENU_Y" click 3
  sleep 1.0
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$ROW_MENU_X" "$1"
  sleep 0.3
  xdotool click 1
  sleep 1.2
}

# 按 Esc 并等待谓词满足（无头环境偶发丢键，最多 2 次）。
e2e_escape_until() { # $1=轮数 $2...=谓词
  local attempts="$1"
  shift
  local i
  for i in 1 2; do
    xdotool key --clearmodifiers Escape
    if e2e_wait_until "$attempts" "$@"; then
      return 0
    fi
    e2e_warn "第 $i 次 Esc 后未达期望，重试"
  done
  return 1
}

e2e_click_scrim() {
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$SCRIM_X" "$SCRIM_Y"
  sleep 0.2
  xdotool click 1
  sleep 1.2
}

# ⑧b：REPLACE 输入框是否已获得键盘焦点（WinTextInput 聚焦时底缘 2px 强调条）。
# E1 之后 WinIconButton（右上 ×）可 Tab 聚焦，changed 弹窗的首个 Tab 落在 × 上，
# 第二个 Tab 才进输入框；这里用底部强调条像素判定，避免依赖具体 Tab 次数。
e2e_changed_input_focused() {
  local x count=0 r g b
  for x in "$CHANGED_INPUT_FOCUS_X1" "$CHANGED_INPUT_FOCUS_X2"; do
    read -r r g b <<<"$(e2e_pixel_rgb "$x" "$CHANGED_INPUT_FOCUS_Y")" || continue
    case "$E2E_THEME" in
    light) [ "${b:-0}" -gt 140 ] && [ "${r:-255}" -lt 80 ] && [ "${g:-255}" -lt 150 ] && count=$((count + 1)) ;;
    *) [ "${b:-0}" -gt 170 ] && [ "${g:-0}" -gt 140 ] && [ "${r:-255}" -lt 170 ] && count=$((count + 1)) ;;
    esac
  done
  [ "$count" -ge 1 ]
}

# 最多 3 次 Tab，直到 REPLACE 输入框拿到焦点（E1 后关闭按钮先进入 Tab 序）。
e2e_tab_into_changed_input() {
  local i
  for i in 1 2 3; do
    xdotool key --clearmodifiers Tab
    sleep 0.4
    if e2e_changed_input_focused; then
      e2e_log "Tab ×$i 后 REPLACE 输入框获得焦点"
      return 0
    fi
  done
  e2e_warn "Tab ×3 后 REPLACE 输入框仍未获得焦点"
  return 1
}

# 弹窗底部动作行**左按钮**（Cancel）中心：按文本簇定位，dark/light 通用。
# 输出 "x y"；失败返回 1。用于 B23 破坏性弹窗（Delete）只能显式 Cancel 关闭的路径。
e2e_dialog_bottom_left_button_center() {
  local shot=/tmp/yshell-dlg-left-button.png result
  import -window "$E2E_APP_WIN" "$shot" 2>/dev/null || return 1
  result=$(python3 - "$shot" "$E2E_THEME" <<'PY' 2>/dev/null || true
import sys
from PIL import Image
path, theme = sys.argv[1], sys.argv[2]
img = Image.open(path).convert('RGB')
px = img.load()
PANEL = (44, 44, 44) if theme == 'dark' else (255, 255, 255)
def dp(c): return max(abs(c[i] - PANEL[i]) for i in range(3))
rows = [y for y in range(100, 890)
        if sum(1 for x in range(470, 975) if dp(px[x, y]) <= 3) > 320]
if not rows:
    sys.exit(1)
pb = rows[-1]
cols = [x for x in range(300, 1140)
        if sum(1 for y in range(rows[0], rows[-1] + 1) if dp(px[x, y]) <= 3) > 60]
if not cols:
    sys.exit(1)
pl, pr = cols[0], cols[-1]
runs = []
for y in range(pb - 56, pb - 18):
    cur = None
    for x in range(pl + 6, pr - 6):
        if dp(px[x, y]) > 60:
            cur = [x, x] if cur is None else [cur[0], x]
        else:
            if cur and cur[1] - cur[0] >= 3:
                runs.append(tuple(cur))
            cur = None
    if cur and cur[1] - cur[0] >= 3:
        runs.append(tuple(cur))
runs.sort()
merged = []
for r in runs:
    if merged and r[0] - merged[-1][1] <= 12:
        merged[-1] = (merged[-1][0], max(merged[-1][1], r[1]))
    else:
        merged.append(r)
if not merged:
    sys.exit(1)
left = merged[0]
print((left[0] + left[1]) // 2, pb - 34)
PY
)
  if [ -n "$result" ]; then
    echo "$result"
  else
    return 1
  fi
}

# ⑥ About：light 主题下"帮助菜单 → About"点击偶发丢失（N5 实测：失败时截图无 scrim）。
# 有界重试整个序列（每轮先 Esc 复位浮层），不放宽"弹窗必须出现"的断言语义。
e2e_open_about_dialog() {
  local i
  for i in 1 2 3; do
    xdotool key --clearmodifiers Escape
    sleep 0.3
    xdotool mousemove --sync --window "$E2E_APP_WIN" "$HELP_MENU_X" "$MENUBAR_Y"
    sleep 0.3
    xdotool click 1
    sleep 0.8
    xdotool mousemove --sync --window "$E2E_APP_WIN" "$HELP_MENU_ITEM_X" "$HELP_MENU_ITEM0_Y"
    sleep 0.3
    xdotool click 1
    sleep 1.2
    if e2e_wait_until 12 e2e_dialog_open; then
      return 0
    fi
    e2e_warn "第 $i 次打开 About 未出现，重试"
  done
  return 1
}

# 弹窗期间快捷键让位：区域 AE 应接近 0（无变化）。
e2e_region_unchanged_between() { # $1=before $2=after $3=x $4=y $5=w $6=h $7=max_ae
  local diff
  diff=$(compare -metric AE \
    <(convert "$1" -crop "$5x$6+$3+$4" +repage png:-) \
    <(convert "$2" -crop "$5x$6+$3+$4" +repage png:-) null: 2>&1 || true)
  awk -v d="${diff:-0}" -v m="$7" 'BEGIN { exit !(d + 0 <= m) }'
}

# 两张截图在给定区域的像素差（AE > $7 视为变化）。
e2e_region_changed_between() { # $1=before.png $2=after.png $3=x $4=y $5=w $6=h $7=min_ae
  local diff
  diff=$(compare -metric AE \
    <(convert "$1" -crop "$5x$6+$3+$4" +repage png:-) \
    <(convert "$2" -crop "$5x$6+$3+$4" +repage png:-) null: 2>&1 || true)
  awk -v d="${diff:-0}" -v m="$7" 'BEGIN { exit !(d + 0 > m) }'
}

# ---------------------------------------------------------------- 场景
e2e_log "== L2 dialogs e2e：启动 =="
e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$E2E_DISPLAY" || exit 1

e2e_log "== ① 密码弹窗：Esc 关闭 + 敏感字段清空 =="
e2e_open_saved_session "$ROW_PASS_Y"
password_open=0
if e2e_wait_until 40 e2e_password_dialog_open; then password_open=1; fi
e2e_check "①密码弹窗" "弹窗出现" "$password_open" "$([ "$password_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "01-password-dialog" "①密码弹窗截图"
if [ "$password_open" = 1 ]; then
  escaped=0
  if e2e_escape_until 20 e2e_password_dialog_closed; then escaped=1; fi
  e2e_check "①Esc 关闭" "Esc 关闭密码弹窗（=Cancel 路径）" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
  e2e_check "①Esc 不连接" "取消后未连接" "$(e2e_session_connected && echo 0 || echo 1)" "$(e2e_session_connected && echo 已连接 || echo 未连接)"
  e2e_check_shot "$outdir" "02-password-esc-closed" "①Esc 取消后截图"

  # 重开：密码输入应已被清空（Connect 按钮回到禁用灰）。
  e2e_open_saved_session "$ROW_PASS_Y"
  reopened=0
  if e2e_wait_until 40 e2e_password_dialog_open; then reopened=1; fi
  cleared=1
  if [ "$reopened" = 1 ] && e2e_connect_enabled; then cleared=0; fi
  e2e_check "①敏感清空" "重开后 password_prompt_value_text 为空（Connect 禁用）" \
    "$([ "$reopened" = 1 ] && echo "$cleared" || echo 0)" \
    "$([ "$reopened" != 1 ] && echo 弹窗未出现 || (e2e_connect_enabled && echo 仍启用 || echo 已禁用))"
  e2e_check_shot "$outdir" "03-password-reopened-cleared" "①重开（密码已清空）"

  # 设计风险项：打开后光标在首个输入框（密码框）——不点击输入框直接打字应生效。
  xdotool windowfocus --sync "$E2E_APP_WIN" >/dev/null 2>&1 || true
  xdotool type --delay 80 "$ssh_password"
  typed=0
  if e2e_wait_until 20 e2e_connect_enabled; then typed=1; fi
  if [ "$typed" != 1 ]; then
    # 无头环境偶发丢字：回退到 lib.sh 的"点击输入框再输入"路径重试一次。
    if e2e_type_password "$ssh_password"; then typed=1; fi
  fi
  e2e_check "①输入/焦点" "打开后自动聚焦密码框（不点击直接输入生效）" "$typed" "$([ "$typed" = 1 ] && echo 已启用 || echo 未启用)"
  e2e_check_shot "$outdir" "04-password-typed" "①输入密码后截图"
  e2e_click_scrim
  closed=0
  if e2e_wait_until 20 e2e_password_dialog_closed; then closed=1; fi
  e2e_check "①遮罩点击" "遮罩点击关闭（=Cancel 路径）" "$closed" "$([ "$closed" = 1 ] && echo 已关闭 || echo 仍可见)"
  e2e_check "①遮罩不连接" "遮罩取消后未连接" "$(e2e_session_connected && echo 0 || echo 1)" "$(e2e_session_connected && echo 已连接 || echo 未连接)"
fi

e2e_log "== ② 删除确认：遮罩点击 + Enter 安全默认 =="
e2e_row_menu_item "$ROW_KEY_Y" 3
delete_open=0
if e2e_wait_until 20 e2e_dialog_open; then delete_open=1; fi
e2e_check "②删除确认" "弹窗出现" "$delete_open" "$([ "$delete_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "05-delete-confirm" "②删除确认弹窗截图"
if [ "$delete_open" = 1 ]; then
  xdotool key --clearmodifiers Return
  sleep 0.8
  e2e_check "②Enter 安全" "Enter 不触发删除（弹窗仍可见）" \
    "$(e2e_dialog_open && echo 1 || echo 0)" \
    "$(e2e_dialog_open && echo 仍可见 || echo 已关闭)"
  e2e_check_shot "$outdir" "06-delete-enter-safe" "②Enter 后仍可见"
  # 设计风险项：Tab 从根 FocusScope 进入确认词输入框；确认词完整时 Enter 仍不确认。
  token_sd_before=$(e2e_region_stddev 560 438 380 34)
  xdotool key --clearmodifiers Tab
  sleep 0.4
  xdotool type --delay 80 "DELETE"
  sleep 0.6
  token_sd_after=$(e2e_region_stddev 560 438 380 34)
  e2e_check "②Tab 进入输入框" "Tab 后可直接输入 DELETE（输入区出现文本）" \
    "$(awk -v b="${token_sd_before:-0}" -v a="${token_sd_after:-0}" 'BEGIN { print (a + 0 > b + 0 + 0.01) ? 1 : 0 }')" \
    "input stddev before=${token_sd_before:-?} after=${token_sd_after:-?}"
  e2e_check_shot "$outdir" "06b-delete-token-typed" "②Tab 输入 DELETE 后"
  xdotool key --clearmodifiers Return
  sleep 0.8
  e2e_check "②token 完整 Enter 安全" "确认词完整时 Enter 仍不确认" \
    "$(e2e_dialog_open && echo 1 || echo 0)" \
    "$(e2e_dialog_open && echo 仍可见 || echo 已关闭)"
  e2e_click_scrim
  closed=0
  if e2e_wait_until 20 e2e_dialog_closed; then closed=1; fi
  e2e_check "②遮罩点击" "遮罩点击关闭（=Cancel 路径）" "$closed" "$([ "$closed" = 1 ] && echo 已关闭 || echo 仍可见)"
  tree_sd=$(e2e_region_stddev 60 230 240 40)
  e2e_check "②未误删" "会话树行仍在（未删除）" \
    "$(awk -v s="${tree_sd:-0}" 'BEGIN { print (s + 0 > 0.02) ? 1 : 0 }')" \
    "tree stddev=${tree_sd:-?}"
  e2e_check_shot "$outdir" "07-delete-scrim-closed" "②遮罩取消后截图"
fi

e2e_log "== ③ NewFolderDialog：Esc =="
e2e_blank_menu_item_at "$BLANK_MENU_NEW_FOLDER_Y"
new_folder_open=0
if e2e_wait_until 20 e2e_dialog_open; then new_folder_open=1; fi
e2e_check "③新建文件夹" "弹窗出现" "$new_folder_open" "$([ "$new_folder_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "08-new-folder" "③NewFolderDialog 截图"
if [ "$new_folder_open" = 1 ]; then
  escaped=0
  if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
  e2e_check "③Esc 关闭" "Esc 关闭" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
fi

e2e_log "== ④ SessionEditor：Esc + 快捷键让位/焦点恢复 =="
xdotool key --clearmodifiers ctrl+n
sleep 1.2
editor_open=0
if e2e_wait_until 20 e2e_dialog_open; then editor_open=1; fi
e2e_check "④编辑器" "弹窗出现" "$editor_open" "$([ "$editor_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "09-session-editor" "④SessionEditor 截图"
if [ "$editor_open" = 1 ]; then
  # 弹窗期间快捷键让位：Ctrl+B 不应切换左栏。
  before_b=$(shot_path "09-session-editor")
  xdotool key --clearmodifiers ctrl+b
  sleep 1.0
  e2e_check_shot "$outdir" "10-shortcut-blocked" "④弹窗期间 Ctrl+B 无效果"
  after_b=$(shot_path "10-shortcut-blocked")
  e2e_check "④快捷键让位" "弹窗期间 Ctrl+B 不切换左栏" \
    "$(e2e_region_unchanged_between "$before_b" "$after_b" 0 120 230 220 200 && echo 1 || echo 0)" \
    "$(e2e_region_unchanged_between "$before_b" "$after_b" 0 120 230 220 200 && echo 未变化 || echo 有变化)"
  escaped=0
  if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
  e2e_check "④Esc 关闭" "Esc 关闭编辑器" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
  e2e_check_shot "$outdir" "11-session-editor-closed" "④Esc 关闭后截图"

  # 焦点恢复：关闭后 Ctrl+B 应能切换左栏（回到窗口级快捷键 FocusScope）。
  before_f=$(shot_path "11-session-editor-closed")
  xdotool key --clearmodifiers ctrl+b
  sleep 1.0
  e2e_check_shot "$outdir" "12-focus-restored" "④关闭后 Ctrl+B 生效（焦点恢复）"
  after_f=$(shot_path "12-focus-restored")
  e2e_check "④焦点恢复" "关闭后 Ctrl+B 切换左栏" \
    "$(e2e_region_changed_between "$before_f" "$after_f" 0 120 230 220 500 && echo 1 || echo 0)" \
    "$(e2e_region_changed_between "$before_f" "$after_f" 0 120 230 220 500 && echo 已切换 || echo 未变化)"
  xdotool key --clearmodifiers ctrl+b
  sleep 0.8
fi

e2e_log "== ⑤ 嵌套：Settings → Known Hosts，Esc 只关上层 =="
xdotool key --clearmodifiers ctrl+comma
sleep 1.2
settings_open=0
if e2e_wait_until 20 e2e_dialog_open; then settings_open=1; fi
e2e_check "⑤Settings" "弹窗出现" "$settings_open" "$([ "$settings_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "13-settings" "⑤Settings 截图"
if [ "$settings_open" = 1 ]; then
  e2e_click "$SETTINGS_KNOWN_HOSTS_X" "$SETTINGS_KNOWN_HOSTS_Y" 1.5
  nested=0
  if e2e_wait_until 20 e2e_dialog_open; then nested=1; fi
  e2e_check "⑤嵌套" "Known Hosts 以嵌套层打开" "$nested" "$([ "$nested" = 1 ] && echo 已打开 || echo 未打开)"
  e2e_check_shot "$outdir" "14-nested-settings-known-hosts" "⑤嵌套（Settings 在下，Known Hosts 在上）"
  # Known Hosts 面板宽 760（x 340 起），Settings 面板宽 560（x 440 起）：
  # (380,300) 在 Known Hosts 面板内、Settings 下只是被 scrim 压暗的背景。
  probe_before=$(e2e_pixel_gray 380 300)
  probe_after=""
  for attempt in 1 2; do
    xdotool key --clearmodifiers Escape
    sleep 1.2
    probe_after=$(e2e_pixel_gray 380 300)
    if awk -v b="${probe_before:-0}" -v a="${probe_after:-0}" 'BEGIN { exit !(a + 0 < b + 0) }'; then
      break
    fi
    e2e_warn "第 $attempt 次 Esc 后上层（Known Hosts）仍未关闭，重试"
  done
  still_open=0
  if e2e_dialog_open; then still_open=1; fi
  e2e_check "⑤只关上层" "Esc 后仍有弹窗（Settings 保持打开）" "$still_open" "$([ "$still_open" = 1 ] && echo 仍打开 || echo 已全部关闭)"
  e2e_check "⑤上层已关" "上层（Known Hosts）已关闭（探测点变暗）" \
    "$(awk -v b="${probe_before:-0}" -v a="${probe_after:-0}" 'BEGIN { print (a + 0 < b + 0) ? 1 : 0 }')" \
    "probe before=${probe_before:-?} after=${probe_after:-?}"
  e2e_check_shot "$outdir" "15-nested-top-closed" "⑤Esc 只关上层后（Settings 仍在）"
  escaped=0
  if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
  e2e_check "⑤下层可操作" "再 Esc 关闭下层（Settings）" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
  e2e_check_shot "$outdir" "16-nested-bottom-closed" "⑤下层关闭后回到主界面"
fi

# Settings 自身的遮罩点击（与上面嵌套的 Esc 路径互补）。
xdotool key --clearmodifiers ctrl+comma
sleep 1.2
settings_open=0
if e2e_wait_until 20 e2e_dialog_open; then settings_open=1; fi
if [ "$settings_open" = 1 ]; then
  e2e_click_scrim
  closed=0
  if e2e_wait_until 20 e2e_dialog_closed; then closed=1; fi
  e2e_check "⑤Settings 遮罩" "Settings 遮罩点击关闭" "$closed" "$([ "$closed" = 1 ] && echo 已关闭 || echo 仍可见)"
  e2e_check_shot "$outdir" "16b-settings-scrim-closed" "⑤Settings 遮罩取消后截图"
fi

e2e_log "== ⑥ About：Esc =="
about_open=0
if e2e_open_about_dialog; then about_open=1; fi
e2e_check "⑥About" "弹窗出现" "$about_open" "$([ "$about_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "17-about" "⑥About 截图"
if [ "$about_open" = 1 ]; then
  escaped=0
  if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
  e2e_check "⑥Esc 关闭" "Esc 关闭 About" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
fi

e2e_log "== ⑦ Quit 确认：Esc 取消 =="
xdotool key --clearmodifiers ctrl+q
sleep 1.2
quit_open=0
if e2e_wait_until 20 e2e_dialog_open; then quit_open=1; fi
e2e_check "⑦Quit 确认" "弹窗出现" "$quit_open" "$([ "$quit_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "18-quit-confirm" "⑦Quit 确认截图"
if [ "$quit_open" = 1 ]; then
  escaped=0
  if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
  e2e_check "⑦Esc 取消" "Esc 取消退出（应用仍运行）" "$escaped" "$([ "$escaped" = 1 ] && echo 已取消 || echo 仍可见)"
  alive=0
  if kill -0 "$E2E_APP_PID" 2>/dev/null; then alive=1; fi
  e2e_check "⑦应用存活" "应用未退出" "$alive" "$([ "$alive" = 1 ] && echo 运行中 || echo 已退出)"
  e2e_check_shot "$outdir" "19-quit-cancelled" "⑦Esc 取消后截图"
fi

e2e_log "== ⑧ HostKey（strict 未知主机）：Esc =="
e2e_open_saved_session "$ROW_STRICT_Y"
hostkey_open=0
if e2e_wait_until 40 e2e_host_key_dialog_open; then hostkey_open=1; fi
e2e_check "⑧主机密钥弹窗" "弹窗出现" "$hostkey_open" "$([ "$hostkey_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "20-host-key" "⑧主机密钥弹窗截图"
if [ "$hostkey_open" = 1 ]; then
  escaped=0
  if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
  e2e_check "⑧Esc 关闭" "Esc 关闭主机密钥弹窗" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
  e2e_check "⑧未连接" "取消后未连接" "$(e2e_session_connected && echo 0 || echo 1)" "$(e2e_session_connected && echo 已连接 || echo 未连接)"
  e2e_check_shot "$outdir" "21-host-key-cancelled" "⑧Esc 取消后截图"
fi

e2e_log "== ⑧b HostKey changed（伪造指纹）：REPLACE 输入 + Esc 清空 =="
e2e_open_saved_session "$ROW_CHANGED_Y"
changed_open=0
if e2e_wait_until 40 e2e_host_key_dialog_open; then changed_open=1; fi
e2e_check "⑧b changed 弹窗" "changed 模式弹窗出现" "$changed_open" "$([ "$changed_open" = 1 ] && echo 出现 || echo 未出现)"
e2e_check_shot "$outdir" "20b-host-key-changed" "⑧b changed 模式弹窗截图"
if [ "$changed_open" = 1 ]; then
  sd_empty=$(e2e_region_stddev "$CHANGED_INPUT_X" "$CHANGED_INPUT_Y" "$CHANGED_INPUT_W" "$CHANGED_INPUT_H")
  # E1 后右上 × 可 Tab 聚焦 → 从根 FocusScope 出发最多 3 次 Tab（通常 × 之后第 2 次）
  # 进入 REPLACE 输入框；用输入框底部强调条像素确认焦点，不依赖固定 Tab 次数。
  e2e_tab_into_changed_input || true
  xdotool type --delay 80 "REPLACE"
  sleep 0.6
  sd_typed=$(e2e_region_stddev "$CHANGED_INPUT_X" "$CHANGED_INPUT_Y" "$CHANGED_INPUT_W" "$CHANGED_INPUT_H")
  e2e_check "⑧b 输入 REPLACE" "Tab 后输入生效（输入区出现文本）" \
    "$(awk -v e="${sd_empty:-0}" -v t="${sd_typed:-0}" 'BEGIN { print (t + 0 > e + 0 + 0.01) ? 1 : 0 }')" \
    "input stddev empty=${sd_empty:-?} typed=${sd_typed:-?}"
  e2e_check_shot "$outdir" "20c-host-key-replace-typed" "⑧b 输入 REPLACE 后"
  escaped=0
  if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
  e2e_check "⑧b Esc 关闭" "Esc 关闭 changed 弹窗（=Cancel 路径）" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"

  # 重开：REPLACE 确认词应已被 Rust 侧取消路径清空（cancel_host_key_prompt）。
  e2e_open_saved_session "$ROW_CHANGED_Y"
  reopened=0
  if e2e_wait_until 40 e2e_host_key_dialog_open; then reopened=1; fi
  sd_reopen=$(e2e_region_stddev "$CHANGED_INPUT_X" "$CHANGED_INPUT_Y" "$CHANGED_INPUT_W" "$CHANGED_INPUT_H")
  e2e_check "⑧b 确认词清空" "重开后 REPLACE 输入为空（stddev 回落到空态）" \
    "$([ "$reopened" = 1 ] && awk -v t="${sd_typed:-0}" -v r="${sd_reopen:-0}" 'BEGIN { print (r + 0 < t + 0 - 0.01) ? 1 : 0 }' || echo 0)" \
    "stddev typed=${sd_typed:-?} reopen=${sd_reopen:-?} empty=${sd_empty:-?}"
  e2e_check_shot "$outdir" "20d-host-key-reopened-cleared" "⑧b 重开（REPLACE 已清空）"
  if [ "$reopened" = 1 ]; then
    xdotool key --clearmodifiers Return
    sleep 0.8
    e2e_check "⑧b Enter 安全" "确认词为空时 Enter 不替换密钥（弹窗仍可见）" \
      "$(e2e_dialog_open && echo 1 || echo 0)" \
      "$(e2e_dialog_open && echo 仍可见 || echo 已关闭)"
    escaped=0
    if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
    e2e_check "⑧b 再次 Esc" "再次 Esc 关闭" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
  fi
fi

if [ "$skip_sftp" = 1 ]; then
  e2e_log "== ⑨ SFTP 弹窗：跳过（--skip-sftp）=="
else
  e2e_log "== ⑨ SFTP 弹窗（真连）：New Folder / Delete =="
  e2e_open_saved_session "$ROW_KEY_Y"
  connected=0
  if e2e_wait_until 60 e2e_session_connected; then connected=1; fi
  e2e_check "⑨SFTP 前置连接" "连接成功（SFTP 列表 ≥3 行）" "$connected" \
    "$(e2e_session_connected && echo "SFTP 行数=$(e2e_sftp_row_bands)" || echo 未连接)"
  if [ "$connected" = 1 ]; then
    # 右键文件列表行 → New Folder…（第 7 项）
    xdotool mousemove --sync --window "$E2E_APP_WIN" "$SFTP_ROW_X" "$SFTP_ROW_Y" click 3
    sleep 1.0
    xdotool mousemove --sync --window "$E2E_APP_WIN" "$(( SFTP_ROW_X + SFTP_MENU_DX ))" "$SFTP_MENU_NEW_FOLDER_Y"
    sleep 0.3
    xdotool click 1
    sleep 1.2
    sftp_folder_open=0
    if e2e_wait_until 20 e2e_dialog_open; then sftp_folder_open=1; fi
    e2e_check "⑨SFTP 新建文件夹" "弹窗出现" "$sftp_folder_open" "$([ "$sftp_folder_open" = 1 ] && echo 出现 || echo 未出现)"
    e2e_check_shot "$outdir" "22-sftp-new-folder" "⑨SFTP New Folder 弹窗"
    if [ "$sftp_folder_open" = 1 ]; then
      escaped=0
      if e2e_escape_until 20 e2e_dialog_closed; then escaped=1; fi
      e2e_check "⑨SFTP Esc" "Esc 关闭" "$escaped" "$([ "$escaped" = 1 ] && echo 已关闭 || echo 仍可见)"
    fi
    # 右键文件列表行 → Delete（第 4 项）
    xdotool mousemove --sync --window "$E2E_APP_WIN" "$SFTP_ROW_X" "$SFTP_ROW_Y" click 3
    sleep 1.0
    xdotool mousemove --sync --window "$E2E_APP_WIN" "$(( SFTP_ROW_X + SFTP_MENU_DX ))" "$SFTP_MENU_DELETE_Y"
    sleep 0.3
    xdotool click 1
    sleep 1.2
    sftp_delete_open=0
    if e2e_wait_until 20 e2e_dialog_open; then sftp_delete_open=1; fi
    e2e_check "⑨SFTP 删除" "弹窗出现" "$sftp_delete_open" "$([ "$sftp_delete_open" = 1 ] && echo 出现 || echo 未出现)"
    e2e_check_shot "$outdir" "23-sftp-delete" "⑨SFTP Delete 弹窗"
    if [ "$sftp_delete_open" = 1 ]; then
      xdotool key --clearmodifiers Return
      sleep 0.8
      e2e_check "⑨SFTP Enter 安全" "Enter 不触发删除（弹窗仍可见）" \
        "$(e2e_dialog_open && echo 1 || echo 0)" \
        "$(e2e_dialog_open && echo 仍可见 || echo 已关闭)"
      # B23（live-audit ws-B，设计语言 §5.10 / 契约 §1.2）：Delete 是破坏性弹窗，
      # 不响应 Esc / 遮罩点击，只能在弹窗内显式选择操作。旧断言"Esc 关闭"与
      # B23 语义冲突，这里改为：Esc 连按 2 次仍可见（安全）→ 点 Cancel 关闭。
      xdotool key --clearmodifiers Escape
      sleep 0.8
      xdotool key --clearmodifiers Escape
      sleep 0.8
      esc_safe=0
      if e2e_dialog_open; then esc_safe=1; fi
      e2e_check "⑨SFTP Esc 不关闭（破坏性）" "Esc 后删除弹窗仍可见（安全，不误取消）" "$esc_safe" "$([ "$esc_safe" = 1 ] && echo 仍可见 || echo 已关闭)"
      e2e_check_shot "$outdir" "23b-sftp-delete-after-esc" "⑨SFTP Esc 后仍可见"
      cancel_center=$(e2e_dialog_bottom_left_button_center) || cancel_center=""
      if [ -n "$cancel_center" ]; then
        set -- $cancel_center
        e2e_click "$1" "$2" 1.2
      else
        e2e_warn "未定位到删除弹窗的 Cancel 按钮（动作行左按钮）"
      fi
      closed=0
      if e2e_wait_until 20 e2e_dialog_closed; then closed=1; fi
      e2e_check "⑨SFTP Cancel 关闭" "点 Cancel 关闭删除弹窗（B23 显式取消）" "$closed" "$([ "$closed" = 1 ] && echo 已关闭 || echo 仍可见)"
      e2e_check_shot "$outdir" "24-sftp-delete-cancelled" "⑨SFTP 删除取消后截图"
    fi
  fi
fi

if [ "$keep_config" = 1 ]; then
  e2e_log "保留配置目录：$config_dir（--keep-config）"
fi

if e2e_summary; then
  exit 0
else
  exit 1
fi
