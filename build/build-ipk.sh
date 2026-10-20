#!/usr/bin/env bash
# 校园网自动认证 ipk 打包(免 OpenWrt SDK):
#   1. 先按 BUILD.md 在 WSL Ubuntu 里交叉编译出 aarch64 musl 静态二进制 portalkeeperd
#   2. 本脚本把 portalkeeperd + 配置/init 脚本 + LuCI 界面(视图/菜单/ACL/rpcd ucode)
#      打成一个单包 portalkeeper_*.ipk(不再拆分 luci-app-portalkeeper)
# 容器格式(与路由器 2021-06-13 版 opkg 实测匹配, 参照官方包与 luci-app-oxidns 实包):
#   ipk 本体 = gzip 压缩的 PAX tar, 成员依次为 control.tar.gz → data.tar.gz
#   (无 ./ 前缀、无 debian-binary 成员; 内层 tar 成员带 ./ 前缀与官方一致)
# 用法: ./build-ipk.sh [portalkeeper 二进制路径(默认取 daemon/target/.../release/portalkeeperd)]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEF_BIN="$ROOT/daemon/target/aarch64-unknown-linux-musl/release/portalkeeperd"
BIN="${1:-$DEF_BIN}"
OUT="$ROOT/build/out"
STAGE="$OUT/.stage"
ARCH="${ARCH:-aarch64_cortex-a53}"
REL="${REL:-1}"

[ -x "$BIN" ] || {
	echo "!! 未找到 portalkeeperd 二进制: $BIN"
	echo "   先按 openwrt/BUILD.md 执行: cargo build --release --target aarch64-unknown-linux-musl"
	exit 1
}

VER="$(grep -m1 '^version' "$ROOT/daemon/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
rm -rf "$STAGE"
mkdir -p "$OUT"

# 打包函数: name version arch data_dir control_dir
# control_dir 内须含 control 文件(可附带 postinst/postrm 等可选脚本)
make_ipk() {
	local name="$1" ver="$2" arch="$3" data="$4" ctldir="$5"
	local work="$STAGE/$name"
	rm -rf "$work"
	mkdir -p "$work/data" "$work/CONTROL"
	cp -a "$ctldir/." "$work/CONTROL/"
	# 快速校验 control 文件: 除空行外每行必须是 "Field: value" 形式,
	# 否则 opkg 会报 "Malformed package file"(如 Installed-Size 拼进多行数字)
	if grep -vE '^[A-Za-z][A-Za-z0-9-]*: ' "$work/CONTROL/control" | grep -q '[^[:space:]]'; then
		echo "ERROR: control 文件存在非法行, 拒绝打包:" >&2
		sed -n '1,40p' "$work/CONTROL/control" >&2
		rm -rf "$work"
		exit 1
	fi
	cp -a "$data/." "$work/data/"
	# 内层 tar 与官方/oxidns 实包一致: 成员带 ./ 前缀、root 属主
	tar --format=ustar --numeric-owner --group=0 --owner=0 -C "$work/CONTROL" -czf "$work/control.tar.gz" .
	tar --format=ustar --numeric-owner --group=0 --owner=0 -C "$work/data" -czf "$work/data.tar.gz" .
	# 外层容器: gzip 压缩 USTAR tar, 成员 control.tar.gz → data.tar.gz(实测路由器可装格式)。
	# 注意必须 ustar: pax 会为亚秒时间戳生成 'x' 扩展头, 老版 opkg 不识别(Typeflag 0x78 报错)
	local ipk="$OUT/${name}_${ver}_${arch}.ipk"
	rm -f "$ipk"
	( cd "$work" && tar --format=ustar --numeric-owner --group=0 --owner=0 \
	  -czf "$ipk" control.tar.gz data.tar.gz )
	# 打包后自检容器结构, 避免把坏包传到路由器
	set -- $(tar tzf "$ipk")
	case "$#:$1:$2" in
		2:control.tar.gz:data.tar.gz) : ;;
		*) echo "ERROR: 容器结构异常($# 成员: $*), 已删除坏包" >&2; rm -f "$ipk"; rm -rf "$work"; exit 1 ;;
	esac
	rm -rf "$work"
	echo "生成: $ipk"
}

