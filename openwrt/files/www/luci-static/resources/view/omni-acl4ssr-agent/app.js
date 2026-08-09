'use strict';
'require view';
'require form';
'require uci';
'require fs';
'require ui';
'require rpc';

const callServiceList = rpc.declare({
	object: 'service',
	method: 'list',
	params: ['name'],
	expect: { '': {} }
});

function isRunning() {
	return L.resolveDefault(callServiceList('omni-acl4ssr-agent'), {}).then(function (res) {
		try {
			return !!res['omni-acl4ssr-agent']['instances']['instance1']['running'];
		} catch (e) {
			return false;
		}
	});
}

function listenBase() {
	const host = window.location.hostname;
	const listen = uci.get('omni_acl4ssr_agent', 'config', 'listen') || '0.0.0.0:8787';
	const port = (listen.split(':').pop()) || '8787';
	return 'http://' + host + ':' + port;
}

function ensureStatusSection() {
	if (!uci.sections('omni_acl4ssr_agent', 'status').length)
		uci.add('omni_acl4ssr_agent', 'status', 'status');
}

return view.extend({
	load: function () {
		return Promise.all([
			uci.load('omni_acl4ssr_agent'),
			isRunning()
		]);
	},

	render: function (data) {
		const running = data[1];
		const base = listenBase();
		let m, s, o;

		ensureStatusSection();

		m = new form.Map('omni_acl4ssr_agent', _('订阅转换'),
			_('omni-acl4ssr-agent：本地 Mihomo 订阅转换（国家分组 / AI·币安·奈飞规则 / SOCKS5·HTTP 落地链式代理）。'));

		/* 状态栏：TableSection 横向排布，避免 NamedSection 在 Argon 下标签/控件间距过大 */
		s = m.section(form.TableSection, 'status', _('状态'));
		s.anonymous = true;
		s.addremove = false;

		o = s.option(form.DummyValue, '_running', _('运行状态'));
		o.cfgvalue = function () {
			return E('span', {
				style: 'font-weight:bold;color:' + (running ? 'green' : 'red')
			}, running ? _('运行中') : _('未运行'));
		};

		o = s.option(form.DummyValue, '_listen_show', _('监听'));
		o.cfgvalue = function () {
			return uci.get('omni_acl4ssr_agent', 'config', 'listen') || '0.0.0.0:8787';
		};

		o = s.option(form.DummyValue, '_sub', _('订阅 /sub'));
		o.cfgvalue = function () {
			return E('a', {
				href: base + '/sub',
				target: '_blank',
				rel: 'noopener'
			}, base + '/sub');
		};

		o = s.option(form.Button, '_open');
		o.inputtitle = _('打开控制台');
		o.inputstyle = 'apply';
		o.onclick = function () {
			window.open(base + '/', '_blank', 'noopener');
		};

		o = s.option(form.Button, '_restart');
		o.inputtitle = _('重启');
		o.inputstyle = 'action';
		o.onclick = function () {
			return fs.exec('/etc/init.d/omni-acl4ssr-agent', ['restart']).then(function () {
				ui.addNotification(null, E('p', _('已重启 omni-acl4ssr-agent')), 'info');
				window.setTimeout(function () { location.reload(); }, 800);
			});
		};

		/* 服务配置：仅保留必要 UCI 项 */
		s = m.section(form.NamedSection, 'config', 'omni_acl4ssr_agent', _('服务'));
		s.anonymous = true;

		o = s.option(form.Flag, 'enabled', _('启用'));
		o.rmempty = false;

		o = s.option(form.Value, 'listen', _('监听地址'));
		o.placeholder = '0.0.0.0:8787';
		o.rmempty = false;
		o.description = _('Nikki 订阅可填写：') + base + '/sub';

		return m.render().then(function (node) {
			const style = E('style', {}, [
				'[data-page="admin-services-omni-acl4ssr-agent"] .cbi-section-table-row .td {',
				'  vertical-align: middle;',
				'}',
				'[data-page="admin-services-omni-acl4ssr-agent"] .omni-acl4ssr-agent-console {',
				'  margin: 0;',
				'  padding: 0;',
				'  overflow: hidden;',
				'}',
				'[data-page="admin-services-omni-acl4ssr-agent"] .omni-acl4ssr-agent-console-head {',
				'  display: flex;',
				'  align-items: center;',
				'  justify-content: space-between;',
				'  gap: 0.75rem;',
				'  padding: 0 0.25rem;',
				'}',
				'[data-page="admin-services-omni-acl4ssr-agent"] .omni-acl4ssr-agent-console-head > h3 {',
				'  margin: 0;',
				'}',
				'[data-page="admin-services-omni-acl4ssr-agent"] .omni-acl4ssr-agent-refresh {',
				'  font-size: 12px;',
				'  font-weight: normal;',
				'  text-decoration: none;',
				'  white-space: nowrap;',
				'}',
				'[data-page="admin-services-omni-acl4ssr-agent"] .omni-acl4ssr-agent-refresh:hover {',
				'  text-decoration: underline;',
				'}',
				'[data-page="admin-services-omni-acl4ssr-agent"] .omni-acl4ssr-agent-frame {',
				'  display: block;',
				'  width: 100%;',
				'  min-height: 560px;',
				'  height: calc(100vh - 18rem);',
				'  border: 0;',
				'  background: #fff;',
				'}'
			].join('\n'));

			/* 每次进入页面带时间戳，避免浏览器把旧 index.html / JS 缓存进 iframe */
			function consoleSrc() {
				return base + '/?_=' + Date.now();
			}

			const frame = E('iframe', {
				src: consoleSrc(),
				title: _('omni-acl4ssr-agent 控制台'),
				class: 'omni-acl4ssr-agent-frame'
			});

			const refreshLink = E('a', {
				href: '#',
				class: 'omni-acl4ssr-agent-refresh',
				title: _('重新加载控制台前端（绕过缓存）')
			}, _('刷新前端'));

			refreshLink.addEventListener('click', function (ev) {
				ev.preventDefault();
				frame.src = consoleSrc();
			});

			const consoleSection = E('div', { class: 'cbi-section omni-acl4ssr-agent-console' }, [
				E('div', { class: 'omni-acl4ssr-agent-console-head' }, [
					E('h3', {}, _('控制台')),
					refreshLink
				]),
				frame
			]);

			node.insertBefore(style, node.firstChild);

			const footer = node.querySelector('.cbi-page-actions');
			if (footer)
				node.insertBefore(consoleSection, footer);
			else
				node.appendChild(consoleSection);

			return node;
		});
	}
});
