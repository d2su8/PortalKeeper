# 更新日志

## v1.3.0

**界面重构 + 单包整合**

- UI 重构: 三个页面合并为「单页 + 顶部状态卡 + 四个标签页」(概览/认证设置/高级设置/日志);
  硬编码色值改用 LuCI 主题语义类(修复暗色主题白块、运行状态无颜色等问题);
  字段级联显示(勾选「掉线自动重试」才显示间隔/次数, 勾选「启用扩展线路」才显示第二线路参数)
- 单包整合: 删除独立的 `luci-app-portalkeeper` 包, 守护进程 + init 脚本 + UCI 配置 + LuCI 界面
  全部并入一个 `portalkeeper` 包; 主包声明 `Provides/Conflicts/Replaces: luci-app-portalkeeper` 平滑迁移旧安装

**状态卡 + 功能增强**

- 状态卡: 「启用」开关上移到顶部状态卡(即时生效, 写 uci + 启停服务, 不依赖「保存并应用」);
  新增「重启服务」按钮(等价 `/etc/init.d/portalkeeper restart`), 改完账号/网卡/门户地址后即时生效
- 运行统计: 记录每次运行的「掉线次数 / 重连成功 / 失败 / 最近失败原因」, 状态卡展示
- 强制下线: 新增 `portalkeeperd offline` 子命令 + LuCI「强制下线本机」按钮, 新版门户令牌 API
  全自动(拿令牌→查在线列表找本机→forceOffline); 认证成功时把门户基地址与网关参数持久化到 uci,
  解决重启后无劫持响应可取网关参数的问题; 旧版门户(表单式)提示走自助后台
- 自定义探测网站支持逗号分隔多个地址(任一可达即算联网)

**打包: 支持新版固件的 apk 包**

- 新固件(OpenWrt 25.12 / ImmortalWRT)包管理器换成 apk-tools 3(包格式 APKv3/ADB), ipk 装不上;
  新增 `build/build-apk.sh`, 生成 `portalkeeper_<版本>-r<N>_<架构>.apk`;
  `build-ipk.sh` 保留给还在用 opkg 的老固件

**自动构建(GitHub Actions)**

- 新增 `.github/workflows/build.yml`: 多架构矩阵交叉编译(aarch64/armv7/x86_64/mipsel 的 musl
  静态二进制) + 打包 ipk 与 apk + 上传 artifact; 打 tag(`v*`)时自动发布 GitHub Release;
  test job(cargo test / clippy / fmt)先行把关, 并校验版本号一致

## v1.2.0

**适配 2026-10 校园网门户改版(SPA + 令牌 API)**

- 登录: 登录页没有 password 输入框时判定为新版 SPA 门户, 改走 `POST <门户>/portal/webauth/auth`
  (表单编码 userId/passwd/loginMode/wlanuserip/wlanacip/wlanacname, 响应 JSON);
  老板牌表单式门户原逻辑不变(两种门户自动识别, 学校再换回去也不用改代码)
- 复核放行抽成 `recheck()`, 两种登录路径共用
- 自助管理链接: 默认 `http://10.255.2.249/self/`; `getstatus` 新增 `self_url`
  (按 uci `portal_url` 拼 `<基地址>/self/`), LuCI 概览页「打开自助管理后台(下线)」按钮跟随
- 注意: 新版接口必填 `wlanacip`/`wlanacname`; 设备在线时没有劫持 URL 可取, 所以
  「门户地址」要么填完整登录页 URL(含 query), 要么在未认证时先认证一次(daemon 从劫持 URL 取值)
- 验证: 单元测试 9 例通过; 用"假 SPA 门户"(本地模拟新版外壳 + `/portal/webauth/auth`)端到端跑通登录

## v1.1.0

**兼容更多使用 zaxsoft(石斧)门户的学校**

- 劫持识别不再认死门户 IP: 302 的目标主机与请求主机不同即判为被劫持(同主机跳转如 baidu→https 仍判无结论),
  相对路径按当前请求主机补全; 登录页请求跟随跳转(最多 3 跳), POST 打到最终 URL
- 新增 uci `portalkeeper.main.portal_url`(LuCI「认证服务 → 门户地址」): 探测不到劫持时用它,
  都没有才报 `no-portal` 并在日志里给出手动获取办法(浏览器打开任意 http 网站 → 复制跳转后的地址)
- 账号/密码字段名按登录页推断(type=password 与其前一个可见文本输入), 推不出来才回退 userId/passwd
- 新增 `portalkeeperd detect-portal` 子命令(只探测门户地址, 不登录, 探测不到退出码 2)与 `--portal` 选项
- 登录探测地址可用环境变量 `CAMPUS_AUTH_PROBE` 覆盖(逗号分隔 host[:port]/path), 与桌面版同名同格式

## v1.0.0

首个版本 —— OpenWrt / LuCI 校园网门户自动认证插件。

**守护进程 `portalkeeperd`(Rust, aarch64 musl 静态)**

- 子命令: `daemon`(守护循环) / `check`(只探测不登录的联通测试) / `login`(手动认证一次) / `status`(查看线路状态)
- 认证引擎: 探测劫持 → 会话参数 → 表单提交 → 复核放行; 撞槽位不顶号
- 启动即检测(开机自动认证), 掉线自动重试(间隔/次数可调, 连续失败暂停防限流), 在线期间不周期发包
- **启动前校验必需参数**: 只有账号 + 密码是必需的, 缺任一项就不启动, 并把缺什么写进日志
  (其余项全可留空: 网卡自动检测、间隔/次数用默认值、探测网站用内置地址表)
- 日志只写 `/tmp/portalkeeper.log`(tmpfs, 不磨损 NAND), 上限 256KB 滚动裁剪; 记录服务启停与每次探测/认证
- 捕获 SIGTERM/SIGINT, 停止时在日志留下「守护进程退出」(含运行时长)
- `probe_url` 可自定义「是否联网」的探测网站, 留空用内置地址表, 非法值自动回退

**服务与打包**

- procd init: uci `enabled` 门控开机自启; `stop_service` 钩子记录「服务停止」; 「保存并应用」即重启服务
- 启动前校验必需参数(账号 + 密码), 缺了不启动并在日志写明缺什么 —— 界面表单也不因为留空而拦保存
- 免 SDK 打包脚本 `build/build-ipk.sh`: 生成 `portalkeeper_*.ipk` 与 `luci-app-portalkeeper_*.ipk`(适配老版 opkg 的容器格式)

**LuCI 界面(服务 → PortalKeeper, 可选安装)**

- 概览: 运行状态(服务状态 + 网络是否正常 + 探测目标 + 更新时间, 标题行右侧「联通测试」)、认证服务(总开关/账号/密码/WAN 口/源 IP/设备槽位/自动重试/自定义探测网站)、线路信息(每条线路: 网卡/IP/MAC + 该线路联网状态, 多线时左右并排)、下线指引
- 高级设置: 第二线路(多线多拨, 默认关闭); 账号/密码留空沿用主线路
- 日志: 展示 / 刷新 / 清空

**文档**

- README: 启动需要什么参数(必需/可选全表) + 不用 LuCI 的纯命令行用法 + 实测环境与兼容性
- BUILD.md: WSL Ubuntu 交叉编译、打包、安装与常见问题