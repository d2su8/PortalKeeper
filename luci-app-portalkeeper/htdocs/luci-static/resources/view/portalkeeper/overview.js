'use strict';
'require view';
'require form';
'require rpc';
'require poll';
'require uci';
'require ui';

var callGetStatus = rpc.declare({ object: 'portalkeeper', method: 'getstatus', expect: {} });
var callGetIfaces = rpc.declare({ object: 'portalkeeper', method: 'getifaces', expect: {} });
var callGetIfInfo = rpc.declare({ object: 'portalkeeper', method: 'getifinfo', params: [ 'device', 'source' ], expect: {} });
var callRunCheck = rpc.declare({ object: 'portalkeeper', method: 'runcheck', params: [ 'device', 'source' ], expect: {} });

var SELF_URL = 'http://10.255.2.252/self/index.html#/Login';

var statusRowEl = null;      // 运行状态行(5 秒轮询重绘)
var checkDetailEl = null;    // 主线路探测的原始输出(只在测试时重绘, 免得轮询把展开的折叠框收起)
var panelsBoxEl = null;      // 线路信息
var lastStatus = null;       // 最近一次 getstatus 结果
var lines = [];              // 线路定义(配置为准, 见 lineDefs)
var infos = [];              // 每条线路的网卡/IP/MAC
var checks = [];             // 每条线路最近一次联网探测结果
var checking = false;        // 联通测试进行中

// 线路定义: 以 uci 配置为准(服务没跑过也能显示线路二), 网卡/源 IP 缺失时用守护进程解析值兜底
function lineDefs(status) {
	var slines = (status && status.lines) ? status.lines : [];
	var byName = {};

	for (var i = 0; i < slines.length; i++)
		byName[slines[i].name || ''] = slines[i];

	var keys = [ [ 'main', '线路一(主线路)' ] ];

	if (uci.get('portalkeeper', 'extra', 'enabled') == '1')
		keys.push([ 'extra', '线路二(扩展线路)' ]);

	var defs = [];

	for (var j = 0; j < keys.length; j++) {
		var name = keys[j][0];
		var s = byName[name] || {};

		defs.push({
			name: name,
			title: keys[j][1],
			device: uci.get('portalkeeper', name, 'device') || s.device || '',
			source: uci.get('portalkeeper', name, 'source') || s.source || '',
			state: s.state || null,
			detail: s.detail || ''
		});
	}

	return defs;
}

// 联通测试结论: 只探测不登录, 输出里的判定词来自 portalkeeper check
function checkVerdict(out) {
	if (out == null)
		return [ 'warning', '尚未测试' ];

	if (out.indexOf('已联网') >= 0 || out.indexOf('已放行') >= 0)
		return [ 'success', '已联网' ];

	if (out.indexOf('未认证') >= 0 || out.indexOf('被劫持') >= 0)
		return [ 'important', '未联网(被门户劫持)' ];

	if (out.indexOf('无法判定') >= 0)
		return [ 'warning', '无法判定' ];

	return [ 'warning', '状态未知' ];
}

// 在线判定更细的说法, 用在上方运行状态行
function netVerdict(out) {
	var v = checkVerdict(out);

	if (v[1] == '已联网')
		return [ 'success', '网络正常 — 已认证在线' ];
	if (v[1] == '未联网(被门户劫持)')
		return [ 'important', '网络已连接但未认证(需要登录)' ];
	if (v[1] == '无法判定')
		return [ 'warning', '无法判定(线路可能未接入校园网)' ];

	return v;
}

// 横向排布工具: 一行 flex 元素
function hrow(children, extra) {
	var style = 'display:flex; flex-wrap:wrap; align-items:center; gap:14px';

	if (extra)
		style = style + '; ' + extra;

	return E('div', { 'style': style }, children);
}

function spacer() {
	return E('span', { 'style': 'flex:1' });
}

// 字段 chip: [标签 值]
function chip(label, value) {
	return E('span', { 'style': 'white-space:nowrap' }, [
		E('span', { 'style': 'color:#888' }, label + ' '),
		value
	]);
}

function badge(verdict) {
	return E('span', {}, [ E('span', { 'class': 'label ' + verdict[0] }, verdict[1]) ]);
}

