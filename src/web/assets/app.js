/* local-router's web page. No framework, no build step. */

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));
const el = (tag, cls, text) => {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
};

let state = { providers: [], groups: [], models: [], presets: [], settings: {}, status: {} };

/* ------------------------------------------------------------------ net -- */

async function api(path, options = {}) {
  const res = await fetch(path, {
    headers: { 'content-type': 'application/json' },
    ...options,
    body: options.body ? JSON.stringify(options.body) : undefined,
  });
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch (_) { data = { error: text }; }
  if (!res.ok) throw new Error((data && data.error) || `请求失败 (${res.status})`);
  return data;
}

let toastTimer = null;
function toast(message, bad = false) {
  const node = $('#toast');
  node.textContent = message;
  node.classList.toggle('bad', bad);
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { node.hidden = true; }, 2600);
}

/* ----------------------------------------------------------------- tabs -- */

$$('#tabs button').forEach((button) => {
  button.addEventListener('click', () => {
    $$('#tabs button').forEach((b) => b.classList.remove('active'));
    button.classList.add('active');
    $$('.panel').forEach((p) => p.classList.remove('active'));
    const panel = document.getElementById(button.dataset.tab);
    if (panel) panel.classList.add('active');
    if (button.dataset.tab === 'usage') loadUsage();
  });
});

/* ----------------------------------------------------------------- load -- */

async function loadAll() {
  const [status, providers, groups, models] = await Promise.all([
    api('/api/status'),
    api('/api/providers'),
    api('/api/groups'),
    api('/api/models'),
  ]);
  state.status = status;
  state.settings = status.settings || {};
  state.providers = providers.providers || [];
  state.groups = groups.groups || [];
  state.models = models.models || [];

  $('#version').textContent = 'v' + (status.version || '');
  const s = state.settings;
  $('#gateway-addr').textContent = `http://${s.gateway_addr || '127.0.0.1'}:${s.gateway_port || 3425}`;

  renderOverview();
  renderProviders();
  renderModels();
  renderGroups();
  renderSettings();
}

async function loadUsage() {
  const data = await api('/api/usage');
  const t = data.totals || {};
  const tiles = $('#usage-tiles');
  tiles.innerHTML = '';
  tiles.append(
    tile(t.requests || 0, '请求'),
    tile(t.failed || 0, '失败', (t.failed || 0) > 0),
    tile((t.input_tokens || 0).toLocaleString(), '输入 token'),
    tile((t.output_tokens || 0).toLocaleString(), '输出 token'),
    tile((t.cache_read_tokens || 0).toLocaleString(), '缓存读取'),
  );

  const models = $('#usage-models');
  models.innerHTML = '';
  const rows = data.by_model || [];
  if (!rows.length) models.append(el('div', 'empty', '还没有记录'));
  else for (const [name, s] of rows) models.append(ratioRow(name, s.requests, s.output_tokens));

  const days = $('#usage-days');
  days.innerHTML = '';
  const dayRows = data.by_day || [];
  if (!dayRows.length) days.append(el('div', 'empty', '还没有记录'));
  else {
    const max = Math.max(...dayRows.map(([, s]) => s.requests), 1);
    for (const [name, s] of dayRows) {
      const row = el('div', 'row');
      const grow = el('div', 'grow');
      grow.append(el('div', 'title', name));
      const bar = el('div', 'bar');
      const fill = el('i');
      fill.style.width = `${Math.round((s.requests / max) * 100)}%`;
      bar.append(fill);
      grow.append(bar);
      row.append(grow, el('span', 'pill', `${s.requests} 次`));
      days.append(row);
    }
  }

  const recent = $('#usage-recent');
  recent.innerHTML = '';
  const list = data.recent || [];
  if (!list.length) recent.append(el('div', 'empty', '还没有记录'));
  else for (const r of list.slice(0, 60)) recent.append(requestRow(r));
}

function tile(n, label, bad) {
  const t = el('div', 'tile' + (bad ? ' bad' : ''));
  t.append(el('div', 'n', String(n)), el('div', 'l', label));
  return t;
}

function ratioRow(name, requests, tokens) {
  const row = el('div', 'row');
  const grow = el('div', 'grow');
  grow.append(el('div', 'title', name));
  grow.append(el('div', 'sub', `${tokens || 0} 输出 token`));
  row.append(grow, el('span', 'pill', `${requests} 次`));
  return row;
}

