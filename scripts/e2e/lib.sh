#!/usr/bin/env bash
# YShell UI E2E 通用库（Xvfb/X11 + Slint 像素断言）。
#
# 只被 `scripts/e2e/*.sh` source，不单独执行。
#
# 依赖（无头环境）：Xvfb、xdotool、ImageMagick（import/convert）、pgrep。
#
# 设计要点：
#   * 按 pid 找窗口（窗口标题会随会话/调试变化，见 docs/product/ui-winui3-verification.md §2）；
#   * 弹窗可见性用「scrim 压暗」相对判定：启动时记录菜单栏基线，弹窗打开时整窗被 scrim
#     压暗（菜单栏在弹窗范围之外）；弹窗类型用 (700,340) 是否落在主机密钥面板内区分；
#   * 会话是否连上只看 SFTP 文件列表是否出现多行（连接成功后 SFTP 才会挂载），
#     对错误路径则是"没有行"——两端都可判定；
#   * 断言逐条记录，`e2e_summary` 打印汇总表并返回非零表示有失败。
#
# 已知限制：无头 Xvfb 下 motion/悬停不产生渲染（连列表行高亮都不变），
#           本库只做点击/输入/截图/像素断言，hover 类交互必须真机人工确认。

# ---------------------------------------------------------------- 全局状态
E2E_APP_PID=""
E2E_APP_WIN=""
E2E_SHOT_PREFIX="e2e"
E2E_THEME="${E2E_THEME:-dark}"
E2E_ROWS=()
# 固定 Xvfb 几何；与 WINIT_X11_SCALE_FACTOR=1 一起保证截图可复现（96dpi、缩放 1.0）。
# 覆盖：E2E_SCREEN=1600x1000x24；HiDPI 对比：E2E_WINIT_SCALE=1.25。
E2E_SCREEN="${E2E_SCREEN:-1920x1200x24}"
E2E_WINIT_SCALE="${E2E_WINIT_SCALE:-1.0}"
# e2e_ensure_xvfb 实际选中的 display（复用 :99 几何不符时会换到空闲 display）。
E2E_DISPLAY=""

e2e_log() { printf '[e2e] %s\n' "$*"; }
e2e_warn() { printf '[e2e] !! %s\n' "$*" >&2; }

# ---------------------------------------------------------------- 前置检查
e2e_require_tools() {
  local missing=0 tool
  for tool in Xvfb xdotool import convert pgrep; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      e2e_warn "缺少工具：$tool"
      missing=1
    fi
  done
  if [ "$missing" != 0 ]; then
    e2e_warn "安装示例（WSL/Debian）：sudo apt-get install -y xvfb xdotool imagemagick"
    return 1
  fi
}

# ---------------------------------------------------------------- Xvfb
# 找一个空闲的 X display（:99 起），用于 :99 已被其它几何的 Xvfb 占用的情况。
e2e_free_display() {
  local port candidate
  for ((port = 99; port <= 220; port++)); do
    candidate=":$port"
    if [ ! -e "/tmp/.X11-unix/X$port" ] && ! pgrep -f "Xvfb $candidate" >/dev/null 2>&1; then
      echo "$candidate"
      return 0
    fi
  done
  return 1
}

# xdotool 输出 "1920 1200"，统一成 WxH 便于比较。
e2e_display_geometry() {
  xdotool getdisplaygeometry 2>/dev/null | awk '{ if (NF >= 2) print $1"x"$2 }'
}