function checkButton(view) {
	return E('button', {
		'class': 'btn cbi-button cbi-button-action',
		'style': 'margin-left:12px',
		'title': '联通测试: 逐条线路探测是否联网, 不会登录认证',
		'click': ui.createHandlerFn(view, 'handleCheck')
	}, '联通测试');
}

// 运行状态行: 服务是否运行 + 网络是否正常(主线路探测结果) + 探测目标 + 状态更新时间
function renderStatusRow(status, chk) {
	var running = (status && status.daemon_running === true);
	var out = (chk && chk.output) ? chk.output : null;
	var m = out ? out.match(/—\s*([^\s\n]+)/) : null;
	var v = netVerdict(out);

	return hrow([
		E('span', { 'style': 'white-space:nowrap' }, [
			'PortalKeeper ',
			E('span', {
				'style': running
					? 'color:#0a0; font-weight:600'
					: 'color:#d33; font-weight:600'
			}, running ? '运行中' : '未运行')
		]),
		checking
			? E('span', { 'class': 'label warning' }, '联通测试中, 请稍候(约 2~15 秒)...')
			: badge(v),
		chip('探测目标:', m ? m[1] : '-'),
		chip('状态更新:', (status && status.updated_human) ? status.updated_human : '-')
	]);
}

// 探测原始输出(折叠)
function renderCheckDetail(chk) {
	var out = (chk && chk.output) ? chk.output : null;

	if (!out)
		return E('span', {});

	return E('details', {}, [
		E('summary', { 'style': 'cursor:pointer; color:#37c; font-size:12px' }, '查看详细探测日志'),
		E('pre', {
			'style': 'white-space:pre-wrap; max-height:140px; overflow:auto; font-size:12px; background:#fff; padding:6px'
		}, out)
	]);
}

// 线路信息: 每条线路一列(两条线路时左右并排), 只列 网卡 / IP / MAC + 本线路联网状态
function lineCol(def, info, chk) {
	var kids = [
		E('h4', { 'style': 'margin:6px 0 4px' }, def.title),
		hrow([
			chip('网卡:', info.device || def.device || '自动检测'),
			chip('IP 地址:', info.ip || '-'),
			chip('MAC 地址:', info.mac || '-'),
			E('span', {}, [
				E('span', { 'style': 'color:#888' }, '联网状态 '),
				checking
					? E('span', { 'class': 'label warning' }, '检测中...')
					: badge(checkVerdict(chk ? chk.output : null))
			])
		])
	];

	// 认证异常(未配置账号密码/认证失败/重试用完)才补一行说明, 正常在线不显示
	if (def.state && def.state != 'online' && def.detail)
		kids.push(E('div', { 'style': 'color:#666; font-size:12px; margin-top:4px' }, def.detail));

	return E('div', { 'style': 'flex:1 1 340px; min-width:280px' }, kids);
}

function renderPanels() {
	if (lines.length == 0)
		return E('em', {}, '暂无线路数据');

	var cols = [];

	for (var i = 0; i < lines.length; i++)
		cols.push(lineCol(lines[i], infos[i] || {}, checks[i]));

	return hrow(cols, 'align-items:flex-start');
}

// 逐条线路探测(带该线路的网卡/源 IP, 各探各的)
function probeLines(defs) {
	var reqs = [];

	for (var i = 0; i < defs.length; i++)
		reqs.push(callRunCheck(defs[i].device || '', defs[i].source || '').catch(function(e) {
			return { output: '联通测试执行失败: ' + e };
		}));

	return reqs.length ? Promise.all(reqs) : Promise.resolve([]);
}

function fetchInfos(defs) {
	var reqs = [];

	for (var i = 0; i < defs.length; i++)
		reqs.push(callGetIfInfo(defs[i].device || '', defs[i].source || '').catch(function() {
			return {};
		}));

	return reqs.length ? Promise.all(reqs) : Promise.resolve([]);
}