# ---------- 单包 portalkeeper (守护进程 + LuCI 界面一体) ----------
DATA="$STAGE/portalkeeper-data"
mkdir -p "$DATA/usr/bin" "$DATA/etc/init.d" "$DATA/etc/uci-defaults" "$DATA/www"
install -m 0755 "$BIN" "$DATA/usr/bin/portalkeeperd"
install -m 0755 "$ROOT/portalkeeper/files/etc/init.d/portalkeeper" "$DATA/etc/init.d/portalkeeper"
# uci-defaults 脚本必须可执行, 否则开机初始化不会运行(实测 0644 导致 Permission denied)
install -m 0755 "$ROOT/portalkeeper/files/etc/uci-defaults/99-portalkeeper" "$DATA/etc/uci-defaults/99-portalkeeper"
# LuCI 界面: usr 下是 menu.d / rpcd(acl.d + ucode); 视图运行时路径是 /www/luci-static(仓库放 htdocs, 映射到 www)
cp -a "$ROOT/portalkeeper/usr" "$DATA/"
cp -a "$ROOT/portalkeeper/htdocs/luci-static" "$DATA/www/"

CTLDIR="$STAGE/portalkeeper-control"
mkdir -p "$CTLDIR"
cat > "$CTLDIR/control" <<EOF
Package: portalkeeper
Version: ${VER}-${REL}
Architecture: ${ARCH}
Maintainer: PortalKeeper contributors
Section: net
Depends: luci-base, rpcd, ucode, ucode-mod-fs, ucode-mod-uci, ucode-mod-ubus
Provides: luci-app-portalkeeper
Conflicts: luci-app-portalkeeper
Replaces: luci-app-portalkeeper
Description: 校园网自动认证守护进程(周期探测门户劫持并认证保活) + LuCI 管理界面。Rust 静态编译, 日志仅写 /tmp。
Installed-Size: $(du -sk "$DATA" | cut -f1)
EOF

# 安装后: 首次安装执行 uci-defaults(建默认配置 + 开机自启链接) + 清 LuCI 缓存 + 重启 rpcd(菜单/视图刷新)
cat > "$CTLDIR/postinst" <<'EOF'
#!/bin/sh
[ -n "${IPKG_INSTROOT:-}" ] && exit 0
[ -x /etc/uci-defaults/99-portalkeeper ] && /etc/uci-defaults/99-portalkeeper || true
rm -f /tmp/luci-indexcache* 2>/dev/null || true
rm -rf /tmp/luci-modulecache/* 2>/dev/null || true
if [ -d /www/luci-static/resources/view/portalkeeper ]; then
	find /www/luci-static/resources/view/portalkeeper -type f -name '*.js' -exec touch {} + 2>/dev/null || true
fi
if [ -x /etc/init.d/rpcd ]; then
	/etc/init.d/rpcd restart >/dev/null 2>&1 || true
fi
exit 0
EOF
cat > "$CTLDIR/postrm" <<'EOF'
#!/bin/sh
[ -n "${IPKG_INSTROOT:-}" ] && exit 0
rm -f /tmp/luci-indexcache* 2>/dev/null || true
rm -rf /tmp/luci-modulecache/* 2>/dev/null || true
exit 0
EOF
chmod 0755 "$CTLDIR/postinst" "$CTLDIR/postrm"
make_ipk "portalkeeper" "${VER}-${REL}" "${ARCH}" "$DATA" "$CTLDIR"

echo
echo "完成。安装方法(单包, 守护进程 + LuCI 界面一体):"
echo "  cat $OUT/portalkeeper_*.ipk | ssh root@<路由器IP> 'cat > /tmp/portalkeeper.ipk'"
echo "  ssh root@<路由器IP> 'opkg install /tmp/portalkeeper.ipk'"