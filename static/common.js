// Shared by every page: helpers, status line, the jobs list and live updates.
// Each page sets <body data-kind="video|scribd"> and only lists its own jobs.
const $ = (s) => document.querySelector(s);
const PAGE_KIND = document.body.dataset.kind;
const jobs = new Map();

const fmtBytes = (n) => {
  if (!n) return '';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
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

const postJson = (path, body) => api(path, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
}).then((r) => r.json());

function extractUrl(text) {
  const m = (text || '').match(/https?:\/\/[^\s<>"']+/);
  return m ? m[0] : null;
}

const isScribd = (url) => {
  try { const h = new URL(url).hostname.toLowerCase(); return h === 'scribd.com' || h.endsWith('.scribd.com'); }
  catch { return false; }
};

function saveBlob(blob, name) {
  const a = el('a', { href: URL.createObjectURL(blob), download: name });
  document.body.append(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 60_000);
}

function flash(btn, text) {
  const old = btn.textContent; btn.textContent = text;
  setTimeout(() => (btn.textContent = old), 1500);
}

// ---------- status ----------
const statusReady = api('/api/status').then((r) => r.json()).then((s) => {
  $('#logout').hidden = !s.auth;
  $('#ttl-note').textContent = `Downloaded files are deleted after ${s.file_ttl_hours}h.`;
  return s;
}).catch(() => ({}));

// ---------- jobs ----------
function upsertJob(job) {
  if ((job.kind || 'video') !== PAGE_KIND) return;
  jobs.set(job.id, job);
  renderJobs();
}

function renderJobs() {
  const list = [...jobs.values()].sort((a, b) => b.created_at - a.created_at);
  const ul = $('#jobs');
  if (!list.length) { ul.replaceChildren(el('li', { className: 'muted', textContent: 'No jobs yet.' })); return; }
  ul.replaceChildren(...list.map((j) => {
    const files = j.files || [];
    const detail = j.status === 'running'
      ? [`${j.progress.toFixed(1)}%`, j.stage, j.speed, j.eta && `ETA ${j.eta}`].filter(Boolean).join(' · ')
      : j.status === 'done' ? files.map((f) => [f.name, fmtBytes(f.size)].filter(Boolean).join(' · ')).join('\n')
      : j.status === 'failed' ? j.error : 'waiting for a free slot…';
    const actions = el('div', { className: 'actions' });
    if (j.status === 'done') {
      files.forEach((f, n) => {
        const label = files.length > 1 ? `Save ${(f.name.split('.').pop() || '').toUpperCase()}` : 'Save file';
        actions.append(el('a', { href: `/files/${j.id}/${n}`, className: 'button small', textContent: label }));
      });
    }
    if (j.status !== 'running' && j.status !== 'queued') {
      const del = el('button', { className: 'small secondary', textContent: 'Remove' });
      del.onclick = async () => { await api(`/api/jobs/${j.id}`, { method: 'DELETE' }).catch(() => {}); jobs.delete(j.id); renderJobs(); };
      actions.append(del);
    }
    return el('li', { className: `job ${j.status}` },
      el('div', { className: 'job-head' },
        el('strong', { textContent: j.title }),
        el('span', { className: `badge ${j.status}`, textContent: `${j.status} · ${j.preset}` })),
      el('progress', { max: 100, value: j.progress }),
      el('div', { className: 'job-foot' }, el('span', { className: 'muted small pre-line', textContent: detail || '' }), actions));
  }));
}

function loadJobs() {
  return api('/api/jobs').then((r) => r.json()).then((list) => {
    jobs.clear();
    list.filter((j) => (j.kind || 'video') === PAGE_KIND).forEach((j) => jobs.set(j.id, j));
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
