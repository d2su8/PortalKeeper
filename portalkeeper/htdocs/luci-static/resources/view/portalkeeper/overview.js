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
var callGetLog = rpc.declare({ object: 'portalkeeper', method: 'getlog', expect: {} });
var callClearLog = rpc.declare({ object: 'portalkeeper', method: 'clearlog', expect: {} });
var callForceOffline = rpc.declare({ object: 'portalkeeper', method: 'force_offline', expect: {} });
// 重启服务(等价 /etc/init.d/portalkeeper restart): 改完账号/网卡等配置后让改动立即生效
var callApply = rpc.declare({ object: 'portalkeeper', method: 'apply', expect: {} });

// 自助管理后台地址(新版门户); 页面加载后会用 getstatus 返回的 self_url 覆盖
var SELF_URL = 'http://10.255.2.249/self/';

// 项目主页(版本号链接指向这里)
var REPO_URL = 'https://github.com/d2su8/PortalKeeper';

var statusBodyEl = null;     // 状态卡动态部分(运行状态/开关/运行统计, 5 秒轮询重绘)
var checkDetailEl = null;    // 主线路探测的原始输出
var panelsBoxEl = null;      // 线路信息
var offlineOutEl = null;     // 强制下线的执行输出
var logBoxEl = null;         // 日志面板
var lastStatus = null;       // 最近一次 getstatus 结果
var lines = [];              // 线路定义(配置为准, 见 lineDefs)
var infos = [];              // 每条线路的网卡/IP/MAC
var checks = [];             // 每条线路最近一次联网探测结果
var checking = false;        // 网络联通测试进行中
var activeTab = 'overview';  // 当前激活的标签页

// 标签页清单(顺序即展示顺序)
var TAB_KEYS = [ 'overview', 'auth', 'advanced', 'log' ];

// 主题语义色(aurora 等主题提供 --success/--danger/--warning 变量, fallback 兜底其他主题)
function vColor(kind) {
	switch (kind) {
		case 'success': return 'var(--success, #0a880a)';
		case 'danger': return 'var(--danger, #c5221f)';
		default: return 'var(--warning, #b45309)';
	}
}

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

// 联通测试结论: 只探测不登录; 返回 [色彩 kind, 中文结论]
function checkVerdict(out) {
	if (out == null)
		return [ 'warning', '尚未测试' ];

	if (out.indexOf('已联网') >= 0 || out.indexOf('已放行') >= 0)
		return [ 'success', '已联网' ];

	if (out.indexOf('未认证') >= 0 || out.indexOf('被劫持') >= 0)
		return [ 'danger', '未联网(被门户劫持)' ];

	if (out.indexOf('无法判定') >= 0)
		return [ 'warning', '无法判定' ];

	return [ 'warning', '状态未知' ];
}

// 彩色 badge(药丸形, 字色用主题语义变量)
function badge(verdict) {
	return E('span', {
		'class': 'label',
		'style': 'color:' + vColor(verdict[0])
	}, verdict[1]);
}

// 汇总各线路的运行统计(掉线次数 / 重连成功 / 失败 / 最近失败原因)
function lineStats(status) {
	var ls = (status && status.lines) ? status.lines : [];
	var off = 0, ok = 0, fail = 0, lastFail = '';

	for (var i = 0; i < ls.length; i++) {
		off += (ls[i].offline_count || 0);
		ok += (ls[i].auth_ok || 0);
		fail += (ls[i].auth_fail || 0);
		if (ls[i].last_fail)
			lastFail = ls[i].last_fail;
	}

	return { off: off, ok: ok, fail: fail, lastFail: lastFail, has: ls.length > 0 };
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
		E('span', { 'class': 'cbi-value-description' }, label + ' '),
		value
	]);
}

// 网络联通测试按钮: 放在「线路信息」标题行, 只探测不登录
function checkButton(view) {
	return E('button', {
		'class': 'btn cbi-button cbi-button-action',
		'style': 'margin-left:12px',
		'title': '网络联通测试: 逐条线路探测是否联网, 不会登录认证',
		'click': ui.createHandlerFn(view, 'handleCheck')
	}, _('网络联通测试'));
}

