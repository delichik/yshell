#!/usr/bin/env bash
# YShell SSH 真连 UI E2E（无头 Xvfb 驱动）。
#
# 覆盖：主机密钥信任弹窗（strict + TOFU「首次必弹」）、Trust Once / Trust and Save 语义、
#       密码弹窗（正确密码 / 错误密码 / 取消）、连接成功后的真实 banner 与 SFTP 列表。
#
# 依赖：
#   * WSL/Linux 无头环境：Xvfb、xdotool、ImageMagick（import/convert）
#   * Docker 容器：scripts/test-ssh/run.sh（默认 root@127.0.0.1:2222 + tester 密码账号）
#   * 已构建的二进制：target/debug/yshell（cargo xtask build）
#
# 用法：
#   bash scripts/e2e/run-ui-ssh.sh [--theme dark|light] [--lang zh-CN|en-US]
#                                  [--stage all|trust|trust-once|pass|pass-wrong|pass-cancel|tofu-recheck]
#                                  [--outdir dist/ui-checks] [--prefix e2e] [--keep-config]
#
# 示例：
#   bash scripts/e2e/run-ui-ssh.sh                       # 全流程（dark / zh-CN）
#   bash scripts/e2e/run-ui-ssh.sh --stage pass          # 只跑密码成功路径
#   bash scripts/e2e/run-ui-ssh.sh --theme light --lang en-US --stage trust
#
# 环境变量（与 scripts/test-ssh/README.md 一致）：
#   YSHELL_TEST_SSH_USER / YSHELL_TEST_SSH_PASSWORD / YSHELL_TEST_SSH_PORT / YSHELL_TEST_SSH_KEY
#   YSHELL_TEST_SSH_CONTAINER（状态检查用）
#   YSHELL_E2E_BIN（默认 target/debug/yshell）；YSHELL_E2E_DISPLAY（默认 :99）
#   E2E_SCREEN（Xvfb 几何，默认 1920x1200x24）；E2E_WINIT_SCALE（设备缩放，默认 1.0）
#   说明：Xvfb 几何固定 + WINIT_X11_SCALE_FACTOR=1，保证终端/UI 位图 1:1，像素断言可复现；
#        若 :99 已被其它几何的 Xvfb 占用，脚本会自动改用空闲 display。
#
# 输出：截图到 dist/ui-checks/<prefix>-<theme>-<lang>-NN-<step>.png；
#       结尾打印断言汇总表；任一断言失败 → 退出码 1。
#
# 已知限制：无头 Xvfb 下 motion/悬停不渲染（见 docs/product/ui-winui3-verification.md §2），
#           本脚本不验证 hover；会话树「双击不激活」（选中态重建吞掉第二次点击），
#           统一用右键菜单第一项 "Open" 打开会话。坐标按 1440×900 窗口校准。
set -u

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

theme="dark"
lang="zh-CN"
stage="all"
outdir="$repo_dir/dist/ui-checks"
prefix="e2e"
keep_config=0
bin="${YSHELL_E2E_BIN:-$repo_dir/target/debug/yshell}"
display="${YSHELL_E2E_DISPLAY:-:99}"
config_dir="/tmp/yshell-e2e-config"
log_file="/tmp/yshell-e2e.log"

ssh_port="${YSHELL_TEST_SSH_PORT:-2222}"
ssh_host="127.0.0.1"
ssh_user="${YSHELL_TEST_SSH_USER:-tester}"
ssh_password="${YSHELL_TEST_SSH_PASSWORD:-yshell-test-pass}"
ssh_key="${YSHELL_TEST_SSH_KEY:-$HOME/.ssh/yshell_test_ed25519}"
container="${YSHELL_TEST_SSH_CONTAINER:-yshell-test-ssh}"

# 会话树行 / 弹窗按钮的窗口内坐标（1440×900 实测；窗口原点已固定到 (0,0)）。
# 密码弹窗内部控件（输入框/连接/取消）的坐标在 lib.sh 里，供输入与提交 helper 复用。
key_row_y=239
pass_row_y=268
trust_once_x=813
trust_save_x=911
dialog_button_y=550

