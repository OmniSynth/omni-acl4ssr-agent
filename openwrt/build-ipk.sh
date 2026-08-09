#!/usr/bin/env bash
# 交叉编译并打成 OpenWrt/ImmortalWrt opkg 包（x86_64 .ipk）
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PKG_NAME="omni-acl4ssr-agent"
ARCH="x86_64"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)}"
OUT_DIR="${OUT_DIR:-$ROOT/dist}"
SKIP_BUILD="${SKIP_BUILD:-0}"

export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-x86_64-linux-musl-gcc}"
export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-x86_64-linux-musl-gcc}"
export COPYFILE_DISABLE=1

BIN="target/$TARGET/release/$PKG_NAME"

if [ "$SKIP_BUILD" != "1" ]; then
  echo "== 交叉编译 ($TARGET) =="
  cargo build --release --target "$TARGET" -p "$PKG_NAME"
  if command -v x86_64-linux-musl-strip >/dev/null 2>&1; then
    x86_64-linux-musl-strip "$BIN" || true
  elif command -v musl-strip >/dev/null 2>&1; then
    musl-strip "$BIN" || true
  elif command -v strip >/dev/null 2>&1; then
    strip "$BIN" || true
  fi

  echo "== 构建前端 =="
  (
    cd web
    if [ -f package-lock.json ]; then
      npm ci
    else
      npm install
    fi
    npm run build
  )
fi

[ -f "$BIN" ] || { echo "缺少二进制: $BIN" >&2; exit 1; }
[ -f web/dist/index.html ] || { echo "缺少前端: web/dist" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
DATA="$STAGE/data"
CTRL="$STAGE/control"
mkdir -p "$DATA" "$CTRL" "$OUT_DIR"

echo "== 组装 rootfs =="
install -d \
  "$DATA/usr/bin" \
  "$DATA/etc/init.d" \
  "$DATA/etc/config" \
  "$DATA/etc/omni-acl4ssr-agent" \
  "$DATA/usr/share/omni-acl4ssr-agent/web" \
  "$DATA/usr/share/luci/menu.d" \
  "$DATA/usr/share/rpcd/acl.d" \
  "$DATA/www/luci-static/resources/view/omni-acl4ssr-agent"

install -m 0755 "$BIN" "$DATA/usr/bin/omni-acl4ssr-agent"
cp -a openwrt/files/. "$DATA/"
chmod 0755 "$DATA/etc/init.d/omni-acl4ssr-agent"
cp -a web/dist/. "$DATA/usr/share/omni-acl4ssr-agent/web/"
install -m 0644 openwrt/seed-config.json "$DATA/usr/share/omni-acl4ssr-agent/seed-config.json"
# 首次安装默认配置；升级时由 conffiles / postinst 保护用户数据
install -m 0644 openwrt/seed-config.json "$DATA/etc/omni-acl4ssr-agent/config.json"

find "$DATA" -name '._*' -delete 2>/dev/null || true
find "$DATA" -name '.DS_Store' -delete 2>/dev/null || true

INSTALLED_SIZE="$(
  if command -v du >/dev/null 2>&1; then
    du -sk "$DATA" | awk '{print $1 * 1024}'
  else
    echo 0
  fi
)"

cat > "$CTRL/control" <<EOF
Package: $PKG_NAME
Version: $VERSION
Depends: libc, luci-base
License: MIT
Section: net
Architecture: $ARCH
Installed-Size: $INSTALLED_SIZE
Description: 本地 Mihomo 订阅转换控制台（LuCI + Web UI + Nikki）
 omni-acl4ssr-agent：国家分组 / 规则集 / 局域网分流 / 落地代理 / AI Agent。
 LuCI：服务 → 订阅转换；控制台 http://路由器IP:8787/ ；订阅 /sub。
EOF

cat > "$CTRL/conffiles" <<'EOF'
/etc/config/omni_acl4ssr_agent
/etc/omni-acl4ssr-agent/config.json
EOF

cat > "$CTRL/postinst" <<'EOF'
#!/bin/sh
[ -n "${IPKG_INSTROOT:-}" ] && exit 0

# 旧品牌迁移
if [ -d /etc/irez-acl4ssr ] && [ ! -f /etc/omni-acl4ssr-agent/config.json ]; then
  mkdir -p /etc/omni-acl4ssr-agent
  cp -a /etc/irez-acl4ssr/. /etc/omni-acl4ssr-agent/ 2>/dev/null || true
fi
if [ -f /etc/config/irez_acl4ssr ] && [ ! -f /etc/config/omni_acl4ssr_agent ]; then
  sed 's/irez_acl4ssr/omni_acl4ssr_agent/g' /etc/config/irez_acl4ssr > /etc/config/omni_acl4ssr_agent
