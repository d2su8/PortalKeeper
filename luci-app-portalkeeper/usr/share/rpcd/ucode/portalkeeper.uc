// rpcd ucode 插件: ubus 对象 "portalkeeper"(对象名由下方 return 的字典键决定,
// 参照 /usr/share/rpcd/ucode/luci.upnp 与 ddns.uc 的真实写法, 路由器实测通过)

'use strict';

import { access, popen, readfile, writefile } from 'fs';
import { connect } from 'ubus';
import { cursor } from 'uci';

const STATUS_PATH = '/tmp/portalkeeper.status';
const TRIGGER_PATH = '/tmp/portalkeeper.trigger';
const LOG_PATH = '/tmp/portalkeeper.log';
const CLI = '/usr/bin/portalkeeperd';
const INIT = '/etc/init.d/portalkeeper';

// 常驻 ubus 连接(参照 luci.upnp 注释, 防止调用间被 GC)
const ubus = connect();

function service_running() {
	// 注意: ucode 函数定义不提升, 此处不可调用文件后面定义的 exec() 助手,
	// 直接用顶层 import 的 popen。pidof 判活(服务停止后 procd 仍留 running:false 实例)
	const p = popen('pidof portalkeeperd 2>/dev/null', 'r');

	if (p == null)
		return false;

	const out = p.read('all');

	p.close();

	return (type(out) == 'string' && length(trim(out)) > 0);
}

function autostart_enabled() {
	const uci = cursor();

	if (uci.get('portalkeeper', 'main', 'enabled') == '1')
		return true;

	return false;
}

function read_status() {
	const out = {
		daemon_running: service_running(),
		autostart: autostart_enabled(),
		updated_human: '',
		lines: []
	};

	if (access(STATUS_PATH)) {
		const txt = readfile(STATUS_PATH);
		let parsed = null;

		if (txt != null)
			parsed = json(txt);

		if (type(parsed) == 'object' && type(parsed.lines) == 'array') {
			out.lines = parsed.lines;

			if (type(parsed.updated_human) == 'string')
				out.updated_human = parsed.updated_human;
		}
	}

	return out;
}

function exec(cmd) {
	const p = popen(cmd, 'r');

	if (p == null)
		return null;

	const out = p.read('all');

	p.close();
	return out;
}

// 秒数 → "1d 2h 3m 4s"(零值段省略)
function uptime_str(secs) {
	let t = secs;
	const d = int(t / 86400);
	t = t % 86400;
	const h = int(t / 3600);
	t = t % 3600;
	const m = int(t / 60);
	const s = t % 60;
	const parts = [];

	if (d > 0)
		push(parts, d + 'd');
	if (h > 0)
		push(parts, h + 'h');
	if (m > 0)
		push(parts, m + 'm');
	if (s > 0 || length(parts) == 0)
		push(parts, s + 's');

	let buf = '';

	for (let p in parts) {
		if (length(buf) > 0)
			buf = buf + ' ';
		buf = buf + p;
	}

	return buf;
}

