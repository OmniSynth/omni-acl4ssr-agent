# omni-acl4ssr-agent

轻量本地 **Mihomo / Clash Meta** 订阅转换控制台（Rust + React），面向 OpenWrt / ImmortalWrt + Nikki。

- 拉取并聚合机场 Clash 订阅
- 按国家 / 自定义策略组编排出口
- 规则集（AI、币安、奈飞等）与局域网分流
- SOCKS5 / HTTP 落地代理，支持 `dialer-proxy` 链式
- 侧栏 AI Agent 协助改配置；一键更新 Nikki、打开控制面板
- 稳定订阅地址：`http://<host>:8787/sub`

> 截图经 [jsDelivr](https://www.jsdelivr.com/) 加速：`cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/...`

## 界面预览

### 概况

上游订阅、档案与 Nikki 订阅地址；侧栏 AI 助手随时可用。

![概况](https://cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/docs/images/overview.png)

### 策略组

自动托管地区组，或完全自定义策略组。

![策略组](https://cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/docs/images/strategy-group.png)

### 规则集

按域名 / GEO 绑定策略组。

![规则集](https://cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/docs/images/rule-set.png)

### 局域网分流

从 OpenWrt DHCP 选择设备（主机名 · IP · MAC），按源 IP 整机分流。

![局域网分流](https://cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/docs/images/lan-split.png)

### 落地代理

SOCKS5 / HTTP 落地，可配合前置策略组做链式代理。

![落地代理](https://cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/docs/images/proxy-egress-rules.png)

### AI 模型与供应商

切换 Gemini / DeepSeek，配置 Key、上下文与思考模式。

![模型与供应商](https://cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/docs/images/model-provider.png)

### 对话历史

多轮配置对话、归档与分支。

![对话历史](https://cdn.jsdelivr.net/gh/OmniSynth/omni-acl4ssr-agent@main/docs/images/chat-history.png)

## 快速开始

### 构建前端

```bash
cd web
npm install
npm run build
```

### 运行后端

```bash
cargo run -p omni-acl4ssr-agent
```

浏览器打开 `http://127.0.0.1:8787/`。

默认监听 `0.0.0.0:8787`，配置写入 `./data/config.json`。

| 变量 | 默认 | 说明 |
|------|------|------|
| `OMNI_LISTEN` | `0.0.0.0:8787` | HTTP 监听 |
| `OMNI_TLS_LISTEN` | HTTP 端口 +1 | HTTPS（语音等）；空字符串关闭 |
| `OMNI_DATA_DIR` | `data` | 配置目录 |
| `OMNI_WEB_DIR` | `web/dist` | 前端静态目录 |

### 前端开发

```bash
cd web && npm install && npm run dev
```

开发服 `http://127.0.0.1:5173`，API 代理到 `8787`。

## OpenWrt / ImmortalWrt

### 安装（推荐）

从 [GitHub Releases](https://github.com/OmniSynth/omni-acl4ssr-agent/releases) 下载 **x86_64** `.ipk`：

```bash
opkg install omni-acl4ssr-agent_1.0.0_x86_64.ipk
```

推送 `v*` tag 时，CI 会自动交叉编译并上传 ipk。

本机打包：

```bash
# 依赖：rustup target x86_64-unknown-linux-musl、x86_64-linux-musl-gcc（或 musl-gcc）、npm
./openwrt/build-ipk.sh
# 产物：dist/omni-acl4ssr-agent_<version>_x86_64.ipk
```

### 开发部署

仓库 [`openwrt/`](openwrt/) 含 procd 服务、UCI、LuCI 菜单；也可直接推到路由器：

```bash
./openwrt/deploy-to-router.sh root@172.16.1.1
```

| 入口 | 地址 |
|------|------|
| LuCI | 服务 → 订阅转换 |
| Web 控制台 | `http://路由器IP:8787/` |
| HTTPS（语音） | `https://路由器IP:8788/` |
| Nikki 订阅 | `http://127.0.0.1:8787/sub` |

顶栏提供 **打开面板**（Nikki UI）与 **更新订阅**（拉取订阅并重载）。

## Nikki 接入

1. 概况页填写上游机场订阅并保存  
2. 调整策略组 / 规则集 / 局域网分流 / 落地代理  
3. 「立即转换」确认成功  
4. Nikki 订阅 URL 填 `http://127.0.0.1:8787/sub`（本机）或路由器局域网地址  
5. 点顶栏「更新订阅」拉取并重载生效  

## API（节选）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/health` | 健康检查 |
| GET/PUT | `/api/profile` | 档案 |
| GET/PUT | `/api/groups` | 策略组 |
| GET/PUT | `/api/rulesets` | 规则集 |
| GET/PUT | `/api/landings` | 落地代理 |
| GET/PUT | `/api/lan-routes` | 局域网分流 |
| GET | `/api/dhcp-clients` | OpenWrt DHCP 列表 |
| POST | `/api/nikki/update-subscription` | 更新 Nikki 订阅并重载 |
| POST | `/api/convert` | 立即转换 |
| GET | `/sub` | Nikki 订阅 YAML |

## 说明

- 不做完整 Subconverter / ACL4SSR `.ini` 兼容  
- 单档案，配置为本地 JSON  
- 订阅结果有短时缓存  
- OpenWrt 提供预编译 x86_64 `.ipk`（非完整 SDK 源码包）  

## 许可证

MIT
