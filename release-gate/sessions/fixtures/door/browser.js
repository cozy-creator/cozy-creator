// door.sh's browser: each hostile answer, opened directly through the door to a pod's Calcifer on this computer's origin, in a
// real Chrome. Prints what each did as JSON, and door.sh judges it: no script ran, no cookie stuck. A page on the plain control
// server does run its script, so the harness can see a script that ran.
//   node browser.js <door base> <bearer> <control url>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: Google Chrome. Every step is bounded.
const { chromium } = require(process.env.PLAYWRIGHT);
const [door, bearer, control] = process.argv.slice(2);
const STEP_MS = 20000;
const PATHS = ['page', 'listed', 'two', 'json', 'video', 'svg', 'none', 'redirect'];

(async () => {
  const out = { control: null, door: {} };
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const open = async (url, headers) => {
    const context = await browser.newContext({ extraHTTPHeaders: headers, acceptDownloads: true });
    context.setDefaultTimeout(STEP_MS);
    const page = await context.newPage();
    const seen = { requests: [], downloaded: false, navigated_to: null, status: null };
    // A video document reads its own URL: only other requests are what a script asked.
    page.on('request', (q) => { if (!q.isNavigationRequest() && q.url() !== url) seen.requests.push(q.url()); });
    page.on('download', () => { seen.downloaded = true; });
    try {
      const answer = await page.goto(url, { timeout: STEP_MS, waitUntil: 'load' });
      seen.status = answer && answer.status();
    } catch (error) {
      seen.goto = String(error).split('\n')[0];
    }
    await new Promise((ok) => setTimeout(ok, 1500));
    seen.navigated_to = page.url();
    seen.cookies = (await context.cookies()).map((c) => c.name);
    // A sandboxed document may refuse this too: what it says is only a second opinion on `requests` and `cookies`.
    seen.ran = await Promise.race([
      page.evaluate(() => document.documentElement.getAttribute('data-ran')).catch(() => null),
      new Promise((ok) => setTimeout(() => ok(null), 3000)),
    ]);
    await context.close();
    return seen;
  };
  // The door's own origin allows its own script: a module a pod answered would run as you, if the door let its type through. The control is
  // a module on this origin that the page's CSP lets run, so the harness can see one that ran.
  const imported = async () => {
    const context = await browser.newContext({ extraHTTPHeaders: { Authorization: `Bearer ${bearer}` } });
    context.setDefaultTimeout(STEP_MS);
    const page = await context.newPage();
    await page.route('**/control-module.js', (route) => route.fulfill({ contentType: 'text/javascript', body: "document.documentElement.setAttribute('data-ran','control');" }));
    await page.goto(`${new URL(door).origin}/`, { timeout: STEP_MS, waitUntil: 'load' });
    const load = (path) => page.evaluate((at) => import(at).then(() => 'loaded', (error) => String(error)), path);
    const ran = () => page.evaluate(() => document.documentElement.getAttribute('data-ran'));
    const seen = { control: await load('/control-module.js'), control_ran: await ran() };
    await page.evaluate(() => document.documentElement.removeAttribute('data-ran'));
    seen.door = await load(`${new URL(door).pathname}/v1/door/module`);
    seen.door_ran = await ran();
    await context.close();
    return seen;
  };
  out.control = await open(control, {});
  for (const name of PATHS) out.door[name] = await open(`${door}/v1/door/${name}`, { Authorization: `Bearer ${bearer}` });
  out.module = await imported();
  await browser.close();
  console.log(JSON.stringify(out));
})().catch((error) => { console.error(error); process.exit(1); });
