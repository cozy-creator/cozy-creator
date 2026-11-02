// The project cell's browser (e2e/cli/project.sh): a page on Calcifer's own origin trades the token in its fragment
// for the session cookie, then on the cookie alone reads the project, seeks a <video> of edit.mp4 (Range) and follows
// the project's EventSource through a re-cut. A page on another site (localhost:<port>) then tries a credentialed
// fetch, a no-cors POST, an <img> GET and a form POST at Calcifer: none carries the cookie, each answered is refused
// (401/403), and the fetch is unreadable (the browser's CORS fails it when Calcifer sends no CORS header).
//   node browser.js <calcifer address> <token> <project id> <cross-site port> <re-cut command>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264).
const { chromium } = require(process.env.PLAYWRIGHT);
const { execSync } = require('child_process');
const [addr, token, id, crossPort, recut] = process.argv.slice(2);
const base = `http://${addr}`, crossOrigin = `http://localhost:${crossPort}`;
(async () => {
  const out = {}, seen = new Map();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext();
  // Every request at Calcifer, as sent (its headers) and as it ended: answered (status), or failed in the browser.
  context.on('request', (q) => {
    if (!q.url().startsWith(base)) return;
    const s = { method: q.method(), path: q.url().slice(base.length), status: null, failed: null };
    seen.set(q, s);
    q.allHeaders().then((h) => Object.assign(s, { range: h['range'] || null, origin: h['origin'] || null,
      site: h['sec-fetch-site'] || null, cookie: /cozy_session=/.test(h['cookie'] || '') })).catch(() => {});
  });
  context.on('requestfinished', async (q) => {
    const s = seen.get(q), r = s && await q.response().catch(() => null);
    if (!r) return;
    s.status = r.status(); s.content_range = r.headers()['content-range'] || null;
    if (s.status >= 400) s.body = (await r.text().catch(() => '')).slice(0, 200);
  });
  context.on('requestfailed', (q) => { const s = seen.get(q); if (s) s.failed = (q.failure() || {}).errorText || 'failed'; });
  const page = await context.newPage();
  await page.goto(`${base}/#token=${token}`);
  out.session = await page.evaluate(async () => {
    const r = await fetch('/v1/session', { method: 'POST', headers: { Authorization: 'Bearer ' + location.hash.slice(7) } });
    return r.status;
  });
  out.cookie = (await context.cookies()).map((c) => ({ name: c.name, httpOnly: c.httpOnly, sameSite: c.sameSite, path: c.path }));
  out.read_on_cookie = await page.evaluate(async (id) => {
    const r = await fetch(`/v1/projects/${id}`);
    return { status: r.status, cuts: r.ok ? (await r.json()).cuts : null };
  }, id);
  out.video = await page.evaluate(async (id) => {
    const v = document.createElement('video'); v.muted = true; v.preload = 'auto';
    v.src = `/v1/projects/${id}/cuts/edit.mp4`; document.body.appendChild(v);
    await new Promise((ok, no) => { v.onloadedmetadata = ok; v.onerror = () => no(new Error('video error ' + (v.error && v.error.code))); });
    const duration = v.duration; v.currentTime = duration * 0.75;
    await new Promise((ok) => { v.onseeked = ok; });
    return { duration, seeked_to: v.currentTime, ready_state: v.readyState };
  }, id);
  // The stream is open before the re-cut is asked; it ends with the walk's `ended`.
  await page.evaluate((id) => new Promise((ok, no) => {
    window.got = []; const s = new EventSource(`/v1/projects/${id}/events`);
    s.onopen = () => { window.got.push('open'); ok(); };
    s.onmessage = (e) => { const m = JSON.parse(e.data); window.got.push(m.type === 'cut' ? `cut ${m.cut}` : m.type); if (m.type === 'ended') s.close(); };
    s.onerror = () => { window.got.push('error'); if (s.readyState === EventSource.CLOSED) no(new Error('events closed')); };
  }), id);
  execSync(recut, { stdio: ['ignore', process.stderr, process.stderr], shell: '/bin/bash' });
  await page.waitForFunction(() => window.got.includes('ended') || window.got.includes('error'));
  out.events_on_cookie = await page.evaluate(() => window.got);
  const cross = await context.newPage();
  await cross.goto(`${crossOrigin}/cross-site.html?target=${encodeURIComponent(base)}&id=${id}`);
  await cross.waitForFunction(() => window.done === true);
  out.cross_site_page = await cross.evaluate(() => window.results);
  const all = [...seen.values()];
  out.same_site_ranges = all.filter((s) => s.path.includes('/cuts/edit.mp4') && s.site !== 'cross-site')
    .map((s) => ({ status: s.status, range: s.range, content_range: s.content_range, cookie: s.cookie }));
  out.cross_site_at_calcifer = all.filter((s) => s.site === 'cross-site' || s.origin === crossOrigin);
  console.log(JSON.stringify(out, null, 2));
  await browser.close();
})().catch((e) => { console.error(e); process.exit(1); });