// 重绘状态行与探测详情(用已有数据, 不触发新的探测)
function paintStatus(status) {
	if (statusRowEl) {
		statusRowEl.innerHTML = '';
		statusRowEl.appendChild(renderStatusRow(status, checks[0]));
	}

	if (checkDetailEl) {
		checkDetailEl.innerHTML = '';
		checkDetailEl.appendChild(renderCheckDetail(checks[0]));
	}
}

function paintPanels() {
	if (panelsBoxEl) {
		panelsBoxEl.innerHTML = '';
		panelsBoxEl.appendChild(renderPanels());
	}
}

return view.extend({
	// 联通测试: 逐条线路只探测, 不登录
	handleCheck: function() {
		checking = true;
		paintStatus(lastStatus);
		paintPanels();

		return probeLines(lines).then(function(list) {
			checking = false;
			checks = list;
			paintStatus(lastStatus);
			paintPanels();
		});
	},

	handleOfflineGuide: function() {
		window.open(SELF_URL, '_blank');
	},

	load: function() {
		return Promise.all([
			callGetStatus().catch(function() { return null; }),
			callGetIfaces().catch(function() { return {}; }),
			uci.load('portalkeeper').catch(function() { return null; })
		]).then(function(r) {
			var st = r[0];
			var defs = lineDefs(st);

			return Promise.all([ fetchInfos(defs), probeLines(defs) ]).then(function(x) {
				return { status: st, ifaces: r[1] || {}, lines: defs, infos: x[0], checks: x[1] };
			});
		});
	},

	render: function(data) {
		lastStatus = data.status;
		lines = data.lines || [];
		infos = data.infos || [];
		checks = data.checks || [];

		var ifs = (data.ifaces && data.ifaces.interfaces) ? data.ifaces.interfaces : [];
		var extra = (data.ifaces && data.ifaces.extra) ? data.ifaces.extra : [];
		var seenDev = {};

		// 认证服务: 总开关 + 主线路的账号与认证方式, 统一走页面底部「保存并应用」生效
		var map = new form.Map('portalkeeper', null, null);
		var sec = map.section(form.NamedSection, 'main', 'portalkeeper', _('认证服务'));
		sec.addremove = false;

		var o = sec.option(form.Flag, 'enabled', _('启用 PortalKeeper 服务'));
		o.default = '0';
		o.rmempty = false;
		o.description =
			'tip: 勾选后点页面底部「保存并应用」生效——认证服务立即启动, 并随系统开机自启; ' +
			'取消勾选并应用则停止服务。运行状态见上方状态行(每 5 秒自动刷新)。';

		// 账号/密码不设「不能为空」校验(rmempty 保持默认 true): 允许随时清空自己的设置,
		// 参数不齐时守护进程不启动, 并把缺什么写进日志
		o = sec.option(form.Value, 'username', _('账号'),
			'tip: 校园网登录账号(与自助管理后台的账号相同)。留空 = 不启动认证(日志会写明缺少账号)。');
		o.rmempty = true;

		o = sec.option(form.Value, 'password', _('密码'),
			'tip: 保存在 /etc/config/portalkeeper(明文), 请勿外传路由器配置文件。留空 = 不启动认证。');
		o.password = true;
		o.rmempty = true;

		o = sec.option(form.ListValue, 'device', _('WAN 口(出口网卡)'),
			'tip: 列表与「网络 → 接口」一致(接口名/设备/协议)。选择接口即用其设备发包; ' +
			'「自动检测」会探测校园网网段所在的网卡。');
		o.rmempty = true;
		o.value('', _('自动检测(推荐)'));

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
			// 备选只列物理口(eth*/lan*), 无线虚拟口(ra*/apcli*)等不堆列表
			if (!seenDev[extra[j]] && /^(eth|lan)/.test(extra[j]))
				o.value(extra[j], extra[j] + ' (未配置接口)');
		}

		o.default = '';

		o = sec.option(form.Value, 'source', _('源 IP'),
			'tip: 绑定该网卡的 IPv4 发包; 留空 = 按所选网卡自动获取。多线多拨时必须保证两条线路源地址不同。');
		o.datatype = 'or(ip4addr, "")';
		o.rmempty = true;

		o = sec.option(form.ListValue, 'ua', _('设备认证类型'),
			'tip: 校园网按设备槽位计数(1 电脑 + 1 手机), 认证类型在每次建立 MAC 绑定时确定。' +
			'如需切换槽位: 先按下方「下线指引」在自助后台执行按 MAC 下线并清除绑定, 再回来选择认证类型重新认证。');
		o.value('pc', _('电脑端 (PC 槽)'));
		o.value('mobile', _('手机端 (手机槽)'));
		o.default = 'pc';

		o = sec.option(form.Flag, 'auto_retry', _('掉线自动重试认证'),
			'tip: 开启后守护进程会周期探测, 发现掉线立即自动重新认证(不做周期发包保活——校园网有流量就不会被判空闲超时)。');
		o.default = '1';
		o.rmempty = false;

		o = sec.option(form.Value, 'retry_interval', _('重试认证间隔(秒)'),
			'tip: 自动重试开启时, 每隔多少秒探测一次线路状态; 探测到掉线会立即尝试认证。建议 ≥60 秒。');
		o.datatype = 'range(15,86400)';
		o.default = '60';

		o = sec.option(form.Value, 'retry_count', _('重试认证次数'),
			'tip: 连续认证失败达到该次数后暂停自动重试(避免无限撞限流), ' +
			'点「保存并应用」重启服务可重新开始。');
		o.datatype = 'range(1,100)';
		o.default = '3';

		o = sec.option(form.Value, 'probe_url', _('自定义探测网站'),
			'tip: 判断「有没有联网」时探测的网站。留空 = 用内置地址(www.msftconnecttest.com/connecttest.txt 等, ' +
			'任一可达即算联网); 填了则只用你设置的这一个。建议填返回 200/204 的地址且支持明文 HTTP, ' +
			'如 connect.rom.miui.com/generate_204; 只写域名时按「http://域名/」访问。');
		o.default = '';
		o.rmempty = true;
		o.placeholder = 'www.msftconnecttest.com/connecttest.txt';

		// 注意: form.Map.render() 返回 Promise(异步渲染), 必须等它 resolve 后
		// 再拼进页面, 否则会显示成 [object Promise] 且表单缺失
		var self = this;

		return map.render().then(function(mapEl) {
			var view = E('div', { 'class': 'cbi-map' }, [
				E('h2', {}, '概览'),
				E('div', { 'class': 'cbi-map-descr' },
					'PortalKeeper —— 校园网门户自动认证(兼容 bossWeb 门户)。'),

				E('div', { 'class': 'cbi-section' }, [
					E('h3', { 'style': 'display:flex; align-items:center' },
						[ '运行状态', checkButton(self) ]),
					(statusRowEl = E('div', { 'id': 'portalkeeper-status' })),
					(checkDetailEl = E('div', { 'id': 'portalkeeper-check-detail', 'style': 'margin-top:4px' }))
				]),

				mapEl,

				E('div', { 'class': 'cbi-section' }, [
					E('h3', {}, '线路信息'),
					(panelsBoxEl = E('div', { 'id': 'portalkeeper-panels' }))
				]),

				E('div', { 'class': 'cbi-section' }, [
					E('h3', {}, '下线指引'),
					hrow([
						E('button', {
							'class': 'btn cbi-button cbi-button-action',
							'click': ui.createHandlerFn(self, 'handleOfflineGuide')
						}, '打开自助管理后台(下线)'),
						spacer(),
						E('span', { 'style': 'color:#666; font-size:12px; flex:1 1 420px' },
							'在自助后台「在线设备」里找到本机(对照上方 IP/MAC), 选择「按 MAC 下线(清除绑定)」; ' +
							'必须清除绑定, 否则重新认证仍会被识别为原设备槽位。本插件不自动顶号。')
					])
				])
			]);

			paintStatus(lastStatus);
			paintPanels();

			// 5 秒轮询: 只刷新状态行与线路信息(不动表单, 不重复探测, 避免打断编辑与打搅门户)
			poll.add(L.bind(function() {
				return callGetStatus().catch(function() { return lastStatus; }).then(function(d) {
					lastStatus = d;

					return fetchInfos(lines).then(function(list) {
						infos = list;
						paintStatus(d);
						paintPanels();
					});
				});
			}, self), 5);

			return view;
		});
	}
});