# 启动/复用固定几何的 Xvfb。几何不符（例如复用了旧的非 1920x1200 :99）时自动换
# 一个空闲 display，避免以 1.0833 之类的非整数缩放渲染、污染像素级对比。
# 结果放在全局 E2E_DISPLAY；同时导出 DISPLAY。
e2e_ensure_xvfb() { # $1 = display（如 :99）
  local display="$1" want="${E2E_SCREEN%x*}" tag current
  if pgrep -f "Xvfb $display" >/dev/null 2>&1; then
    current=$(DISPLAY="$display" e2e_display_geometry)
    if [ -n "$current" ] && [ "$current" != "$want" ]; then
      e2e_warn "已有 Xvfb $display 几何为 $current（期望 $want），换用专用 display"
      display=$(e2e_free_display) || {
        e2e_warn "找不到空闲 display（:99–:220）"
        return 1
      }
    fi
  fi
  if ! pgrep -f "Xvfb $display" >/dev/null 2>&1; then
    tag="${display#:}"
    e2e_log "启动 Xvfb $display（-screen 0 $E2E_SCREEN）"
    Xvfb "$display" -screen 0 "$E2E_SCREEN" >"/tmp/xvfb$tag.log" 2>&1 &
    sleep 1.5
  fi
  E2E_DISPLAY="$display"
  export DISPLAY="$display"
  current=$(e2e_display_geometry)
  e2e_log "Xvfb $display 几何：${current:-未知}（期望 $want），WINIT_X11_SCALE_FACTOR=$E2E_WINIT_SCALE"
  if [ "$current" != "$want" ]; then
    e2e_warn "Xvfb $display 几何 ${current:-未知} ≠ $want，像素断言不可复现"
    return 1
  fi
}

# ---------------------------------------------------------------- 应用进程/窗口
e2e_wait_window() { # $1 = pid，$2 = 轮数（每轮 2s）→ stdout 窗口 id
  local pid="$1" attempts="$2" i candidate
  for ((i = 1; i <= attempts; i++)); do
    sleep 2
    for candidate in $(xdotool search --pid "$pid" 2>/dev/null); do
      [ "$(xdotool getwindowpid "$candidate" 2>/dev/null || echo '')" = "$pid" ] || continue
      eval "$(xdotool getwindowgeometry --shell "$candidate" 2>/dev/null)" || continue
      if [ "${WIDTH:-0}" -gt 200 ]; then
        echo "$candidate"
        return 0
      fi
    done
  done
  return 1
}

# 启动 app：固定 1440x900、窗口移动到 (0,0)（坐标均为窗口内坐标）。
# 固定 WINIT_X11_SCALE_FACTOR=1，保证 winit 设备缩放 = 1.0（见 lib.sh 全局说明）。
# 不读任何菜单：桌面版默认 native SSH，UI 已无后端切换入口；YSHELL_SSH_BACKEND
# 只供 e2e/测试覆盖（fake 离线路径等），这里显式钉住 native-ssh。
e2e_launch_app() { # $1=bin $2=config_dir $3=theme $4=lang $5=log $6=display
  local bin="$1" config_dir="$2" theme="$3" lang="$4" log="$5" display="$6"
  if [ ! -x "$bin" ]; then
    e2e_warn "找不到可执行文件：$bin（先 cargo xtask build）"
    return 1
  fi
  DISPLAY="${E2E_DISPLAY:-$display}" \
    env -u YSHELL_MASTER_PASSWORD \
    WINIT_X11_SCALE_FACTOR="$E2E_WINIT_SCALE" \
    YSHELL_SSH_BACKEND=native-ssh \
    YSHELL_THEME="$theme" \
    YSHELL_LANG="$lang" \
    YSHELL_CONFIG_DIR="$config_dir" \
    "$bin" >"$log" 2>&1 &
  E2E_APP_PID=$!
  E2E_APP_WIN=$(e2e_wait_window "$E2E_APP_PID" 15) || {
    e2e_warn "未找到窗口（pid $E2E_APP_PID），日志尾部："
    tail -5 "$log" >&2 || true
    e2e_stop_app
    return 1
  }
  xdotool windowsize --sync "$E2E_APP_WIN" 1440 900 >/dev/null 2>&1
  xdotool windowmove --sync "$E2E_APP_WIN" 0 0 >/dev/null 2>&1
  xdotool windowfocus --sync "$E2E_APP_WIN" >/dev/null 2>&1 || true
  e2e_wait_paint || true
  sleep 1
  e2e_capture_dialog_baseline
  e2e_log "app 已就绪：win=$E2E_APP_WIN pid=$E2E_APP_PID（$theme/$lang）"
}

e2e_stop_app() {
  if [ -n "$E2E_APP_PID" ]; then
    kill "$E2E_APP_PID" 2>/dev/null || true
    wait "$E2E_APP_PID" 2>/dev/null || true
    E2E_APP_PID=""
    E2E_APP_WIN=""
  fi
}

