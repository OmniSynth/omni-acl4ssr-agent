#!/bin/sh
# BusyBox 可用：将已打包的 files + 二进制装到 rootfs（一般请用 deploy-to-router.sh）
set -e
ROOT="${1:-/}"
BIN="${2:-}"
[ -n "$BIN" ] && [ -f "$BIN" ] || { echo "用法: $0 <rootfs前缀> <二进制>"; exit 1; }

FILES_DIR="$(CDPATH= cd -- "$(dirname "$0")/files" && pwd)"
WEB_SRC="$(CDPATH= cd -- "$(dirname "$0")/../web/dist" && pwd)"
SEED="$(dirname "$0")/seed-config.json"

mkdir -p "$ROOT/usr/bin" "$ROOT/etc/omni-acl4ssr-agent" "$ROOT/usr/share/omni-acl4ssr-agent/web"
cp -f "$BIN" "$ROOT/usr/bin/omni-acl4ssr-agent"
chmod 0755 "$ROOT/usr/bin/omni-acl4ssr-agent"
cp -a "$FILES_DIR"/. "$ROOT/"
chmod 0755 "$ROOT/etc/init.d/omni-acl4ssr-agent"
cp -a "$WEB_SRC"/. "$ROOT/usr/share/omni-acl4ssr-agent/web/"
[ -f "$SEED" ] && cp -f "$SEED" "$ROOT/etc/omni-acl4ssr-agent/config.json"
echo "安装完成"
