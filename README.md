# PortalKeeper

OpenWrt 路由器上的校园网自动认证插件：在路由器上认证一次，全屋设备共享在线。

认证协议按 bossWeb / zaxsoft（石斧软件）门户实测：未认证时网关 302 劫持任意 HTTP → 取会话参数 →
提交表单 → 复核放行；判定只看响应头，不解析中文文案。已在真实路由器与校园网环境跑通。

- Rust 静态单二进制 `portalkeeperd` + LuCI 中文界面，opkg 安装，无运行库依赖
- 开机自动认证，掉线自动重连；在线期间不发多余流量
- 逐线路联通测试（可自定义探测网站）、多线多拨（默认关闭）、电脑端/手机端槽位选择
- 日志只写 `/tmp`（tmpfs），不写闪存；记录服务启停与每次认证过程

## 安装

```sh
scp portalkeeper_*.ipk luci-app-portalkeeper_*.ipk root@<路由器IP>:/tmp/
ssh root@<路由器IP>
opkg install /tmp/portalkeeper_*.ipk /tmp/luci-app-portalkeeper_*.ipk
```

然后打开 LuCI → **服务 → PortalKeeper** → 填账号密码 → 勾选「启用」→ 点页面底部「保存并应用」。

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
| 概览 | 运行状态（含逐线路联通测试）、认证服务（账号 / WAN 口 / 槽位 / 掉线自动重试 / 自定义探测网站）、线路信息、下线指引 |
| 高级设置 | 第二线路（多线多拨，默认关闭） |
| 日志 | 查看 / 刷新 / 清空 |

## 实测环境

CMCC RAX3000M（NAND）/ MediaTek mt798x / QWRT R26.9.18（LuCI openwrt-25.12 代）/ 内核 6.6.129 / opkg。
门户是贺州学院校园网的 bossWeb（zaxsoft 石斧），换成同类门户只需改 `daemon/src/portal.rs` 里的门户地址与表单字段。

## 编译

见 [BUILD.md](BUILD.md)：WSL Ubuntu 里
`cargo build --release --target aarch64-unknown-linux-musl`，再用 `build/build-ipk.sh` 打成两个 ipk。

## 安全声明

- 本项目**仅限个人学习交流**与在自己的校园网账号下做自动化，请遵守所在学校的网络使用规定，使用后果自负。
- 只做认证自动化：不修改、不绕过门户的认证逻辑，也不会挤掉他人的在线设备。
- 账号密码以明文保存在路由器 `/etc/config/portalkeeper`，请勿把配置文件或其备份外传。
- 卸载：`opkg remove luci-app-portalkeeper portalkeeper`；配置需另行删除 `/etc/config/portalkeeper`。

## License

[MIT](LICENSE)