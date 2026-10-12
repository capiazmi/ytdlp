const $ = (s) => document.querySelector(s);
const state = { info: null, url: null, transcript: '', jobs: new Map() };

const fmtBytes = (n) => {
  if (!n) return '';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
};
const fmtDur = (s) => {
  if (!s) return '';
  s = Math.round(s);
  const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, sec = s % 60;
  return (h ? `${h}:${String(m).padStart(2, '0')}` : m) + ':' + String(sec).padStart(2, '0');
};
const el = (tag, props = {}, ...children) => {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children.filter((c) => c != null));
  return e;
};

async function api(path, opts) {
  const r = await fetch(path, opts);
  if (r.status === 401) { location.href = '/login'; throw new Error('login required'); }
  if (!r.ok) {
    let msg = r.statusText;
    try { msg = (await r.json()).error || msg; } catch {}
    throw new Error(msg);
  }
  return r;
}

// ---------- status ----------
api('/api/status').then((r) => r.json()).then((s) => {
  $('#version').textContent = s.ytdlp_version ? `yt-dlp ${s.ytdlp_version}` : 'yt-dlp not found';
  $('#logout').hidden = !s.auth;
  $('#ttl-note').textContent = `Downloaded files are deleted after ${s.file_ttl_hours}h.`;
}).catch(() => {});

// ---------- fetch info ----------
$('#url-form').addEventListener('submit', (e) => { e.preventDefault(); fetchInfo(); });

async function fetchInfo() {
  const url = $('#url').value.trim();
  if (!url) return;
  const btn = $('#fetch-btn');
  btn.disabled = true; btn.textContent = 'Fetching…';
  $('#url-error').hidden = true;
  try {
    const info = await (await api('/api/info?url=' + encodeURIComponent(url))).json();
    state.info = info; state.url = url;
    renderInfo(info);
  } catch (err) {
    $('#url-error').textContent = err.message; $('#url-error').hidden = false;
  } finally {
    btn.disabled = false; btn.textContent = 'Fetch';
  }
}

// Paste button (clipboard read needs a user gesture; not available everywhere).
if (navigator.clipboard?.readText) {
  $('#paste-btn').hidden = false;
  $('#paste-btn').addEventListener('click', async () => {
    try {
      const text = await navigator.clipboard.readText();
      const found = extractUrl(text);
      if (found) { $('#url').value = found; fetchInfo(); }
    } catch {}
  });
}

function extractUrl(text) {
  const m = (text || '').match(/https?:\/\/[^\s<>"']+/);
  return m ? m[0] : null;
}

// Share target (/share?url=…&text=…) and deep links (/?url=…).
(() => {
  const q = new URLSearchParams(location.search);
  const found = extractUrl(q.get('url')) || extractUrl(q.get('text')) || extractUrl(q.get('title'));
  if (found) {
    history.replaceState(null, '', '/');
    $('#url').value = found;
    fetchInfo();
  }
})();

function renderInfo(info) {
  $('#video').hidden = false;
  $('#thumb').src = info.thumbnail || '';
  $('#title').textContent = info.title;
  $('#meta').textContent = [info.uploader, fmtDur(info.duration)].filter(Boolean).join(' · ');
  $('#transcript').hidden = true; $('#copy-transcript').disabled = true; $('#share-transcript').disabled = true;

  const tbody = $('#formats');
  tbody.replaceChildren(...info.formats.map((f) => {
    const btn = el('button', { className: 'small secondary', textContent: 'Get' });
    btn.onclick = () => startJob({ preset: 'custom', format_id: f.id, format_kind: f.kind });
    return el('tr', {},
      el('td', { textContent: f.id }),
      el('td', { textContent: f.kind }),
      el('td', { textContent: f.height ? `${f.height}p${f.fps > 30 ? Math.round(f.fps) : ''}` : (f.note || '') }),
      el('td', { textContent: f.ext }),
      el('td', { textContent: fmtBytes(f.filesize) }),
      el('td', {}, btn));
  }));

  const c = info.captions;
  $('#caption-summary').textContent =
    `${c.manual.length} manual · ${c.auto.length} auto-generated · ${c.translated.length} auto-translated`;
  renderLangs();
}

function renderLangs() {
  const c = state.info.captions;
  const sel = $('#lang');
  const group = (label, list, kind) => list.length
    ? el('optgroup', { label }, ...list.map((t) => el('option', { value: `${kind}:${t.code}`, textContent: `${t.name} (${t.code})` })))
    : null;
  const groups = [group('Manual', c.manual, 'manual'), group('Auto-generated', c.auto, 'auto')];
  if ($('#show-translated').checked) groups.push(group('Auto-translated', c.translated, 'auto'));
  sel.replaceChildren(...groups.filter(Boolean));
  const none = !sel.options.length;
  if (none) sel.append(el('option', { textContent: 'No captions available', disabled: true }));
  document.querySelectorAll('#show-transcript, [data-export]').forEach((b) => (b.disabled = none));
}
$('#show-translated').addEventListener('change', renderLangs);

// ---------- transcript ----------
function transcriptUrl(format, download) {
  const [kind, lang] = $('#lang').value.split(/:(.*)/s);
  const p = new URLSearchParams({ url: state.url, lang, kind, format, timestamps: $('#timestamps').checked });
  if (download) p.set('download', 'true');
  return '/api/transcript?' + p;
}

$('#show-transcript').addEventListener('click', async () => {
  const btn = $('#show-transcript'), pre = $('#transcript');
  btn.disabled = true; btn.textContent = 'Loading…';
  try {
    state.transcript = await (await api(transcriptUrl('txt', false))).text();
    pre.textContent = state.transcript || '(empty)';
    pre.classList.remove('error');
    $('#copy-transcript').disabled = $('#share-transcript').disabled = !state.transcript;
    pre.hidden = false;
    pre.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  } catch (err) {
    pre.textContent = err.message; pre.classList.add('error'); pre.hidden = false;
  } finally {
    btn.disabled = false; btn.textContent = 'View';
  }
});
// Re-render the open transcript when the timestamps toggle changes.
$('#timestamps').addEventListener('change', () => { if (!$('#transcript').hidden) $('#show-transcript').click(); });

$('#copy-transcript').addEventListener('click', async () => {
  await navigator.clipboard.writeText(state.transcript);
  flash($('#copy-transcript'), 'Copied!');
});

if (navigator.share) {
  $('#share-transcript').hidden = false;
  $('#share-transcript').addEventListener('click', () =>
    navigator.share({ title: state.info.title, text: state.transcript }).catch(() => {}));
}

document.querySelectorAll('[data-export]').forEach((b) => b.addEventListener('click', async () => {
  // Fetch first so errors show inline instead of navigating to a JSON error page.
  const label = b.textContent;
  b.disabled = true; b.textContent = '…';
  try {
    const r = await api(transcriptUrl(b.dataset.export, true));
    const name = (r.headers.get('content-disposition') || '').match(/filename="([^"]+)"/)?.[1] || 'transcript';
    saveBlob(await r.blob(), name);
  } catch (err) {
    alert(err.message);
  } finally {
    b.disabled = false; b.textContent = label;
  }
}));

function saveBlob(blob, name) {
  const a = el('a', { href: URL.createObjectURL(blob), download: name });
  document.body.append(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 60_000);
}

function flash(btn, text) {
  const old = btn.textContent; btn.textContent = text;
  setTimeout(() => (btn.textContent = old), 1500);
}

// ---------- jobs ----------
document.querySelectorAll('[data-preset]').forEach((b) =>
  b.addEventListener('click', () => startJob({ preset: b.dataset.preset })));

async function startJob(opts) {
  try {
    const job = await (await api('/api/jobs', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url: state.url, title: state.info?.title, ...opts }),
    })).json();
    upsertJob(job);
  } catch (err) { alert(err.message); }
}

