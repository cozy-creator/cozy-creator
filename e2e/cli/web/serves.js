// The web-serves cell's browser (e2e/cli/web.sh): Calcifer's page opened in a real Chrome, as any browser of this account opens it.
// Nothing is minted or kept (design/calcifer-access.md): the page renders under its CSP, opens GET /v1/events with no Authorization,
// and a stream here then sees `hello` and the `project.changed` of an import the CLI makes. A URL opened in the address bar
// (`Sec-Fetch-Site: none`, a navigation) reads /v1. Prints what it saw as JSON, and web.sh judges it.
//   node serves.js <project id> <project folder> <file to import>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: Google Chrome; $COZY: the CLI; $HOLDER: Calcifer's lock record. Every browser step is bounded.
const { chromium } = require(process.env.PLAYWRIGHT);
const { importFile, calcifer, signIn } = require('./page');
const [id, folder, file] = process.argv.slice(2);
const origin = calcifer();
const STEP_MS = 60000;
const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));
const bounded = (what, promise) => {
  let timer;
  const late = new Promise((_, no) => { timer = setTimeout(() => no(new Error(`no ${what} in ${STEP_MS / 1000} s`)), STEP_MS); });
  return Promise.race([promise, late]).finally(() => clearTimeout(timer));
};
const out = { violations: [], console_errors: [], page_errors: [], assets: [], minted: [], authorization: [], page_stream: null, opened: {} };
let mine = false;

(async () => {
  const began = Date.now();
  const browser = await bounded('browser', chromium.launch({ executablePath: process.env.CHROME, headless: true }));
  out.opened.launch_ms = Date.now() - began;
  const context = await browser.newContext();
  context.setDefaultTimeout(STEP_MS);
  await context.addInitScript(() => {
    window.__violations = [];
    addEventListener('securitypolicyviolation', (e) => window.__violations.push(`${e.violatedDirective} blocked ${e.blockedURI}`));
  });
  const page = await context.newPage();
  page.on('console', (m) => {
    if (m.type() !== 'error') return;
    (/Content Security Policy/.test(m.text()) ? out.violations : out.console_errors).push(`${m.text()} ${m.location().url}`);
  });
  page.on('pageerror', (e) => out.page_errors.push(String(e)));
  // Until `what` in the page holds, by polling from here.
  const until = async (what, check, arg) => {
    const deadline = Date.now() + STEP_MS;
    for (;;) {
      if (await bounded(what, page.evaluate(check, arg))) return;
      if (Date.now() > deadline) throw new Error(`no ${what} in ${STEP_MS / 1000} s`);
      await sleep(100);
    }
  };
  context.on('request', async (q) => {
    const path = q.url().slice(origin.length);
    if (!path.startsWith('/v1/')) return;
    // Nothing the page sends is a credential, and it asks for none.
    if (/^\/v1\/session/.test(path)) out.minted.push(`${q.method()} ${path}`);
    const authorization = (await q.allHeaders()).authorization;
    if (authorization) out.authorization.push(path);
  });
  context.on('response', async (r) => {
    const q = r.request(), path = r.url().slice(origin.length);
    if (path === '/v1/events' && !out.page_stream && !mine) {
      const headers = await q.allHeaders();
      out.page_stream = { status: r.status(), content_type: r.headers()['content-type'] || null, authorization: headers.authorization || null, site: headers['sec-fetch-site'] || null };
    } else if (path.startsWith('/assets/')) {
      out.assets.push({ path, status: r.status(), cache_control: r.headers()['cache-control'] || null });
    }
  });

  const doc = await signIn(page, { waitUntil: 'load' });
  const h = doc.headers();
  out.document = { status: doc.status(), csp: h['content-security-policy'] || null, referrer_policy: h['referrer-policy'] || null, set_cookie: h['set-cookie'] || null };
  await until('rendered page', () => document.getElementById('root') && document.getElementById('root').childNodes.length > 0);
  out.opened.rendered_ms = Date.now() - began;
  out.text = await page.evaluate(() => document.body.innerText.slice(0, 200));
  out.gate = await page.locator('.gate').count();
  const deadline = Date.now() + STEP_MS;
  while (!out.page_stream && Date.now() < deadline) await sleep(100);

  // Headers come first, so a stream that opens answers at once; what it carries then arrives in the page.
  mine = true;
  out.stream_status = await bounded('answer on GET /v1/events', page.evaluate(async () => {
    window.streamed = [];
    const r = await fetch('/v1/events', { headers: { 'cozy-project-storage': '3' } });
    if (!r.ok) return r.status;
    const reader = r.body.getReader(), decode = new TextDecoder();
    window.endStream = () => reader.cancel();
    (async () => {
      let held = '';
      for (;;) {
        const { done, value } = await reader.read();
        if (done) return;
        held += decode.decode(value, { stream: true });
        for (let at; (at = held.indexOf('\n\n')) >= 0; held = held.slice(at + 2)) {
          for (const line of held.slice(0, at).split('\n')) if (line.startsWith('data:')) window.streamed.push(JSON.parse(line.slice(5)));
        }
      }
    })().catch((e) => window.streamed.push({ type: 'broken', error: String(e) }));
    return r.status;
  }));
  await until('hello on the stream', () => window.streamed.some((e) => e.type === 'hello'));
  out.stream_interest = await page.evaluate(async (id) => {
    const stream = window.streamed.find((event) => event.type === 'hello').stream;
    const response = await fetch(`/v1/events/${encodeURIComponent(stream)}`, {
      method: 'PUT', headers: { 'cozy-project-storage': '3', 'Content-Type': 'application/json' }, body: JSON.stringify({ projects: [id] }),
    });
    const reply = await response.json();
    if (!response.ok || !reply.held.includes(id)) throw new Error(`project interest refused: ${response.status}`);
    return reply.held;
  }, id);
  out.imported = importFile(folder, file);
  await until('project.changed on the stream', (id) => window.streamed.some((e) => e.type === 'project.changed' && e.id === id && e.rev), id);
  out.streamed = await page.evaluate(() => { window.endStream(); return window.streamed; });
  out.violations.push(...await page.evaluate(() => window.__violations));
  // The project storage the page proved: bound to this OS account, not a token.
  out.storage = await page.evaluate(async () => (await fetch('/v1/project-storage', { headers: { 'cozy-project-storage': '3' } })).json());
  // The address bar: a person's own navigation to a /v1 URL reads it.
  const typed = await context.newPage();
  const status = await typed.goto(`${origin}/v1/status`);
  out.typed = { status: status.status(), site: (await status.request().allHeaders())['sec-fetch-site'] || null };
  out.cookies = await context.cookies();
  out.stored = await page.evaluate(() => [...Object.keys(localStorage), ...Object.keys(sessionStorage)].filter((key) => /session|token|code/i.test(key)));
  await browser.close();
  console.log(JSON.stringify(out, null, 2));
})().catch((e) => { console.error(e); console.log(JSON.stringify(out, null, 2)); process.exit(1); });
