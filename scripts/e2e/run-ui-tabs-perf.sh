#!/usr/bin/env bash
# N0 性能抽查（可选）：10 个标签同时输出时 UI 可响应。
#
# 方法（N0 设计 §8「性能」）：
#   1) 开 10 个真实 SSH 标签；每开一个就在其中输入一个 5 次/秒的 ticker
#      （新开的标签一定是活动标签，因此不需要点击不可见的标签）。
#   2) 后台轮询单 tick 上限 8：游标轮转保证 10 个会话的输出都会被消费；
#      采样 app 进程 CPU%/RSS（3 次）。
#   3) 持续输出期间滚回标签条最左，点击标签 1/2，断言活动强调条跟随
#      （UI 仍可交互）。
#   4) 截图留证（<prefix>-<theme>-<lang>-10-tabs-streaming.png）。
#
# 用法：
#   bash scripts/e2e/run-ui-tabs-perf.sh [--theme dark|light] [--lang zh-CN|en-US]
#                                        [--outdir dist/ui-checks] [--prefix n0-perf]
#
# 依赖与 run-ui-tabs.sh 相同（Xvfb/xdotool/ImageMagick + 测试容器 + 已构建二进制）。
set -u

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

theme="dark"
lang="zh-CN"
outdir="$repo_dir/dist/ui-checks"
prefix="n0-perf"
bin="${YSHELL_E2E_BIN:-$repo_dir/target/debug/yshell}"
display="${YSHELL_E2E_DISPLAY:-:99}"
config_dir="/tmp/yshell-e2e-tabs-perf-config"
log_file="/tmp/yshell-e2e-tabs-perf.log"
ssh_key="${HOME}/.ssh/yshell_test_ed25519"
ssh_port="${YSHELL_TEST_SSH_PORT:-2222}"

usage() {
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
  --theme) theme="${2:-}" ; shift 2 ;;
  --lang) lang="${2:-}" ; shift 2 ;;
  --outdir) outdir="${2:-}" ; shift 2 ;;
  --prefix) prefix="${2:-}" ; shift 2 ;;
  -h | --help) usage; exit 0 ;;
  *) echo "未知参数：$1" >&2; usage >&2; exit 2 ;;
  esac
done

E2E_SHOT_PREFIX="$prefix-$theme-$lang"
E2E_THEME="$theme"
mkdir -p "$outdir"

trap 'e2e_stop_app' EXIT
e2e_require_tools || exit 1
if [ ! -x "$bin" ]; then
  e2e_warn "找不到可执行文件：$bin（先 cargo xtask build）"
  exit 1
fi
container_status=$(bash "$repo_dir/scripts/test-ssh/run.sh" status 2>&1 || true)
if ! grep -q 'Up ' <<<"$container_status"; then
  e2e_warn "测试容器未运行：先执行 bash scripts/test-ssh/run.sh up"
  exit 1
fi
e2e_ensure_xvfb "$display" || exit 1

rm -rf "$config_dir"
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
    echo "  { id = \"perf-$i\", name = \"P$i\", host = \"127.0.0.1\", port = $ssh_port, username = \"root\", auth_profile_id = \"auth-key\", host_key_policy = \"accept_any_for_testing\" },"
  done
  echo ']'
} >"$config_dir/config.toml"

e2e_launch_app "$bin" "$config_dir" "$theme" "$lang" "$log_file" "$E2E_DISPLAY" || exit 1

tab_active() { # 活动标签底部强调条（accent 蓝），$1 = 标签中心 x
  local r g b
  read -r r g b <<<"$(e2e_pixel_rgb "$1" 112)" || return 1
  [ "${b:-0}" -gt 120 ] && [ "${b:-0}" -gt $(( ${r:-0} + 40 )) ]
}

for i in 1 2 3 4 5 6 7 8 9 10; do
  row_y=$((239 + (i - 1) * 29))
  e2e_open_saved_session "$row_y"
  sleep 0.6
  # 新标签必为活动标签（即使滚出视口）：直接聚焦终端输入 ticker。
  xdotool mousemove --sync --window "$E2E_APP_WIN" 700 500
  sleep 0.2
  xdotool click 1
  sleep 0.3
  xdotool type --delay 25 "while true; do echo p$i-o; sleep 0.2; done"
  xdotool key Return
  sleep 0.8
  e2e_log "tab $i ticker started"
done

sleep 3
e2e_log "CPU%/RSS 采样："
for _ in 1 2 3; do
  ps -p "$E2E_APP_PID" -o %cpu=,rss= 2>/dev/null || true
  sleep 1
done

# 交互性：滚回标签条最左，点击标签 1/2，断言强调条跟随。
xdotool mousemove --sync --window "$E2E_APP_WIN" 600 100
for _ in 1 2 3 4 5 6 7 8; do
  xdotool click 4
  sleep 0.1
done
sleep 0.5
e2e_click 330 100 0.8
tab1=$([ "$(tab_active 330 && echo 1 || echo 0)" = 1 ] && echo 1 || echo 0)
e2e_click 480 100 0.8
tab2=$([ "$(tab_active 480 && echo 1 || echo 0)" = 1 ] && echo 1 || echo 0)
e2e_log "responsive: tab1=$tab1 tab2=$tab2"
e2e_check_shot "$outdir" "10-tabs-streaming" "10 标签同时输出（可交互）"
e2e_check "perf/UI 可响应" "点击标签 1/2 强调条跟随" \
  "$([ "$tab1" = 1 ] && [ "$tab2" = 1 ] && echo 1 || echo 0)" \
  "tab1=$tab1 tab2=$tab2"

if e2e_summary; then
  exit 0
else
  exit 1
fi
