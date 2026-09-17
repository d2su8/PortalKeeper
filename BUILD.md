# PortalKeeper OpenWrt 插件 — 编译与安装指南(WSL Ubuntu)

> 实测/目标平台: CMCC RAX3000M(NAND) / MediaTek mt798x / ARMv8 Cortex-A53 ×2 /
> QWRT R26.9.18(LuCI openwrt-25.12 代) / 内核 6.6.129 / 包管理 opkg;
> 其他 aarch64 的 OpenWrt 系固件同理, 换 `build/build-ipk.sh` 里的 `ARCH` 重新打包即可。
>
> **推荐流程: 把源码复制进 WSL 的 Linux 文件系统(~)里编译。**
> 不要直接在 `/mnt/d/...` 下跑 cargo —— /mnt 走 9p 文件协议, cargo 的上万个小文件会慢 5~10 倍。
> 下文用 `<仓库>` 表示本仓库在你机器上的位置; 命令里的 `192.168.5.1` 是本机路由器的管理地址, 按自己的改。
>
> 产物: `portalkeeper_*.ipk`(核心守护, 静态编译) + `luci-app-portalkeeper_*.ipk`(Web 管理界面)
> 原则: 日志只写 /tmp(tmpfs), 不落闪存避免 NAND 磨损; Rust 静态链接, 无任何 .so 依赖

---

## 0. 前置环境(一次性, 已完成的可跳过)

```bash
sudo apt update
sudo apt install -y build-essential curl wget file binutils

# Rust + aarch64 musl 目标
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup target add aarch64-unknown-linux-musl

# musl 交叉工具链(提供 aarch64-linux-musl-gcc 链接器)
cd ~
wget https://musl.cc/aarch64-linux-musl-cross.tgz
tar -xzf aarch64-linux-musl-cross.tgz
echo 'export PATH="$HOME/aarch64-linux-musl-cross/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
aarch64-linux-musl-gcc --version   # 能打印版本号即就绪
```

## 1. 复制源码进 WSL 文件系统

```bash
cp -r "<仓库>" ~/portalkeeper
```

之后每次在 Windows 侧改了代码, **只重拷改动的部分**(整个重拷会把 WSL 里的编译缓存也覆盖掉,
导致下次全量重编):

```bash
# 例: 只更新 Rust 源码
cp -r "<仓库>/daemon/src" ~/portalkeeper/daemon/
```

## 2. 修复 Windows 行尾(CRLF, 必做一次)

Windows 侧 git 签出的脚本可能带 CRLF, 在 Linux 里会报 `$'\r': command not found`:

```bash
find ~/portalkeeper -type f \( -name '*.sh' -o -path '*/etc/init.d/*' -o -name '99-portalkeeper' \) \
     -exec sed -i 's/\r$//' {} +
```

## 3. 编译守护进程

```bash
cd ~/portalkeeper/daemon
cargo build --release --target aarch64-unknown-linux-musl

# 确认是静态链接
file target/aarch64-unknown-linux-musl/release/portalkeeperd
# 应显示: ELF 64-bit LSB executable, ARM aarch64, statically linked
```

> 仓库自带 `daemon/.cargo/config.toml`(指定 musl 链接器 + crt-static)。
> 若工具链装在别的路径, 把其中 `linker` 改成绝对路径即可。

## 4. 打包 ipk

```bash
cd ~/portalkeeper/build
./build-ipk.sh
# 产物:
#   ~/portalkeeper/build/out/portalkeeper_1.0.0-1_aarch64_cortex-a53.ipk
#   ~/portalkeeper/build/out/luci-app-portalkeeper_1.0.0-1_all.ipk
```

## 5. 安装到路由器

```bash
scp ~/portalkeeper/build/out/*.ipk root@192.168.5.1:/tmp/
ssh root@192.168.5.1
opkg install /tmp/portalkeeper_*.ipk /tmp/luci-app-portalkeeper_*.ipk
```

> 不想用界面的话可以不装 `luci-app-portalkeeper`: 核心包自带 init 脚本 + UCI 配置, 设好
> `portalkeeper.main.username` / `password` 与 `enabled='1'` 后 `uci commit` +
> `/etc/init.d/portalkeeper start` 即可(详见 README「不用 LuCI，纯命令行怎么跑」)。
> 参数不全时服务不会启动, 原因写在 `/tmp/portalkeeper.log`。

