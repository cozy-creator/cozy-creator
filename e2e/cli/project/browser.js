// The project cell's browser (e2e/cli/project.sh). A page on Calcifer's own origin trades a code for {token, media} and
// keeps them in localStorage. On those alone it reads the project, seeks a <video> of edit.mp4 (?media=, Range) and follows
// the project's event stream through a re-cut, then goes on across a Calcifer restart with no new link. A run's page and
// drawing, opened directly, run no script. A page on another site (localhost:<port>) and on another port of this host
// (127.0.0.1:<port>) then try a fetch, a no-cors POST, an <img> GET and a form POST at Calcifer: they hold no credential,
// each answered is refused (401/403), and the fetch is unreadable (the browser's CORS fails it when Calcifer sends no CORS header).
//   node browser.js <calcifer address> <code> <project id> <hostile run> <other port> <re-cut YAML file> <restart command>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264).
const { chromium } = require(process.env.PLAYWRIGHT);
const { execSync } = require('child_process');
const { readFileSync } = require('fs');
const [addr, code, id, hostile, otherPort, recut, restart] = process.argv.slice(2);
const base = `http://${addr}`, others = [`http://localhost:${otherPort}`, `http://127.0.0.1:${otherPort}`];
// Polled from here: Playwright's waitForFunction evals in the page, and the page's CSP refuses eval.
const until = async (page, ask, ms = 120000) => {
  for (const end = Date.now() + ms; Date.now() < end; await new Promise((ok) => setTimeout(ok, 200))) {
    if (await page.evaluate(ask)) return;
  }
  throw new Error(`timed out waiting for ${ask}`);
};
(async () => {
  const out = {}, seen = new Map();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext();
  context.setDefaultTimeout(60000);
  // Every request at Calcifer, as sent (its headers) and as it ended: answered (status), or failed in the browser.
  context.on('request', (q) => {
    if (!q.url().startsWith(base)) return;
    const s = { method: q.method(), path: q.url().slice(base.length), status: null, failed: null };
    seen.set(q, s);
    q.allHeaders().then((h) => Object.assign(s, { range: h['range'] || null, site: h['sec-fetch-site'] || null })).catch(() => {});
  });
  context.on('requestfinished', async (q) => {
    const s = seen.get(q), r = s && await q.response().catch(() => null);
    if (!r) return;
    s.status = r.status(); s.content_range = r.headers()['content-range'] || null;
    if (s.status >= 400) s.body = (await r.text().catch(() => '')).slice(0, 200);
  });
  context.on('requestfailed', (q) => { const s = seen.get(q); if (s) s.failed = (q.failure() || {}).errorText || 'failed'; });
  const page = await context.newPage();
  // No fragment: this build's app, if it has one, mustn't spend the code. Without one, / is a 503 of the same origin.
  await page.goto(`${base}/`);
  out.session = await page.evaluate(async (code) => {
    const post = () => fetch('/v1/session', { method: 'POST', body: JSON.stringify({ code }) });
    const first = await post(), body = await first.json(), again = await post();
    localStorage.setItem('e2e-token', body.token); localStorage.setItem('e2e-media', body.media);
    return { status: first.status, token: !!body.token, media: !!body.media, again: again.status, again_code: (await again.json()).error.code };
  }, code);
  const media = await page.evaluate(() => localStorage.getItem('e2e-media'));
  out.cookies = (await context.cookies()).map((c) => c.name);
  out.read = await page.evaluate(async (id) => {
    const r = await fetch(`/v1/projects/${id}`, { headers: { Authorization: 'Bearer ' + localStorage.getItem('e2e-token') } });
    return { status: r.status, cuts: r.ok ? (await r.json()).cuts : null };
  }, id);
  out.video = await page.evaluate(async ({ id, media }) => {
    const v = document.createElement('video'); v.muted = true; v.preload = 'auto';
    v.src = `/v1/projects/${id}/cuts/edit.mp4?media=${media}`; document.body.appendChild(v);
    await new Promise((ok, no) => { v.onloadedmetadata = ok; v.onerror = () => no(new Error('video error ' + (v.error && v.error.code))); });
    const duration = v.duration; v.currentTime = duration * 0.75;
    await new Promise((ok) => { v.onseeked = ok; });
    return { duration, seeked_to: v.currentTime, ready_state: v.readyState };
  }, { id, media });
  out.media_elsewhere = await page.evaluate(async ({ id, media }) => {
    const status = async (path, init) => (await fetch(`${path}?media=${media}`, init)).status;
    return { machines: await status('/v1/machines'), project: await status(`/v1/projects/${id}`), post: await status(`/v1/projects/${id}/runs`, { method: 'POST', body: '{}' }) };
  }, { id, media });
  // `GET /v1/events` is open before the re-cut is asked: the project's board as it goes, its cut a job whose file is the film at its
  // root, until the board says it ended.
  await page.evaluate((id) => new Promise((ok, no) => {
    window.got = [];
    const headers = { Authorization: 'Bearer ' + localStorage.getItem('e2e-token') };
    fetch('/v1/events', { headers }).then(async (r) => {
      if (!r.ok) throw new Error(`events: ${r.status}`);
      const lines = r.body.pipeThrough(new TextDecoderStream()).getReader();
      let held = '';
      for (let read = await lines.read(); !read.done; read = await lines.read()) {
        held += read.value;
        const blocks = held.split('\n\n'); held = blocks.pop();
        for (const block of blocks) {
          const data = block.split('\n').filter((line) => line.startsWith('data:')).map((line) => line.slice(5)).join('\n');
          if (!data) continue;
          const m = JSON.parse(data);
          if (m.type === 'hello') {
            const shown = await fetch(`/v1/events/${encodeURIComponent(m.stream)}`, {
              method: 'PUT', headers: { ...headers, 'Content-Type': 'application/json' }, body: JSON.stringify({ projects: [id] }),
            });
            if (!shown.ok || !(await shown.json()).held.includes(id)) throw new Error(`project stream interest: ${shown.status}`);
            window.got.push('open'); ok();
          }
          if (m.project !== id) continue;
          // A board with no jobs yet (a walk just begun) leaves `jobs` out.
          const jobs = m.type === 'board.updated' ? m.board.jobs ?? [] : m.type === 'job.updated' ? [m.job] : [];
          for (const job of jobs) {
            const cut = job.state === 'done' && job.path && !job.path.startsWith('files/') && `cut ${job.path}`;
            if (cut && !window.got.includes(cut)) window.got.push(cut);
          }
          window.got.push(m.type);
          if (m.type === 'board.updated' && m.board.ended) window.got.push('ended');
        }
      }
      window.got.push('closed');
    }).catch((e) => { window.got.push('error'); no(e); });
  }), id);
  await page.evaluate(async ({ id, yaml }) => {
    const project = `/v1/projects/${encodeURIComponent(id)}`;
    const auth = { Authorization: 'Bearer ' + localStorage.getItem('e2e-token') };
    const edit = await fetch(`${project}/yaml`, { method: 'PUT', headers: { ...auth, 'Content-Type': 'application/yaml' }, body: yaml });
    if (!edit.ok) throw new Error(`re-cut YAML: ${edit.status} ${await edit.text()}`);
    const ask = await fetch(`${project}/runs`, { method: 'POST', headers: { ...auth, 'Content-Type': 'application/json', 'Idempotency-Key': crypto.randomUUID() }, body: JSON.stringify({ names: ['all'], on: ['local'] }) });
    if (!ask.ok) throw new Error(`re-cut render: ${ask.status} ${await ask.text()}`);
  }, { id, yaml: readFileSync(recut, 'utf8') });
  await until(page, () => ['ended', 'error', 'closed'].some((end) => window.got.includes(end)));
  out.events = await page.evaluate(() => window.got);
  execSync(restart, { stdio: ['ignore', process.stderr, process.stderr], shell: '/bin/bash' });
  out.after_restart = await page.evaluate(async (id) => {
    const r = await fetch(`/v1/projects/${id}`, { headers: { Authorization: 'Bearer ' + localStorage.getItem('e2e-token') } });
    return r.status;
  }, id);
  // Opened directly, as a link in a page would: a script that ran would be seen asking for /v1/runs.
  out.hostile = {};
  for (const output of ['page', 'drawing']) {
    const tab = await context.newPage(), asked = [], o = { download: false };
    tab.on('download', () => { o.download = true; });
    tab.on('request', (q) => asked.push(q.url()));
    const answer = await tab.goto(`${base}/v1/runs/${hostile}/outputs/${output}?media=${media}`).catch(() => null);
    o.status = answer && answer.status();
    await tab.waitForTimeout(500);
    o.reached = asked.filter((url) => url.includes('/v1/') && !url.includes('/outputs/'));
    out.hostile[output] = o;
    await tab.close();
  }
  out.other_pages = [];
  for (const origin of others) {
    const other = await context.newPage(), asked = [], settling = [];
    other.on('request', (q) => {
      if (!q.url().startsWith(base)) return;
      const s = { method: q.method(), path: q.url().slice(base.length), status: null, credential: null };
      asked.push(s);
      settling.push(q.allHeaders().then((h) => { s.credential = !!h['authorization'] || !!h['cookie']; }).catch(() => {}));
      settling.push(q.response().then((r) => { s.status = r && r.status(); }).catch(() => {}));
    });
    await other.goto(`${origin}/cross-site.html?target=${encodeURIComponent(base)}&id=${id}`);
    await until(other, () => window.done === true);
    await Promise.all(settling);
    out.other_pages.push({ origin, asked, ...await other.evaluate(() => ({ results: window.results, kept: localStorage.length })) });
    await other.close();
  }
  out.ranges = [...seen.values()].filter((s) => s.path.includes('/cuts/edit.mp4') && s.site === 'same-origin')
    .map((s) => ({ status: s.status, range: s.range, content_range: s.content_range }));
  console.log(JSON.stringify(out, null, 2));
  await browser.close();
})().catch((e) => { console.error(e); process.exit(1); });