# 首帧可能是全黑（软件渲染启动慢），等到真的画出内容再交互。
e2e_wait_paint() {
  local i maxima ok
  for ((i = 1; i <= 30; i++)); do
    maxima=$(import -window "$E2E_APP_WIN" png:- 2>/dev/null \
      | convert - -colorspace gray -format '%[fx:maxima]' info: 2>/dev/null || echo 0)
    ok=$(awk -v m="${maxima:-0}" 'BEGIN { print (m > 0.5) ? 1 : 0 }')
    if [ "$ok" = "1" ]; then
      e2e_log "窗口已绘制（${i} 次探测）"
      return 0
    fi
    sleep 1
  done
  e2e_warn "窗口 30s 内未绘制出内容"
  return 1
}

# ---------------------------------------------------------------- 截图 / 交互
e2e_shot() { # $1=outdir $2=文件名（不含前缀/.png）→ stdout 文件路径
  local outdir="$1" name="$2" path
  path="$outdir/${E2E_SHOT_PREFIX}-$name.png"
  if import -window "$E2E_APP_WIN" "$path" 2>/dev/null && [ -s "$path" ]; then
    echo "$path"
    return 0
  fi
  return 1
}

e2e_click() { # $1=x $2=y [$3=点击后等待秒数]
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$1" "$2"
  sleep 0.2
  xdotool click 1
  sleep "${3:-0.5}"
}

# 右键会话树行 → 点第一项 "Open"（双击会被选中态引起的行重建吞掉，故不用双击）
e2e_open_saved_session() { # $1 = 行 y 坐标
  xdotool mousemove --sync --window "$E2E_APP_WIN" 100 "$1" click 3
  sleep 1.0
  xdotool mousemove --sync --window "$E2E_APP_WIN" 150 "$(( $1 + 20 ))"
  sleep 0.2
  xdotool click 1
  sleep 2.0
}

# ---------------------------------------------------------------- 密码弹窗坐标
# 1440×900 窗口内实测（原点已固定在 (0,0)）：输入框 / 连接 / 取消。
E2E_PASSWORD_INPUT_X=720
E2E_PASSWORD_INPUT_Y=460
E2E_PASSWORD_CONNECT_X=886
E2E_PASSWORD_CONNECT_Y=504
E2E_PASSWORD_CANCEL_X=814
E2E_PASSWORD_CANCEL_Y=504

# ---------------------------------------------------------------- 像素探测
e2e_pixel_rgb() { # $1=x $2=y → stdout "r g b"
  import -window "$E2E_APP_WIN" png:- 2>/dev/null \
    | convert - -crop 1x1+"$1"+"$2" +repage txt:- 2>/dev/null \
    | sed -n 's/.*srgb(\([0-9]*\),\([0-9]*\),\([0-9]*\)).*/\1 \2 \3/p' | tail -1
}

e2e_pixel_gray() { # $1=x $2=y → stdout 0..255
  import -window "$E2E_APP_WIN" png:- 2>/dev/null \
    | convert - -crop 1x1+"$1"+"$2" +repage -colorspace gray -format '%[fx:int(255*r)]' info: 2>/dev/null
}

e2e_region_stddev() { # $1=x $2=y $3=w $4=h → stdout 0..1
  import -window "$E2E_APP_WIN" png:- 2>/dev/null \
    | convert - -crop "${3}x${4}+${1}+${2}" +repage -colorspace gray -format '%[fx:standard_deviation]' info: 2>/dev/null
}

# 区域内出现次数最多的灰度值（mode）：对稀疏文字（终端文本、字形）比均值/单像素更稳。
e2e_region_mode_gray() { # $1=x $2=y $3=w $4=h → stdout 0..255
  import -window "$E2E_APP_WIN" png:- 2>/dev/null \
    | convert - -crop "${3}x${4}+${1}+${2}" +repage -colorspace gray -depth 8 -format %c histogram:info:- 2>/dev/null \
    | sort -rn | head -1 | sed -n 's/.*gray(\([0-9]*\)).*/\1/p'
}

# 弹窗判定（主题无关）：
#   1) 任意弹窗 → scrim 压暗整窗；菜单栏（700,20）在弹窗范围之外，用启动时基线做相对判断；
#   2) 区分类型 → (700,340) 只在主机密钥弹窗面板内（密码弹窗面板从 y=362 起），
#      弹窗打开时该点是否"面板亮"即可区分（阈值按主题）。
E2E_DIALOG_BASELINE=""

