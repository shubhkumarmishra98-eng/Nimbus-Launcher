'use strict';
const T = window.__TAURI__;
const invoke = T.core.invoke, listen = T.event.listen;
const $ = (s, r = document) => r.querySelector(s);
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const cap = s => s ? s[0].toUpperCase() + s.slice(1) : s;
const fmtN = n => n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e3 ? (n / 1e3).toFixed(1) + 'k' : String(n);
const fmtSize = b => b > 1073741824 ? (b / 1073741824).toFixed(1) + ' GB' : b > 1048576 ? (b / 1048576).toFixed(1) + ' MB' : Math.max(1, Math.round(b / 1024)) + ' KB';
const LOADERS = ['vanilla', 'fabric', 'quilt', 'forge', 'neoforge'];
const KINDS = [['mod', 'Mods'], ['resourcepack', 'Resource packs'], ['shader', 'Shaders'], ['modpack', 'Modpacks']];
const FOLDERS = [['saves', 'Worlds'], ['resourcepacks', 'Resource packs'], ['shaderpacks', 'Shader packs'], ['screenshots', 'Screenshots']];

const S = {
  page: 'library', instances: [], accounts: [], settings: {}, sys: { total_mb: 8192 },
  running: new Set(), starting: new Set(), logs: {}, prog: {}, cur: null, tab: 'mods', libQ: '',
  mods: [], updates: null, files: {}, browse: { q: '', source: 'modrinth', kind: 'mod', hits: [], offset: 0, busy: false },
  skinAcc: null
};

async function call(cmd, args) {
  try { return await invoke(cmd, args || {}); }
  catch (e) { toast(String(e), true); return undefined; }
}
function toast(msg, err) {
  const t = document.createElement('div');
  t.className = 't' + (err ? ' err' : '');
  t.textContent = msg;
  $('#toast').appendChild(t);
  setTimeout(() => t.remove(), err ? 9000 : 3500);
}
function modal(html) { $('#modal-root').innerHTML = `<div class="modal-bg" data-act="modal-bg"><div class="modal">${html}</div></div>`; }
function closeModal() { $('#modal-root').innerHTML = ''; }
async function pickFile(name, exts) {
  const p = await T.dialog.open({ multiple: false, filters: [{ name, extensions: exts }] });
  return typeof p === 'string' ? p : null;
}
const curInst = () => S.instances.find(i => i.id === S.cur);
const curAcc = () => S.accounts.find(a => a.selected) || S.accounts[0];
async function reloadInstances() { S.instances = (await call('instances_list')) || S.instances; }
function applyTheme() { document.documentElement.dataset.theme = S.settings.theme === 'light' ? 'light' : 'dark'; }
function findingsHtml(list) {
  return list.map(f => `<div class="finding"><b>${esc(f.title)}</b><div>${esc(f.advice)}</div></div>`).join('');
}

