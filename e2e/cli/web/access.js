// Who may talk to Calcifer, with no sessions (design/calcifer-access.md, cl-985), on Linux: Calcifer's own page works in two tabs and
// through a kill -9 with nothing minted; another site's and another port's fetch, <img> and WebSocket are refused, a framed page
// doesn't render, a rebinding Host is turned away on the page and /v1, another OS account is refused (curl, and a real Chrome of its
// own that shows only "No project open here"), and in dev mode the page and its hot reload come from Vite through Calcifer alone.
//   node access.js: by web.sh's web_proof, as `web-access`
// $PLAYWRIGHT, $CHROME, $HOLDER (Calcifer's lock record), $RESTART (kill -9 of Calcifer, then back), $COZY, $LAST_LOOK. Prints what it
// saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync, spawn, spawnSync } = require('child_process');
const { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } = require('fs');
const http = require('http');
const { homedir } = require('os');
const { dirname, join, resolve } = require('path');
const { calcifer, holderToken, importFile, launch, newProject, png, projectJson, STEP_MS, until } = require('./page');

const failures = [];
const seen = {};
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const OTHER = 'cozy-access-other';
const REPO = resolve(__dirname, '../../..');
const WEB = join(REPO, 'web/app');
const CONFIG = join(homedir(), '.cozy/config.toml');
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const restart = () => execFileSync('bash', ['-c', process.env.RESTART], { stdio: 'ignore', timeout: 5 * STEP_MS });

/** `curl` as `user` (this account when none): its status and error code. Headers come on stdin, so the bearer is in no argv. */
function curl(path, headers = {}, user) {
  const lines = Object.entries(headers).map(([name, value]) => `${name}: ${value}`).join('\n');
  const words = ['curl', '-sS', '-o', '-', '-w', '\n%{http_code}', '-H', '@-', `${calcifer()}${path}`];
  const ran = user ? spawnSync('sudo', ['-n', '-u', user, '-H', ...words], { input: lines, encoding: 'utf8' }) : spawnSync(words[0], words.slice(1), { input: lines, encoding: 'utf8' });
  const text = ran.stdout ?? '';
  const status = Number(text.slice(text.lastIndexOf('\n') + 1));
  let code = null;
  try { code = JSON.parse(text.slice(0, text.lastIndexOf('\n'))).error?.code ?? null; } catch { /* not an envelope: a page or a file */ }
  return { status, code, body: text.slice(0, text.lastIndexOf('\n')) };
}

/** A page of another site (`localhost`) or another port of this computer (`127.0.0.1`), as an attacker serves one. */
function attacker() {
  const server = http.createServer((request, response) => {
    response.writeHead(200, { 'Content-Type': 'text/html' });
    response.end('<!doctype html><title>elsewhere</title><body></body>');
  });
  return new Promise((done) => server.listen(0, '127.0.0.1', () => done(server)));
}

/** From a page at `origin`, what Calcifer answered its fetch, <img>, WebSocket and frame (the browser's own record of each). */
async function from(context, origin, base, id, file, code) {
  const page = await context.newPage();
  // The status Calcifer answered, from the network itself: CORS and ORB keep a refused answer from the page, and from Playwright.
  const statuses = {}, asked = {};
  const network = await context.newCDPSession(page);
  await network.send('Network.enable');
  network.on('Network.requestWillBeSent', ({ requestId, request }) => { asked[requestId] = request.url; });
  network.on('Network.responseReceivedExtraInfo', ({ requestId, statusCode }) => {
    const url = asked[requestId] && new URL(asked[requestId]);
    if (url && url.origin === base) statuses[url.pathname] = statusCode;
  });
  const refusedFrame = [];
  page.on('console', (message) => { if (/frame-ancestors/.test(message.text())) refusedFrame.push(message.text()); });
  await page.goto(`${origin}/`);
  const said = await page.evaluate(async ({ base, id, file, code }) => {
    const fetched = await fetch(`${base}/v1/status`, { mode: 'no-cors' }).then(() => 'answered', () => 'failed');
    const read = await fetch(`${base}/v1/projects/${id}?assets=false`).then(() => 'read', () => 'refused');
    const written = await fetch(`${base}/v1/run-estimates`, { method: 'POST', body: '{}', headers: { 'Content-Type': 'text/plain' } }).then(() => 'sent', () => 'refused');
    const image = await new Promise((done) => {
      const img = new Image();
      img.onload = () => done('loaded');
      img.onerror = () => done('error');
      img.src = `${base}/v1/projects/${id}/files/${file}`;
    });
    const socket = await new Promise((done) => {
      const ws = new WebSocket(`${base.replace('http', 'ws')}/__cozy_hmr`, 'vite-hmr');
      ws.onopen = () => { ws.close(); done('open'); };
      ws.onerror = () => done('error');
      setTimeout(() => done('silent'), 10000);
    });
    const frame = document.createElement('iframe');
    frame.src = `${base}/p/${code}`;
    document.body.append(frame);
    await new Promise((done) => { frame.onload = done; setTimeout(done, 10000); });
    return { fetched, read, written, image, socket };
  }, { base, id, file, code });
  await sleep(1000);
  const frames = page.frames().map((frame) => frame.url());
  const framed = page.frames().find((frame) => frame.url().startsWith(base));
  const rendered = framed ? await framed.evaluate(() => document.getElementById('root')?.childNodes.length ?? 0).catch(() => 0) : 0;
  await page.close();
  return { ...said, statuses, rendered, frames, refused_frame: refusedFrame.length > 0 || frames.some((url) => url.startsWith('chrome-error:')) };
}