function requestRow(r) {
  const row = el('div', 'row');
  const grow = el('div', 'grow');
  grow.append(el('div', 'title', r.model || '—'));
  const when = (r.time || '').replace('T', ' ').slice(0, 19);
  grow.append(el('div', 'sub', `${when} · ${r.provider || '—'} · ${r.latency_ms || 0}ms`));
  row.append(grow);
  row.append(el('span', 'pill' + (r.ok ? ' on' : ' bad'), r.ok ? '成功' : '失败'));
  if (r.output_tokens) row.append(el('span', 'pill', `${r.output_tokens} out`));
  return row;
}

/* ------------------------------------------------------------- overview -- */

function renderOverview() {
  const tiles = $('#tiles');
  tiles.innerHTML = '';
  tiles.append(
    tile(state.providers.length, '服务商'),
    tile(state.models.length, '模型'),
    tile(state.groups.length, '路由组'),
    tile(state.status.version ? 'v' + state.status.version : '—', '版本'),
  );

  const s = state.settings;
  const addr = `http://${s.gateway_addr || '127.0.0.1'}:${s.gateway_port || 3425}`;
  $('#env-box').textContent = [
    `export OPENAI_BASE_URL=${addr}/v1`,
    `export OPENAI_API_KEY=${s.default_caller_key || 'local-router'}`,
    `export ANTHROPIC_BASE_URL=${addr}`,
    `export ANTHROPIC_API_KEY=${s.default_caller_key || 'local-router'}`,
    `export GOOGLE_GEMINI_BASE_URL=${addr}`,
    `export GEMINI_API_KEY=${s.default_caller_key || 'local-router'}`,
  ].join('\n');

  const box = $('#overview-requests');
  box.innerHTML = '';
  box.append(el('div', 'muted', '打开「用量」看最近的请求。'));
}

/* ------------------------------------------------------------ providers -- */

function renderProviders() {
  const list = $('#providers-list');
  list.innerHTML = '';
  if (!state.providers.length) {
    list.append(el('div', 'empty', '还没有服务商。点右上角「添加服务商」。'));
    return;
  }
  for (const p of state.providers) {
    const row = el('div', 'row');
    const grow = el('div', 'grow');
    const title = el('div', 'title');
    title.append(document.createTextNode(p.name + ' '));
    title.append(el('span', 'pill' + (p.enabled ? ' on' : ' off'), p.enabled ? '开' : '关'));
    title.append(el('span', 'pill', p.id));
    grow.append(title);
    grow.append(el('div', 'sub', (p.base_url || '—') + ` · ${p.keys.length} 个密钥 · ${p.models.length} 个模型`));
    if (p.models.length) grow.append(el('div', 'sub', p.models.join(', ')));
    row.append(grow);

    const actions = el('div', 'actions');
    actions.append(button('切换', 'tiny', () => toggleProvider(p)));
    actions.append(button('刷新模型', 'tiny', () => refreshProvider(p)));
    actions.append(button('测试', 'tiny', () => testProvider(p)));
    actions.append(button('加密钥', 'tiny', () => addProviderKey(p)));
    actions.append(button('删除', 'tiny danger', () => deleteProvider(p)));
    row.append(actions);
    list.append(row);
  }
}

function button(text, cls, onClick) {
  const b = el('button', cls, text);
  b.addEventListener('click', onClick);
  return b;
}

async function toggleProvider(p) {
  await api(`/api/providers/${p.id}`, { method: 'PATCH', body: { enabled: !p.enabled } });
  await loadAll();
}

async function refreshProvider(p) {
  try {
    const data = await api(`/api/providers/${p.id}/refresh`, { method: 'POST' });
    if (!data.models || !data.models.length) return toast('厂商没有返回模型', true);
    await api(`/api/providers/${p.id}/models`, { method: 'POST', body: { models: data.models } });
    toast(`已更新 ${data.models.length} 个模型`);
    await loadAll();
  } catch (err) { toast(err.message, true); }
}

async function testProvider(p) {
  toast('正在测试…');
  try {
    const data = await api(`/api/providers/${p.id}/test`, { method: 'POST' });
    toast(data.ok ? `通了，${data.latency_ms}ms` : `失败：${data.status}`, !data.ok);
  } catch (err) { toast(err.message, true); }
}

async function addProviderKey(p) {
  const key = prompt('这个服务商的密钥');
  if (!key) return;
  try {
    await api(`/api/providers/${p.id}/keys`, { method: 'POST', body: { key } });
    toast('密钥已加上');
    await loadAll();
  } catch (err) { toast(err.message, true); }
}

