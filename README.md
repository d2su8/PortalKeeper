# PortalKeeper

本项目为https://github.com/d2su8/HZXYNET-AutoLogin 认证项目的延伸至OpenWrt 路由器上的校园网认证插件

认证协议按 bossWeb / zaxsoft（石斧软件）门户实测：未认证时网关 302 劫持任意 HTTP → 取会话参数 →
提交表单 → 复核放行；判定只看响应头，不解析中文文案。

- Rust 静态单二进制 `portalkeeperd` + LuCI 中文界面，opkg 安装，无运行库依赖
- 开机自动认证，掉线自动重连；在线期间不发多余流量
- 逐线路联通测试（可自定义探测网站）、多线多拨（默认关闭）、电脑端/手机端槽位选择
- 日志只写 `/tmp`（tmpfs），不写闪存；记录服务启停与每次认证过程

## 插件截图
<img width="1901" height="913" alt="屏幕截图 2026-09-17 191621" src="https://github.com/user-attachments/assets/54e79d9e-b1a6-47af-ad52-aab24308f038" />
<img width="1896" height="915" alt="屏幕截图 2026-09-17 191629" src="https://github.com/user-attachments/assets/ec141332-8270-4d9a-9968-3873a1ea8703" />


## 安装

```sh
scp portalkeeper_*.ipk luci-app-portalkeeper_*.ipk root@<路由器IP>:/tmp/
ssh root@<路由器IP>
opkg install /tmp/portalkeeper_*.ipk /tmp/luci-app-portalkeeper_*.ipk
```

然后打开 LuCI → **服务 → PortalKeeper** → 填账号密码 → 勾选「启用」→ 点页面底部「保存并应用」。

- 账号密码以明文保存在路由器 `/etc/config/portalkeeper`，请勿把配置文件或其备份外传。
- 卸载：`opkg remove luci-app-portalkeeper portalkeeper`；配置需另行删除 `/etc/config/portalkeeper`。

### 不用 LuCI 也能跑

界面包 `luci-app-portalkeeper` 是可选的，核心包自带 init 脚本与 UCI 配置：

```sh
uci set portalkeeper.main.username='账号'
uci set portalkeeper.main.password='密码'
uci set portalkeeper.main.enabled='1'
uci commit portalkeeper
/etc/init.d/portalkeeper enable          # 开机自启
/etc/init.d/portalkeeper start
```

必需参数只有账号和密码；网卡留空自动检测，重试间隔/次数、探测网站留空就用默认值。
缺必需参数时服务不会启动，缺什么会写进 `/tmp/portalkeeper.log`。

命令行工具：`portalkeeperd check`（只探测不登录）、`login`（认证一次）、`status`（看线路状态）、
`daemon`（前台跑，调试用）。

## 页面

| 页面 | 内容 |
|---|---|
| 概览 | 运行状态（含逐线路联通测试）、认证服务（账号 / WAN 口 / 槽位 / 掉线自动重试 / 自定义探测网站 / 门户地址）、线路信息、下线指引 |
| 高级设置 | 第二线路（多线多拨，默认关闭） |
| 日志 | 查看 / 刷新 / 清空 |

## 换学校 / 换门户

门户地址不写死：未认证时网关会 302 劫持任意 HTTP，程序直接跟着跳转地址走，
表单字段与账号/密码字段名也从登录页现解析，所以换到别的同类门户（web 门户 + 表单提交）通常不用改任何东西。

探测不到劫持时（该校 AC 不用 302 劫持、或路由器不在校园网），日志会提示手填「门户地址」：
浏览器打开任意 http 网站（如 `http://www.msftconnecttest.com/connecttest.txt`）→ 把地址栏跳转后的地址
整条复制 → 填进 LuCI「概览 → 认证服务 → 门户地址」保存并应用（或 `uci set portalkeeper.main.portal_url='地址'`）。

排查用：`portalkeeperd detect-portal`（只探测门户地址，不登录），`--portal '地址'` 可临时指定。

## 实测环境

CMCC RAX3000M（NAND）/ MediaTek mt798x / QWRT R26.9.18（LuCI openwrt-25.12 代）/ 内核 6.6.129 / opkg。
门户是贺州学院校园网的 bossWeb（zaxsoft 石斧）。

## 编译

见 [BUILD.md](BUILD.md)：WSL Ubuntu 里
`cargo build --release --target aarch64-unknown-linux-musl`，再用 `build/build-ipk.sh` 打成两个 ipk。


## 相关项目

同属「校园网 + OpenWrt」场景，常与本插件一起使用：

- [UA3F](https://github.com/SunBK201/UA3F) —— HTTP(S) 重写代理，以 HTTP / SOCKS5 / TPROXY / REDIRECT / NFQUEUE 等服务方式透明重写 HTTP(S) 流量（如 User-Agent）
- [UA2F](https://github.com/Zxilly/UA2F) —— 在 OpenWrt 路由器上把 User-Agent 改成固定字符串，避免被网关检测

分工上互不冲突，可以同时装：UA3F / UA2F 处理「网关看到几台设备」，PortalKeeper 处理「这条线路怎么认证上线」。

## 免责声明
  
- 本项目仅供学习交流使用，请遵守所在学校/机构的网络使用规定。使用本项目产生的一切后果由使用者自行承担。
- 本脚本并非破解软件，不提供破解功能，无任何入侵和破解行为。
- 本脚本免费发布并无任何盈利行为，请勿商用。

## License

[MIT](LICENSE)