usage() {
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
  --theme) theme="${2:-}" ; shift 2 ;;
  --lang) lang="${2:-}" ; shift 2 ;;
  --stage) stage="${2:-}" ; shift 2 ;;
  --outdir) outdir="${2:-}" ; shift 2 ;;
  --prefix) prefix="${2:-}" ; shift 2 ;;
  --keep-config) keep_config=1 ; shift ;;
  -h | --help) usage; exit 0 ;;
  *) echo "未知参数：$1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$theme" in dark | light) ;; *) echo "--theme 只支持 dark|light" >&2; exit 2 ;; esac
case "$lang" in zh-CN | en-US) ;; *) echo "--lang 只支持 zh-CN|en-US" >&2; exit 2 ;; esac
case "$stage" in
all | trust | trust-once | pass | pass-wrong | pass-cancel | tofu-recheck) ;;
*) echo "--stage 不支持：$stage" >&2; usage >&2; exit 2 ;;
esac

E2E_SHOT_PREFIX="$prefix-$theme-$lang"
E2E_THEME="$theme"
mkdir -p "$outdir"

# 退出时确保 app 进程被回收（Xvfb 保留给后续运行复用）。
cleanup() { e2e_stop_app; }
trap cleanup EXIT

# ---------------------------------------------------------------- 前置检查
e2e_require_tools || exit 1
if [ ! -x "$bin" ]; then
  e2e_warn "找不到可执行文件：$bin（先 cargo xtask build）"
  exit 1
fi
container_status=$(bash "$repo_dir/scripts/test-ssh/run.sh" status 2>&1 || true)
printf '%s\n' "$container_status"
if ! grep -q 'Up ' <<<"$container_status"; then
  e2e_warn "测试容器未运行（$container）。先执行：bash scripts/test-ssh/run.sh up"
  exit 1
fi

e2e_log "配置：theme=$theme lang=$lang stage=$stage outdir=$outdir"
e2e_log "目标：root(key)@$ssh_host:$ssh_port / $ssh_user(password)@$ssh_host:$ssh_port"

e2e_ensure_xvfb "$display" || exit 1

# ---------------------------------------------------------------- 配置与文件状态
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
    { id = "e2e-key", name = "Docker Key", host = "$ssh_host", port = $ssh_port, username = "root", auth_profile_id = "auth-key", host_key_policy = "strict" },
    { id = "e2e-pass", name = "Docker Pass", host = "$ssh_host", port = $ssh_port, username = "$ssh_user", auth_profile_id = "auth-pass", host_key_policy = "trust_on_first_use" },
]
TOML
}

# 需要"没有已信任主机"的 stage 从干净配置开始；其余 stage 保留 known_hosts。
reset_config() {
  rm -rf "$config_dir"
  write_config
}
prepare_config() {
  write_config
}

known_hosts_file="$config_dir/known_hosts.toml"
known_hosts_has_entry() {
  [ -f "$known_hosts_file" ] && grep -q "${ssh_host}:${ssh_port}" "$known_hosts_file"
}

# ---------------------------------------------------------------- 阶段
# 前置：用 strict 会话走一次 Trust and Save，确保后续密码会话不会先撞主机密钥弹窗。
ensure_known_host() { # $1 = 断言行前缀（stage 名）
  local label="$1"
  if known_hosts_has_entry; then
    e2e_check "$label/前置已信任主机" "known_hosts 已有条目" 1 "复用已有条目"
    return 0
  fi
  e2e_log "known_hosts 缺少 $ssh_host:$ssh_port，先用 strict 会话 Trust and Save"
  e2e_open_saved_session "$key_row_y"
  local dialog_ok=0
  if e2e_wait_until 40 e2e_host_key_dialog_open; then dialog_ok=1; fi
  e2e_check "$label/前置信任弹窗" "弹窗出现" "$dialog_ok" "$([ "$dialog_ok" = 1 ] && echo 出现 || echo 未出现)"
  local connected=0
  if [ "$dialog_ok" = 1 ] && e2e_click_until "$trust_save_x" "$dialog_button_y" 60 e2e_session_connected; then
    connected=1
  fi
  e2e_check "$label/前置连接" "连接成功（SFTP 列表 ≥3 行）" "$connected" "$(e2e_session_connected && echo "SFTP 行数=$(e2e_sftp_row_bands)" || echo "未连接")"
  [ "$connected" = 1 ]
}