// 状态行: 左标签(定宽对齐) + 右值, 竖排布局(参考同类插件的状态卡)
function statusRow(label, valueNode) {
	return E('div', { 'style': 'display:flex; align-items:flex-start; gap:12px' }, [
		E('span', { 'class': 'cbi-value-description', 'style': 'flex:0 0 5em; white-space:nowrap' }, label),
		E('div', { 'style': 'flex:1; min-width:0' }, valueNode)
	]);
}

// 状态卡动态部分: 运行状态(红/绿) + 运行统计(掉线/重连成功/失败/最近失败原因), 5 秒轮询重绘
// (开关态看第一行的复选框本身, 不再单列一行)
function renderStatusBody(status) {
	var running = (status && status.daemon_running === true);
	var st = lineStats(status);
	var statsText = st.has
		? '掉线 %d 次 · 重连成功 %d 次 · 重连失败 %d 次'.format(st.off, st.ok, st.fail) +
		  (st.lastFail ? ' · 最近失败: ' + st.lastFail : '')
		: '暂无数据(守护进程未运行或尚未检测)';

	return E('div', { 'style': 'display:flex; flex-direction:column; gap:6px' }, [
		statusRow('运行状态', E('span', {
			'style': 'color:' + vColor(running ? 'success' : 'danger') + '; font-weight:600'
		}, running ? '运行中' : '未运行')),
		statusRow('运行统计', E('span', {}, statsText))
	]);
}

// 探测原始输出(折叠)
function renderCheckDetail(chk) {
	var out = (chk && chk.output) ? chk.output : null;

	if (!out)
		return E('span', {});

	return E('details', {}, [
		E('summary', { 'style': 'cursor:pointer; font-size:12px' }, '查看详细探测日志'),
		E('pre', {
			'class': 'cbi-value-field',
			'style': 'white-space:pre-wrap; max-height:140px; overflow:auto; font-size:12px; padding:6px'
		}, out)
	]);
}

// 探测原始输出(折叠): 单独更新, 不放进 paintStatus ——
// 5 秒轮询会重绘状态卡, 若连 details 一起重建, 用户展开的日志会被打回收起
// (新 details 元素默认收起, 表现为「自己回弹」); 探测输出只在测试完成时变化,
// 没有跟着轮询重绘的必要
function paintCheckDetail() {
	if (!checkDetailEl)
		return;

	// 记住展开状态: 之前是展开的, 更新后保持展开(新日志到了直接看到)
	var wasOpen = false;
	var old = checkDetailEl.querySelector('details');

	if (old)
		wasOpen = old.open;

	checkDetailEl.innerHTML = '';
	checkDetailEl.appendChild(renderCheckDetail(checks[0]));

	if (wasOpen) {
		var now = checkDetailEl.querySelector('details');
		if (now)
			now.open = true;
	}
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
				E('span', { 'class': 'cbi-value-description' }, '联网状态 '),
				checking
					? E('span', { 'class': 'label', 'style': 'color:' + vColor('warning') }, '检测中...')
					: badge(checkVerdict(chk ? chk.output : null))
			])
		])
	];

	// 认证异常(未配置账号密码/认证失败/重试用完)才补一行说明, 正常在线不显示
	if (def.state && def.state != 'online' && def.detail)
		kids.push(E('div', { 'class': 'cbi-value-description', 'style': 'font-size:12px; margin-top:4px' }, def.detail));

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
			return { output: '网络联通测试执行失败: ' + e };
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

// 重绘状态卡动态部分(用已有数据, 不触发新的探测)。
// 注意: 不含探测日志详情 —— 它在 checkDetailEl 里, 由 paintCheckDetail 单独更新,
// 避免 5 秒轮询重建 <details> 把用户展开的内容打回收起状态
function paintStatus(status) {
	if (statusBodyEl) {
		statusBodyEl.innerHTML = '';
		statusBodyEl.appendChild(renderStatusBody(status));
	}
}

function paintPanels() {
	if (panelsBoxEl) {
		panelsBoxEl.innerHTML = '';
		panelsBoxEl.appendChild(renderPanels());
	}
}

