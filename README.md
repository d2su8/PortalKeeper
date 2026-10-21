# PortalKeeper

[![CI](https://github.com/d2su8/PortalKeeper/actions/workflows/build.yml/badge.svg)](https://github.com/d2su8/PortalKeeper/actions/workflows/build.yml)
[![Release](https://img.shields.io/github/v/release/d2su8/PortalKeeper)](https://github.com/d2su8/PortalKeeper/releases)
[![License: MIT](https://img.shields.io/github/license/d2su8/PortalKeeper)](LICENSE)
[![Downloads](https://img.shields.io/github/downloads/d2su8/PortalKeeper/total)](https://github.com/d2su8/PortalKeeper/releases)

OpenWrt 上的 zaxsoft（石斧软件）portal 校园网认证小工具。开机自动认证、掉线自动重连，新版 SPA 与旧版表单门户自动识别。

- Rust 静态单二进制 + LuCI 界面，单包安装，无运行库依赖
- 开机自动认证，掉线自动重连；在线期间不发多余流量
- 逐线路联通测试、多线多拨（默认关闭）、电脑/手机槽位选择
- 日志只写 `/tmp`（tmpfs），不磨损闪存

<details>
<summary><b>English</b> — OpenWrt campus network auto-login (captive portal) plugin</summary>

**PortalKeeper** is an OpenWrt / LuCI plugin for automatic campus network (captive portal)
authentication, built as a single static Rust binary.

- Auto-login on boot, auto-reconnect on disconnect; no extra traffic while online
- Supports both new SPA token-based portals (e.g. zaxsoft) and legacy form-based portals
- Per-line connectivity test, multi-WAN multi-dial (off by default), device slot selection
- Logs to tmpfs only — no flash wear
- Single-package install (`.apk` / `.ipk`) for aarch64 / armv7 / x86_64 / mipsel

</details>

## 安装

### 使用预编译包

1. 前往 [Releases 页面](https://github.com/d2su8/PortalKeeper/releases)。
2. 根据路由器架构下载对应的 `.apk`（新版固件，apk-tools）或 `.ipk`（老固件，opkg）包：

   | 包名中的架构 | 适用设备 |
   |---|---|
   | `aarch64_cortex-a53` | 主流新路由器（MediaTek mt798x 等 ARMv8 机型） |
   | `arm_cortex-a7` | ARMv7 老机型 |
   | `x86_64` | x86 软路由 |
   | `mipsel_24kc` | 老 MIPS 路由器 |

   不确定架构时，在路由器上执行 `ubus call system board`，看返回里的 `release.target`（apk 固件）；老固件用 `opkg print-architecture`。
3. 上传到路由器后安装：

   ```sh
   # 新版固件(apk)
   apk add --allow-untrusted /tmp/portalkeeper_*.apk
   # 老固件(opkg)
   opkg install /tmp/portalkeeper_*.ipk
   ```

   卸载：

   ```sh
   apk del portalkeeper      # 新版固件
   opkg remove portalkeeper  # 老固件(配置 /etc/config/portalkeeper 会保留, 需手动删除)
   ```

打开 LuCI → **服务 → PortalKeeper** → 填账号密码 → 在顶部状态卡勾选「启用」→ 点「保存并应用」。

账号密码明文存于 `/etc/config/portalkeeper`，请勿外传配置文件。

## 仓库结构

```text
PortalKeeper/
├── daemon/                  # Rust 守护进程(认证引擎, 静态单二进制)
│   ├── src/
│   │   ├── main.rs          #   CLI 入口: daemon/check/login/offline/status/detect-portal
│   │   ├── daemon.rs        #   守护循环: 周期探测/掉线重连/重试策略/状态文件
│   │   ├── portal.rs        #   门户协议: 302 劫持识别、SPA 令牌登录、强制下线
│   │   ├── http.rs          #   极简 HTTP 客户端: 绑定源 IP/出口网卡(多线多拨)
│   │   ├── uci.rs           #   UCI 配置解析
│   │   └── logging.rs       #   日志(只写 /tmp, 超限滚动裁剪)
│   └── .cargo/config.toml   #   4 架构 musl 静态链接配置
├── portalkeeper/            # OpenWrt 单包(守护进程 + LuCI 界面一体)
│   ├── Makefile             #   OpenWrt SDK 打包定义
│   ├── files/               #   procd init 脚本 + UCI 默认配置
│   ├── htdocs/              #   LuCI 视图(打包时映射到 /www/luci-static)
│   └── usr/                 #   菜单、rpcd ACL、ucode 后端(ubus 对象)
├── build/                   # 免 SDK 打包脚本(build-ipk.sh / build-apk.sh)
├── .github/workflows/       # CI: 多架构交叉编译 + 打包, tag 自动发 Release
├── BUILD.md                 # 编译/打包/安装详细指南(WSL)
├── CHANGELOG.md
└── README.md
```

## 编译

见 [BUILD.md](BUILD.md)。

## 免责声明

- 本项目仅供学习交流使用，请遵守所在学校/机构的网络使用规定。使用本项目产生的一切后果由使用者自行承担。
- 本脚本并非破解软件，不提供破解功能，无任何入侵和破解行为。
- 本脚本免费发布并无任何盈利行为，请勿商用。