async function deleteProvider(p) {
  if (!confirm(`删除 ${p.name}？`)) return;
  try {
    await api(`/api/providers/${p.id}`, { method: 'DELETE' });
    toast('已删除');
    await loadAll();
  } catch (err) { toast(err.message, true); }
}

/* --------------------------------------------------------------- models -- */

function renderModels() {
  const list = $('#models-list');
  list.innerHTML = '';
  const q = ($('#model-search').value || '').toLowerCase();
  const models = state.models.filter((m) => !q || m.id.toLowerCase().includes(q));
  if (!models.length) {
    list.append(el('div', 'empty', q ? '没有匹配的模型' : '还没有模型。先加一个服务商。'));
    return;
  }
  for (const m of models) {
    const row = el('div', 'row');
    const grow = el('div', 'grow');
    grow.append(el('div', 'title', m.id));
    grow.append(el('div', 'sub', m.provider_name || m.provider));
    row.append(grow);
    row.append(el('span', 'pill' + (m.enabled ? ' on' : ' off'), m.group ? '路由组' : '模型'));
    list.append(row);
  }
}

$('#model-search').addEventListener('input', renderModels);

/* --------------------------------------------------------------- groups -- */

function renderGroups() {
  const list = $('#groups-list');
  list.innerHTML = '';
  if (!state.groups.length) {
    list.append(el('div', 'empty', '还没有路由组。'));
    return;
  }
  for (const g of state.groups) {
    const row = el('div', 'row');
    const grow = el('div', 'grow');
    const title = el('div', 'title');
    title.append(document.createTextNode(g.name + ' '));
    title.append(el('span', 'pill', 'group/' + g.id));
    title.append(el('span', 'pill', g.routing));
    title.append(el('span', 'pill', '粘性 ' + g.stays));
    grow.append(title);
    grow.append(el('div', 'sub', g.models.join(', ') || '（没有成员）'));
    row.append(grow);
    const actions = el('div', 'actions');
    actions.append(button('编辑', 'tiny', () => openGroupModal(g)));
    actions.append(button('删除', 'tiny danger', () => deleteGroup(g)));
    row.append(actions);
    list.append(row);
  }
}

async function deleteGroup(g) {
  if (!confirm(`删除路由组 ${g.name}？`)) return;
  try {
    await api(`/api/groups/${g.id}`, { method: 'DELETE' });
    toast('已删除');
    await loadAll();
  } catch (err) { toast(err.message, true); }
}

/* ------------------------------------------------------------- settings -- */

function renderSettings() {
  const s = state.settings;
  const form = $('#settings-form');
  form.innerHTML = '';
  form.append(
    field('网关端口', 'gateway_port', 'number', s.gateway_port),
    field('网页端口', 'web_port', 'number', s.web_port),
    field('网关地址', 'gateway_addr', 'text', s.gateway_addr),
    field('网页地址', 'web_addr', 'text', s.web_addr),
    field('默认调用密钥', 'default_caller_key', 'text', s.default_caller_key),
    toggle('局域网共享', 'lan', s.lan),
    toggle('把 agent 自己的 User-Agent 发给厂商', 'send_agent_user_agent', s.send_agent_user_agent),
    toggle('记录用量', 'record_usage', s.record_usage),
  );
  loadCallerKeys();
}

function field(label, name, type, value) {
  const wrap = el('label');
  wrap.append(document.createTextNode(label));
  const input = el('input');
  input.type = type;
  input.value = value === undefined || value === null ? '' : value;
  input.dataset.name = name;
  wrap.append(input);
  return wrap;
}

function toggle(label, name, value) {
  const wrap = el('label', 'check');
  const input = el('input');
  input.type = 'checkbox';
  input.checked = !!value;
  input.dataset.name = name;
  wrap.append(input, document.createTextNode(label));
  return wrap;
}

$('#save-settings').addEventListener('click', async () => {
  const body = {};
  $$('#settings-form [data-name]').forEach((input) => {
    const name = input.dataset.name;
    if (input.type === 'checkbox') body[name] = input.checked;
    else if (input.type === 'number') body[name] = Number(input.value);
    else body[name] = input.value;
  });
  try {
    await api('/api/settings', { method: 'PUT', body });
    toast('已保存（重启后生效）');
    await loadAll();
  } catch (err) { toast(err.message, true); }
});

