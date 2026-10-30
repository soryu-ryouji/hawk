//! 设备选择器页（原生 HTML，无构建步骤）：两级登录（service-key 换门令牌 → hawk-app key 换 bt）
//! → 设备列表（bt 绑定的单个设备）→ 打开查看器。

use axum::response::Html;

pub async fn picker() -> Html<&'static str> {
    Html(PICKER_HTML)
}

const PICKER_HTML: &str = r##"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>hawk 网页版</title>
<style>
  :root { color-scheme: light dark; }
  * { box-sizing: border-box; }
  body { margin: 0; font-family: system-ui, -apple-system, sans-serif; background: #121212; color: #e8e8e8; min-height: 100vh; display: flex; justify-content: center; padding: 48px 16px; }
  main { width: 100%; max-width: 480px; }
  h1 { font-size: 20px; margin: 0 0 4px; }
  .sub { color: #999; font-size: 13px; margin-bottom: 24px; }
  input { width: 100%; padding: 10px 12px; border-radius: 8px; border: 1px solid #444; background: #1e1e1e; color: #e8e8e8; font-size: 14px; }
  input:focus { outline: none; border-color: #4a9eff; }
  button { padding: 8px 16px; border-radius: 8px; border: none; background: #4a9eff; color: #fff; font-size: 14px; cursor: pointer; }
  button:disabled { opacity: .5; cursor: default; }
  button.ghost { background: transparent; border: 1px solid #555; color: #ccc; }
  .row { display: flex; gap: 8px; margin-top: 12px; }
  .err { color: #ff6b6b; font-size: 13px; margin-top: 10px; }
  .device { display: flex; align-items: center; gap: 12px; padding: 12px; border: 1px solid #333; border-radius: 10px; margin-bottom: 10px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: #666; flex-shrink: 0; }
  .dot.on { background: #4caf50; }
  .info { flex: 1; min-width: 0; }
  .name { font-size: 14px; }
  .meta { font-size: 12px; color: #999; }
  .current { display: flex; align-items: center; justify-content: space-between; padding: 12px; border: 1px solid #4a9eff; border-radius: 10px; margin-bottom: 16px; font-size: 13px; }
  .hidden { display: none; }
</style>
</head>
<body>
<main>
  <h1>hawk 网页版</h1>
  <div class="sub">远程浏览你的素材库（只读）</div>

  <section id="login">
    <div id="stepKey" class="hidden">
      <input id="appKey" type="password" placeholder="访问密钥（hawk-app key）" autocomplete="off">
      <div class="row">
        <button onclick="doLogin()">登录</button>
        <button class="ghost" onclick="backToServiceKey()">上一步</button>
      </div>
    </div>
    <div id="stepService">
      <input id="serviceKey" type="password" placeholder="服务密钥（service-key）" autocomplete="off">
      <div class="row"><button onclick="doGate()">下一步</button></div>
    </div>
    <div id="loginErr" class="err"></div>
  </section>

  <section id="list" class="hidden">
    <div id="current" class="current hidden">
      <span id="currentLabel"></span>
      <div class="row" style="margin:0">
        <button onclick="enter()">进入</button>
        <button class="ghost" onclick="doDisconnect()">断开</button>
      </div>
    </div>
    <div id="devices"></div>
    <div id="listErr" class="err"></div>
    <div class="row" style="margin-top:16px"><button class="ghost" onclick="doLogout()">注销</button></div>
  </section>
</main>
<script>
const KEY = 'hawk_bt';
let token = new URLSearchParams(location.search).get('token') || localStorage.getItem(KEY) || '';
if (token) localStorage.setItem(KEY, token);

const $ = (id) => document.getElementById(id);

async function call(method, path, body) {
  const res = await fetch(path, {
    method,
    headers: { 'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const env = await res.json().catch(() => null);
  if (!res.ok || !env || env.status === 'error') {
    throw new Error(env?.error?.message || ('HTTP ' + res.status));
  }
  return env.data;
}

let gateToken = sessionStorage.getItem('hawk_gt') || '';
// 有门令牌（刷新/回退后）直接进第二级
if (gateToken) { $('stepService').classList.add('hidden'); $('stepKey').classList.remove('hidden'); }

function backToServiceKey() {
  gateToken = ''; sessionStorage.removeItem('hawk_gt');
  $('stepKey').classList.add('hidden'); $('stepService').classList.remove('hidden');
  $('loginErr').textContent = '';
}

async function doGate() {
  $('loginErr').textContent = '';
  const serviceKey = $('serviceKey').value.trim();
  if (!serviceKey) return;
  try {
    const res = await fetch('/api/v1/viewer/gate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ service_key: serviceKey }),
    });
    const env = await res.json().catch(() => null);
    if (!res.ok || !env || env.status === 'error') throw new Error(env?.error?.message || '验证失败');
    gateToken = env.data.gate_token;
    sessionStorage.setItem('hawk_gt', gateToken);
    $('stepService').classList.add('hidden'); $('stepKey').classList.remove('hidden');
  } catch (e) { $('loginErr').textContent = e.message; }
}

async function doLogin() {
  $('loginErr').textContent = '';
  const key = $('appKey').value.trim();
  if (!key) return;
  try {
    const res = await fetch('/api/v1/viewer/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ gate_token: gateToken, key }),
    });
    const env = await res.json().catch(() => null);
    if (!res.ok || !env || env.status === 'error') {
      // 门令牌过期/耗尽：回第一级重新验证 service-key
      if (res.status === 401) backToServiceKey();
      throw new Error(env?.error?.message || '登录失败');
    }
    token = env.data.browser_token;
    localStorage.setItem(KEY, token);
    render();
  } catch (e) { $('loginErr').textContent = e.message; }
}

async function doLogout() {
  try { await call('POST', '/api/v1/viewer/logout'); } catch {}
  token = ''; localStorage.removeItem(KEY);
  render();
}

async function doDisconnect() {
  try { await call('DELETE', '/api/v1/viewer/session'); } catch {}
  render();
}

function enter() { location.href = '/app?token=' + encodeURIComponent(token); }

async function openDevice(deviceId, btn) {
  $('listErr').textContent = '';
  btn.disabled = true; btn.textContent = '接入中…';
  try {
    await call('POST', '/api/v1/viewer/session', { target_device_id: deviceId });
    enter();
  } catch (e) {
    $('listErr').textContent = e.message;
    btn.disabled = false; btn.textContent = '浏览素材库';
  }
}

async function render() {
  if (!token) { $('login').classList.remove('hidden'); $('list').classList.add('hidden'); return; }
  $('login').classList.add('hidden'); $('list').classList.remove('hidden');
  try {
    const [devices, current] = await Promise.all([
      call('GET', '/api/v1/viewer/devices'),
      call('GET', '/api/v1/viewer/session').catch(() => null),
    ]);
    if (current) {
      $('current').classList.remove('hidden');
      $('currentLabel').textContent = '当前浏览：' + current.target_name + ' · ' + current.target_library_name;
    } else {
      $('current').classList.add('hidden');
    }
    const list = (devices.devices || []).filter((d) => d.platform !== 'server');
    $('devices').innerHTML = '';
    if (!list.length) $('devices').innerHTML = '<div class="meta">暂无设备。在本机 hawk 设置 → 远程中接入本服务端即可出现。</div>';
    for (const d of list) {
      const el = document.createElement('div');
      el.className = 'device';
      el.innerHTML = '<span class="dot' + (d.online ? ' on' : '') + '"></span>' +
        '<div class="info"><div class="name"></div><div class="meta"></div></div>';
      el.querySelector('.name').textContent = d.name;
      el.querySelector('.meta').textContent = d.library_name + ' · ' + d.platform + (d.allow_remote ? '' : ' · 已关闭访问');
      if (d.online && d.allow_remote) {
        const btn = document.createElement('button');
        btn.textContent = '浏览素材库';
        btn.onclick = () => openDevice(d.device_id, btn);
        el.appendChild(btn);
      } else {
        const span = document.createElement('span');
        span.className = 'meta';
        span.textContent = d.online ? '不可访问' : '离线';
        el.appendChild(span);
      }
      $('devices').appendChild(el);
    }
  } catch (e) {
    if (e.message.includes('未登录') || e.message.includes('令牌')) {
      token = ''; localStorage.removeItem(KEY); render();
    } else {
      $('listErr').textContent = e.message;
    }
  }
}
render();
</script>
</body>
</html>
"##;
