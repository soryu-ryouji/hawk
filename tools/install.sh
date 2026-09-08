#!/usr/bin/env bash
# 本机安装：构建 hawk 桌面应用并安装到本机（编译复用 build-app.sh，不重复实现）。
# macOS 安装到 /Applications/hawk.app；Linux 安装到 ~/.local/bin/hawk 并注册用户级桌面启动器
# （AppImage 同时归置到仓库根目录的 out/）。
#
# 用法: ./tools/install.sh（仓库根目录或任意位置执行均可）
# 前置: 最新 Node.js 与 Rust 工具链（https://rustup.rs/）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$REPO_ROOT/hawk-app"
BUILD="$REPO_ROOT/tools/build-app.sh"

case "$(uname -s)" in
  Darwin)
    # --unpacked 跳过 zip 压缩，安装只需要 .app 目录
    "$BUILD" --unpacked
    APP="$(ls -d "$APP_DIR"/dist/mac*/hawk.app 2>/dev/null | head -n1)"
    [ -n "$APP" ] || { echo "打包产物不存在: dist/mac*/hawk.app（electron-builder 未产出）"; exit 1; }
    rm -rf "/Applications/hawk.app"
    cp -R "$APP" "/Applications/hawk.app"
    echo "完成：应用已安装到 /Applications/hawk.app。"
    ;;
  Linux)
    # 复用完整构建（产物同时归置到 out/），再执行用户级安装：
    # AppImage → ~/.local/bin/hawk；图标 → hicolor；启动器 → 用户应用目录（GNOME 应用列表可搜索）
    "$BUILD"
    APPIMAGE="$REPO_ROOT/out/hawk-linux-x64.AppImage"
    [ -f "$APPIMAGE" ] || { echo "打包产物不存在: $APPIMAGE（electron-builder 未产出）"; exit 1; }

    BIN_DIR="$HOME/.local/bin"
    DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
    DESKTOP_DIR="$DATA_DIR/applications"
    ICON_DIR="$DATA_DIR/icons/hicolor/512x512/apps"
    mkdir -p "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"

    # 同目录临时文件 + 原子改名：旧版本正在运行时旧 inode 继续服务运行中的进程，不会读到写了一半的文件
    TMP_BIN="$BIN_DIR/.hawk-install.$$"
    trap 'rm -f "$TMP_BIN"' EXIT
    install -m 755 "$APPIMAGE" "$TMP_BIN"
    mv -f "$TMP_BIN" "$BIN_DIR/hawk"
    install -m 644 "$APP_DIR/build/icon.png" "$ICON_DIR/hawk.png"

    # Exec 路径按 freedesktop 规范转义（双引号内需转义 \ " $ `；HOME 含空格等字符时不至于失效）
    EXEC_PATH="$(printf '%s' "$BIN_DIR/hawk" | sed 's/[\\"$`]/\\&/g')"
    cat > "$DESKTOP_DIR/hawk.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=hawk
Comment=对标 Eagle 的开源图片素材管理工具
Exec="$EXEC_PATH" %U
Icon=hawk
Terminal=false
Categories=Graphics;
StartupWMClass=hawk
EOF
    chmod 644 "$DESKTOP_DIR/hawk.desktop"

    # 刷新桌面数据库与图标缓存（GNOME 即时感知新文件，这两步主要服务其他桌面环境与 MIME 关联；缺工具则跳过）
    update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
    gtk-update-icon-cache -q -t -f "$DATA_DIR/icons/hicolor" >/dev/null 2>&1 || true

    echo "完成：应用已安装到 $BIN_DIR/hawk，启动器已注册（GNOME 应用列表搜索 hawk 即可启动）。"
    case ":$PATH:" in
      *":$BIN_DIR:"*) ;;
      *) echo "提示：$BIN_DIR 不在 PATH，终端启动请用完整路径或把该目录加入 PATH。" ;;
    esac
    ;;
  *)
    echo "不支持的平台: $(uname -s)"; exit 1
    ;;
esac
