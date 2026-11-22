// Scribd page. Shared helpers and the jobs list live in common.js.
const links = (text) => (text || '').split(/\s+/).filter((t) => /(^|[/.])scribd\.com\//i.test(t));
const formats = () => [...document.querySelectorAll('input[name=fmt]:checked')].map((c) => c.value);

statusReady.then((s) => {
  if (s.scribd === false) {
    $('#unavailable').textContent = `Scribd downloader is not available on this server: ${s.scribd_error}`;
    $('#unavailable').hidden = false;
    $('#go-btn').disabled = true;
  }
});

// Grow the box when several links are pasted; Enter submits.
const box = $('#url');
const fit = () => { box.style.height = 'auto'; box.style.height = `${Math.min(box.scrollHeight + 2, 240)}px`; };
box.addEventListener('input', fit);
box.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); $('#scribd-form').requestSubmit(); }
});

$('#scribd-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const err = $('#url-error'), btn = $('#go-btn');
  err.hidden = true;
  const urls = [...new Set(links(box.value))];
  const fmts = formats();
  if (!urls.length) { err.textContent = 'No Scribd link found. Expected something like scribd.com/document/123456/Title'; err.hidden = false; return; }
  if (!fmts.length) { err.textContent = 'Pick at least one format.'; err.hidden = false; return; }

  btn.disabled = true;
  const failed = [];
  for (const url of urls) {
    try { upsertJob(await postJson('/api/scribd/jobs', { url, formats: fmts })); }
    catch (ex) { failed.push(`${url}: ${ex.message}`); }
  }
  btn.disabled = false;
  if (failed.length) { err.textContent = failed.join('\n'); err.hidden = false; box.value = failed.map((f) => f.split(': ')[0]).join('\n'); }
  else box.value = '';
  fit();
});

if (navigator.clipboard?.readText) {
  $('#paste-btn').hidden = false;
  $('#paste-btn').addEventListener('click', async () => {
    try {
      const found = links(await navigator.clipboard.readText());
      if (found.length) { box.value = found.join('\n'); fit(); }
    } catch {}
  });
}

// Deep link / share target: /scribd?url=… starts right away with the chosen formats.
(() => {
  const q = new URLSearchParams(location.search);
  const found = links([q.get('url'), q.get('text'), q.get('title')].join(' '));
  if (found.length) {
    history.replaceState(null, '', '/scribd');
    box.value = found.join('\n'); fit();
    statusReady.then((s) => { if (s.scribd !== false) $('#scribd-form').requestSubmit(); });
  }
})();