fi
if [ -x /etc/init.d/irez-acl4ssr ]; then
  /etc/init.d/irez-acl4ssr stop 2>/dev/null || true
  /etc/init.d/irez-acl4ssr disable 2>/dev/null || true
fi
rm -f /usr/bin/irez-acl4ssr \
  /etc/init.d/irez-acl4ssr \
  /usr/share/luci/menu.d/luci-app-irez-acl4ssr.json \
  /usr/share/rpcd/acl.d/luci-app-irez-acl4ssr.json 2>/dev/null || true
rm -rf /usr/share/irez-acl4ssr \
  /www/luci-static/resources/view/irez-acl4ssr 2>/dev/null || true

if [ ! -f /etc/omni-acl4ssr-agent/config.json ] && [ -f /usr/share/omni-acl4ssr-agent/seed-config.json ]; then
  mkdir -p /etc/omni-acl4ssr-agent
  cp -f /usr/share/omni-acl4ssr-agent/seed-config.json /etc/omni-acl4ssr-agent/config.json
fi

if command -v uci >/dev/null 2>&1; then
  uci -q get omni_acl4ssr_agent.status >/dev/null || {
    uci -q set omni_acl4ssr_agent.status=status
    uci -q commit omni_acl4ssr_agent
  }
fi

[ -x /etc/init.d/omni-acl4ssr-agent ] && {
  /etc/init.d/omni-acl4ssr-agent enable
  /etc/init.d/omni-acl4ssr-agent restart
}
/etc/init.d/rpcd restart 2>/dev/null || true
/etc/init.d/uhttpd restart 2>/dev/null || true
exit 0
EOF

cat > "$CTRL/prerm" <<'EOF'
#!/bin/sh
[ -n "${IPKG_INSTROOT:-}" ] && exit 0
[ -x /etc/init.d/omni-acl4ssr-agent ] && {
  /etc/init.d/omni-acl4ssr-agent stop 2>/dev/null || true
  /etc/init.d/omni-acl4ssr-agent disable 2>/dev/null || true
}
exit 0
EOF

chmod 0755 "$CTRL/postinst" "$CTRL/prerm"

echo "== 打包 ipk =="
# GNU tar 用 --owner/--group；BSD tar（macOS）用 --uid/--gid；都不支持则不加
tar_pack() {
  local out="$1"
  shift
  if tar --owner=0 --group=0 -czf "$out" "$@" 2>/dev/null; then
    return 0
  fi
  if tar --uid=0 --gid=0 -czf "$out" "$@" 2>/dev/null; then
    return 0
  fi
  tar -czf "$out" "$@"
}

(
  cd "$CTRL"
  tar_pack "$STAGE/control.tar.gz" .
)
(
  cd "$DATA"
  tar_pack "$STAGE/data.tar.gz" .
)

printf '2.0\n' > "$STAGE/debian-binary"
IPK_NAME="${PKG_NAME}_${VERSION}_${ARCH}.ipk"
IPK_PATH="$OUT_DIR/$IPK_NAME"

rm -f "$IPK_PATH"
# macOS BSD ar 会写成带 __.SYMDEF 的 ranlib 库，opkg 无法识别；用 GNU ar 或手写 Debian ar
make_ipk_ar() {
  local out="$1"
  shift
  if command -v gar >/dev/null 2>&1; then
    gar rcS "$out" "$@"
    return
  fi
  if command -v gnar >/dev/null 2>&1; then
    gnar rcS "$out" "$@"
    return
  fi
  # GNU ar（Linux CI）：-S 禁止生成符号表
  if ar --version 2>&1 | grep -qi gnu; then
    ar rcS "$out" "$@"
    return
  fi
  # 手写 !<arch> 成员头（Debian/OpenWrt ipk 兼容）
  python3 - "$out" "$@" <<'PY'
import sys
from pathlib import Path

out = Path(sys.argv[1])
members = [Path(p) for p in sys.argv[2:]]
with out.open("wb") as f:
    f.write(b"!<arch>\n")
    for p in members:
        data = p.read_bytes()
        name = (p.name + "/").encode("ascii")[:16]
        header = (
            name.ljust(16)
            + b"0".ljust(12)
            + b"0".ljust(6)
            + b"0".ljust(6)
            + b"100644".ljust(8)
            + str(len(data)).encode("ascii").ljust(10)
            + b"`\n"
        )
        f.write(header)
        f.write(data)
        if len(data) % 2 == 1:
            f.write(b"\n")
PY
}

(
  cd "$STAGE"
  make_ipk_ar "$IPK_PATH" debian-binary control.tar.gz data.tar.gz
)

(
  cd "$OUT_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$IPK_NAME" > SHA256SUMS
  else
    shasum -a 256 "$IPK_NAME" > SHA256SUMS
  fi
)

echo "完成: $IPK_PATH"
cat "$OUT_DIR/SHA256SUMS"