// 读取指定网卡的 sysfs/netifd 信息; device 为空时按 source IP 反查
function ifinfo(device, source) {
	const out = {
		device: device,
		proto: '-',
		link: '-',
		mac: '-',
		ip: '-',
		uptime: '-'
	};

	// 设备未知但有源 IP: 从 netifd 状态反查
	if ((device == null || device == '') && source != null && source != '') {
		const dump = ubus.call('network.interface', 'dump', {});

		if (type(dump) == 'object' && type(dump.interface) == 'array') {
			for (let e in dump.interface) {
				if (type(e['ipv4-address']) == 'array') {
					for (let a in e['ipv4-address']) {
						if (a.address == source) {
							if (type(e.l3_device) == 'string' && e.l3_device != '')
								device = e.l3_device;
							else if (type(e.device) == 'string' && e.device != '')
								device = e.device;
							break;
						}
					}
				}

				if (device != null && device != '')
					break;
			}
		}

		out.device = device;
	}

	if (device != null && device != '') {
		const sys = '/sys/class/net/' + device;
		let t = readfile(sys + '/address');

		if (t != null && trim(t) != '')
			out.mac = trim(t);

		t = readfile(sys + '/operstate');

		if (t != null)
			out.link = (trim(t) == 'up') ? '已连接' : '未连接';

		const p = popen('ip -4 -o addr show dev ' + device + ' 2>/dev/null', 'r');

		if (p != null) {
			let line = p.read('line');

			while (length(line) > 0) {
				const m = match(line, /inet ([0-9.]+)\//);

				if (m != null) {
					out.ip = m[1];
					break;
				}

				line = p.read('line');
			}

			p.close();
		}

		const dump = ubus.call('network.interface', 'dump', {});

		if (type(dump) == 'object' && type(dump.interface) == 'array') {
			for (let e in dump.interface) {
				const ed = (type(e.l3_device) == 'string' && e.l3_device != '') ? e.l3_device : ((type(e.device) == 'string') ? e.device : '');

				if (ed == device) {
					if (type(e.proto) == 'string' && e.proto != '')
						out.proto = (e.proto == 'dhcp') ? 'DHCP 客户端' : ((e.proto == 'static') ? '静态地址' : e.proto);

					if (type(e.uptime) == 'int' || type(e.uptime) == 'double')
						out.uptime = uptime_str(int(e.uptime));

					break;
				}
			}
		}
	}

	return out;
}

const methods = {
	getstatus: {
		call: function(req) {
			return read_status();
		}
	},

	runcheck: {
		args: { device: 'string', source: 'string' },
		call: function(req) {
			// 可选指定某条线路(device/source)单独探测; 不传=按默认路由探测
			let device = '';
			let source = '';

			if (type(req.args) == 'object') {
				if (type(req.args.device) == 'string')
					device = req.args.device;
				if (type(req.args.source) == 'string')
					source = req.args.source;
			}
			else if (type(req.args) == 'array') {
				if (type(req.args[0]) == 'string')
					device = req.args[0];
				if (type(req.args[1]) == 'string')
					source = req.args[1];
			}

			let extra = '';

			// 白名单校验: 参数会被拼进 shell 命令, 只放行网卡名与 IPv4
			if (length(device) > 0) {
				if (match(device, /^[A-Za-z0-9._-]+$/) != null)
					extra = extra + ' --device ' + device;
			}

			if (length(source) > 0) {
				if (match(source, /^[0-9.]+$/) != null)
					extra = extra + ' --source ' + source;
			}

			let out = exec(CLI + ' check --timeout 6' + extra + ' 2>&1');

			if (out == null)
				out = '无法执行 /usr/bin/portalkeeperd';

			return { output: out };
		}
	},

	trigger: {
		call: function(req) {
			writefile(TRIGGER_PATH, '1');

			return { ok: true };
		}
	},

	autostart_on: {
		call: function(req) {
			const uci = cursor();

			uci.set('portalkeeper', 'main', 'autostart', '1');
			uci.commit('portalkeeper');

			exec(INIT + ' enable 2>&1');

			return { ok: true };
		}
	},

	autostart_off: {
		call: function(req) {
			const uci = cursor();

			uci.set('portalkeeper', 'main', 'autostart', '0');
			uci.commit('portalkeeper');

			exec(INIT + ' disable 2>&1');

			return { ok: true };
		}
	},

	service_start: {
		call: function(req) {
			return { output: exec(INIT + ' start 2>&1') };
		}
	},

	service_stop: {
		call: function(req) {
			return { output: exec(INIT + ' stop 2>&1') };
		}
	},

	apply: {
		call: function(req) {
			return { ok: true, output: exec(INIT + ' restart 2>&1') };
		}
	},

	getlog: {
		call: function(req) {
			if (!access(LOG_PATH))
				return { log: '' };

			const text = readfile(LOG_PATH);

			if (text == null)
				return { log: '' };

			// 不用 join/slice(在 rpcd 沙箱里行为异常), 直接朴素拼接
			const lines = split(text, '\n');
			let buf = '';

			for (let line in lines) {
				if (length(buf) > 0)
					buf = buf + '\n';

				buf = buf + line;
			}

			return { log: buf };
		}
	},

	clearlog: {
		call: function(req) {
			writefile(LOG_PATH, '');

			return { ok: true };
		}
	},

	getifaces: {
		call: function(req) {
			// 主列表: 与「网络 → 接口」一致的 netifd 接口(名称/设备/协议/连接状态)
			const interfaces = [];
			const seen = {};
			const dump = ubus.call('network.interface', 'dump', {});

			if (type(dump) == 'object' && type(dump.interface) == 'array') {
				for (let e in dump.interface) {
					let dev = '';

					if (type(e.l3_device) == 'string' && e.l3_device != '')
						dev = e.l3_device;
					else if (type(e.device) == 'string')
						dev = e.device;

					if (dev == '' || seen[dev] == true)
						continue;

					// 过滤 loopback 与隧道类接口(校园网认证用不到)
					if (dev == 'lo' || match(dev, /^ipsec|^sit|^gre|gretap|erspan|teql|ip6tnl|6rd|6to4/) != null)
						continue;

					seen[dev] = true;
					push(interfaces, {
						name: (type(e.interface) == 'string') ? e.interface : dev,
						device: dev,
						up: (e.up == true),
						proto: (type(e.proto) == 'string') ? e.proto : ''
					});
				}
			}

			// 补充: 未挂到任何网络接口的物理网卡(过滤隧道/虚拟口)
			const extra = [];
			const text = readfile('/proc/net/dev');

			if (text != null) {
				const lines = split(text, '\n');

				for (let line in lines) {
					const pos = index(line, ':');

					if (pos <= 0)
						continue;

					const name = trim(substr(line, 0, pos));

					if (name == 'lo' || name == '' || seen[name] == true)
						continue;

					if (match(name, /^sit|gre|gretap|erspan|teql|ipsec|ip6tnl|6rd|6to4/) != null)
						continue;

					push(extra, name);
				}
			}

			return { interfaces: interfaces, extra: extra };
		}
	},

	getifinfo: {
		args: { device: 'string', source: 'string' },
		call: function(req) {
			// 兼容两种传参: 对象 {device,source} 与位置数组 [device,source]
			// (LuCI 新版 rpc.js 带 params 声明时可能按位置打包参数)
			let device = '';
			let source = '';

			if (type(req.args) == 'object') {
				if (type(req.args.device) == 'string')
					device = req.args.device;
				if (type(req.args.source) == 'string')
					source = req.args.source;
			}
			else if (type(req.args) == 'array') {
				if (type(req.args[0]) == 'string')
					device = req.args[0];
				if (type(req.args[1]) == 'string')
					source = req.args[1];
			}

			return ifinfo(device, source);
		}
	}
};

// rpcd 要求: return 的字典键即为 ubus 对象名
return { 'portalkeeper': methods };