e2e_capture_dialog_baseline() {
  E2E_DIALOG_BASELINE=$(e2e_region_mode_gray 700 20 8 8)
  e2e_log "scrim 基线（菜单栏亮度）：${E2E_DIALOG_BASELINE:-未知}"
}

e2e_dialog_open() { # 任意模态弹窗
  [ -n "${E2E_DIALOG_BASELINE:-}" ] || return 1
  local current threshold
  current=$(e2e_pixel_gray 700 20)
  threshold=$(( E2E_DIALOG_BASELINE * 80 / 100 ))
  [ "${current:-999}" -lt "$threshold" ]
}

e2e_host_key_dialog_open() {
  e2e_dialog_open || return 1
  local probe
  probe=$(e2e_region_mode_gray 700 340 8 8)
  case "${E2E_THEME:-dark}" in
  light) [ "${probe:-0}" -ge 200 ] ;;
  *) [ "${probe:-0}" -ge 30 ] ;;
  esac
}

e2e_password_dialog_open() {
  e2e_dialog_open || return 1
  ! e2e_host_key_dialog_open
}

# 密码弹窗的"连接"按钮已启用（强调色比灰底更蓝；空密码时是禁用灰）。
e2e_connect_enabled() {
  local r g b
  read -r r g b <<<"$(e2e_pixel_rgb "$E2E_PASSWORD_CONNECT_X" "$E2E_PASSWORD_CONNECT_Y")" || return 1
  [ "${b:-0}" -gt $(( ${r:-0} + 20 )) ]
}

# SFTP 文件列表出现了多行 → 会话已连接且 SFTP 已挂载。
# 判据用「行带的灰度标准差」：有图标/文字的行对比度高，空行接近 0（深浅主题通用）。
#
# N3（2026-09-28）重新校准：采样区从右侧 260px 改到**名称列**（x=1108 w=70）。
# N1 之后字体/列宽的渲染使短文件名（boot/dev/…）在 x≥1180 处不再有像素，
# 但行位置未变（数据行 y=262/294/326/358/390，采样带命中 358/390）。
# 连接后 4 个采样带全命中（≥3 判定成立）；未连接时名称列无行内容，
# 只有抽屉/本地栏头部两行（计数 2）→ 与旧校准语义一致。
e2e_sftp_row_bands() {
  local i y sd count=0
  for i in 0 1 2 3; do
    y=$((352 + i * 32))
    sd=$(e2e_region_stddev 1108 "$y" 70 16)
    if awk -v s="${sd:-0}" 'BEGIN { exit !(s > 0.025) }'; then
      count=$((count + 1))
    fi
  done
  echo "$count"
}

e2e_session_connected() { [ "$(e2e_sftp_row_bands)" -ge 3 ]; }