stage_trust() {
  e2e_log "== stage trust：strict 首次连接 → 信任弹窗 → Trust and Save =="
  reset_config
  e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$display" || return 1
  e2e_open_saved_session "$key_row_y"

  local dialog_ok=0
  if e2e_wait_until 40 e2e_host_key_dialog_open; then dialog_ok=1; fi
  e2e_check "trust/信任弹窗" "弹窗出现" "$dialog_ok" "$([ "$dialog_ok" = 1 ] && echo 出现 || echo 未出现)"
  e2e_check_shot "$outdir" "01-trust-dialog" "trust/信任弹窗截图"
  [ "$dialog_ok" = 1 ] || return 0

  e2e_click_until "$trust_save_x" "$dialog_button_y" 60 e2e_session_connected
  local connected=0
  if e2e_session_connected; then connected=1; fi
  e2e_check "trust/连接成功" "SFTP 列表 ≥3 行" "$connected" "$(e2e_session_connected && echo "SFTP 行数=$(e2e_sftp_row_bands)" || echo "未连接")"

  local kh=0
  if known_hosts_has_entry; then kh=1; fi
  e2e_check "trust/known_hosts" "写入 $ssh_host:$ssh_port" "$kh" "$([ "$kh" = 1 ] && echo 已写入 || echo "未写入")"
  e2e_check_shot "$outdir" "02-trust-connected" "trust/Trust and Save 后截图"
}

stage_trust_once() {
  e2e_log "== stage trust-once：Trust Once 只信任本次、不落盘 =="
  reset_config
  e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$display" || return 1
  e2e_open_saved_session "$key_row_y"

  local dialog_ok=0
  if e2e_wait_until 40 e2e_host_key_dialog_open; then dialog_ok=1; fi
  e2e_check "trust-once/信任弹窗" "弹窗出现" "$dialog_ok" "$([ "$dialog_ok" = 1 ] && echo 出现 || echo 未出现)"
  [ "$dialog_ok" = 1 ] || return 0

  e2e_click_until "$trust_once_x" "$dialog_button_y" 60 e2e_session_connected
  local connected=0
  if e2e_session_connected; then connected=1; fi
  e2e_check "trust-once/连接成功" "SFTP 列表 ≥3 行" "$connected" "$(e2e_session_connected && echo "SFTP 行数=$(e2e_sftp_row_bands)" || echo "未连接")"

  local kh=0
  if known_hosts_has_entry; then kh=1; fi
  e2e_check "trust-once/不落盘" "known_hosts 无 $ssh_host:$ssh_port 条目" "$([ "$kh" = 0 ] && echo 1 || echo 0)" "$([ "$kh" = 0 ] && echo 无条目 || echo 已落盘)"
  e2e_check_shot "$outdir" "03-trust-once-connected" "trust-once/连接后截图"
}

stage_pass() {
  e2e_log "== stage pass：密码弹窗 → 正确密码 → 连接成功 =="
  prepare_config
  e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$display" || return 1
  ensure_known_host "pass" || return 0

  e2e_open_saved_session "$pass_row_y"
  local dialog_ok=0
  if e2e_wait_until 40 e2e_password_dialog_open; then dialog_ok=1; fi
  e2e_check "pass/密码弹窗" "弹窗出现" "$dialog_ok" "$([ "$dialog_ok" = 1 ] && echo 出现 || echo 未出现)"
  e2e_check_shot "$outdir" "04-pass-dialog" "pass/密码弹窗截图"
  [ "$dialog_ok" = 1 ] || return 0

  local typed=0
  if e2e_type_password "$ssh_password"; then typed=1; fi
  e2e_check "pass/密码输入生效" "连接按钮变为可用" "$typed" "$([ "$typed" = 1 ] && echo 已启用 || echo 未启用)"
  e2e_check_shot "$outdir" "05-pass-typed" "pass/输入后截图"
  [ "$typed" = 1 ] || return 0

  e2e_submit_until 60 e2e_session_connected
  local connected=0
  if e2e_session_connected; then connected=1; fi
  e2e_check "pass/连接成功" "SFTP 列表 ≥3 行（真实 banner）" "$connected" "$(e2e_session_connected && echo "SFTP 行数=$(e2e_sftp_row_bands)" || echo "未连接")"
  local dialog_gone=0
  if ! e2e_password_dialog_open; then dialog_gone=1; fi
  e2e_check "pass/弹窗关闭" "提交后密码弹窗消失" "$dialog_gone" "$([ "$dialog_gone" = 1 ] && echo 已关闭 || echo 仍可见)"
  e2e_check_shot "$outdir" "06-pass-connected" "pass/连接后截图"
}

