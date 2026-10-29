#!/usr/bin/env bash
# 以真实 API + 本地库启动 GUI。
#
# 读取（已被 .gitignore 忽略的）tmp/tela.env，避免每次手动 source。
# 用法：
#   scripts/tela-gui-real.sh            # 打开窗口
#   scripts/tela-gui-real.sh --check    # 无窗口数据冒烟
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [[ -f tmp/tela.env ]]; then
  # shellcheck disable=SC1091
  source tmp/tela.env
fi

if [[ -z "${TELA_CLIENT_ID:-}" ]]; then
  cat >&2 <<'MSG'
tela-gui-real: 缺少 TELA_CLIENT_ID。
先在 tmp/tela.env 写入：
  export TELA_CLIENT_ID=tela_app_…
  export TELA_DB=/绝对路径/tela.db
  export TELA_API_BASE_URL=https://api.telaread.com
MSG
  exit 10
fi

export TELA_DEMO=0
exec cargo run -p tela-app --features gui --bin tela-gui --locked -- "$@"