// 构造 WAN 口下拉选项(与「网络 → 接口」一致), 供主/副线路两个下拉共用
function fillDeviceOptions(o, ifaces, extras) {
	var seen = {};
	o.value('', _('自动检测(推荐)'));

	for (var i = 0; i < ifaces.length; i++) {
		var e = ifaces[i];
		var protoName = (e.proto == 'dhcp') ? 'DHCP' : ((e.proto == 'static') ? '静态' : (e.proto || ''));
		var lab = e.name + ' (' + e.device + ')';

		if (protoName != '')
			lab = lab + ' — ' + protoName;
		if (e.up === false)
			lab = lab + ' [未连接]';

		o.value(e.device, lab);
		seen[e.device] = true;
	}

	for (var j = 0; j < extras.length; j++) {
		// 备选只列物理口(eth*/lan*), 无线虚拟口(ra*/apcli*)等不堆列表
		if (!seen[extras[j]] && /^(eth|lan)/.test(extras[j]))
			o.value(extras[j], extras[j] + ' (未配置接口)');
	}
}

// 「启用」开关(顶部状态卡): 独立小表单, main section 只有 enabled 一个字段。
// 生效方式与其它表单一致: 页面底部「保存并应用」→ luci.js 的 View.handleSave 会遍历
// #maincontent 下所有 .cbi-map 逐一 save → uci commit → config.change → procd reload
// → init.d reload_service 按 main.enabled 门控启停(该链路已实测)。
// 不用即时生效方案(勾选即写 uci + 启停服务): 误触就停认证, 用户要求保存并应用后才生效。
// 外观由 render() 里 #pk-enabled-wrap 的样式拍平(去框/去说明), 与参考插件一样竖排。
function buildEnabledMap() {
	var m = new form.Map('portalkeeper', null, null);
	var s = m.section(form.NamedSection, 'main', 'portalkeeper', null);
	s.addremove = false;

	var o = s.option(form.Flag, 'enabled', _('启用'));
	o.default = '0';
	o.rmempty = false;

	return m;
}