stage_pass_wrong() {
  e2e_log "== stage pass-wrong：错误密码 → 状态栏错误、弹窗关闭、未连接 =="
  prepare_config
  e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$display" || return 1
  ensure_known_host "pass-wrong" || return 0

  e2e_open_saved_session "$pass_row_y"
  local dialog_ok=0
  if e2e_wait_until 40 e2e_password_dialog_open; then dialog_ok=1; fi
  e2e_check "pass-wrong/密码弹窗" "弹窗出现" "$dialog_ok" "$([ "$dialog_ok" = 1 ] && echo 出现 || echo 未出现)"
  [ "$dialog_ok" = 1 ] || return 0

  local typed=0
  if e2e_type_password "wrong-password"; then typed=1; fi
  e2e_check "pass-wrong/密码输入生效" "连接按钮变为可用" "$typed" "$([ "$typed" = 1 ] && echo 已启用 || echo 未启用)"
  e2e_check_shot "$outdir" "07-pass-wrong-typed" "pass-wrong/输入错误密码后截图"
  [ "$typed" = 1 ] || return 0

  e2e_submit_until 30 e2e_password_failed
  local failed=0
  # 失败判定：未出现 SFTP 列表（N4 后认证弹窗保留并内联错误提示以便重试，
  # 状态栏错误文案由截图留证；这里只断言"未连接"）。
  if e2e_password_failed; then failed=1; fi
  e2e_check "pass-wrong/认证失败" "未连接（无 SFTP 列表；弹窗内联错误）" "$failed" "$([ "$failed" = 1 ] && echo 未连接 || echo "仍连接/仍可见")"
  e2e_check_shot "$outdir" "08-pass-wrong-result" "pass-wrong/结果截图（状态栏错误文案）"
}

e2e_password_failed() {
  # N4：认证失败后弹窗保留（内联错误 + 重试），因此不再要求弹窗关闭。
  ! e2e_session_connected
}

stage_pass_cancel() {
  e2e_log "== stage pass-cancel：取消密码弹窗 → 不连接 =="
  prepare_config
  e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$display" || return 1

  e2e_open_saved_session "$pass_row_y"
  local dialog_ok=0
  if e2e_wait_until 40 e2e_password_dialog_open; then dialog_ok=1; fi
  e2e_check "pass-cancel/密码弹窗" "弹窗出现" "$dialog_ok" "$([ "$dialog_ok" = 1 ] && echo 出现 || echo 未出现)"
  [ "$dialog_ok" = 1 ] || return 0

  e2e_click "$E2E_PASSWORD_CANCEL_X" "$E2E_PASSWORD_CANCEL_Y" 1.0
  local cancelled=0
  if e2e_wait_until 20 e2e_password_cancelled; then cancelled=1; fi
  e2e_check "pass-cancel/取消生效" "弹窗关闭且未连接" "$cancelled" "$([ "$cancelled" = 1 ] && echo 已关闭且未连接 || echo 状态异常)"
  e2e_check_shot "$outdir" "09-pass-cancel" "pass-cancel/取消后截图"
}

e2e_password_cancelled() {
  # 取消路径必须关闭弹窗且不连接（错误路径见 e2e_password_failed）。
  ! e2e_password_dialog_open && ! e2e_session_connected
}

