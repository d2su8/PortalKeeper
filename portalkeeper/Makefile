# 校园网自动认证 单包: portalkeeper(守护进程 + LuCI 界面一体)
# 说明: 二进制由 build/build-ipk.sh 交叉编译(aarch64 musl 静态)后
#       放到 files/usr/bin/portalkeeperd(不入库); 本 Makefile 供 OpenWrt SDK 标准打包使用,
#       也可以完全不用 SDK —— 直接运行 build-ipk.sh 生成 ipk。
#
# 单包整合: 守护进程 + init 脚本 + UCI 默认配置 + LuCI 界面(视图/菜单/ACL/rpcd ucode)
# 全部装进一个 portalkeeper 包, 不再拆分 luci-app-portalkeeper。
include $(TOPDIR)/rules.mk

PKG_NAME:=portalkeeper
PKG_VERSION:=1.3.1
PKG_RELEASE:=1
PKG_MAINTAINER:=PortalKeeper contributors
PKG_LICENSE:=MIT

include $(INCLUDE_DIR)/package.mk

define Package/portalkeeper
  SECTION:=net
  CATEGORY:=Network
  TITLE:=Campus portal auto-auth daemon + LuCI UI (Rust, static)
  # 单包自带 LuCI 界面, 直接依赖 LuCI 相关组件
  DEPENDS:=+luci-base +rpcd +ucode +ucode-mod-fs +ucode-mod-uci +ucode-mod-ubus
  # 旧包迁移: 声明替代旧的 luci-app-portalkeeper, 避免升级残留/冲突
  PROVIDES:=luci-app-portalkeeper
  CONFLICTS:=luci-app-portalkeeper
  URL:=https://github.com/d2su8/PortalKeeper
endef

define Package/portalkeeper/description
  校园网自动认证守护进程(周期探测门户劫持并认证保活) + LuCI 管理界面。
  Rust 静态编译(aarch64 musl), 无额外运行库依赖; 日志只写 /tmp(tmpfs), 避免磨损 NAND。
endef

define Build/Compile
	# 交叉编译按 BUILD.md 在 WSL 中手动执行, 这里只打包预编译产物:
	# 请先把 portalkeeperd 放到 ./files/usr/bin/portalkeeperd
endef

define Package/portalkeeper/install
	# —— 守护进程与系统脚本 ——
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) ./files/usr/bin/portalkeeperd $(1)/usr/bin/portalkeeperd
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./files/etc/init.d/portalkeeper $(1)/etc/init.d/portalkeeper
	$(INSTALL_DIR) $(1)/etc/uci-defaults
	$(INSTALL_BIN) ./files/etc/uci-defaults/99-portalkeeper $(1)/etc/uci-defaults/99-portalkeeper

	# —— LuCI 界面(视图运行时路径是 /www/luci-static, 仓库放 htdocs, 打包时映射到 www) ——
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/portalkeeper
	$(CP) ./htdocs/luci-static/resources/view/portalkeeper/* $(1)/www/luci-static/resources/view/portalkeeper/
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) ./usr/share/luci/menu.d/*.json $(1)/usr/share/luci/menu.d/
	$(INSTALL_DIR) $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) ./usr/share/rpcd/acl.d/*.json $(1)/usr/share/rpcd/acl.d/
	$(INSTALL_DIR) $(1)/usr/share/rpcd/ucode
	$(INSTALL_DATA) ./usr/share/rpcd/ucode/*.uc $(1)/usr/share/rpcd/ucode/
endef

$(eval $(call BuildPackage,portalkeeper))
