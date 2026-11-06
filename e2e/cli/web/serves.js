// The web-serves cell's browser (e2e/cli/web.sh): `cozy open --no-open`'s link, opened in a real Chrome once it's up. The page redeems
// its code and clears it, renders under its CSP and opens GET /v1/events. On its session a stream here then sees `hello`, and the
// `project.changed` of an import the CLI makes. Prints what it saw as JSON, and web.sh judges it: `signed_in` says how long the browser
// took to launch and the code to be redeemed, and a refusal's code.
//   node serves.js <project id> <project folder> <file to import>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: Google Chrome; $COZY: the CLI; $HOLDER: Calcifer's lock record. Every browser step is bounded.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { calcifer, signIn } = require('./page');
const [id, folder, file] = process.argv.slice(2);
const origin = calcifer();
const STEP_MS = 60000;
const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));
const bounded = (what, promise) => {
  let timer;
  const late = new Promise((_, no) => { timer = setTimeout(() => no(new Error(`no ${what} in ${STEP_MS / 1000} s`)), STEP_MS); });
  return Promise.race([promise, late]).finally(() => clearTimeout(timer));
};
const out = { violations: [], console_errors: [], page_errors: [], assets: [], session: null, page_stream: null, signed_in: {} };
let [mine, asked] = [false, 0];

(async () => {
  const began = Date.now();
  const browser = await bounded('browser', chromium.launch({ executablePath: process.env.CHROME, headless: true }));
  out.signed_in.launch_ms = Date.now() - began;
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
  context.on('response', async (r) => {
    const q = r.request(), path = r.url().slice(origin.length);
    if (path === '/v1/session' && q.method() === 'POST') {
      const body = await r.json().catch(() => null);
      out.signed_in.redeemed_ms = Date.now() - asked;
      out.signed_in.refused = body?.error?.code;
      out.session = { status: r.status(), token: body && body.token, media: body && body.media, set_cookie: r.headers()['set-cookie'] || null };
    } else if (path === '/v1/events' && !out.page_stream && !mine) {
      out.page_stream = { status: r.status(), content_type: r.headers()['content-type'] || null, authorization: (await q.allHeaders()).authorization || null };
    } else if (path.startsWith('/assets/')) {
      out.assets.push({ path, status: r.status(), cache_control: r.headers()['cache-control'] || null });
    }
  });

  asked = Date.now();
  const doc = await signIn(page, { waitUntil: 'load' });
  const h = doc.headers();
  out.document = { status: doc.status(), csp: h['content-security-policy'] || null, referrer_policy: h['referrer-policy'] || null, set_cookie: h['set-cookie'] || null };
  const deadline = Date.now() + STEP_MS;
  while (!out.session && Date.now() < deadline) await sleep(100);
  if (!out.session || out.session.status !== 200) throw new Error('the page did not redeem its code on POST /v1/session');
  await until('cleared fragment', () => !location.hash.includes('code='));
  await until('rendered page', () => document.getElementById('root') && document.getElementById('root').childNodes.length > 0);
  out.text = await page.evaluate(() => document.body.innerText.slice(0, 200));
  while (!out.page_stream && Date.now() < deadline) await sleep(100);

  // Headers come first, so a stream that opens answers at once; what it carries then arrives in the page.
  mine = true;
  out.stream_status = await bounded('answer on GET /v1/events', page.evaluate(async (token) => {
    window.streamed = [];
    const r = await fetch('/v1/events', { headers: { Authorization: `Bearer ${token}` } });
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
  }, out.session.token));
  await until('hello on the stream', () => window.streamed.some((e) => e.type === 'hello'));
  out.imported = JSON.parse(execFileSync(process.env.COZY, ['--json', 'project', '-p', folder, 'import', file], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }));
  await until('project.changed on the stream', (id) => window.streamed.some((e) => e.type === 'project.changed' && e.id === id && e.rev), id);
  out.streamed = await page.evaluate(() => { window.endStream(); return window.streamed; });
  out.violations.push(...await page.evaluate(() => window.__violations));
  out.cookies = await context.cookies();
  await browser.close();
  console.log(JSON.stringify(out, null, 2));
})().catch((e) => { console.error(e); console.log(JSON.stringify(out, null, 2)); process.exit(1); });
