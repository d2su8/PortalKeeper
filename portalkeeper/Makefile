# 校园网自动认证 核心包: portalkeeper
# 说明: 二进制由 openwrt/build/build-ipk.sh 交叉编译(aarch64 musl 静态)后
#       放到 files/usr/bin/portalkeeperd(不入库); 本 Makefile 供 OpenWrt SDK 标准打包使用,
#       也可以完全不用 SDK —— 直接运行 build-ipk.sh 生成 ipk。
include $(TOPDIR)/rules.mk

PKG_NAME:=portalkeeper
PKG_VERSION:=1.0.0
PKG_RELEASE:=1
PKG_MAINTAINER:=PortalKeeper contributors
PKG_LICENSE:=MIT

include $(INCLUDE_DIR)/package.mk

define Package/portalkeeper
  SECTION:=net
  CATEGORY:=Network
  TITLE:=Campus portal auto-auth daemon (Rust, static)
  DEPENDS:=
  URL:=https://github.com/d2su8/PortalKeeper
endef

define Package/portalkeeper/description
  校园网自动认证核心守护进程: 周期探测门户劫持并自动认证保活。
  Rust 静态编译(aarch64 musl), 无额外运行库依赖; 日志只写 /tmp(tmpfs), 避免磨损 NAND。
endef

define Build/Compile
	# 交叉编译按 BUILD.md 在 WSL 中手动执行, 这里只打包预编译产物:
	# 请先把 portalkeeperd 放到 ./files/usr/bin/portalkeeperd
endef

define Package/portalkeeper/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) ./files/usr/bin/portalkeeperd $(1)/usr/bin/portalkeeperd
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./files/etc/init.d/portalkeeper $(1)/etc/init.d/portalkeeper
	$(INSTALL_DIR) $(1)/etc/uci-defaults
	$(INSTALL_BIN) ./files/etc/uci-defaults/99-portalkeeper $(1)/etc/uci-defaults/99-portalkeeper
endef

$(eval $(call BuildPackage,portalkeeper))