/** `config.toml` with dev mode on: a top-level `dev = true`, and `[calcifer] development_web` this Vite. */
function devConfig(text) {
  const lines = text.split('\n').filter((line) => !/^\s*dev\s*=/.test(line) && !/^\s*development_web\s*=/.test(line));
  const table = lines.findIndex((line) => /^\s*\[calcifer\]\s*$/.test(line));
  if (table >= 0) lines.splice(table + 1, 0, 'development_web = "127.0.0.1:5173"');
  else lines.push('[calcifer]', 'development_web = "127.0.0.1:5173"');
  return ['dev = true', ...lines].join('\n') + '\n';
}

(async () => {
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const base = calcifer();
  const token = holderToken();
  const out = dirname(process.env.LAST_LOOK);
  const folder = join(out, 'web-access-project');
  rmSync(folder, { recursive: true, force: true });
  await newProject(folder);
  const project = projectJson(folder, 'GET', '?assets=false');
  seen.project = { id: project.id, folder };
  const picture = join(out, 'web-access.png');
  writeFileSync(picture, png(40));
  const file = importFile(folder, picture).path.split('/').pop();
  const bearer = { Authorization: `Bearer ${token}` };
  check('the_cli_with_its_bearer', curl('/v1/status', bearer).status === 200);
  const bare = curl('/v1/status');
  check('the_cli_without_its_bearer', bare.status === 401 && bare.code === 'unauthenticated', bare);

  const browser = await launch(playwright, { args: ['--host-resolver-rules=MAP *.nip.io 127.0.0.1'] });
  const context = await browser.newContext({ viewport: { width: 1400, height: 800 } });
  context.setDefaultTimeout(STEP_MS);
  const minted = [];
  context.on('request', async (request) => {
    const url = new URL(request.url());
    if (url.origin !== base || !url.pathname.startsWith('/v1/')) return;
    const headers = await request.allHeaders().catch(() => ({}));
    if (url.pathname.startsWith('/v1/session') || headers.authorization) minted.push(url.pathname);
  });
  const read = (page) => page.evaluate(async (id) => (await fetch(`/v1/projects/${id}?assets=false`, { headers: { 'cozy-project-storage': '3' } })).status, project.id);

  // Two tabs, each Calcifer's own page: nothing minted, nothing sent but the page's own requests.
  const tabs = [await context.newPage(), await context.newPage()];
  for (const tab of tabs) {
    await tab.goto(`${base}/p/${project.code}`);
    await tab.waitForSelector('.top');
  }
  for (const [at, tab] of tabs.entries()) check(`tab_${at + 1}_works`, (await read(tab)) === 200 && (await tab.locator('.gate').count()) === 0);
  check('nothing_minted', minted.length === 0, minted);

  // Another site, and another port of this computer: refused, however it asks.
  const elsewhere = await attacker();
  const port = elsewhere.address().port;
  for (const [name, origin] of [['cross_site', `http://localhost:${port}`], ['same_site', `http://127.0.0.1:${port}`]]) {
    const got = await from(context, origin, base, project.id, file, project.code);
    seen[`${name}_saw`] = got;
    check(`${name}_fetch_refused`, got.statuses['/v1/status'] === 403 && got.read === 'refused' && got.statuses[`/v1/projects/${project.id}`] === 403, got.statuses);
    check(`${name}_write_refused`, got.statuses['/v1/run-estimates'] === 403, got.statuses);
    check(`${name}_img_refused`, got.image === 'error' && got.statuses[`/v1/projects/${project.id}/files/${file}`] === 403, got.statuses);
    check(`${name}_websocket_refused`, got.socket !== 'open', got.socket);
    check(`${name}_frame_refused`, got.rendered === 0 && got.refused_frame, { rendered: got.rendered, said: got.refused_frame });
  }
  elsewhere.close();

  // A rebinding name for 127.0.0.1: neither the page nor /v1 answers it.
  const rebound = await context.newPage();
  for (const path of ['/', '/v1/status']) {
    const answer = await rebound.goto(`http://127.0.0.1.nip.io:${new URL(base).port}${path}`);
    const body = await answer.text();
    check(`rebinding_host_refused_${path === '/' ? 'page' : 'v1'}`, answer.status() === 403 && body.includes('host_not_allowed'), { status: answer.status(), body: body.slice(0, 200) });
  }
  await rebound.close();

  // A kill -9 and back: both tabs go on with nothing to open again.
  restart();
  for (const [at, tab] of tabs.entries()) {
    await until(`tab ${at + 1} answered after the restart`, () => read(tab).catch(() => 0), (status) => status === 200);
    check(`tab_${at + 1}_after_restart`, (await tab.locator('.gate').count()) === 0);
  }
  check('nothing_minted_after_restart', minted.length === 0, minted);

  // Another OS account: refused on /v1 with any labels or the bearer; the page's files are its alone to read, and its own Chrome says
  // only that no project is open here.
  if (spawnSync('sudo', ['-n', 'true']).status !== 0) {
    check('sudo_for_another_account', false);
  } else {
    spawnSync('sudo', ['-n', 'useradd', '-m', OTHER]);
    const asOther = [
      ['bearer', bearer, `/v1/status`],
      ['page', { 'Sec-Fetch-Site': 'same-origin', 'Sec-Fetch-Mode': 'cors', 'cozy-project-storage': '3' }, `/v1/projects/${project.id}?assets=false`],
      ['img', { 'Sec-Fetch-Site': 'same-origin', 'Sec-Fetch-Mode': 'no-cors', 'Sec-Fetch-Dest': 'image' }, `/v1/projects/${project.id}/files/${file}`],
    ];
    for (const [name, headers, path] of asOther) {
      const got = curl(path, headers, OTHER);
      check(`other_account_refused_${name}`, got.status === 403 && got.code === 'other_user', got);
    }
    check('other_account_reads_the_page_files', curl('/', {}, OTHER).status === 200);
    const gate = await otherChrome(base, project);
    seen.other_chrome = gate;
    check('other_account_chrome_sees_no_project', gate.gate === 1 && !gate.text.includes(folder) && gate.project_reads.every((status) => status === 403), gate);
  }

  // Dev mode: Vite's code and hot reload through Calcifer, which alone the browser talks to.
  await devMode(context, base, project, token);

  check('no_session_anywhere', minted.length === 0, minted);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

/** Another account's own headless Chrome, attached over its DevTools port: Calcifer's page as that account sees it. */
async function otherChrome(base, project) {
  const profile = `/tmp/${OTHER}-chrome`;
  const port = 9333;
  spawnSync('sudo', ['-n', '-u', OTHER, 'mkdir', '-p', profile]);
  const chrome = spawn('sudo', ['-n', '-u', OTHER, '-H', process.env.CHROME, '--headless=new', '--no-sandbox', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'pipe', 'pipe'] });
  let said = '';
  chrome.stdout.on('data', (chunk) => { said += chunk; });
  chrome.stderr.on('data', (chunk) => { said += chunk; });
  try {
    const version = `http://127.0.0.1:${port}/json/version`;
    const endpoint = await until('the other account\'s Chrome listens', () => fetch(version).then((r) => r.json()).then((v) => v.webSocketDebuggerUrl, () => null), Boolean);
    const other = await playwright.chromium.connectOverCDP(endpoint);
    const page = await other.contexts()[0].newPage();
    const reads = [];
    page.on('response', (response) => { if (new URL(response.url()).pathname.startsWith('/v1/projects')) reads.push(response.status()); });
    await page.goto(`${base}/p/${project.code}`);
    await page.getByRole('heading', { name: 'No project open here' }).waitFor({ timeout: STEP_MS });
    const seenThere = { gate: await page.locator('.gate').count(), text: await page.locator('body').innerText(), project_reads: reads };
    await other.close();
    return seenThere;
  } catch (error) {
    return { gate: 0, text: String(error), project_reads: [], chrome_said: said.slice(-1500), exited: chrome.exitCode };
  } finally {
    spawnSync('sudo', ['-n', 'pkill', '-u', OTHER]);
  }
}

async function devMode(context, base, project, token) {
  if (!existsSync(join(WEB, 'node_modules/.bin/vite'))) { check('dev_vite_installed', false, WEB); return; }
  const original = existsSync(CONFIG) ? readFileSync(CONFIG, 'utf8') : null;
  const vite = spawn(join(WEB, 'node_modules/.bin/vite'), ['--host', '127.0.0.1', '--port', '5173', '--strictPort'], { cwd: WEB, detached: true, stdio: 'ignore' });
  const tokens = join(WEB, 'src/ui/tokens.css');
  const css = readFileSync(tokens, 'utf8');
  try {
    await until('Vite answers', () => fetch('http://127.0.0.1:5173/@vite/client').then((r) => r.status, () => 0), (status) => status === 200, 2 * STEP_MS);
    const api = await fetch('http://127.0.0.1:5173/v1/status').then(async (r) => ({ status: r.status, body: await r.text() }));
    check('dev_vite_answers_no_api', api.status === 404 && api.body.includes('development_api_in_calcifer'), api);
    mkdirSync(dirname(CONFIG), { recursive: true });
    writeFileSync(CONFIG, devConfig(original ?? ''));
    restart();
    await until('Calcifer is back in dev mode', async () => curl('/v1/status', { Authorization: `Bearer ${holderToken()}` }).status, (status) => status === 200);

    const page = await context.newPage();
    const said = [];
    const fetched = [];
    page.on('console', (message) => said.push(message.text()));
    page.on('request', (request) => { if (/^(https?|wss?):/.test(request.url())) fetched.push(new URL(request.url()).origin); });
    await page.goto(`${base}/p/${project.code}`);
    await page.waitForSelector('.top');
    await until('the hot reload connects', async () => said.some((line) => line.includes('[vite] connected')), Boolean);
    check('dev_page_through_calcifer', fetched.every((origin) => origin === base), [...new Set(fetched)]);
    writeFileSync(tokens, `${css}\n/* access.js: a hot update */\n`);
    await until('a hot update arrives', async () => said.some((line) => line.includes('hot updated') && line.includes('tokens.css')), Boolean);
    check('dev_hot_reload_through_calcifer', true);
    check('dev_page_still_works', (await page.evaluate(async (id) => (await fetch(`/v1/projects/${id}?assets=false`, { headers: { 'cozy-project-storage': '3' } })).status, project.id)) === 200);
    await page.close();

    // Another account gets none of this checkout's code through Calcifer, and the lock record is no file Vite serves.
    if (spawnSync('sudo', ['-n', 'true']).status === 0) {
      for (const path of ['/', '/src/main.tsx', '/@vite/client']) {
        const got = curl(path, {}, OTHER);
        check(`dev_other_account_refused_${path}`, got.status === 403 && got.code === 'other_user', got);
      }
    }
    const lock = curl(`/@fs${homedir()}/.cozy/calcifer/calcifer.lock.holder`, { 'Sec-Fetch-Site': 'same-origin', 'Sec-Fetch-Mode': 'no-cors' });
    check('dev_lock_record_not_served', !lock.body.includes(token) && !lock.body.includes(holderToken()), lock.status);
  } catch (error) {
    check('dev_mode', false, String(error));
  } finally {
    writeFileSync(tokens, css);
    if (original === null) rmSync(CONFIG, { force: true });
    else writeFileSync(CONFIG, original);
    try { process.kill(-vite.pid, 'SIGKILL'); } catch { /* gone */ }
    restart();
  }
}

async function died(error) {
  console.error(error);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
