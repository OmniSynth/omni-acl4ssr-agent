#!/usr/bin/env bash
# 本机交叉编译并部署到 ImmortalWrt/OpenWrt
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
HOST="${1:-root@172.16.1.1}"
TARGET=x86_64-unknown-linux-musl

cd "$ROOT"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-x86_64-linux-musl-gcc}"
export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-x86_64-linux-musl-gcc}"

echo "== 交叉编译 =="
cargo build --release --target "$TARGET" -p omni-acl4ssr-agent
x86_64-linux-musl-strip "target/$TARGET/release/omni-acl4ssr-agent" || true

echo "== 构建前端 =="
(cd web && npm run build)

echo "== 打包 =="
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/omni-pkg"
cp "target/$TARGET/release/omni-acl4ssr-agent" "$STAGE/omni-pkg/"
cp -a openwrt/files "$STAGE/omni-pkg/"
cp -a web/dist "$STAGE/omni-pkg/web-dist"
cp openwrt/seed-config.json "$STAGE/omni-pkg/"
# 去掉 macOS 扩展属性文件
find "$STAGE/omni-pkg" -name '._*' -delete 2>/dev/null || true

cat > "$STAGE/omni-pkg/do-install.sh" <<'EOF'
#!/bin/sh
set -e
cd /tmp/omni-pkg
mkdir -p /usr/bin /etc/omni-acl4ssr-agent /usr/share/omni-acl4ssr-agent/web
mkdir -p /etc/init.d /etc/config
mkdir -p /usr/share/luci/menu.d /usr/share/rpcd/acl.d
mkdir -p /www/luci-static/resources/view/omni-acl4ssr-agent

cp -f omni-acl4ssr-agent /usr/bin/omni-acl4ssr-agent
chmod 0755 /usr/bin/omni-acl4ssr-agent
cp -a files/. /
chmod 0755 /etc/init.d/omni-acl4ssr-agent
find /usr/share/omni-acl4ssr-agent /www/luci-static/resources/view/omni-acl4ssr-agent -name '._*' -exec rm -f {} \;
rm -rf /usr/share/omni-acl4ssr-agent/web/*
cp -a web-dist/. /usr/share/omni-acl4ssr-agent/web/
# 从旧品牌 irez-acl4ssr 迁移数据（若存在）
if [ -d /etc/irez-acl4ssr ] && [ ! -f /etc/omni-acl4ssr-agent/config.json ]; then
  cp -a /etc/irez-acl4ssr/. /etc/omni-acl4ssr-agent/ 2>/dev/null || true
fi
if [ -f /etc/config/irez_acl4ssr ] && [ ! -f /etc/config/omni_acl4ssr_agent ]; then
  sed 's/irez_acl4ssr/omni_acl4ssr_agent/g' /etc/config/irez_acl4ssr > /etc/config/omni_acl4ssr_agent
fi
# 停用并移除旧品牌 irez-acl4ssr，避免仍打开旧控制台
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
# 已有配置不覆盖，避免冲掉用户订阅与落地代理
if [ ! -f /etc/omni-acl4ssr-agent/config.json ]; then
  cp -f seed-config.json /etc/omni-acl4ssr-agent/config.json
fi
[ -f /etc/config/omni_acl4ssr_agent ] || cp files/etc/config/omni_acl4ssr_agent /etc/config/omni_acl4ssr_agent
# 旧安装补齐 status 段（LuCI TableSection 依赖）
uci -q get omni_acl4ssr_agent.status >/dev/null || {
  uci -q set omni_acl4ssr_agent.status=status
  uci -q commit omni_acl4ssr_agent
}
chown -R root:root /usr/bin/omni-acl4ssr-agent /etc/omni-acl4ssr-agent /usr/share/omni-acl4ssr-agent \
  /usr/share/luci/menu.d/luci-app-omni-acl4ssr-agent.json \
  /usr/share/rpcd/acl.d/luci-app-omni-acl4ssr-agent.json \
  /www/luci-static/resources/view/omni-acl4ssr-agent \
  /etc/init.d/omni-acl4ssr-agent /etc/config/omni_acl4ssr_agent 2>/dev/null || true

/etc/init.d/omni-acl4ssr-agent enable
/etc/init.d/omni-acl4ssr-agent restart
sleep 2
wget -qO- http://127.0.0.1:8787/api/health || true
echo
echo INSTALL_DONE
EOF
chmod +x "$STAGE/omni-pkg/do-install.sh"

export COPYFILE_DISABLE=1
tar -C "$STAGE" -czf /tmp/omni-pkg.tar.gz omni-pkg

echo "== 上传并安装到 $HOST =="
cat /tmp/omni-pkg.tar.gz | ssh -o BatchMode=yes "$HOST" \
  'rm -rf /tmp/omni-pkg; cat > /tmp/omni-pkg.tar.gz && cd /tmp && tar xzf omni-pkg.tar.gz && sh /tmp/omni-pkg/do-install.sh'

ssh -o BatchMode=yes "$HOST" '/etc/init.d/rpcd restart; /etc/init.d/uhttpd restart' || true
echo "完成。LuCI：服务 → 订阅转换；控制台 http://路由器IP:8787/ ；语音请用 https://路由器IP:8788/ （自签证书需浏览器信任一次）；Nikki 订阅可用 http://127.0.0.1:8787/sub"