浏览器打开 `http://192.168.5.1/cgi-bin/luci/` → **服务 → PortalKeeper**:

1. **概览 → 运行状态**: 看服务是否「运行中」、网络是否正常(打开页面会自动逐条线路做一次联通测试, 也可点标题行右侧「联通测试」重测)
2. **概览 → 认证服务**: 填账号密码, 选 WAN 口(或留空自动检测), 选设备认证类型, 需要的话填「自定义探测网站」, 勾选「启用 PortalKeeper 服务」→ 点页面底部「保存并应用」启动
3. **概览 → 线路信息**: 核对每条线路的网卡/IP/MAC 与各自联网状态(开了多线多拨时线路一/线路二左右并排)
4. **日志**: 查看服务启停记录与每次探测/认证的详细过程

## 6. 改代码后的迭代流程

```bash
# Windows 侧改完源码后:
cp -r "<仓库>/daemon/src" ~/portalkeeper/daemon/
cd ~/portalkeeper/daemon
cargo build --release --target aarch64-unknown-linux-musl
cd ../build && ./build-ipk.sh && scp out/*.ipk root@192.168.5.1:/tmp/
```

---

## 路径 B(可选): 完整 OpenWrt SDK 打包

适合想走标准 feed/源码包流程的情况。本包设计为「外部交叉编译 + SDK/脚本打包」两段式,
Rust 交叉编译仍建议按路径 A 完成:

```bash
# 1. 下载与固件匹配的 SDK: mediatek/filogic (aarch64_cortex-a53), 解压进入 SDK 根目录
./scripts/feeds update -a
./scripts/feeds install -a luci-base

# 2. 链接两个包目录
ln -s <仓库>/portalkeeper package/portalkeeper
ln -s <仓库>/luci-app-portalkeeper package/luci-app-portalkeeper

# 3. 先按路径 A 编译好 portalkeeper, 放进包的 files 下
cp ~/portalkeeper/daemon/target/aarch64-unknown-linux-musl/release/portalkeeperd \
   <仓库>/portalkeeper/files/usr/bin/portalkeeper

# 4. 编译
make defconfig
make package/portalkeeper/compile V=s
make package/luci-app-portalkeeper/compile V=s
# 产物在 bin/packages/aarch64_cortex-a53/
```

---

## 常见问题

| 现象 | 处理 |
|---|---|
| `cd D:\git\...` 报 No such file | WSL 用 `/mnt/d/...` 正斜杠路径; 但编译请按 §1 复制到 `~` 下进行 |
| shell 脚本报 `$'\r': command not found` | CRLF 行尾, 执行 §2 的 sed 命令 |
| cargo 在 /mnt/d 下奇慢 | /mnt 走 9p 协议, 按推荐流程复制到 `~` 下编译 |
| `aarch64-linux-musl-gcc: not found` | musl 交叉工具链未进 PATH, 见 §0 |
| 编译产物 `dynamically linked` | 确认用了 `--target aarch64-unknown-linux-musl` 且 `.cargo/config.toml` 生效(crt-static) |
| opkg 安装报架构不符 | 固件须为 aarch64_cortex-a53(mediatek/filogic); 其他架构改 build-ipk.sh 的 ARCH 并重编 |
| LuCI 菜单不出现 | `rm /tmp/luci-indexcache* && /etc/init.d/rpcd restart && /etc/init.d/uhttpd restart` |
| 概览显示「未运行」 | `/etc/init.d/portalkeeper start`; 仍不行看 `logread \| grep portalkeeper` |
| 概览显示「未运行」但服务已启动 | 刷新页面(运行状态按 `pidof portalkeeperd` 实时查询) |
| 联通测试「无法判定」 | 该线路未接校园网口, 或 WAN 口/源 IP 配置错误 |
| 状态显示「no-credentials」 | 概览页「认证服务」里未填账号密码 |
| 状态显示「exhausted」 | 连续认证失败已达次数上限被暂停; 排查后点「保存并应用」重启服务重置 |
| 认证频繁失败提示限流 | 门户限流: 把「重试认证间隔」调大到 ≥60 秒再试 |

## 卸载

```sh
opkg remove luci-app-portalkeeper
opkg remove portalkeeper
# 配置 /etc/config/portalkeeper 会保留, 手动删除即可
```
