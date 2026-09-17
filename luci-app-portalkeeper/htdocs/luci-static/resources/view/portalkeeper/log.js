'use strict';
'require view';
'require rpc';
'require poll';
'require ui';

var callGetLog = rpc.declare({ object: 'portalkeeper', method: 'getlog' });
var callClearLog = rpc.declare({ object: 'portalkeeper', method: 'clearlog' });

return view.extend({
	handleRefresh: function(ev) {
		return callGetLog().then(function(res) {
			var el = document.getElementById('portalkeeper-log');
			if (el)
				el.textContent = (res && res.log) ? res.log : '(暂无日志)';
		});
	},

	handleClear: function(ev) {
		var self = this;
		return callClearLog().then(function() {
			return self.handleRefresh();
		});
	},

	load: function() {
		return callGetLog();
	},

	render: function(data) {
		var view = E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, '日志'),
			E('div', { 'class': 'cbi-map-descr' },
				'认证守护进程的运行日志(含服务启动/停止记录)。日志只保存在 /tmp(tmpfs), 不写入闪存, ' +
				'因此不会磨损 NAND, 但路由器重启后自动清空(上限约 256KB, 自动滚动裁剪)。'),

			E('div', { 'class': 'cbi-page-actions' }, [
				E('button', {
					'class': 'btn cbi-button cbi-button-apply',
					'click': ui.createHandlerFn(this, 'handleRefresh')
				}, '刷新'),
				'\u00a0',
				E('button', {
					'class': 'btn cbi-button cbi-button-negative',
					'click': ui.createHandlerFn(this, 'handleClear')
				}, '清空日志')
			]),

			E('div', { 'class': 'cbi-section' }, [
				E('pre', {
					'id': 'portalkeeper-log',
					'style': 'max-height:480px; overflow:auto; font-size:12px; background:#fff; padding:8px; white-space:pre-wrap'
				}, [ data && data.log ? data.log : '(暂无日志)' ])
			]),

			E('div', { 'class': 'cbi-section-descr' },
				'tip: 日志包含服务启动/停止记录与每次探测/认证的详细过程(账号不会出现在日志中); ' +
				'时间为路由器本地时间。页面每 5 秒自动刷新一次, 也可手动点「刷新」。')
		]);

		poll.add(L.bind(function() {
			return callGetLog().then(function(res) {
				var el = document.getElementById('portalkeeper-log');
				if (el)
					el.textContent = (res && res.log) ? res.log : '(暂无日志)';
			});
		}, this), 5);

		return view;
	}
});
