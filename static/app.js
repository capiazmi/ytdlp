// YouTube / yt-dlp page. Shared helpers and the jobs list live in common.js.
const state = { info: null, url: null, transcript: '' };

const fmtDur = (s) => {
  if (!s) return '';
  s = Math.round(s);
  const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, sec = s % 60;
  return (h ? `${h}:${String(m).padStart(2, '0')}` : m) + ':' + String(sec).padStart(2, '0');
};

// ---------- status ----------
statusReady.then((s) => {
  if ('ytdlp_version' in s) $('#version').textContent = s.ytdlp_version ? `yt-dlp ${s.ytdlp_version}` : 'yt-dlp not found';
});

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

// Share target (/share?url=…&text=…) and deep links (/?url=…).
(() => {
  const q = new URLSearchParams(location.search);
  const found = extractUrl(q.get('url')) || extractUrl(q.get('text')) || extractUrl(q.get('title'));
  // Shared Scribd links go to the Scribd page.
  if (found && isScribd(found)) {
    location.replace('/scribd?url=' + encodeURIComponent(found));
    return;
  }
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

// ---------- jobs ----------
document.querySelectorAll('[data-preset]').forEach((b) =>
  b.addEventListener('click', () => startJob({ preset: b.dataset.preset })));

async function startJob(opts) {
  try {
    upsertJob(await postJson('/api/jobs', { url: state.url, title: state.info?.title, ...opts }));
  } catch (err) { alert(err.message); }
}

// ---------- shortcuts ----------
$('#shortcut-url').textContent = `${location.origin}/share?url=`;
$('#bookmarklet').href = `javascript:location.href='${location.origin}/share?url='+encodeURIComponent(location.href)`;
