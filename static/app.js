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
$('#url-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const url = $('#url').value.trim();
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
});

function renderInfo(info) {
  $('#video').hidden = false;
  $('#thumb').src = info.thumbnail || '';
  $('#title').textContent = info.title;
  $('#meta').textContent = [info.uploader, fmtDur(info.duration)].filter(Boolean).join(' · ');
  $('#transcript').hidden = true; $('#copy-transcript').disabled = true;

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
  for (const id of ['#show-transcript', '#download-transcript']) $(id).disabled = none;
}
$('#show-translated').addEventListener('change', renderLangs);

// ---------- transcript ----------
function transcriptUrl(download) {
  const [kind, lang] = $('#lang').value.split(/:(.*)/s);
  const p = new URLSearchParams({ url: state.url, lang, kind, format: $('#tformat').value, id: state.info.id });
  if (download) p.set('download', 'true');
  return '/api/transcript?' + p;
}

$('#show-transcript').addEventListener('click', async () => {
  const btn = $('#show-transcript'), pre = $('#transcript');
  btn.disabled = true; btn.textContent = 'Loading…';
  try {
    state.transcript = await (await api(transcriptUrl(false))).text();
    pre.textContent = state.transcript || '(empty)';
    pre.classList.remove('error');
    $('#copy-transcript').disabled = !state.transcript;
  } catch (err) {
    pre.textContent = err.message; pre.classList.add('error');
  } finally {
    pre.hidden = false; btn.disabled = false; btn.textContent = 'Show';
  }
});

$('#copy-transcript').addEventListener('click', async () => {
  await navigator.clipboard.writeText(state.transcript);
  const b = $('#copy-transcript'); b.textContent = 'Copied!';
  setTimeout(() => (b.textContent = 'Copy'), 1500);
});

$('#download-transcript').addEventListener('click', () => { location.href = transcriptUrl(true); });

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

api('/api/jobs').then((r) => r.json()).then((jobs) => jobs.forEach((j) => state.jobs.set(j.id, j))).then(renderJobs).catch(() => {});

const es = new EventSource('/api/events');
es.addEventListener('job', (e) => upsertJob(JSON.parse(e.data)));
