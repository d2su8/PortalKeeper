# LuCI 前端包: luci-app-portalkeeper (LuCI2 / openwrt-25.12 JS 视图 + rpcd ucode 后端)
include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-portalkeeper
PKG_VERSION:=1.0.0
PKG_RELEASE:=1
PKG_LICENSE:=MIT
PKG_MAINTAINER:=PortalKeeper contributors

LUCI_TITLE:=校园网自动认证 LuCI 界面 (Portal Auto-Auth)
LUCI_DEPENDS:=+portalkeeper +luci-base +rpcd +ucode +ucode-mod-fs +ucode-mod-uci +ucode-mod-ubus
LUCI_PKGARCH:=all

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature
