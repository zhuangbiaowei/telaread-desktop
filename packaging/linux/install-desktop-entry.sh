#!/usr/bin/env bash
# 把本地 release 构建装成桌面应用：
#
#   1. cargo build --release -p tela-app --features gui --bin tela-gui
#   2. ~/.local/bin/tela-desktop            ← 启动脚本（探测代理、写日志、缺二进制时构建）
#   3. ~/.local/share/applications/tela-desktop.desktop
#   4. ~/.local/share/icons/hicolor/.../tela-desktop.png
#
# 装完后 Omarchy 的 SUPER + SPACE → 输入 tela → 回车即可启动；
# 菜单里也会出现「Tela Desktop / Tela 桌面版」。
#
# 用法：
#   packaging/linux/install-desktop-entry.sh             # 构建 + 安装
#   packaging/linux/install-desktop-entry.sh --no-build  # 只装入口（用现有 release 产物）
#   packaging/linux/install-desktop-entry.sh --uninstall # 移除入口（不动仓库与数据库）
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
launcher_src="$root/packaging/linux/tela-desktop-launcher.sh"
icon_src="$root/packaging/linux/tela-desktop.png"

bin_dir="$HOME/.local/bin"
launcher="$bin_dir/tela-desktop"
app_dir="$HOME/.local/share/applications"
desktop="$app_dir/tela-desktop.desktop"
icon_dir="$HOME/.local/share/icons/hicolor/512x512/apps"
icon="$icon_dir/tela-desktop.png"

build=1
uninstall=0
for arg in "$@"; do
  case "$arg" in
    --no-build) build=0 ;;
    --uninstall) uninstall=1 ;;
    -h | --help)
      cat <<'USAGE'
用法：packaging/linux/install-desktop-entry.sh [--no-build | --uninstall]

  （无参数）     构建 release 并安装启动脚本 / .desktop / 图标
  --no-build    只安装入口，复用现有 target/release 产物
  --uninstall   移除已安装的入口（不动 target/ 与本地数据库）
USAGE
      exit 0
      ;;
    *)
      echo "未知参数：$arg（可用：--no-build / --uninstall / --help）" >&2
      exit 2
      ;;
  esac
done

if [[ $uninstall == 1 ]]; then
  rm -f "$launcher" "$desktop" "$icon"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$app_dir" || true
  echo "已移除桌面入口：$launcher / $desktop"
  echo "（target/ 与本地数据库未删除；重新安装直接再跑一次本脚本）"
  exit 0
fi

if [[ $build == 1 ]]; then
  echo "==> 构建 release（tela-gui + tela-app）"
  (cd "$root" && cargo build --release -p tela-app --features gui \
    --bin tela-gui --bin tela-app --locked)
elif [[ ! -x "$root/target/release/tela-gui" ]]; then
  echo "--no-build 但 $root/target/release/tela-gui 不存在，先构建一次" >&2
  (cd "$root" && cargo build --release -p tela-app --features gui --bin tela-gui --locked)
fi

echo "==> 安装启动脚本 $launcher"
mkdir -p "$bin_dir"
# 模板里的 @ROOT@ 换成这台机器上的仓库绝对路径（可用 TELA_PROJECT_ROOT 覆盖）。
sed "s|@ROOT@|$root|g" "$launcher_src" >"$launcher.tmp"
chmod 755 "$launcher.tmp"
mv "$launcher.tmp" "$launcher"

echo "==> 安装桌面入口 $desktop"
mkdir -p "$app_dir"
cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.5
Name=Tela Desktop
Name[zh_CN]=Tela 桌面版
GenericName=Reader
GenericName[zh_CN]=阅读器
Comment=Tela desktop reader (release build)
Comment[zh_CN]=Tela 桌面阅读器（release 构建）
Exec=$launcher
Icon=tela-desktop
Terminal=false
Categories=Network;News;
Keywords=tela;read;reader;rss;news;feed;阅读;订阅;
StartupWMClass=tela-desktop
StartupNotify=true
EOF
chmod 644 "$desktop"

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$desktop" || echo "警告：desktop-file-validate 有告警（通常不影响启动）" >&2
fi

echo "==> 安装图标 $icon"
mkdir -p "$icon_dir"
install -m 644 "$icon_src" "$icon"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$app_dir" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 &&
  gtk-update-icon-cache -qtf "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

cat <<MSG

完成。SUPER + SPACE → 输入 tela → 回车启动（也可以用菜单里的「Tela 桌面版」）。

  二进制：$root/target/release/tela-gui
  入口：  $desktop
  日志：  \${XDG_STATE_HOME:-\$HOME/.local/state}/tela-desktop.log
  调试：  TELA_PROFILE=debug tela-desktop
  配置：  $root/tmp/tela.env（TELA_CLIENT_ID / TELA_DB / 代理…）
MSG