// 认证设置表单: main section(账号参数; 总开关在顶部状态卡, 见 buildEnabledMap)
function buildAuthMap(ifaces, extras) {
	var m = new form.Map('portalkeeper', _('认证设置'),
		_('主线路认证所需的账号与参数。填好后到顶部状态卡勾选「启用」, 点页面底部「保存并应用」启动服务; 留空的项使用自动检测或内置默认值。'));
	var s = m.section(form.NamedSection, 'main', 'portalkeeper', _('认证服务'));
	s.addremove = false;

	// 账号/密码不设「不能为空」校验(rmempty 保持默认 true): 允许随时清空自己的设置,
	// 参数不齐时守护进程不启动, 并把缺什么写进日志
	var o = s.option(form.Value, 'username', _('账号'),
		'tip: 校园网登录账号(与自助管理后台的账号相同)。留空 = 不启动认证(日志会写明缺少账号)。');
	o.rmempty = true;

	o = s.option(form.Value, 'password', _('密码'),
		'tip: 保存在 /etc/config/portalkeeper(明文), 请勿外传路由器配置文件。留空 = 不启动认证。');
	o.password = true;
	o.rmempty = true;

	o = s.option(form.ListValue, 'device', _('WAN 口(出口网卡)'),
		'tip: 列表与「网络 → 接口」一致(接口名/设备/协议)。选择接口即用其设备发包; ' +
		'「自动检测」会探测校园网网段所在的网卡。');
	o.rmempty = true;
	fillDeviceOptions(o, ifaces, extras);
	o.default = '';

	o = s.option(form.Value, 'source', _('源 IP'),
		'tip: 绑定该网卡的 IPv4 发包; 留空 = 按所选网卡自动获取。多线多拨时必须保证两条线路源地址不同。');
	o.datatype = 'or(ip4addr, "")';
	o.rmempty = true;

	o = s.option(form.ListValue, 'ua', _('设备认证类型'),
		'tip: 校园网按设备槽位计数(1 电脑 + 1 手机), 认证类型在每次建立 MAC 绑定时确定。' +
		'如需切换槽位: 先在「概览」里强制下线本机(清除绑定), 再回来重新认证。');
	o.value('pc', _('电脑端 (PC 槽)'));
	o.value('mobile', _('手机端 (手机槽)'));
	o.default = 'pc';

	o = s.option(form.Flag, 'auto_retry', _('掉线自动重试认证'),
		'tip: 开启后守护进程会周期探测, 发现掉线立即自动重新认证(不做周期发包保活——校园网有流量就不会被判空闲超时)。');
	o.default = '1';
	o.rmempty = false;

	o = s.option(form.Value, 'retry_interval', _('重试认证间隔(秒)'),
		'tip: 自动重试开启时, 每隔多少秒探测一次线路状态; 探测到掉线会立即尝试认证。建议 ≥60 秒。');
	o.datatype = 'range(15,86400)';
	o.default = '60';
	o.depends('auto_retry', '1');

	o = s.option(form.Value, 'retry_count', _('重试认证次数'),
		'tip: 连续认证失败达到该次数后暂停自动重试(避免无限撞限流), ' +
		'点「保存并应用」重启服务可重新开始。');
	o.datatype = 'range(1,100)';
	o.default = '3';
	o.depends('auto_retry', '1');

	o = s.option(form.Value, 'probe_url', _('自定义探测网站'),
		'tip: 判断「有没有联网」时探测的网站, 可填多个(用逗号分隔, 任一可达即算联网)。' +
		'留空 = 用内置地址(www.msftconnecttest.com/connecttest.txt 等); 填了则只用你设置的。' +
		'建议填返回 200/204 的地址且支持明文 HTTP, 如 connect.rom.miui.com/generate_204; ' +
		'只写域名时按「http://域名/」访问。');
	o.default = '';
	o.rmempty = true;
	o.placeholder = 'www.msftconnecttest.com/connecttest.txt';

	o = s.option(form.Value, 'portal_url', _('门户地址'),
		'tip: 认证服务器地址。留空 = 自动发现(未认证时网关会 302 劫持, 程序直接跟着跳转地址走, ' +
		'换学校通常不用填); 探测不到时才需要手填 —— 浏览器打开任意 http 网站(如 ' +
		'http://www.msftconnecttest.com/connecttest.txt), 把地址栏跳转后的地址整条粘过来。' +
		'认证成功后会自动把门户地址记录到这一项(强制下线功能要用), 可以手动改动。');
	o.default = '';
	o.rmempty = true;
	o.placeholder = 'http://10.10.0.1 (留空=自动发现)';

	return m;
}

// 高级设置表单: extra section(第二线路, 默认关闭)
function buildAdvancedMap(ifaces, extras) {
	var m = new form.Map('portalkeeper', _('高级设置'), _('校园网「单线多拨」不可行(边缘交换机会丢弃同一物理口上第二个 MAC 的流量), ' +
		'多拨只能走「多线多拨」: 用另一条物理线路(另一个校园网口)再认证一个会话。此功能默认关闭, 不影响单线路用户。'));
	var s = m.section(form.NamedSection, 'extra', 'line', _('扩展线路(第二 WAN)'));
	s.addremove = false;

	var o = s.option(form.Flag, 'enabled', _('启用扩展线路'),
		'tip: 开启后守护进程除了主 WAN, 还会用下面的网卡+账号再做一次独立认证, ' +
		'与主 WAN 各占一个槽位(互不影响)。两个线路必须来自不同物理口, 源 IP 不同。');
	o.default = '0';
	o.rmempty = false;

	o = s.option(form.ListValue, 'device', _('出口网卡'),
		'tip: 列表与「网络 → 接口」一致。第二条线路必须与主 WAN 选不同的设备(如主 WAN 用 eth1 则这里选 eth0)。');
	o.rmempty = true;
	fillDeviceOptions(o, ifaces, extras);
	o.default = '';
	o.rmempty = false;
	o.depends('enabled', '1');

	o = s.option(form.Value, 'source', _('源 IP'),
		'tip: 绑定第二线路网卡的 IPv4; 留空 = 按网卡自动获取。多线多拨时两线路源 IP 必须不同, 建议都显式指定。');
	o.datatype = 'or(ip4addr, "")';
	o.rmempty = true;
	o.depends('enabled', '1');

	o = s.option(form.Value, 'username', _('账号'),
		'tip: 可以与主 WAN 使用不同账号(各占各的槽位互不冲突)。留空 = 沿用「认证设置」里的账号。');
	o.rmempty = true;
	o.depends('enabled', '1');

	o = s.option(form.Value, 'password', _('密码'),
		'tip: 对应上面账号的密码; 留空 = 沿用「认证设置」里的密码。');
	o.password = true;
	o.rmempty = true;
	o.depends('enabled', '1');

	o = s.option(form.ListValue, 'ua', _('设备认证类型'),
		'tip: 该线路占用的设备槽位, 与主线路互不影响(如主 WAN 占电脑槽, 这里占手机槽)。');
	o.value('pc', _('电脑端 (PC 槽)'));
	o.value('mobile', _('手机端 (手机槽)'));
	o.default = 'mobile';
	o.depends('enabled', '1');

	return m;
}

