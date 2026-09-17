'use strict';
'require view';
'require form';
'require rpc';
'require uci';
'require ui';

var callGetIfaces = rpc.declare({ object: 'portalkeeper', method: 'getifaces' });

return view.extend({
	load: function() {
		return callGetIfaces().then(function(res) {
			return res || {};
		});
	},

	render: function(res) {
		var ifs = (res && res.interfaces) ? res.interfaces : [];
		var extra = (res && res.extra) ? res.extra : [];
		var seenDev = {};

		var m = new form.Map('portalkeeper', _('高级设置 — 第二线路(多线多拨)'),
			_('校园网「单线多拨」不可行(边缘交换机会丢弃同一物理口上第二个 MAC 的流量), ' +
			  '多拨只能走「多线多拨」: 用另一条物理线路(另一个校园网口)再认证一个会话。' +
			  '此功能默认关闭, 不影响单线路用户。'));

		var s = m.section(form.NamedSection, 'extra', 'line', '扩展线路(第二 WAN)');
		s.addremove = false;

		var o = s.option(form.Flag, 'enabled', _('启用扩展线路'),
			'tip: 开启后守护进程除了主 WAN, 还会用下面的网卡+账号再做一次独立认证, ' +
			'与主 WAN 各占一个槽位(互不影响)。两个线路必须来自不同物理口, 源 IP 不同。');
		o.default = '0';
		o.rmempty = false;

		o = s.option(form.ListValue, 'device', _('出口网卡'),
			'tip: 列表与「网络 → 接口」一致。第二条线路必须与主 WAN 选不同的设备(如主 WAN 用 eth1 则这里选 eth0)。');
		o.rmempty = true;
		o.value('', _('自动检测'));

		for (var i = 0; i < ifs.length; i++) {
			var e = ifs[i];
			var protoName = (e.proto == 'dhcp') ? 'DHCP' : ((e.proto == 'static') ? '静态' : (e.proto || ''));
			var lab = e.name + ' (' + e.device + ')';
			if (protoName != '')
				lab = lab + ' — ' + protoName;
			if (e.up === false)
				lab = lab + ' [未连接]';
			o.value(e.device, lab);
			seenDev[e.device] = true;
		}

		for (var j = 0; j < extra.length; j++) {
			// 备选只列物理口(eth*/lan*)
			if (!seenDev[extra[j]] && /^(eth|lan)/.test(extra[j]))
				o.value(extra[j], extra[j] + ' (未配置接口)');
		}

		o.default = '';
		o.rmempty = false;

		o = s.option(form.Value, 'source', _('源 IP'),
			'tip: 绑定第二线路网卡的 IPv4; 留空 = 按网卡自动获取。多线多拨时两线路源 IP 必须不同, 建议都显式指定。');
		o.datatype = 'or(ip4addr, "")';
		o.rmempty = true;

		o = s.option(form.Value, 'username', _('账号'),
			'tip: 可以与主 WAN 使用不同账号(各占各的槽位互不冲突)。留空 = 沿用概览页「认证服务」里的账号。');
		o.rmempty = true;

		o = s.option(form.Value, 'password', _('密码'),
			'tip: 对应上面账号的密码; 留空 = 沿用概览页「认证服务」里的密码。');
		o.password = true;
		o.rmempty = true;

		o = s.option(form.ListValue, 'ua', _('设备认证类型'),
			'tip: 该线路占用的设备槽位, 与主线路互不影响(如主 WAN 占电脑槽, 这里占手机槽)。');
		o.value('pc', _('电脑端 (PC 槽)'));
		o.value('mobile', _('手机端 (手机槽)'));
		o.default = 'mobile';

		return m.render();
	}
});