async function loadCallerKeys() {
  const data = await api('/api/caller-keys');
  const list = $('#caller-keys');
  list.innerHTML = '';
  if (!data.keys.length) list.append(el('div', 'muted', '还没有别的密钥。'));
  for (const k of data.keys) {
    const row = el('div', 'row');
    const grow = el('div', 'grow');
    grow.append(el('div', 'title', k.name));
    grow.append(el('div', 'sub', k.key));
    row.append(grow);
    row.append(el('span', 'pill' + (k.enabled ? ' on' : ' off'), k.enabled ? '开' : '关'));
    row.append(button('删除', 'tiny danger', async () => {
      await api(`/api/caller-keys/${k.id}`, { method: 'DELETE' });
      loadCallerKeys();
    }));
    list.append(row);
  }
}

$('#add-caller-key').addEventListener('click', async () => {
  const name = prompt('这个密钥给谁用？');
  if (!name) return;
  await api('/api/caller-keys', { method: 'POST', body: { name } });
  loadCallerKeys();
});

/* --------------------------------------------------------------- modal -- */

$$('[data-close]').forEach((b) => b.addEventListener('click', closeModals));
function closeModals() {
  $('#provider-modal').hidden = true;
  $('#group-modal').hidden = true;
}

let selectedPreset = null;

$('#add-provider').addEventListener('click', async () => {
  selectedPreset = null;
  $('#p-name').value = '';
  $('#p-url').value = '';
  $('#p-key').value = '';
  $('#p-models').value = '';
  const data = await api('/api/presets');
  const tiles = $('#preset-tiles');
  tiles.innerHTML = '';
  for (const group of data.presets) {
    for (const p of group.presets) {
      const node = el('div', 'preset');
      node.append(el('div', 'pn', p.name));
      node.append(el('div', 'pg', group.group + ' · ' + p.protocol));
      node.addEventListener('click', () => {
        selectedPreset = p.id;
        $$('#preset-tiles .preset').forEach((n) => n.classList.remove('sel'));
        node.classList.add('sel');
        if (!$('#p-name').value) $('#p-name').value = p.name;
        if (!$('#p-url').value && p.base_url) $('#p-url').value = p.base_url;
        if (!$('#p-models').value && p.default_models && p.default_models.length) {
          $('#p-models').value = p.default_models.join(', ');
        }
        $('#p-hint').textContent = p.key_hint ? `密钥形如 ${p.key_hint}` : '';
      });
      tiles.append(node);
    }
  }
  $('#provider-modal').hidden = false;
});

$('#save-provider').addEventListener('click', async () => {
  const name = $('#p-name').value.trim();
  const url = $('#p-url').value.trim();
  const key = $('#p-key').value.trim();
  const models = $('#p-models').value.split(',').map((s) => s.trim()).filter(Boolean);
  if (!selectedPreset && !name) return toast('先选一个厂商，或者写个名称', true);
  try {
    await api('/api/providers', {
      method: 'POST',
      body: { preset: selectedPreset || 'custom', name, base_url: url || null, key: key || null, models },
    });
    closeModals();
    toast('已添加');
    await loadAll();
  } catch (err) { toast(err.message, true); }
});

let editingGroup = null;

$('#add-group').addEventListener('click', () => openGroupModal(null));

function openGroupModal(group) {
  editingGroup = group || null;
  $('#group-modal-title').textContent = group ? '编辑路由组' : '添加路由组';
  $('#g-name').value = group ? group.name : '';
  $('#g-models').value = group ? group.models.join(', ') : '';
  $('#g-routing').value = group ? group.routing : 'order';
  $('#g-stays').value = group ? group.stays : 'auto';
  $('#g-listed').checked = group ? group.listed : true;
  $('#group-modal').hidden = false;
}

$('#save-group').addEventListener('click', async () => {
  const name = $('#g-name').value.trim();
  if (!name) return toast('给路由组起个名字', true);
  const models = $('#g-models').value.split(',').map((s) => s.trim()).filter(Boolean);
  const body = {
    name,
    models,
    routing: $('#g-routing').value,
    stays: $('#g-stays').value,
    listed: $('#g-listed').checked,
  };
  try {
    if (editingGroup) await api(`/api/groups/${editingGroup.id}`, { method: 'PATCH', body });
    else await api('/api/groups', { method: 'POST', body });
    closeModals();
    toast('已保存');
    await loadAll();
  } catch (err) { toast(err.message, true); }
});

$('#refresh-usage').addEventListener('click', loadUsage);

/* ---------------------------------------------------------------- boot -- */

loadAll().catch((err) => toast(err.message, true));
setInterval(() => {
  if (document.querySelector('#usage').classList.contains('active')) loadUsage();
}, 10000);