# ---------------------------------------------------------------- 轮询 / 输入
e2e_wait_until() { # $1=轮数（每轮 0.5s） $2...=谓词命令
  local attempts="$1"
  shift
  local i
  for ((i = 1; i <= attempts; i++)); do
    if "$@"; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

# 输入密码：聚焦输入框 → 用"连接按钮已启用"确认输入生效 → 必要时重试一次。
e2e_type_password() { # $1 = 密码
  local password="$1"
  e2e_wait_until 40 e2e_password_dialog_open || {
    e2e_warn "密码弹窗未出现，无法输入"
    return 1
  }
  e2e_click "$E2E_PASSWORD_INPUT_X" "$E2E_PASSWORD_INPUT_Y" 0.8
  xdotool windowfocus --sync "$E2E_APP_WIN" >/dev/null 2>&1 || true
  xdotool type --delay 100 "$password"
  if e2e_wait_until 20 e2e_connect_enabled; then
    return 0
  fi
  e2e_warn "输入未生效，重试一次"
  e2e_click "$E2E_PASSWORD_INPUT_X" "$E2E_PASSWORD_INPUT_Y" 0.5
  xdotool windowfocus --sync "$E2E_APP_WIN" >/dev/null 2>&1 || true
  xdotool type --delay 120 "$password"
  e2e_wait_until 20 e2e_connect_enabled
}

e2e_submit_password() { # 点密码弹窗的"连接"按钮（只点一次；由调用方等结果）
  xdotool windowfocus --sync "$E2E_APP_WIN" >/dev/null 2>&1 || true
  sleep 0.3
  xdotool mousemove --sync --window "$E2E_APP_WIN" "$E2E_PASSWORD_CONNECT_X" "$E2E_PASSWORD_CONNECT_Y"
  sleep 0.2
  xdotool click 1
}

# 点到谓词满足为止（无头环境偶发点击丢失；最多 2 次，每次等 attempts 轮）。
e2e_click_until() { # $1=x $2=y $3=轮数 $4...=谓词命令
  local x="$1" y="$2" attempts="$3"
  shift 3
  local i
  for ((i = 1; i <= 2; i++)); do
    e2e_click "$x" "$y" 1.0
    if e2e_wait_until "$attempts" "$@"; then
      return 0
    fi
    e2e_warn "第 $i 次点击 ($x,$y) 后未达期望，重试"
  done
  return 1
}

# 提交密码并等到谓词满足（最多 3 次点击，防止单次点击丢失）。
e2e_submit_until() { # $1=轮数 $2...=谓词命令
  local attempts="$1"
  shift
  local i
  for ((i = 1; i <= 3; i++)); do
    e2e_submit_password
    if e2e_wait_until "$attempts" "$@"; then
      return 0
    fi
    e2e_warn "第 $i 次提交后未达期望，重试点击"
  done
  return 1
}

# ---------------------------------------------------------------- 断言汇总
e2e_check() { # $1=步骤 $2=期望 $3=ok(0/1) $4=实际
  local step="$1" expected="$2" ok="$3" actual="$4"
  E2E_ROWS+=("$step|$expected|$ok|$actual")
  if [ "$ok" = 1 ]; then
    e2e_log "PASS  $step（$actual）"
  else
    e2e_warn "FAIL  $step — 期望：$expected；实际：$actual"
  fi
}

# 按"显示宽度"补空格（CJK 3 字节字符按 2 列），让汇总表在终端里对齐。
e2e_pad() { # $1=文本 $2=目标显示宽度
  local text="$1" target="$2" width=0 i ch
  for ((i = 0; i < ${#text}; i++)); do
    ch="${text:i:1}"
    if [ "${#ch}" -eq 3 ]; then
      width=$((width + 2))
    else
      width=$((width + 1))
    fi
  done
  printf '%s' "$text"
  while [ "$width" -lt "$target" ]; do
    printf ' '
    width=$((width + 1))
  done
}

# 截图并记录一条断言（截图本身就是证据）。
e2e_check_shot() { # $1=outdir $2=文件名 $3=步骤名
  local path
  if path=$(e2e_shot "$1" "$2"); then
    e2e_check "$3" "截图已生成" 1 "$(basename "$path")"
  else
    e2e_check "$3" "截图已生成" 0 "截图失败"
  fi
}

e2e_summary() { # 打印汇总表；有失败返回 1
  local total=0 passed=0 row step expected ok actual mark
  printf '\n================ E2E 断言汇总 ================\n'
  printf '%s | %s | %s | %s\n' \
    "$(e2e_pad "步骤" 34)" "$(e2e_pad "期望" 34)" "$(e2e_pad "实际" 40)" "结果"
  printf '%s\n' "--------------------------------------------------------------------------------------------------------"
  for row in "${E2E_ROWS[@]}"; do
    IFS='|' read -r step expected ok actual <<<"$row"
    total=$((total + 1))
    if [ "$ok" = 1 ]; then
      passed=$((passed + 1))
      mark="PASS"
    else
      mark="FAIL"
    fi
    printf '%s | %s | %s | %s\n' \
      "$(e2e_pad "$step" 34)" "$(e2e_pad "$expected" 34)" "$(e2e_pad "$actual" 40)" "$mark"
  done
  printf '%s\n' "--------------------------------------------------------------------------------------------------------"
  printf '合计 %d 项：%d 通过 / %d 失败\n' "$total" "$passed" "$((total - passed))"
  [ "$total" -gt 0 ] && [ "$passed" -eq "$total" ]
}