stage_tofu_recheck() {
  e2e_log "== stage tofu-recheck：TOFU 未知主机首次连接必须弹信任弹窗 =="
  reset_config
  e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$display" || return 1

  e2e_open_saved_session "$pass_row_y"
  local dialog_ok=0
  if e2e_wait_until 40 e2e_password_dialog_open; then dialog_ok=1; fi
  e2e_check "tofu/密码弹窗" "弹窗出现" "$dialog_ok" "$([ "$dialog_ok" = 1 ] && echo 出现 || echo 未出现)"
  if [ "$dialog_ok" != 1 ]; then
    return 0
  fi

  local typed=0
  if e2e_type_password "$ssh_password"; then typed=1; fi
  e2e_check "tofu/密码输入生效" "连接按钮变为可用" "$typed" "$([ "$typed" = 1 ] && echo 已启用 || echo 未启用)"
  if [ "$typed" != 1 ]; then
    e2e_check_shot "$outdir" "10a-tofu-password-typed" "tofu/输入后截图"
    return 0
  fi

  e2e_submit_until 60 e2e_host_key_dialog_open
  local host_dialog=0
  if e2e_host_key_dialog_open; then host_dialog=1; fi
  e2e_check "tofu/首次必弹（回归）" "提交密码后出现主机密钥弹窗（不静默信任）" "$host_dialog" "$([ "$host_dialog" = 1 ] && echo 出现 || echo "未出现（静默信任！）")"
  local kh_before=0
  if known_hosts_has_entry; then kh_before=1; fi
  e2e_check "tofu/信任前不落盘" "known_hosts 无 $ssh_host:$ssh_port 条目" "$([ "$kh_before" = 0 ] && echo 1 || echo 0)" "$([ "$kh_before" = 0 ] && echo 无条目 || echo 已落盘)"
  e2e_check_shot "$outdir" "10-tofu-host-key-prompt" "tofu/信任弹窗截图"
  [ "$host_dialog" = 1 ] || return 0

  e2e_click_until "$trust_save_x" "$dialog_button_y" 60 e2e_session_connected
  local connected=0
  if e2e_session_connected; then connected=1; fi
  e2e_check "tofu/Trust and Save 后连接" "SFTP 列表 ≥3 行" "$connected" "$(e2e_session_connected && echo "SFTP 行数=$(e2e_sftp_row_bands)" || echo "未连接")"
  local kh_after=0
  if known_hosts_has_entry; then kh_after=1; fi
  e2e_check "tofu/信任后落盘" "known_hosts 写入 $ssh_host:$ssh_port" "$kh_after" "$([ "$kh_after" = 1 ] && echo 已写入 || echo 未写入)"
  e2e_check_shot "$outdir" "11-tofu-connected" "tofu/信任后连接截图"
}

# ---------------------------------------------------------------- 主流程
# 顺序说明：trust 会落盘 known_hosts，后面 pass* 直接复用；trust-once / tofu-recheck
# 会重置配置，所以放在依赖已信任主机的 stage 之后。
stages_all="trust pass pass-wrong pass-cancel trust-once tofu-recheck"

run_stage() {
  case "$1" in
  trust) stage_trust ;;
  trust-once) stage_trust_once ;;
  pass) stage_pass ;;
  pass-wrong) stage_pass_wrong ;;
  pass-cancel) stage_pass_cancel ;;
  tofu-recheck) stage_tofu_recheck ;;
  esac
}

if [ "$stage" = "all" ]; then
  stage_list="$stages_all"
else
  stage_list="$stage"
fi

for current_stage in $stage_list; do
  e2e_log "---------- 开始 stage：$current_stage ----------"
  if ! run_stage "$current_stage"; then
    e2e_warn "stage $current_stage 基础设施失败（app 启动/前置不满足）"
  fi
  e2e_stop_app
done

if [ "$keep_config" = 1 ]; then
  e2e_log "保留配置目录：$config_dir（--keep-config）"
fi

if e2e_summary; then
  exit 0
else
  exit 1
fi
