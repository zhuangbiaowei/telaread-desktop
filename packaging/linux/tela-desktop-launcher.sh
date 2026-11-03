#!/usr/bin/env bash
# 从桌面启动器（Omarchy: SUPER + SPACE → 输入 tela）启动 Tela Desktop。
#
# 由 ~/.local/share/applications/tela-desktop.desktop 调用，因此没有终端窗口，
# 所有输出都写进 ~/.local/state/tela-desktop.log。
#
# 默认运行 release 构建（target/release/tela-gui）；不存在时自动构建一次。
# 需要调试二进制时：TELA_PROFILE=debug tela-desktop
#
# 这个文件是模板：`packaging/linux/install-desktop-entry.sh` 把 @ROOT@ 替换成
# 仓库绝对路径后装到 ~/.local/bin/tela-desktop。
set -euo pipefail

profile="${TELA_PROFILE:-release}"
case "$profile" in
  release | debug) ;;
  *)
    echo "tela-desktop: TELA_PROFILE 只能是 release 或 debug（当前：$profile）" >&2
    exit 2
    ;;
esac

root="${TELA_PROJECT_ROOT:-@ROOT@}"
cd "$root"

# 本地配置（client id / 数据库路径 / API 地址），该文件被 .gitignore 忽略。
if [[ -f tmp/tela.env ]]; then
  # shellcheck disable=SC1091
  source tmp/tela.env
fi
export TELA_DEMO="${TELA_DEMO:-0}"

log="${XDG_STATE_HOME:-$HOME/.local/state}/tela-desktop.log"
mkdir -p "${log%/*}"
{
  echo "---- $(date '+%F %T') tela-desktop ($profile) $* ----"
} >>"$log"

notify() {
  command -v notify-send >/dev/null 2>&1 || return 0
  notify-send -a "Tela Desktop" "$1" "$2" >/dev/null 2>&1 || true
}

# ---- 本地代理探测 ----
# 启动前先探测本地代理（默认 127.0.0.1:7888，其次常见端口）：
#   探测成功 -> 导出 http_proxy/https_proxy/all_proxy，Tela 走代理；
#   探测失败 -> 不导出任何代理变量，Tela 直连。
# 覆盖方式（写进 tmp/tela.env 即可）：
#   TELA_PROXY=http://127.0.0.1:7890              # 只用这个地址，不做多端口探测
#   TELA_PROXY_CANDIDATES="http://127.0.0.1:7888 http://127.0.0.1:7890"  # 自定义候选
#   TELA_PROXY=off                                # 完全不探测，强制直连
#   探测方式：直接对本机端口发一个 CONNECT，只要拿到 HTTP 状态行就算「已启动」；
#   不依赖外网可达性，代理自己回 5xx 也算启动。
proxy_probe() {
  local host_port=${1#*://} host port line=""
  host_port=${host_port%%/*}
  host=${host_port%:*}
  port=${host_port##*:}
  [[ -n $host && -n $port ]] || return 1
  # 先重定向 stderr，再打开 /dev/tcp：端口没监听时会报 Connection refused，不刷屏。
  {
    printf 'CONNECT api.telaread.com:443 HTTP/1.1\r\nHost: api.telaread.com:443\r\n\r\n' >&3
    IFS= read -r -t 3 -u 3 line
  } 2>/dev/null 3<>"/dev/tcp/${host}/${port}" || true
  line=${line%$'\r'}
  [[ $line == HTTP/* ]] || return 1
  printf '%s' "$line"
}

detect_proxy() {
  local url line chosen=""
  local candidates=() extra=()

  if [[ ${TELA_PROXY:-} == off ]]; then
    echo "proxy: TELA_PROXY=off，跳过探测，直连" >>"$log"
    return 0
  fi

  [[ -n ${TELA_PROXY:-} ]] && candidates+=("$TELA_PROXY")
  read -r -a extra \
    <<<"${TELA_PROXY_CANDIDATES:-http://127.0.0.1:7888 http://127.0.0.1:7890 http://127.0.0.1:1080}"
  ((${#extra[@]})) && candidates+=("${extra[@]}")

  for url in "${candidates[@]}"; do
    [[ -z $url ]] && continue
    line=$(proxy_probe "$url") || continue
    chosen=$url
    [[ $line =~ ^HTTP/[0-9.]+[[:space:]]+2 ]] ||
      echo "proxy: 注意 $url 返回「$line」，可能需要认证或尚未就绪" >>"$log"
    break
  done

  if [[ -n $chosen ]]; then
    export http_proxy="$chosen" https_proxy="$chosen" all_proxy="$chosen"
    export HTTP_PROXY="$chosen" HTTPS_PROXY="$chosen" ALL_PROXY="$chosen"
    export no_proxy="localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"
    export NO_PROXY="$no_proxy"
    echo "proxy: 本地代理已启动，使用 $chosen" >>"$log"
  else
    echo "proxy: 未检测到本地代理（尝试过：${candidates[*]:-无}），直连" >>"$log"
  fi
}

detect_proxy

bin="target/$profile/tela-gui"

if [[ ! -x $bin ]]; then
  echo "tela-desktop: $bin 不存在，先做 $profile 构建" >>"$log"
  notify "正在构建 Tela Desktop" "首次 $profile 构建较慢，窗口稍后出现。"
  if ! cargo build --profile "$profile" -p tela-app --features gui --bin tela-gui --locked >>"$log" 2>&1; then
    echo "tela-desktop: $profile 构建失败，详见 $log" >>"$log"
    notify "Tela Desktop 启动失败" "$profile 构建失败，详见 $log"
    exit 1
  fi
fi

exec "$bin" "$@" >>"$log" 2>&1
