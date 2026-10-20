#!/usr/bin/env bash
# PortalKeeper 打包(新版 OpenWrt / apk): 生成 APKv3 单包
#   portalkeeper_<版本>-r1.apk   守护进程 + LuCI 界面一体(静态二进制 + procd init + UCI 默认配置 + 视图/菜单/ACL/rpcd ucode)
#
# 单包整合, 不再拆分 luci-app-portalkeeper。
#
# 适用固件: 包管理器为 apk 的 OpenWrt(apk-tools 3.x, 包格式 APKv3/ADB, 魔数 "ADBd")。
#           老固件(opkg, 包格式 ipk)请用同目录的 build-ipk.sh。
#
# 依赖 apk-tools v3 的 mkpkg 子命令。取静态版(能在任意 x86_64 Linux 跑):
#   mkdir -p ~/apktool && cd ~/apktool
#   curl -L -o apk-tools-static.apk \
#     https://dl-cdn.alpinelinux.org/alpine/edge/main/x86_64/apk-tools-static-3.0.8-r0.apk
#   tar xzf apk-tools-static.apk sbin/apk.static && chmod +x sbin/apk.static
# 包内文件属主必须是 root:root, 所以本脚本要用 root 跑(WSL: wsl -d Ubuntu -u root bash build-apk.sh)。
#
# 用法: sudo/root:  ./build-apk.sh [portalkeeperd 二进制路径]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEF_BIN="$ROOT/daemon/target/aarch64-unknown-linux-musl/release/portalkeeperd"
BIN="${1:-$DEF_BIN}"
OUT="$ROOT/build/out-apk"
STAGE="$OUT/.stage"
ARCH="${ARCH:-aarch64_cortex-a53}"
REL="${REL:-1}"
APK="${APK:-$HOME/apktool/apk.static}"
URL_REPO="https://github.com/d2su8/PortalKeeper"

[ -x "$BIN" ] || {
	echo "!! 未找到 portalkeeperd 二进制: $BIN"
	echo "   先按 openwrt/BUILD.md 执行: cargo build --release --target aarch64-unknown-linux-musl"
	exit 1
}
[ -x "$APK" ] || {
	echo "!! 未找到 apk 工具(apk-tools v3): $APK"
	echo "   见本脚本头部注释下载 apk-tools-static"
	exit 1
}
[ "$(id -u)" = "0" ] || {
	echo "!! 需要 root 运行(否则包内文件属主是当前用户, 路由器上装不上/权限不对)"
	echo "   WSL: wsl -d Ubuntu -u root bash $0"
	exit 1
}

VER="$(grep -m1 '^version' "$ROOT/daemon/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
rm -rf "$STAGE"
mkdir -p "$OUT"

# 归一化属主/权限: 包内必须是 root:root, 且 rpcd 会拒绝加载"非 root 属主或对外可写"的 ucode 插件
# (实测: nobody:nogroup 0777 的 portalkeeper.uc 不会被 rpcd 加载, ubus 里查不到对象)
fix_ownership() {
	local dir="$1"
	chown -R 0:0 "$dir"
	find "$dir" -type d -exec chmod 0755 {} +
	find "$dir" -type f -exec chmod 0644 {} +
	# 可执行文件按 0755(init.d / uci-defaults / 二进制)
	find "$dir" -type f -path '*/etc/init.d/*' -exec chmod 0755 {} +
	find "$dir" -type f -path '*/etc/uci-defaults/*' -exec chmod 0755 {} +
	[ -f "$dir/usr/bin/portalkeeperd" ] && chmod 0755 "$dir/usr/bin/portalkeeperd"
	return 0
}

# 打一个 apk: name 数据目录 描述 依赖 安装后脚本
# 说明: --info 的 "键:值" 只在第一个冒号处切分, 值里可以带 https:// 这类冒号
make_apk() {
	local name="$1" data="$2" desc="$3" depends="$4" postinst="${5:-}"
	local out="$OUT/${name}_${VER}-r${REL}_${ARCH}.apk"
	rm -f "$out"
	local args=(
		mkpkg
		--files "$data"
		--info "name:${name}"
		--info "version:${VER}-r${REL}"
		--info "arch:${ARCH}"
		--info "description:${desc}"
		--info "license:MIT"
		--info "origin:${name}"
		--info "url:${URL_REPO}"
		--output "$out"
	)
	[ -n "$depends" ] && args+=(--info "depends:${depends}")
	[ -n "$postinst" ] && args+=(--script "post-install:${postinst}")
	"$APK" "${args[@]}"
	[ -s "$out" ] || { echo "ERROR: 未生成 $out" >&2; exit 1; }
	# 自检: 必须是 ADBv3 容器, 且元数据齐全
	[ "$(head -c 4 "$out")" = "ADBd" ] || { echo "ERROR: $out 不是 ADBv3 包(魔数不对)" >&2; rm -f "$out"; exit 1; }
	"$APK" adbdump "$out" > "$out.dump"
	for k in "name: ${name}" "version: ${VER}-r${REL}" "arch: ${ARCH}"; do
		grep -q "^  ${k}$" "$out.dump" || { echo "ERROR: $out 元数据缺 ${k}" >&2; rm -f "$out" "$out.dump"; exit 1; }
	done
	rm -f "$out.dump"
	echo "生成: $out"
}

# ---------- 单包 portalkeeper (守护进程 + LuCI 界面一体) ----------
DATA="$STAGE/portalkeeper-data"
mkdir -p "$DATA/usr/bin" "$DATA/etc/init.d" "$DATA/etc/uci-defaults" "$DATA/www"
install -m 0755 "$BIN" "$DATA/usr/bin/portalkeeperd"
install -m 0755 "$ROOT/portalkeeper/files/etc/init.d/portalkeeper" "$DATA/etc/init.d/portalkeeper"
install -m 0755 "$ROOT/portalkeeper/files/etc/uci-defaults/99-portalkeeper" "$DATA/etc/uci-defaults/99-portalkeeper"
# LuCI 界面: usr 下是 menu.d / rpcd(acl.d + ucode); 视图运行时路径是 /www/luci-static(仓库放 htdocs, 映射到 www)
cp -a "$ROOT/portalkeeper/usr" "$DATA/"
cp -a "$ROOT/portalkeeper/htdocs/luci-static" "$DATA/www/"

POST="$STAGE/portalkeeper-postinst"
cat > "$POST" <<'EOF'
#!/bin/sh
# 首次安装即执行 uci-defaults(建默认配置 + 建开机自启链接); 再清 LuCI 缓存并重启 rpcd(菜单/视图刷新)
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
chmod 0755 "$POST"

fix_ownership "$DATA"
make_apk "portalkeeper" "$DATA" \
	"校园网自动认证守护进程(周期探测门户劫持并认证保活) + LuCI 管理界面。Rust 静态编译, 日志仅写 /tmp。" \
	"luci-base rpcd ucode" "$POST"

rm -rf "$STAGE"

echo
echo "完成。安装方法(新版 apk 固件, 单包):"
echo "  cat $OUT/portalkeeper_*.apk | ssh root@<路由器IP> 'cat > /tmp/portalkeeper.apk'"
echo "  ssh root@<路由器IP> 'apk add --allow-untrusted /tmp/portalkeeper.apk'"
echo "(本地未签名的包要带 --allow-untrusted; 若缺依赖先 apk update)"