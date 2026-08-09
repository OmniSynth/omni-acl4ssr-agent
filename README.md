# omni-acl4ssr-agent

轻量本地 **Mihomo / Clash Meta** 订阅转换控制台（Rust + React）。

- 拉取机场 Clash 订阅
- 按正则做国家 / 自定义策略组
- 合并本地规则集（AI、币安、奈飞等）
- 追加 SOCKS5 / HTTP 落地，并支持 `dialer-proxy` 前置梯子
- 提供稳定订阅地址给 Nikki：`http://<host>:8787/sub`

## 快速开始

### 0. 构建前端（首次或改 UI 后）

```bash
cd web
npm install
npm run build
```

### 1. 后端

```bash
cargo run -p omni-acl4ssr-agent
```

浏览器打开 `http://127.0.0.1:8787/` 进入控制台。

默认监听 `0.0.0.0:8787`，配置写入 `./data/config.json`。

环境变量：

| 变量 | 默认 | 说明 |
|------|------|------|
| `OMNI_LISTEN` | `0.0.0.0:8787` | 监听地址 |
| `OMNI_DATA_DIR` | `data` | 配置目录 |
| `OMNI_WEB_DIR` | `web/dist` | 前端静态目录 |

### 2. 前端（开发）

```bash
cd web
npm install
npm run dev
```

开发服 `http://127.0.0.1:5173`，API 代理到 `8787`。

### 3. 前端（生产）

```bash
cd web && npm run build
# 然后 cargo run，由 Axum 托管 web/dist
```

## Nikki 接入

1. 概况页填写**上游机场订阅 URL** 并保存
2. 按需调整策略组 / 规则集 / 落地代理
3. 预览页点「立即转换」确认成功
4. Nikki → 订阅 → URL 填：

```text
http://172.16.1.2:8787/sub
```

（把 IP 换成实际跑本服务的机器）

5. 更新订阅；默认出口为「🚀 默认」（骨架里指向香港等）

## API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/health` | 健康检查 |
| GET/PUT | `/api/profile` | 档案 |
| GET/PUT | `/api/groups` | 策略组 |
| GET/PUT | `/api/rulesets` | 规则集 |
| GET/PUT | `/api/landings` | 落地代理 |
| POST | `/api/convert` | `{"include_yaml":true}` 立刻转换 |
| GET | `/sub` | Nikki 订阅（YAML） |

## 落地链式代理

在「落地代理」中新增 SOCKS5/HTTP，`dialer-proxy` 选 `🇭🇰 香港`（或其它组）。  
转换结果里该节点带 `dialer-proxy`，并加入「⛓ 链路」组；规则集可把指定域名指到该组。

## OpenWrt / ImmortalWrt 插件

仓库内 [`openwrt/`](openwrt/) 含：

- `files/etc/init.d/omni-acl4ssr-agent`：procd 服务
- `files/etc/config/omni_acl4ssr_agent`：UCI
- `files/usr/share/luci/menu.d/...`：LuCI **服务 → 订阅转换**
- `files/www/luci-static/resources/view/omni-acl4ssr-agent/app.js`：控制台 iframe 页
- `seed-config.json`：默认上游订阅与策略骨架
- `deploy-to-router.sh`：本机 musl 交叉编译并一键部署

```bash
# 依赖：rustup target x86_64-unknown-linux-musl、x86_64-linux-musl-gcc、npm
./openwrt/deploy-to-router.sh root@172.16.1.1
```

部署后：

| 入口 | 地址 |
|------|------|
| LuCI | 服务 → 订阅转换 |
| Web 控制台 | `http://路由器IP:8787/` |
| Nikki 订阅 | `http://127.0.0.1:8787/sub` |

当前测试机已验证：`/api/convert` 成功（约 180 节点 / 8 组），Nikki 运行配置含香港/美国/新加坡/AI/币安/奈飞/默认组。

## 说明

- 不做完整 Subconverter / ACL4SSR `.ini` 兼容
- MVP 单档案，配置为本地 JSON 文件
- 订阅结果缓存约 60 秒
- OpenWrt 包为预编译二进制安装（非完整 SDK 源码包）