/* ---------------- visual helpers ---------------- */
const svg = p => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${p}</svg>`;
const ICON = {
  library: svg('<path d="M4 7l8-4 8 4v10l-8 4-8-4z"/><path d="M4 7l8 4 8-4M12 11v10"/>'),
  accounts: svg('<circle cx="12" cy="8" r="4"/><path d="M4 21c0-4 4-6 8-6s8 2 8 6"/>'),
  skins: svg('<path d="M8 3L3 7l3 3 2-1v12h8V9l2 1 3-3-5-4c-1 2-5 2-6 0z"/>'),
  settings: svg('<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9L7 7M17 17l2.1 2.1M19.1 4.9L17 7M7 17l-2.1 2.1"/>')
};
function hueOf(str) { let h = 0; for (const c of String(str)) h = (h * 31 + c.charCodeAt(0)) % 360; return h; }
function cube(name, size = 44) {
  const h = hueOf(name), f = l => `hsl(${h} 58% ${l}%)`;
  return `<svg class="cube" width="${size}" height="${size}" viewBox="0 0 48 48" aria-hidden="true"><polygon points="24,3 43,14 24,25 5,14" fill="${f(66)}"/><polygon points="5,14 24,25 24,45 5,34" fill="${f(47)}"/><polygon points="43,14 24,25 24,45 43,34" fill="${f(35)}"/></svg>`;
}
const metaOf = i => 'Minecraft ' + i.mc_version + (i.loader !== 'vanilla' ? ' with ' + cap(i.loader) : '');
const barHtml = id => `<div class="bar" id="bar-${esc(id)}"><i></i><span></span></div><button class="btn sm cancel" data-act="cancel">Cancel</button>`;

/* ---------------- shell ---------------- */
function renderNav() {
  const items = [['library', 'Library'], ['accounts', 'Accounts'], ['skins', 'Skins'], ['settings', 'Settings']];
  $('#nav').innerHTML = items.map(([k, l]) =>
    `<button class="nav-item ${S.page === k || (k === 'library' && S.page === 'instance') ? 'on' : ''}" data-act="nav" data-page="${k}">${ICON[k]}<span>${l}</span></button>`).join('');
}
function renderAccountBox() {
  const cur = curAcc();
  $('#account-box').innerHTML = cur
    ? `<button class="acct" data-act="acctmenu" title="Switch account"><span class="acct-av">${esc(cur.username[0].toUpperCase())}</span><span class="acct-name">${esc(cur.username)}</span></button>`
    : `<button class="acct" data-act="nav" data-page="accounts"><span class="acct-av">+</span><span class="acct-name">Add account</span></button>`;
}
function render() {
  renderNav(); renderAccountBox();
  const fn = { library: pageLibrary, instance: pageInstance, accounts: pageAccounts, skins: pageSkins, settings: pageSettings }[S.page];
  $('#main').innerHTML = fn();
  if (S.page === 'library' || S.page === 'instance') { refreshPlay(); for (const p of Object.values(S.prog)) onProgress(p); }
  if (S.page === 'instance') renderTab();
  if (S.page === 'skins') loadSkins();
}
function go(page) { S.page = page; render(); }

/* ---------------- library ---------------- */
function tileHtml(i) {
  return `<div class="tile">${cube(i.name, 40)}
    <div class="tile-main"><h3>${esc(i.name)}</h3><p>${esc(i.mc_version)}${i.loader !== 'vanilla' ? ' · ' + esc(cap(i.loader)) : ''}</p></div>
    <div class="tile-actions"><button class="btn primary sm" data-act="play" data-id="${esc(i.id)}">Play</button>
    <button class="btn sm" data-act="manage" data-id="${esc(i.id)}">Open</button></div>${barHtml(i.id)}</div>`;
}
function heroHtml(i) {
  return `<section class="hero"><div class="hero-art">${cube(i.name, 150)}</div>
    <div class="hero-body"><h1>${esc(i.name)}</h1><p class="hero-meta">${esc(metaOf(i))}</p>${barHtml(i.id)}
    <div class="row"><button class="btn primary big" data-act="play" data-id="${esc(i.id)}">Play</button>
    <button class="btn" data-act="manage" data-id="${esc(i.id)}">Mods &amp; settings</button></div></div></section>`;
}
function emptyHero() {
  return `<section class="hero"><div class="hero-art">${cube('Nimbus', 150)}</div>
    <div class="hero-body"><h1>Start your first world</h1>
    <p class="hero-meta">We download the latest Minecraft and open it for you. The first start takes a few minutes; after that it’s quick.</p>
    <div class="row"><button class="btn primary big" data-act="quickstart">Create &amp; play</button>
    <button class="btn" data-act="new">Custom instance</button></div></div></section>`;
}
function libCards() {
  const q = S.libQ.trim().toLowerCase(), all = S.instances;
  if (q) {
    const list = all.filter(i => (i.name + ' ' + i.mc_version + ' ' + i.loader).toLowerCase().includes(q));
    return list.length ? `<div class="tiles">${list.map(tileHtml).join('')}</div>` : '<p class="muted">Nothing matches your search.</p>';
  }
  if (!all.length) return emptyHero();
  return heroHtml(all[0]) + (all.length > 1 ? `<h2 class="section">Your instances</h2><div class="tiles">${all.slice(1).map(tileHtml).join('')}</div>` : '');
}
function pageLibrary() {
  return `<div class="head"><h1>Library</h1><div class="row">
      <input id="lib-q" placeholder="Search" value="${esc(S.libQ)}" data-input="lib-q" style="width:190px">
      <button class="btn" data-act="import">Import modpack</button>
      <button class="btn primary" data-act="new">New instance</button></div></div>
    <div id="grid">${libCards()}</div>`;
}
function refreshPlay() {
  document.querySelectorAll('[data-act="play"]').forEach(b => {
    const id = b.dataset.id, run = S.running.has(id), st = S.starting.has(id);
    b.textContent = run ? 'Stop' : st ? 'Starting…' : 'Play';
    b.disabled = st && !run;
  });
}
function onProgress(p) {
  S.prog[p.task] = p;
  const el = document.getElementById('bar-' + p.task);
  if (!el) return;
  if (p.label === 'Running' || p.label === 'Ready') { el.classList.remove('on'); delete S.prog[p.task]; return; }
  el.classList.add('on'); el.classList.toggle('ind', !p.total);
  el.querySelector('i').style.width = p.total ? (p.done / p.total * 100) + '%' : '';
  el.querySelector('span').textContent = p.label + (p.total ? ` ${p.done}/${p.total}` : '…');
}
function hideBar(id) { delete S.prog[id]; const el = document.getElementById('bar-' + id); if (el) el.classList.remove('on'); }
function onState(s) {
  if (s.state === 'running') S.running.add(s.id);
  if (s.state === 'exited') { S.running.delete(s.id); hideBar(s.id); reloadInstances(); }
  if (s.state === 'idle') { S.starting.delete(s.id); hideBar(s.id); }
  refreshPlay();
}
function onCrash(c) {
  const inst = S.instances.find(i => i.id === c.id);
  modal(`<h2>${esc(inst ? inst.name : 'The game')} crashed${c.code != null ? ' (exit code ' + c.code + ')' : ''}</h2>
    ${findingsHtml(c.findings || [])}
    <div class="row end"><button class="btn" data-act="gotoconsole" data-id="${esc(c.id)}">Open console</button>
    <button class="btn" data-act="repairid" data-id="${esc(c.id)}">Repair files</button><button class="btn primary" data-act="close">Close</button></div>`);
}
function onLog(l) {
  const arr = (S.logs[l.id] ||= []);
  for (const line of l.lines) arr.push(line);
  if (arr.length > 3000) arr.splice(0, arr.length - 2500);
  const c = document.getElementById('console');
  if (c && S.cur === l.id) {
    const frag = document.createDocumentFragment();
    l.lines.forEach(x => frag.appendChild(logLine(x)));
    c.appendChild(frag);
    while (c.childNodes.length > 3000) c.removeChild(c.firstChild);
    c.scrollTop = c.scrollHeight;
  }
}
function logLine(t) {
  const d = document.createElement('div');
  d.textContent = t;
  if (/ERROR|Exception|FATAL/.test(t)) d.className = 'e'; else if (/WARN/.test(t)) d.className = 'w';
  return d;
}

/* ---------------- new instance ---------------- */
async function openNew() {
  const total = Math.max(2048, Math.min(S.sys.total_mb || 8192, 32768));
  const def = autoMem();
  modal(`<h2>New instance</h2>
    <label class="f"><span>Name</span><input id="n-name" placeholder="My survival world"></label>
    <label class="chk"><input type="checkbox" id="n-snap" data-change="n-snap" ${S.settings.show_snapshots ? 'checked' : ''}> Show snapshots</label>
    <label class="f"><span>Minecraft version</span><select id="n-ver" data-change="n-ver"><option>Loading…</option></select></label>
    <div class="kv"><label class="f"><span>Mod loader</span><select id="n-loader" data-change="n-loader">${LOADERS.map(l => `<option value="${l}">${cap(l)}</option>`).join('')}</select></label>
    <label class="f"><span>Loader version</span><select id="n-lv"><option value="">Latest</option></select></label></div>
    <label class="f"><span>Memory: <b id="n-memv">${def}</b> MB</span><input type="range" id="n-mem" min="1024" max="${total}" step="256" value="${def}" data-input="n-mem"></label>
    <label class="chk" id="n-perf-row" style="display:none"><input type="checkbox" id="n-perf" checked> Install performance mods (Sodium, Lithium, FerriteCore…)</label>
    <div class="row end"><button class="btn" data-act="close">Cancel</button><button class="btn primary" data-act="create">Create</button></div>`);
  loadVersionList();
}
async function loadVersionList(p = 'n', sel, selLv) {
  const snap = $('#' + p + '-snap'); if (!snap) return;
  const v = await call('list_versions', { snapshots: snap.checked });
  const el = $('#' + p + '-ver'); if (!el) return;
  const list = v || [];
  el.innerHTML = list.map(x => `<option value="${esc(x.id)}">${esc(x.id)}</option>`).join('');
  if (sel && list.some(x => x.id === sel)) el.value = sel;
  await loadLoaderVersions(p, selLv);
}
async function loadLoaderVersions(p = 'n', selLv) {
  const loaderEl = $('#' + p + '-loader'); if (!loaderEl) return;
  const loader = loaderEl.value, mc = $('#' + p + '-ver').value;
  const perf = $('#' + p + '-perf-row'); if (perf) perf.style.display = loader === 'vanilla' ? 'none' : 'flex';
  const lv = $('#' + p + '-lv');
  if (loader === 'vanilla') { lv.innerHTML = '<option value="">—</option>'; lv.disabled = true; return; }
  lv.disabled = false; lv.innerHTML = '<option value="">Loading…</option>';
  const v = await call('loader_versions', { loader, mcVersion: mc });
  if (!$('#' + p + '-lv')) return;
  lv.innerHTML = (v && v.length) ? v.map((x, k) => `<option value="${esc(x)}">${esc(x)}${k === 0 ? ' (latest)' : ''}</option>`).join('')
    : '<option value="">Not available for this version</option>';
  if (selLv && v && v.includes(selLv)) lv.value = selLv;
}

/* ---------------- instance page ---------------- */
function pageInstance() {
  const i = curInst();
  if (!i) { S.page = 'library'; return pageLibrary(); }
  const tabs = [['mods', 'Mods'], ['browse', 'Browse'], ['files', 'Files'], ['settings', 'Settings'], ['console', 'Console']];
  return `<div class="head"><div class="row" style="gap:16px;flex-wrap:nowrap">
      <button class="btn sm" data-act="nav" data-page="library">Back</button>${cube(i.name, 54)}
      <div><h1>${esc(i.name)}</h1><div class="muted">${esc(metaOf(i))}${i.loader_version ? ' ' + esc(i.loader_version) : ''}</div></div></div>
      <button class="btn primary big" data-act="play" data-id="${esc(i.id)}">Play</button></div>
    ${barHtml(i.id)}
    <div class="tabs" style="margin-top:18px">${tabs.map(([k, l]) => `<button class="tab ${S.tab === k ? 'on' : ''}" data-act="tab" data-tab="${k}">${l}</button>`).join('')}</div>
    <div id="tabbody"></div>`;
}
function renderTab() {
  const el = $('#tabbody'); if (!el) return;
  document.querySelectorAll('.tab').forEach(t => t.classList.toggle('on', t.dataset.tab === S.tab));
  const i = curInst(); if (!i) return;
  if (S.tab === 'mods') el.innerHTML = modsHtml(i);
  else if (S.tab === 'browse') el.innerHTML = browseHtml(i);
  else if (S.tab === 'files') el.innerHTML = filesHtml();
  else if (S.tab === 'settings') { el.innerHTML = instSettingsHtml(i); initRetarget(i); }
  else if (S.tab === 'console') {
    el.innerHTML = `<div class="row" style="margin-bottom:8px"><button class="btn sm" data-act="clearlog">Clear</button>
      <button class="btn sm" data-act="analyze">Analyze last run</button><button class="btn sm" data-act="sharelog">Share log (mclo.gs)</button></div><div class="console" id="console"></div>`;
    const c = $('#console'); (S.logs[i.id] || []).forEach(l => c.appendChild(logLine(l))); c.scrollTop = c.scrollHeight;
  }
}
async function loadMods() {
  S.mods = (await call('mods_list', { instanceId: S.cur })) || [];
  if (S.tab === 'mods') renderTab();
  const id = S.cur;
  invoke('mods_info', { instanceId: id }).then(r => {
    if (S.cur !== id) return;
    S.modInfo = Object.fromEntries((r || []).map(x => [x.file, x]));
    if (S.tab === 'mods') renderTab();
  }).catch(() => { });
}
async function loadFiles() {
  const res = await Promise.all(FOLDERS.map(([k]) => call('folder_list', { instanceId: S.cur, kind: k })));
  FOLDERS.forEach(([k], n) => S.files[k] = res[n] || []);
  S.servers = (await call('servers_list', { instanceId: S.cur })) || [];
  S.enabledPacks = (await call('packs_enabled', { instanceId: S.cur })) || [];
  if (S.tab === 'files') renderTab();
}
function modsHtml(i) {
  const upd = S.updates, info = S.modInfo || {};
  const updHtml = upd === null ? '' : upd.length
    ? `<div class="panel" style="margin-bottom:12px"><div class="row" style="justify-content:space-between"><b>${upd.length} update${upd.length > 1 ? 's' : ''} available</b>
        <button class="btn primary sm" data-act="applyupd">Update all</button></div>
        <div class="muted small" style="margin-top:6px">${upd.map(u => esc(u.title) + ' → ' + esc(u.latest)).join(' · ')}</div></div>`
    : `<div class="panel muted" style="margin-bottom:12px">Everything is up to date.</div>`;
  const list = S.mods.map(m => {
    const n = info[m.file];
    return `<div class="item">${n && n.icon ? `<img class="icon" src="${esc(n.icon)}" loading="lazy">` : ''}
      <div class="grow"><b>${esc(n ? n.title : m.file)}</b><p class="muted small">${n ? esc(n.version) + ' · ' : ''}${esc(m.file)} · ${fmtSize(m.size)}${m.enabled ? '' : ' · disabled'}</p></div>
      <button class="btn sm" data-act="togglemod" data-file="${esc(m.file)}" data-on="${m.enabled ? 0 : 1}">${m.enabled ? 'Disable' : 'Enable'}</button>
      <button class="btn sm danger" data-act="delmod" data-file="${esc(m.file)}">Remove</button></div>`;
  }).join('');
  return `<div class="row" style="margin-bottom:12px"><button class="btn" data-act="addjar">Add .jar</button>
      <button class="btn" data-act="checkupd" ${i.loader === 'vanilla' ? 'disabled' : ''}>Check for updates</button>
      <button class="btn" data-act="openmods">Open folder</button></div>${updHtml}
    <div class="list">${list || `<p class="muted">${i.loader === 'vanilla' ? 'This is a vanilla instance — create one with Fabric, Quilt, Forge or NeoForge to use mods.' : 'No mods installed yet. Try the Browse tab.'}</p>`}</div>`;
}
function browseHtml(i) {
  const b = S.browse;
  const hits = b.hits.map(h => `<div class="item">${h.icon ? `<img class="icon" src="${esc(h.icon)}" loading="lazy">` : '<div class="icon item-ph"></div>'}
      <div class="grow"><b>${esc(h.title)}</b><p class="muted small">${esc(h.description)}</p>
      <p class="muted small">by ${esc(h.author)} · ${fmtN(h.downloads)} downloads</p></div>
      <button class="btn primary sm" data-act="install" data-pid="${esc(h.id)}" data-src="${esc(h.source)}">${b.kind === 'modpack' ? 'Install as new instance' : 'Install'}</button></div>`).join('');
  const note = b.kind === 'mod' && i.loader === 'vanilla' ? '<p class="muted">Vanilla instances can’t use mods — create an instance with a mod loader.</p>' : '';
  return `<div class="row" style="margin-bottom:12px"><select id="bkind">${KINDS.map(([k, l]) => `<option value="${k}" ${b.kind === k ? 'selected' : ''}>${l}</option>`).join('')}</select>
      <input id="bq" style="flex:1;min-width:160px" placeholder="Search…" value="${esc(b.q)}" data-enter="search">
      <select id="bsrc"><option value="modrinth" ${b.source === 'modrinth' ? 'selected' : ''}>Modrinth</option><option value="curseforge" ${b.source === 'curseforge' ? 'selected' : ''}>CurseForge (mods)</option></select>
      <button class="btn primary" data-act="search">Search</button></div>${note}
    <div class="list" id="hits">${hits || '<p class="muted">Press Search to browse popular results for ' + esc(i.mc_version) + '.</p>'}</div>
    ${b.hits.length ? '<div style="margin-top:12px"><button class="btn" data-act="more">Load more</button></div>' : ''}`;
}
async function runSearch(append) {
  const i = curInst(), b = S.browse; if (!i || b.busy) return;
  b.busy = true;
  const r = await call('mods_search', { query: b.q, mcVersion: b.kind === 'modpack' ? '' : i.mc_version, loader: i.loader, source: b.source, offset: b.offset, kind: b.kind });
  b.busy = false;
  if (r === undefined) return;
  b.hits = append ? b.hits.concat(r) : r;
  if (S.tab === 'browse') renderTab();
}
function serversHtml() {
  const rows = (S.servers || []).map((v, n) => `<div class="item"><input class="srv-name" placeholder="Name" value="${esc(v.name)}" style="width:32%">
      <input class="srv-ip" placeholder="host:port" value="${esc(v.ip)}" style="flex:1">
      <button class="btn sm" data-act="srvplay" data-i="${n}">Auto-join</button><button class="btn sm danger" data-act="srvdel" data-i="${n}">Remove</button></div>`).join('');
  return `<div class="panel" style="margin-bottom:12px"><div class="fl-head"><h2>Servers</h2><div class="row">
      <button class="btn sm" data-act="srvadd">Add server</button><button class="btn sm primary" data-act="srvsave">Save list</button></div></div>
      <div class="list">${rows || '<p class="muted small">No servers saved yet.</p>'}</div></div>`;
}
function collectServers() {
  const names = [...document.querySelectorAll('.srv-name')], ips = [...document.querySelectorAll('.srv-ip')];
  S.servers = names.map((n, k) => ({ name: n.value, ip: ips[k].value }));
}
function filesHtml() {
  return serversHtml() + FOLDERS.map(([k, l]) => `<div class="panel" style="margin-bottom:12px"><div class="fl-head"><h2>${l}</h2>
      <div class="row">${k !== 'screenshots' ? `<button class="btn sm" data-act="fadd" data-kind="${k}">Add…</button>` : ''}<button class="btn sm" data-act="fopen" data-kind="${k}">Open folder</button></div></div>
      <div class="list">${(S.files[k] || []).map(e => `<div class="item"><div class="grow"><b>${esc(e.name)}</b><p class="muted small">${e.size ? fmtSize(e.size) : ''}${e.is_dir ? ' · folder' : ''}</p></div>
        ${k === 'saves' ? `<button class="btn sm" data-act="wbackup" data-name="${esc(e.name)}">Back up</button>` : ''}${k === 'resourcepacks' ? packBtn(e.name) : ''}
        <button class="btn sm danger" data-act="fdel" data-kind="${k}" data-name="${esc(e.name)}">Delete</button></div>`).join('') || '<p class="muted small">Nothing here yet.</p>'}</div></div>`).join('');
}
function instSettingsHtml(i) {
  const total = Math.max(2048, Math.min(S.sys.total_mb || 8192, 32768));
  return `<div class="panel" style="max-width:600px">
    <label class="f"><span>Name</span><input id="is-name" value="${esc(i.name)}"></label>
    <label class="f"><span>Memory: <b id="is-memv">${i.memory_mb}</b> MB</span><input type="range" id="is-mem" min="1024" max="${total}" step="256" value="${i.memory_mb}" data-input="is-mem"></label>
    <label class="f"><span>Java executable (leave empty to use the managed runtime)</span><input id="is-java" value="${esc(i.java_path)}"></label>
    <label class="f"><span>Extra JVM arguments</span><input id="is-jvm" value="${esc(i.jvm_args)}"></label>
    <label class="f"><span>Join server on launch (host or host:port)</span><input id="is-server" value="${esc(i.server || '')}" placeholder="play.example.net"></label>
    <label class="f"><span>Launch wrapper command (e.g. gamemoderun) — optional</span><input id="is-wrap" value="${esc(i.wrapper || '')}"></label>
    <div class="kv"><label class="f"><span>Window width (0 = default)</span><input id="is-w" type="number" min="0" value="${i.width || 0}"></label>
    <label class="f"><span>Window height</span><input id="is-h" type="number" min="0" value="${i.height || 0}"></label></div>
    <label class="chk"><input type="checkbox" id="is-shared" ${i.use_shared !== false ? 'checked' : ''}> Use the shared mods and resource packs folders</label>
    <label class="chk"><input type="checkbox" id="is-fs" ${i.fullscreen ? 'checked' : ''}> Start in fullscreen</label>
    <div class="row"><button class="btn primary" data-act="saveinst">Save</button>
      <button class="btn" data-act="preinstall">Download files now</button>
      <button class="btn" data-act="repair">Repair / verify files</button></div>
    <div class="row" style="margin-top:10px"><button class="btn" data-act="dupinst">Duplicate</button>
      <button class="btn" data-act="exportinst">Export (.mrpack)</button>
      <button class="btn" data-act="openinst">Open folder</button>
      <button class="btn danger" data-act="delinst">Delete instance</button></div></div>
  <div class="panel" style="max-width:600px;margin-top:14px"><h2>Minecraft version &amp; loader</h2>
    <p class="muted small">Switching keeps your worlds and settings, but mods built for the old version may stop working.</p>
    <label class="chk"><input type="checkbox" id="r-snap" data-change="r-snap"> Show snapshots</label>
    <label class="f"><span>Minecraft version</span><select id="r-ver" data-change="r-ver"></select></label>
    <div class="kv"><label class="f"><span>Mod loader</span><select id="r-loader" data-change="r-loader">${LOADERS.map(l => `<option value="${l}">${cap(l)}</option>`).join('')}</select></label>
    <label class="f"><span>Loader version</span><select id="r-lv"></select></label></div>
    <button class="btn" data-act="retarget">Apply</button></div>`;
}
async function initRetarget(i) {
  const l = $('#r-loader'); if (!l) return;
  l.value = i.loader;
  await loadVersionList('r', i.mc_version, i.loader_version);
}

/* ---------------- accounts ---------------- */
function pageAccounts() {
  const list = S.accounts.map(a => `<div class="item"><div class="avatar">${esc(a.username[0].toUpperCase())}</div>
      <div class="grow"><b>${esc(a.username)}</b><p class="muted small">${a.kind === 'microsoft' ? 'Microsoft account' : 'Offline account'}${a.selected ? ' · selected' : ''}</p></div>
      ${a.selected ? '' : `<button class="btn sm" data-act="selacc" data-id="${esc(a.id)}">Use</button>`}
      <button class="btn sm danger" data-act="rmacc" data-id="${esc(a.id)}">Remove</button></div>`).join('');
  return `<div class="head"><h1>Accounts</h1></div>
    <div class="list" style="max-width:640px;margin-bottom:22px">${list || '<p class="muted">No accounts yet.</p>'}</div>
    <div class="grid" style="max-width:820px">
      <div class="panel"><h2>Microsoft account</h2><p class="muted small">Sign in with the account that owns Minecraft: Java Edition. Needed for online servers and uploading skins.</p>
        ${S.msOk ? '<button class="btn primary" data-act="msadd">Sign in with Microsoft</button>' : '<p class="muted small">Microsoft sign-in isn’t set up in this build — use a local account.</p>'}</div>
      <div class="panel"><h2>Offline account</h2><p class="muted small">Play single-player and offline-mode servers with any username.</p>
        <div class="row"><input id="off-name" placeholder="Username" maxlength="16" data-enter="offadd"><button class="btn" data-act="offadd">Add</button></div></div>
    </div>`;
}
async function msLogin() {
  const d = await call('ms_login_start');
  if (!d) return;
  if (navigator.clipboard) navigator.clipboard.writeText(d.user_code).catch(() => { });
  call('open_url', { url: d.verification_uri });
  modal(`<h2>Sign in with Microsoft</h2>
    <p>We opened <b>${esc(d.verification_uri)}</b> in your browser and copied this code — paste it there:</p><div class="code">${esc(d.user_code)}</div>
    <p class="muted small">Waiting for you to finish signing in…</p>
    <div class="row end"><button class="btn" data-act="openms" data-url="${esc(d.verification_uri)}">Open page again</button><button class="btn" data-act="mscancel">Cancel</button></div>`);
  const r = await invoke('ms_login_poll', { deviceCode: d.device_code, interval: d.interval, expiresIn: d.expires_in })
    .catch(e => { if (!/cancel/i.test(String(e))) toast(String(e), true); return undefined; });
  if (r) { S.accounts = r; closeModal(); toast('Signed in'); if (S.page === 'accounts') render(); else renderAccountBox(); afterLogin(); }
}
function autoMem() {
  const total = Math.max(2048, Math.min(S.sys.total_mb || 8192, 32768));
  return Math.min(S.settings.default_memory_mb || 4096, Math.max(2048, Math.floor(total / 2 / 256) * 256));
}
function showWelcome() {
  modal(`<h2>Welcome to Nimbus 👋</h2><p class="muted">Pick how you want to play — you can add more accounts later.</p>
    <div class="panel" style="margin-bottom:12px"><h3>Microsoft account</h3>
      <p class="muted small">${S.msOk ? 'For online servers, your own skin and capes.' : 'Not available in this build — use a local account below.'}</p>
      <button class="btn primary" data-act="welcomems" ${S.msOk ? '' : 'disabled'}>Sign in with Microsoft</button></div>
    <div class="panel"><h3>Local (offline) account</h3><p class="muted small">Just pick a name — no sign-in needed.</p>
      <div class="row"><input id="w-name" placeholder="Username" maxlength="16" data-enter="welcomeoff" style="flex:1"><button class="btn primary" data-act="welcomeoff">Play offline</button></div></div>
    <div class="row end"><button class="btn" data-act="close">Skip</button></div>`);
}
function showQuickStart() {
  modal(`<h2>Create your first game</h2><p class="muted">One click: we’ll download the latest Minecraft and start it.</p>
    <label class="chk"><input type="checkbox" id="q-opt" checked> Optimised (Fabric + performance mods — higher FPS)</label>
    <div class="row end"><button class="btn" data-act="close">Not now</button><button class="btn primary" data-act="quickplay">Create &amp; play</button></div>`);
}
function afterLogin() { if (!S.instances.length) showQuickStart(); }
async function importPath(p) {
  toast('Importing modpack…');
  const r = await call('import_pack', { path: p });
  if (r) { await reloadInstances(); toast('Imported ' + r.name); if (S.page === 'library') render(); }
}
function dropModal(p) {
  S.dropPath = p;
  modal(`<h2>What is this .zip?</h2><p class="muted small" style="word-break:break-all">${esc(p)}</p>
    <div class="row"><button class="btn primary" data-act="dropas" data-kind="resourcepacks">Resource pack</button><button class="btn" data-act="dropas" data-kind="shaderpacks">Shader pack</button>
    <button class="btn" data-act="dropas" data-kind="saves">World</button><button class="btn" data-act="dropas" data-kind="modpack">Modpack</button></div>
    <div class="row end"><button class="btn" data-act="close">Cancel</button></div>`);
}
async function onDrop(paths) {
  for (const p of paths) {
    const e = p.split('.').pop().toLowerCase();
    if (e === 'mrpack') await importPath(p);
    else if (e === 'jar' && S.page === 'instance') {
      if ((await call('mods_add_local', { instanceId: S.cur, path: p })) !== undefined) { toast('Mod added'); if (S.tab === 'mods') loadMods(); }
    } else if (e === 'zip') { if (S.page === 'instance') dropModal(p); else await importPath(p); }
    else if (e === 'png' && S.page === 'skins') {
      const acc = S.accounts.find(a => a.id === S.skinAcc); if (!acc) continue;
      const ok = acc.kind === 'offline'
        ? await call('offline_skin_set', { accountId: acc.id, skinPath: p, capePath: null })
        : await call('skin_upload', { accountId: acc.id, path: p, variant: ($('#sk-var') || {}).value || 'classic' });
      if (ok !== undefined) { toast('Skin updated'); loadSkins(); }
    }
  }
}
function packBtn(name) {
  const on = (S.enabledPacks || []).some(x => x === name || x === 'file/' + name);
  return `<button class="btn sm ${on ? 'primary' : ''}" data-act="packtoggle" data-name="${esc(name)}" data-on="${on ? 0 : 1}">${on ? 'Enabled ✓' : 'Enable'}</button>`;
}

/* ---------------- skins (3D viewer) ---------------- */
function skin3d(host, src, slimHint) {
  host.innerHTML = '<div class="scene"><div class="model"></div></div>';
  const model = host.querySelector('.model');
  const img = new Image();
  img.onload = () => {
    const modern = img.height >= 64, K = 7;
    let slim = slimHint;
    if (slim === undefined && modern) {
      const c = document.createElement('canvas'); c.width = c.height = 64;
      const x = c.getContext('2d'); x.drawImage(img, 0, 0);
      slim = x.getImageData(54, 20, 1, 1).data[3] === 0;
    }
    const aw = slim ? 3 : 4;
    const crop = (x, y, w, h) => { const c = document.createElement('canvas'); c.width = w; c.height = h; c.getContext('2d').drawImage(img, x, y, w, h, 0, 0, w, h); return c.toDataURL(); };
    const box = (cx, cy, cz, w, h, d, u, v, grow) => {
      const el = document.createElement('div'); el.className = 'box';
      el.style.transform = `translate3d(${cx * K}px,${cy * K}px,${cz * K}px)`;
      const W = (w + grow) * K, H = (h + grow) * K, D = (d + grow) * K;
      const faces = [
        [w, h, u + d, v + d, `translateZ(${D / 2}px)`, W, H],
        [w, h, u + d + w + d, v + d, `rotateY(180deg) translateZ(${D / 2}px)`, W, H],
        [d, h, u, v + d, `rotateY(-90deg) translateZ(${W / 2}px)`, D, H],
        [d, h, u + d + w, v + d, `rotateY(90deg) translateZ(${W / 2}px)`, D, H],
        [w, d, u + d, v, `rotateX(90deg) translateZ(${H / 2}px)`, W, D],
        [w, d, u + d + w, v, `rotateX(-90deg) translateZ(${H / 2}px)`, W, D]];
      for (const [fw, fh, fx, fy, tf, pw, ph] of faces) {
        const f = document.createElement('i');
        f.style.cssText = `width:${pw}px;height:${ph}px;margin:${-ph / 2}px 0 0 ${-pw / 2}px;transform:${tf};background-image:url(${crop(fx, fy, fw, fh)})`;
        el.appendChild(f);
      }
      model.appendChild(el);
    };
    box(0, -12, 0, 8, 8, 8, 0, 0, 0);
    box(0, -2, 0, 8, 12, 4, 16, 16, 0);
    box(-(4 + aw / 2), -2, 0, aw, 12, 4, 40, 16, 0);
    box(4 + aw / 2, -2, 0, aw, 12, 4, modern ? 32 : 40, modern ? 48 : 16, 0);
    box(-2, 10, 0, 4, 12, 4, 0, 16, 0);
    box(2, 10, 0, 4, 12, 4, modern ? 16 : 0, modern ? 48 : 16, 0);
    box(0, -12, 0, 8, 8, 8, 32, 0, 1);
    if (modern) {
      box(0, -2, 0, 8, 12, 4, 16, 32, 1);
      box(-(4 + aw / 2), -2, 0, aw, 12, 4, 40, 32, 1);
      box(4 + aw / 2, -2, 0, aw, 12, 4, 48, 48, 1);
      box(-2, 10, 0, 4, 12, 4, 0, 32, 1);
      box(2, 10, 0, 4, 12, 4, 0, 48, 1);
    }
    let ang = 25, drag = false, lastX = 0;
    host.onpointerdown = e => { drag = true; lastX = e.clientX; host.setPointerCapture(e.pointerId); };
    host.onpointermove = e => { if (drag) { ang += (e.clientX - lastX) * 0.8; lastX = e.clientX; } };
    host.onpointerup = () => { drag = false; };
    const tick = () => {
      if (!host.isConnected) return;
      if (!drag) ang += 0.5;
      model.style.transform = `rotateX(-8deg) rotateY(${ang}deg)`;
      requestAnimationFrame(tick);
    };
    tick();
  };
  img.src = src;
}
function pageSkins() {
  if (!S.accounts.length) return `<div class="head"><h1>Skins &amp; capes</h1></div><p class="muted">Add an account first.</p>`;
  if (!S.accounts.find(a => a.id === S.skinAcc)) S.skinAcc = curAcc().id;
  return `<div class="head"><h1>Skins &amp; capes</h1>
    <select data-change="skin-acc">${S.accounts.map(a => `<option value="${esc(a.id)}" ${a.id === S.skinAcc ? 'selected' : ''}>${esc(a.username)} · ${a.kind === 'microsoft' ? 'Microsoft' : 'offline'}</option>`).join('')}</select></div>
    <div id="skinbody" class="muted">Loading…</div>`;
}
async function loadSkins() {
  const body = $('#skinbody'); if (!body) return;
  const acc = S.accounts.find(a => a.id === S.skinAcc); if (!acc) return;
  if (acc.kind === 'offline') {
    const d = await call('offline_skin_get', { accountId: acc.id }); if (!d || !$('#skinbody')) return;
    body.className = '';
    body.innerHTML = `<div class="split"><div class="viewer" id="skincv"></div>
      <div><div class="panel"><h2>Local skin</h2>
        <p class="muted small">Offline accounts have no skin server, so Nimbus installs the CustomSkinLoader mod into your modded instances and feeds it this skin. Vanilla instances can’t show custom skins. Drag the preview to rotate it.</p>
        <div class="row"><button class="btn primary" data-act="oskin">Choose skin PNG…</button><button class="btn" data-act="oskin-rm" ${d.skin ? '' : 'disabled'}>Remove skin</button>
        <button class="btn" data-act="ocape">Choose cape PNG…</button><button class="btn" data-act="ocape-rm" ${d.cape ? '' : 'disabled'}>Remove cape</button></div>
        <p class="muted small" style="margin-top:10px">${d.cape ? 'A cape is set.' : 'No cape set.'}</p></div></div></div>`;
    if (d.skin) skin3d($('#skincv'), d.skin); else $('#skincv').innerHTML = '<p class="muted" style="padding:20px">No skin chosen yet.</p>';
    return;
  }
  const p = await call('skin_profile', { accountId: acc.id }); if (!p || !$('#skinbody')) return;
  const skin = (p.skins || []).find(s => s.state === 'ACTIVE');
  const capes = p.capes || [];
  body.className = '';
  body.innerHTML = `<div class="split"><div class="viewer" id="skincv"></div>
    <div><div class="panel" style="margin-bottom:14px"><h2>Skin</h2>
      <div class="row"><select id="sk-var"><option value="classic" ${skin && skin.variant === 'CLASSIC' ? 'selected' : ''}>Classic (wide arms)</option><option value="slim" ${skin && skin.variant === 'SLIM' ? 'selected' : ''}>Slim (thin arms)</option></select>
      <button class="btn primary" data-act="pskin">Upload 64×64 PNG…</button></div></div>
    <div class="panel"><h2>Capes</h2>${capes.length ? `<div class="list">${capes.map(c => `<div class="item"><div class="grow"><b>${esc(c.alias || 'Cape')}</b><p class="muted small">${c.state === 'ACTIVE' ? 'Equipped' : 'Owned'}</p></div>
        ${c.state === 'ACTIVE' ? '' : `<button class="btn sm" data-act="cape" data-id="${esc(c.id)}">Equip</button>`}</div>`).join('')}</div>
        <div style="margin-top:10px"><button class="btn sm" data-act="cape" data-id="">Hide cape</button></div>` : '<p class="muted">This account doesn’t own any capes.</p>'}</div></div></div>`;
  if (skin) {
    const data = await call('skin_image', { url: skin.url });
    if (data && $('#skincv')) skin3d($('#skincv'), data, skin.variant === 'SLIM');
  }
}

/* ---------------- settings ---------------- */
function pageSettings() {
  const s = S.settings, total = Math.max(2048, Math.min(S.sys.total_mb || 8192, 32768));
  return `<div class="head"><h1>Settings</h1></div><div class="panel" style="max-width:640px">
    <label class="f"><span>Theme</span><select id="st-theme"><option value="dark" ${s.theme !== 'light' ? 'selected' : ''}>Dark</option><option value="light" ${s.theme === 'light' ? 'selected' : ''}>Light</option></select></label>
    <label class="f"><span>Default memory for new instances: <b id="st-memv">${s.default_memory_mb || 4096}</b> MB</span><input type="range" id="st-mem" min="1024" max="${total}" step="256" value="${s.default_memory_mb || 4096}" data-input="st-mem"></label>
    <label class="f"><span>Parallel downloads</span><input type="number" id="st-conc" min="4" max="64" value="${s.concurrency || 24}"></label>
    <label class="f"><span>Global Java executable (optional)</span><input id="st-java" value="${esc(s.java_path || '')}"></label>
    <label class="chk"><input type="checkbox" id="st-snap" ${s.show_snapshots ? 'checked' : ''}> Show snapshots by default</label>
    <label class="f"><span>Microsoft client ID — optional override (official builds embed one; see README)</span><input id="st-ms" value="${esc(s.ms_client_id || '')}" placeholder="00000000-0000-0000-0000-000000000000"></label>
    <label class="f"><span>CurseForge API key (optional, enables CurseForge browsing and pack import)</span><input id="st-cf" type="password" value="${esc(s.curseforge_key || '')}"></label>
    <label class="f"><span>Launcher update source — GitHub repo as owner/repo (optional)</span><input id="st-repo" value="${esc(s.update_repo || '')}" placeholder="yourname/nimbus-launcher"></label>
    <div class="row"><button class="btn primary" data-act="savesettings">Save</button><button class="btn" data-act="checkself">Check for launcher update</button><button class="btn" data-act="opendata">Open data folder</button></div></div>${sharedPanel()}`;
}
const FOLDER_ICON = svg('<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>');
function sharedRow(kind, label) {
  const custom = kind === 'mods' ? S.settings.mods_custom : S.settings.packs_custom;
  return `<div class="item"><div class="grow"><b>${label}</b><p class="muted small">${custom ? 'Custom folder in use' : 'Default folder'}</p></div>
    ${custom ? '' : `<button class="btn sm icon" data-act="sharedopen" data-kind="${kind}" title="Open folder" aria-label="Open the ${label.toLowerCase()} folder">${FOLDER_ICON}</button>`}
    <button class="btn sm" data-act="sharedchange" data-kind="${kind}">Change…</button>
    ${custom ? `<button class="btn sm" data-act="sharedreset" data-kind="${kind}">Reset to default</button>` : ''}</div>`;
}
function sharedPanel() {
  return `<div class="panel" style="max-width:640px;margin-top:14px"><h2>Shared folders</h2>
    <p class="muted small" style="margin-bottom:14px">Every instance reads mods and resource packs from these two folders, so something you add once shows up everywhere.
    Mods only work with the Minecraft version and loader they were made for. If one instance should keep its own mods, switch sharing off in that instance’s Settings tab.</p>
    <div class="list">${sharedRow('mods', 'Mods')}${sharedRow('packs', 'Resource packs')}</div></div>`;
}
async function checkSelfUpdate(silent) {
  const repo = (S.settings.update_repo || '').trim();
  if (!repo) { if (!silent) toast('Set the update source (owner/repo) first', true); return; }
  const r = await call('launcher_update_check', { repo });
  if (r) { toast(`Version ${r.version} is available — opening the release page`); call('open_url', { url: r.url }); }
  else if (r === null && !silent) toast('You’re on the latest version');
}

/* ---------------- actions ---------------- */
async function instAction(cmd, id, label) {
  toast(label + '…');
  const r = await call(cmd, { id });
  if (r) { const i = S.instances.find(x => x.id === id); if (i) Object.assign(i, r); toast('Done'); }
}
const ACT = {
  'modal-bg': (el, e) => { if (e.target === el) closeModal(); },
  close: closeModal,
  nav: el => go(el.dataset.page),
  new: openNew,
  manage: el => { S.cur = el.dataset.id; S.tab = 'mods'; S.updates = null; S.mods = []; S.modInfo = {}; S.servers = []; S.files = {}; S.browse.hits = []; S.page = 'instance'; render(); loadMods(); },
  tab: el => { S.tab = el.dataset.tab; renderTab(); if (S.tab === 'mods') loadMods(); if (S.tab === 'files') loadFiles(); },
  gotoconsole: el => { closeModal(); S.cur = el.dataset.id; S.tab = 'console'; S.page = 'instance'; render(); },
  repairid: async el => { closeModal(); await instAction('instance_repair', el.dataset.id, 'Repairing files'); },
  async create() {
    const name = $('#n-name').value.trim(); if (!name) return toast('Please enter a name', true);
    const loader = $('#n-loader').value;
    const r = await call('instance_create', { spec: {
      name, mc_version: $('#n-ver').value, loader, loader_version: $('#n-lv').value || '',
      memory_mb: +$('#n-mem').value, perf_pack: loader !== 'vanilla' && $('#n-perf').checked } });
    if (r) { closeModal(); await reloadInstances(); render(); toast('Instance created — press Play to download everything'); }
  },
  async import() {
    const p = await pickFile('Modpack (.mrpack / CurseForge .zip)', ['mrpack', 'zip']); if (!p) return;
    toast('Importing modpack…');
    const r = await call('import_pack', { path: p });
    if (r) { await reloadInstances(); render(); toast('Imported ' + r.name); }
  },
  async play(el) {
    const id = el.dataset.id;
    if (S.running.has(id)) { await call('instance_kill', { id }); return; }
    if (S.starting.has(id)) return;
    const acc = curAcc();
    if (!acc) { toast('Add an account first', true); return go('accounts'); }
    S.starting.add(id); refreshPlay();
    const r = await call('instance_launch', { id, accountId: acc.id });
    S.starting.delete(id); if (r === undefined) hideBar(id);
    refreshPlay(); reloadInstances();
  },
  async saveinst() {
    const i = curInst(); if (!i) return;
    const r = await call('instance_save', { instance: { ...i, name: $('#is-name').value, memory_mb: +$('#is-mem').value,
      java_path: $('#is-java').value, jvm_args: $('#is-jvm').value, server: $('#is-server').value, wrapper: $('#is-wrap').value,
      fullscreen: $('#is-fs').checked, use_shared: $('#is-shared').checked, width: +$('#is-w').value || 0, height: +$('#is-h').value || 0 } });
    if (r) { Object.assign(i, r); toast('Saved'); render(); }
  },
  preinstall: () => instAction('instance_install', S.cur, 'Downloading files'),
  repair: () => instAction('instance_repair', S.cur, 'Verifying and repairing files'),
  async dupinst() {
    const r = await call('instance_duplicate', { id: S.cur });
    if (r) { await reloadInstances(); toast('Created “' + r.name + '”'); }
  },
  async exportinst() {
    const i = curInst();
    const p = await T.dialog.save({ defaultPath: i.name + '.mrpack', filters: [{ name: 'Modrinth pack', extensions: ['mrpack'] }] });
    if (!p) return;
    toast('Exporting…');
    if ((await call('instance_export', { id: S.cur, path: p })) !== undefined) toast('Exported');
  },
  openinst: async () => { const d = await call('data_dir'); if (d) call('open_path', { path: d + '/instances/' + S.cur + '/minecraft' }); },
  async delinst() {
    if (!confirm('Delete this instance and all its worlds? This cannot be undone.')) return;
    const r = await call('instance_delete', { id: S.cur });
    if (r !== undefined) { await reloadInstances(); go('library'); }
  },
  clearlog() { S.logs[S.cur] = []; renderTab(); },
  async analyze() {
    const r = await call('crash_analyze', { instanceId: S.cur });
    if (r) modal(`<h2>Log analysis</h2>${findingsHtml(r)}<div class="row end"><button class="btn primary" data-act="close">Close</button></div>`);
  },
  async sharelog() {
    if (!confirm('This uploads your latest game log to mclo.gs (a public log-sharing site) and gives you a link. Continue?')) return;
    const url = await call('log_share', { instanceId: S.cur });
    if (url) modal(`<h2>Log uploaded</h2><p>Share this link:</p><div class="code" style="font-size:14px;letter-spacing:0;word-break:break-all">${esc(url)}</div>
      <div class="row end"><button class="btn" data-act="openms" data-url="${esc(url)}">Open</button><button class="btn primary" data-act="close">Close</button></div>`);
  },
  async addjar() { const p = await pickFile('Mod', ['jar']); if (!p) return; if ((await call('mods_add_local', { instanceId: S.cur, path: p })) !== undefined) loadMods(); },
  openmods: async () => { const d = await call('data_dir'); if (d) call('open_path', { path: d + '/instances/' + S.cur + '/minecraft/mods' }); },
  async togglemod(el) { if ((await call('mods_toggle', { instanceId: S.cur, file: el.dataset.file, enabled: el.dataset.on === '1' })) !== undefined) loadMods(); },
  async delmod(el) { if (!confirm('Remove ' + el.dataset.file + '?')) return; if ((await call('mods_delete', { instanceId: S.cur, file: el.dataset.file })) !== undefined) loadMods(); },
  async checkupd(el) {
    el.disabled = true; el.textContent = 'Checking…';
    const r = await call('mods_check_updates', { instanceId: S.cur });
    S.updates = r || []; renderTab();
  },
  async applyupd(el) {
    el.disabled = true; el.textContent = 'Updating…';
    const n = await call('mods_apply_updates', { instanceId: S.cur });
    if (n !== undefined) { toast(`Updated ${n} mod${n === 1 ? '' : 's'}`); S.updates = null; loadMods(); } else renderTab();
  },
  async search() { const b = S.browse; b.q = $('#bq').value; b.source = $('#bsrc').value; b.kind = $('#bkind').value; b.offset = 0; await runSearch(false); },
  async more() { S.browse.offset += 20; await runSearch(true); },
  async install(el) {
    const b = S.browse, pid = el.dataset.pid;
    el.disabled = true; el.textContent = 'Installing…';
    let r;
    if (b.kind === 'mod') r = await call('mods_install', { instanceId: S.cur, projectId: pid, source: el.dataset.src });
    else if (b.kind === 'modpack') r = await call('modpack_install', { projectId: pid });
    else r = await call('content_install', { instanceId: S.cur, projectId: pid, kind: b.kind });
    if (r) {
      if (b.kind === 'modpack') { await reloadInstances(); toast('Modpack installed as “' + r.name + '” — find it in the Library'); }
      else toast(Array.isArray(r) ? `Installed ${r.length} file${r.length === 1 ? '' : 's'} (including dependencies)` : 'Installed ' + r);
      el.textContent = 'Installed ✓';
    } else { el.disabled = false; el.textContent = b.kind === 'modpack' ? 'Install as new instance' : 'Install'; }
  },
  async fadd(el) {
    const k = el.dataset.kind;
    const p = await pickFile(k === 'saves' ? 'World (.zip)' : 'Pack (.zip)', ['zip']); if (!p) return;
    if ((await call('folder_add', { instanceId: S.cur, kind: k, path: p })) !== undefined) { toast('Added'); loadFiles(); }
  },
  async fopen(el) { const d = await call('folder_path', { instanceId: S.cur, kind: el.dataset.kind }); if (d) call('open_path', { path: d }); },
  async fdel(el) {
    if (!confirm('Delete “' + el.dataset.name + '”? This cannot be undone.')) return;
    if ((await call('folder_delete', { instanceId: S.cur, kind: el.dataset.kind, name: el.dataset.name })) !== undefined) loadFiles();
  },
  async wbackup(el) {
    toast('Backing up…');
    const p = await call('world_backup', { instanceId: S.cur, name: el.dataset.name });
    if (p) toast('Backup saved: ' + p);
  },
  async offadd() {
    const name = $('#off-name').value.trim();
    let r = await call('account_add_offline', { username: name });
    if (!r) return;
    const added = [...r].reverse().find(a => a.kind === 'offline' && a.username === name);
    if (added && !added.selected) r = (await call('account_select', { id: added.id })) || r;
    S.accounts = r; render(); toast('Added ' + name + ' — now playing as ' + name);
  },
  quickstart: showQuickStart,
  acctmenu() {
    const old = $('#acct-menu'); if (old) { old.remove(); return; }
    const m = document.createElement('div'); m.id = 'acct-menu';
    m.innerHTML = S.accounts.map(a => `<button class="mi" data-act="accsel" data-id="${esc(a.id)}"><span class="acct-av sm">${esc(a.username[0].toUpperCase())}</span>
      <span>${esc(a.username)}<small>${a.kind === 'microsoft' ? 'Microsoft account' : 'Local account'}</small></span>${a.selected ? '<b>✓</b>' : ''}</button>`).join('')
      + '<button class="mi plus" data-act="acctmanage">Add or manage accounts</button>';
    document.body.appendChild(m);
  },
  async accsel(el) {
    const m = $('#acct-menu'); if (m) m.remove();
    const r = await call('account_select', { id: el.dataset.id });
    if (r) { S.accounts = r; renderAccountBox(); if (S.page === 'accounts') render(); toast('Playing as ' + (r.find(a => a.selected) || {}).username); }
  },
  acctmanage() { const m = $('#acct-menu'); if (m) m.remove(); go('accounts'); },
  msadd: msLogin,
  openms: el => call('open_url', { url: el.dataset.url }),
  async selacc(el) { const r = await call('account_select', { id: el.dataset.id }); if (r) { S.accounts = r; render(); } },
  async rmacc(el) { if (!confirm('Remove this account from the launcher?')) return; const r = await call('account_remove', { id: el.dataset.id }); if (r) { S.accounts = r; render(); } },
  async oskin() { const p = await pickFile('Skin', ['png']); if (p && (await call('offline_skin_set', { accountId: S.skinAcc, skinPath: p, capePath: null })) !== undefined) loadSkins(); },
  async 'oskin-rm'() { if ((await call('offline_skin_set', { accountId: S.skinAcc, skinPath: '', capePath: null })) !== undefined) loadSkins(); },
  async ocape() { const p = await pickFile('Cape', ['png']); if (p && (await call('offline_skin_set', { accountId: S.skinAcc, skinPath: null, capePath: p })) !== undefined) loadSkins(); },
  async 'ocape-rm'() { if ((await call('offline_skin_set', { accountId: S.skinAcc, skinPath: null, capePath: '' })) !== undefined) loadSkins(); },
  async pskin() {
    const p = await pickFile('Skin', ['png']); if (!p) return;
    if ((await call('skin_upload', { accountId: S.skinAcc, path: p, variant: $('#sk-var').value })) !== undefined) { toast('Skin updated'); loadSkins(); }
  },
  async cape(el) { if ((await call('cape_set', { accountId: S.skinAcc, capeId: el.dataset.id || null })) !== undefined) { toast('Cape updated'); loadSkins(); } },
  async savesettings() {
    const s = { ms_client_id: $('#st-ms').value.trim(), curseforge_key: $('#st-cf').value.trim(), default_memory_mb: +$('#st-mem').value,
      concurrency: +$('#st-conc').value || 24, show_snapshots: $('#st-snap').checked, java_path: $('#st-java').value.trim(),
      theme: $('#st-theme').value, update_repo: $('#st-repo').value.trim() };
    if ((await call('settings_save', { settings: s })) !== undefined) { S.settings = s; applyTheme(); toast('Settings saved'); }
  },
  checkself: () => checkSelfUpdate(false),
  sharedopen: el => call('shared_open', { kind: el.dataset.kind }),
  async sharedchange(el) {
    const p = await T.dialog.open({ directory: true, multiple: false });
    if (typeof p !== 'string') return;
    toast('Updating your instances…');
    if ((await call('shared_set', { kind: el.dataset.kind, path: p })) !== undefined) {
      S.settings = (await call('settings_get')) || S.settings; render(); toast('Folder changed');
    }
  },
  async sharedreset(el) {
    if ((await call('shared_reset', { kind: el.dataset.kind })) !== undefined) {
      S.settings = (await call('settings_get')) || S.settings; render(); toast('Back to the default folder');
    }
  },
  mscancel() { invoke('ms_login_cancel').catch(() => { }); closeModal(); },
  welcomems() { closeModal(); msLogin(); },
  async welcomeoff() {
    const r = await call('account_add_offline', { username: $('#w-name').value });
    if (r) { S.accounts = r; closeModal(); renderAccountBox(); afterLogin(); }
  },
  async quickplay() {
    const opt = $('#q-opt').checked;
    const v = await call('list_versions', { snapshots: false }); if (!v || !v.length) return;
    const ver = v[0].id;
    const r = await call('instance_create', { spec: { name: 'Minecraft ' + ver, mc_version: ver, loader: opt ? 'fabric' : 'vanilla',
      loader_version: '', memory_mb: autoMem(), perf_pack: opt } });
    if (!r) return;
    closeModal(); await reloadInstances(); S.page = 'library'; render();
    ACT.play({ dataset: { id: r.id } });
  },
  async dropas(el) {
    closeModal(); const k = el.dataset.kind;
    if (k === 'modpack') return importPath(S.dropPath);
    if ((await call('folder_add', { instanceId: S.cur, kind: k, path: S.dropPath })) !== undefined) { toast('Added'); if (S.tab === 'files') loadFiles(); }
  },
  async packtoggle(el) {
    if ((await call('pack_toggle', { instanceId: S.cur, name: el.dataset.name, enabled: el.dataset.on === '1' })) !== undefined) {
      S.enabledPacks = (await call('packs_enabled', { instanceId: S.cur })) || []; renderTab();
    }
  },
  cancel: () => { call('instance_cancel'); toast('Cancelling…'); },
  async retarget() {
    const i = curInst(); if (!i) return;
    if (!confirm('Switch this instance to the selected version/loader? Mods made for the old version may stop working.')) return;
    const r = await call('instance_retarget', { id: i.id, mcVersion: $('#r-ver').value, loader: $('#r-loader').value, loaderVersion: $('#r-lv').value || '' });
    if (r) { Object.assign(i, r); toast('Switched — press Play to download the new version'); render(); }
  },
  srvadd() { collectServers(); S.servers.push({ name: '', ip: '' }); renderTab(); },
  srvdel(el) { collectServers(); S.servers.splice(+el.dataset.i, 1); renderTab(); },
  async srvsave() {
    collectServers();
    if ((await call('servers_save', { instanceId: S.cur, servers: S.servers })) !== undefined) { toast('Server list saved'); loadFiles(); }
  },
  async srvplay(el) {
    collectServers();
    const i = curInst(), ip = (S.servers[+el.dataset.i] || {}).ip;
    if (!i || !ip) return toast('Enter the server address first', true);
    const r = await call('instance_save', { instance: { ...i, server: ip } });
    if (r) { Object.assign(i, r); toast('This instance will now join ' + ip + ' on launch'); }
  },
  opendata: async () => { const d = await call('data_dir'); if (d) call('open_path', { path: d }); }
};
const CHG = {
  'acc-select': async el => { const r = await call('account_select', { id: el.value }); if (r) { S.accounts = r; } },
  'skin-acc': el => { S.skinAcc = el.value; loadSkins(); },
  'n-snap': () => loadVersionList('n'),
  'n-loader': () => loadLoaderVersions('n'),
  'n-ver': () => loadLoaderVersions('n'),
  'r-snap': () => loadVersionList('r', $('#r-ver').value),
  'r-loader': () => loadLoaderVersions('r'),
  'r-ver': () => loadLoaderVersions('r')
};
const INP = {
  'n-mem': el => $('#n-memv').textContent = el.value,
  'is-mem': el => $('#is-memv').textContent = el.value,
  'st-mem': el => $('#st-memv').textContent = el.value,
  'lib-q': el => { S.libQ = el.value; const g = $('#grid'); if (g) g.innerHTML = libCards(); refreshPlay(); for (const p of Object.values(S.prog)) onProgress(p); }
};
document.addEventListener('click', e => {
  if (!e.target.closest('#acct-menu') && !e.target.closest('[data-act="acctmenu"]')) { const m = $('#acct-menu'); if (m) m.remove(); }
  const el = e.target.closest('[data-act]'); if (!el) return;
  const fn = ACT[el.dataset.act]; if (fn) fn(el, e);
});
document.addEventListener('change', e => { const k = e.target.dataset && e.target.dataset.change; if (k && CHG[k]) CHG[k](e.target, e); });
document.addEventListener('input', e => { const k = e.target.dataset && e.target.dataset.input; if (k && INP[k]) INP[k](e.target); });
document.addEventListener('keydown', e => { const k = e.target.dataset && e.target.dataset.enter; if (e.key === 'Enter' && k && ACT[k]) ACT[k](e.target, e); });

/* ---------------- boot ---------------- */
(async function boot() {
  S.settings = (await call('settings_get')) || {};
  applyTheme();
  S.accounts = (await call('accounts_list')) || [];
  S.instances = (await call('instances_list')) || [];
  S.sys = (await call('system_info')) || S.sys;
  S.running = new Set((await call('instance_running')) || []);
  await listen('progress', e => onProgress(e.payload));
  await listen('game-log', e => onLog(e.payload));
  await listen('game-state', e => onState(e.payload));
  await listen('game-crash', e => onCrash(e.payload));
  await listen('notice', e => toast(String(e.payload)));
  await listen('tauri://drag-drop', e => onDrop((e.payload && e.payload.paths) || []));
  S.msOk = await invoke('ms_available').catch(() => false);
  render();
  if (!S.accounts.length) showWelcome(); else if (!S.instances.length) showQuickStart();
  checkSelfUpdate(true);
})();