function upsertJob(job) { state.jobs.set(job.id, job); renderJobs(); }

function renderJobs() {
  const list = [...state.jobs.values()].sort((a, b) => b.created_at - a.created_at);
  const ul = $('#jobs');
  if (!list.length) { ul.replaceChildren(el('li', { className: 'muted', textContent: 'No jobs yet.' })); return; }
  ul.replaceChildren(...list.map((j) => {
    const detail = j.status === 'running'
      ? [`${j.progress.toFixed(1)}%`, j.stage, j.speed, j.eta && `ETA ${j.eta}`].filter(Boolean).join(' · ')
      : j.status === 'done' ? [j.filename, fmtBytes(j.filesize)].filter(Boolean).join(' · ')
      : j.status === 'failed' ? j.error : 'waiting for a free slot…';
    const actions = el('div', { className: 'actions' });
    if (j.status === 'done') actions.append(el('a', { href: `/files/${j.id}`, className: 'button small', textContent: 'Save file' }));
    if (j.status !== 'running' && j.status !== 'queued') {
      const del = el('button', { className: 'small secondary', textContent: 'Remove' });
      del.onclick = async () => { await api(`/api/jobs/${j.id}`, { method: 'DELETE' }).catch(() => {}); state.jobs.delete(j.id); renderJobs(); };
      actions.append(del);
    }
    return el('li', { className: `job ${j.status}` },
      el('div', { className: 'job-head' },
        el('strong', { textContent: j.title }),
        el('span', { className: `badge ${j.status}`, textContent: `${j.status} · ${j.preset}` })),
      el('progress', { max: 100, value: j.progress }),
      el('div', { className: 'job-foot' }, el('span', { className: 'muted small', textContent: detail || '' }), actions));
  }));
}


function loadJobs() {
  return api('/api/jobs').then((r) => r.json()).then((jobs) => {
    state.jobs = new Map(jobs.map((j) => [j.id, j]));
    renderJobs();
  }).catch(() => {});
}

// Mobile browsers drop the SSE stream in the background; reconnect and resync on return.
let es;
function connectEvents() {
  es?.close();
  es = new EventSource('/api/events');
  es.addEventListener('job', (e) => upsertJob(JSON.parse(e.data)));
}
connectEvents();
loadJobs();
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') { loadJobs(); if (es.readyState === EventSource.CLOSED) connectEvents(); }
});

// ---------- PWA ----------
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js').catch(() => {});

$('#shortcut-url').textContent = `${location.origin}/share?url=`;
$('#bookmarklet').href = `javascript:location.href='${location.origin}/share?url='+encodeURIComponent(location.href)`;