// 标签页导航条
function renderTabBar(self) {
	var items = [
		[ 'overview', _('概览') ],
		[ 'auth', _('认证设置') ],
		[ 'advanced', _('高级设置') ],
		[ 'log', _('日志') ]
	];

	return E('ul', { 'class': 'cbi-tabmenu' }, items.map(function(it) {
		return E('li', {
			'id': 'pk-tab-' + it[0],
			'class': it[0] === activeTab ? 'cbi-tab cbi-tab-active' : 'cbi-tab'
		}, E('a', {
			'href': '#',
			'click': L.bind(function(ev) {
				ev.preventDefault();
				self.switchTab(it[0]);
			}, self)
		}, it[1]));
	}));
}

function makePane(key, content) {
	return E('div', {
		'id': 'pk-pane-' + key,
		'class': 'cbi-tab-pane',
		'style': key === activeTab ? '' : 'display:none'
	}, content);
}

return view.extend({
	// 切换标签页: 更新导航条高亮 + 面板显隐(不重渲染, 保留表单输入状态)
	switchTab: function(key) {
		activeTab = key;

		for (var i = 0; i < TAB_KEYS.length; i++) {
			var k = TAB_KEYS[i];
			var li = document.getElementById('pk-tab-' + k);
			var pane = document.getElementById('pk-pane-' + k);

			if (li)
				li.className = (k === key) ? 'cbi-tab cbi-tab-active' : 'cbi-tab';
			if (pane)
				pane.style.display = (k === key) ? '' : 'none';
		}
	},

	// 网络联通测试: 逐条线路只探测, 不登录
	handleCheck: function() {
		checking = true;
		paintStatus(lastStatus);
		paintPanels();

		return probeLines(lines).then(function(list) {
			checking = false;
			checks = list;
			paintStatus(lastStatus);
			paintCheckDetail();
			paintPanels();
		});
	},

	// 强制下线本机(新版门户令牌 API, 守护进程自动完成拿令牌→查本机→下线)
	handleForceOffline: function() {
		return callForceOffline().then(function(res) {
			var out = (res && res.output) ? res.output : '(无输出)';

			if (offlineOutEl) {
				offlineOutEl.innerHTML = '';
				offlineOutEl.appendChild(E('pre', {
					'class': 'cbi-value-field',
					'style': 'white-space:pre-wrap; max-height:220px; overflow:auto; font-size:12px; padding:6px'
				}, out));
			}
		});
	},

	// 重启服务: 让「保存并应用」之外也多一个即时生效入口
	// (账号/网卡/门户地址等改动保存后, 点这里重启即按新配置运行)
	handleRestart: function() {
		return callApply().then(function() {
			ui.showNotification(_('认证服务已重启'), 3000);

			return callGetStatus().catch(function() { return lastStatus; }).then(function(d) {
				lastStatus = d;
				paintStatus(d);
			});
		}).catch(function(e) {
			ui.showNotification(_('重启失败: ') + e, 5000);
		});
	},

	handleOfflineGuide: function() {
		window.open(SELF_URL, '_blank');
	},

	handleRefreshLog: function() {
		return callGetLog().then(function(res) {
			if (logBoxEl)
				logBoxEl.textContent = (res && res.log) ? res.log : '(暂无日志)';
		});
	},

	handleClearLog: function() {
		var self = this;

		return callClearLog().then(function() {
			return self.handleRefreshLog();
		});
	},

	load: function() {
		return Promise.all([
			callGetStatus().catch(function() { return null; }),
			callGetIfaces().catch(function() { return {}; }),
			callGetLog().catch(function() { return { log: '' }; }),
			uci.load('portalkeeper').catch(function() { return null; })
		]).then(function(r) {
			var st = r[0];
			var defs = lineDefs(st);

			return Promise.all([ fetchInfos(defs), probeLines(defs) ]).then(function(x) {
				return { status: st, ifaces: r[1] || {}, log: r[2] || {}, lines: defs, infos: x[0], checks: x[1] };
			});
		});
	},

	render: function(data) {
		lastStatus = data.status;
		if (data.status && data.status.self_url)
			SELF_URL = data.status.self_url;
		lines = data.lines || [];
		infos = data.infos || [];
		checks = data.checks || [];

		var ifs = (data.ifaces && data.ifaces.interfaces) ? data.ifaces.interfaces : [];
		var extra = (data.ifaces && data.ifaces.extra) ? data.ifaces.extra : [];
		var version = (data.status && data.status.version) ? data.status.version : '';

		var self = this;

		// 三个表单各自渲染(异步): 启用开关放进状态卡, 账号参数/第二线路放各自标签页
		return Promise.all([
			buildEnabledMap().render(),
			buildAuthMap(ifs, extra).render(),
			buildAdvancedMap(ifs, extra).render()
		]).then(function(res) {
			var enabledMapEl = res[0];
			var authEl = res[1];
			var advancedEl = res[2];

			var view = E('div', { 'class': 'cbi-map' }, [
				E('h2', {}, 'PortalKeeper'),
				// 一句话描述 + 版本号(红色链接, 点开跳 GitHub)
				E('div', { 'class': 'cbi-map-descr' }, [
					_('zaxsoft(石斧软件) portal 校园网认证小工具 —— 开机自动认证, 掉线自动重连。'),
					E('div', { 'style': 'margin-top:4px' }, [
						E('span', {
							'style': 'color:' + vColor('danger') + '; font-weight:600'
						}, _('版本') + ': '),
						E('a', {
							'href': REPO_URL,
							'target': '_blank',
							'rel': 'noopener',
							'style': 'color:' + vColor('danger') + '; font-weight:600'
						}, version || _('未知'))
					])
				]),

				// 顶部状态卡(整模块保留主题的卡片框): 「启用」开关(小表单, 保存并应用生效)
				// + 重启服务按钮(旁边) + 只读的运行状态/运行统计(竖排, 常驻所有标签页;
				// 开关态看复选框本身)。小表单自身那层嵌套框由下面 #pk-enabled-wrap 的
				// 样式拍平, 只保留最外层这一张卡
				E('div', { 'class': 'cbi-section' }, [
					E('h3', {}, _('状态')),
					// 拍平小表单的卡片外观(去框/去内边距), 标签定宽与下面两行对齐
					E('style', {}, '#pk-enabled-wrap .cbi-section{border:0;background:none;box-shadow:none;border-radius:0;padding:0;margin:0}' +
						'#pk-enabled-wrap .cbi-value{margin:0;padding:0;align-items:center}' +
						'#pk-enabled-wrap .cbi-value>.cbi-value-title{flex:0 0 5em;width:5em;text-align:left;padding-top:0}' +
						'#pk-enabled-wrap .cbi-value>.cbi-value-field{flex:0 0 auto;padding:0;max-width:none}'),
					// 第一行: 启用开关 + 重启服务按钮
					E('div', { 'style': 'display:flex; align-items:center; flex-wrap:wrap; gap:8px 16px; margin:6px 0' }, [
						E('div', { 'id': 'pk-enabled-wrap' }, enabledMapEl),
						E('button', {
							'class': 'btn cbi-button cbi-button-apply',
							'title': '重启认证服务(等价 /etc/init.d/portalkeeper restart); 改完账号/网卡/门户地址等配置并「保存并应用」后, 也可以用这个按钮立即重启',
							'click': ui.createHandlerFn(self, 'handleRestart')
						}, _('重启服务'))
					]),
					// 下面: 只读运行状态/运行统计(5 秒轮询重绘)
					(statusBodyEl = E('div', { 'id': 'pk-status-body' }))
				]),

				// 标签页导航
				renderTabBar(self),

				// 「概览」: 线路信息(含网络联通测试) + 下线指引(含强制下线)
				makePane('overview', [
					E('div', { 'class': 'cbi-section' }, [
						E('h3', { 'style': 'display:flex; align-items:center' },
							[ _('线路信息'), checkButton(self) ]),
						(panelsBoxEl = E('div', { 'id': 'portalkeeper-panels' })),
						(checkDetailEl = E('div', { 'id': 'portalkeeper-check-detail', 'style': 'margin-top:4px' }))
					]),
					E('div', { 'class': 'cbi-section' }, [
						E('h3', {}, _('下线指引')),
						hrow([
							E('button', {
								'class': 'btn cbi-button cbi-button-negative',
								'title': '通过门户令牌 API 强制下线本机(需先认证过一次); 下线后守护进程会按「掉线自动重试」配置自动重连',
								'click': ui.createHandlerFn(self, 'handleForceOffline')
							}, _('强制下线本机')),
							E('button', {
								'class': 'btn cbi-button cbi-button-action',
								'click': ui.createHandlerFn(self, 'handleOfflineGuide')
							}, _('打开自助管理后台(下线)')),
							spacer(),
							E('span', { 'class': 'cbi-value-description', 'style': 'font-size:12px; flex:1 1 420px' },
								'「强制下线本机」自动完成拿令牌→查本机→下线, 适合换槽位前释放本机; ' +
								'下线后守护进程会按「掉线自动重试」配置自动重新认证, 如需保持下线请先到顶部状态卡取消「启用」并保存应用。' +
								'换槽位必须清除绑定, 可到自助后台「在线设备」对本机执行「按 MAC 下线(清除绑定)」。本插件不自动顶号。')
						]),
						(offlineOutEl = E('div', { 'id': 'portalkeeper-offline-out', 'style': 'margin-top:6px' }))
					])
				]),

				// 「认证设置」: 主线路表单
				makePane('auth', authEl),

				// 「高级设置」: 第二线路表单
				makePane('advanced', advancedEl),

				// 「日志」: 内嵌日志查看
				makePane('log', [
					E('div', { 'class': 'cbi-section' }, [
						E('div', { 'class': 'cbi-section-descr' },
							'认证守护进程的运行日志(含服务启动/停止记录)。日志只保存在 /tmp(tmpfs), 不写入闪存, ' +
							'因此不会磨损 NAND, 但路由器重启后自动清空(上限约 256KB, 自动滚动裁剪)。'),
						E('div', { 'class': 'cbi-page-actions' }, [
							E('button', {
								'class': 'btn cbi-button cbi-button-apply',
								'click': ui.createHandlerFn(self, 'handleRefreshLog')
							}, _('刷新')),
							'\u00a0',
							E('button', {
								'class': 'btn cbi-button cbi-button-negative',
								'click': ui.createHandlerFn(self, 'handleClearLog')
							}, _('清空日志'))
						]),
						(logBoxEl = E('pre', {
							'id': 'portalkeeper-log',
							'class': 'cbi-value-field',
							'style': 'max-height:480px; overflow:auto; font-size:12px; padding:8px; white-space:pre-wrap'
						}, [ (data.log && data.log.log) ? data.log.log : '(暂无日志)' ]))
					])
				])
			]);

			paintStatus(lastStatus);
			paintCheckDetail();
			paintPanels();

			// 5 秒轮询: 刷新状态卡与线路信息(不动表单, 不重复探测, 避免打断编辑与打搅门户)
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

			poll.add(L.bind(function() {
				return callGetLog().catch(function() { return null; }).then(function(res) {
					if (logBoxEl)
						logBoxEl.textContent = (res && res.log) ? res.log : '(暂无日志)';
				});
			}, self), 5);

			return view;
		});
	}
});
