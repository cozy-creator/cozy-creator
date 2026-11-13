// cl-980 (Paul, 11-13): a tab at /p/<code> went back and forth between "This project can't be opened yet" and / forever once the project's
// folder was deleted: / opened the last project by itself, and a project gone sent the page to /. Now / opens nothing by itself; a project
// whose folder goes says so where it stands ("This project's folder is gone: <path>"), and a reload says the same. Calcifer killed and back
// leaves the tab working where it is, with no gate at any moment: it keeps no session (cl-985), so there is nothing to sign in again. After
// the page opens, nothing here moves it. The gate a page Calcifer refuses draws is access.js's, with a real second OS account.
//   node stays.js: opened by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $RESTART: a shell command that kills this Calcifer with -9 and brings it
// back; $SHOT: where its screenshots go. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { exec } = require('child_process');
const { rmSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { holderToken, calcifer, signIn, STEP_MS, launch, lastLook, lookAt, newProject, noStall, removeGeneratedProject, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok === true ? true : (detail ?? false);
  if (ok !== true) failures.push(name);
}
const restart = () => new Promise((done, fail) => exec(process.env.RESTART, { shell: 'bash' }, (error) => (error ? fail(error) : done())));
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
/** Long enough for a page that sends itself somewhere to have done it: the loop went round several times a second. */
const STILL_MS = 3000;

/** Every move of the page's address that changes its path, and every gate drawn, per document. */
function watch() {
  window.__moves = [];
  for (const name of ['pushState', 'replaceState']) {
    const native = history[name].bind(history);
    history[name] = (state, title, url) => {
      const from = location.pathname;
      native(state, title, url);
      if (location.pathname !== from) window.__moves.push(`${from} -> ${location.pathname}`);
    };
  }
  window.__gates = [];
  new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) if (node instanceof Element && node.matches('main.gate')) window.__gates.push(node.textContent);
    }
  }).observe(document, { childList: true, subtree: true });
}

(async () => {
  const base = calcifer();
  const stamp = Date.now();
  const gone = await newProject(join(homedir(), `web-stays-${stamp}-gone`));
  const kept = await newProject(join(homedir(), `web-stays-${stamp}-kept`));
  seen.project = { id: kept.id, folder: kept.path };
  seen.gone_project = { id: gone.id, folder: gone.path };
  const browser = await launch(playwright);
  const shot = (page, name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());
  const errors = [];
  const tab = async (context) => {
    await context.addInitScript(watch);
    const page = await context.newPage();
    page.setDefaultTimeout(STEP_MS);
    page.on('pageerror', (error) => errors.push(error.message));
    lookAt(page);
    return page;
  };
  const moves = (page) => page.evaluate(() => window.__moves);
  const gates = (page) => page.evaluate(() => window.__gates);
  /** Where it is, and it stays there with nothing moving it. */
  const stays = async (page, name, path) => {
    const at = new URL(page.url()).pathname;
    await pause(STILL_MS);
    const now = { path: new URL(page.url()).pathname, moves: await moves(page) };
    check(name, now.path === (path ?? at) && now.moves.length === 0, { was: at, ...now });
  };

  // `/` opens no project by itself, though the index has two.
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await tab(context);
  await signIn(page);
  await page.locator('.top').waitFor();
  await stays(page, 'home_opens_nothing', '/');

  await page.goto(`${base}/p/${gone.code}`);
  await page.locator('.react-flow__node').first().waitFor();
  check('opens', (await page.locator('.unread, .gate').count()) === 0);

  // Calcifer killed and back: the tab goes on where it is, its canvas kept, no gate drawn at any moment, and the new Calcifer answers its
  // own reads, which carry no Authorization. The folder's going below reaches it on the stream it opened again.
  if (process.env.RESTART) {
    const pid = JSON.parse(require('fs').readFileSync(process.env.HOLDER, 'utf8')).pid;
    const restarted = restart();
    seen.said_during_restart = await until('it said offline', () => page.locator('.pill.warn').allTextContents(), (lines) => lines.some((line) => line.startsWith('Calcifer offline')));
    await restarted;
    await until('back', () => page.locator('.pill.warn').allTextContents(), (lines) => !lines.some((line) => line.includes('offline')));
    const now = JSON.parse(require('fs').readFileSync(process.env.HOLDER, 'utf8')).pid;
    check('restart_is_a_new_calcifer', now !== pid, { was: pid, now });
    await stays(page, 'restart_stays', `/p/${gone.code}`);
    check('restart_draws_no_gate', (await gates(page)).length === 0 && (await page.locator('.unread, .gate').count()) === 0, await gates(page));
    check('restart_keeps_the_canvas', (await page.locator('.react-flow__node').count()) > 0);
    const read = await page.evaluate((id) => fetch(`/v1/projects/${id}`, { headers: { 'cozy-project-storage': '3' } }).then((answer) => answer.status), gone.id);
    check('restart_answers_its_reads', read === 200, read);
    await shot(page, 'restarted');
  }

  // Its folder deleted under the open tab: said in place, in plain words, and the page stays.
  rmSync(gone.path, { recursive: true, force: true });
  const said = `This project's folder is gone: ${gone.path}`;
  await page.locator('.unread h2', { hasText: said }).waitFor();
  await stays(page, 'gone_stays', `/p/${gone.code}`);
  check('gone_said_once', (await page.locator('.unread').count()) === 1 && (await page.locator('.gate').count()) === 0);
  const holder = JSON.parse(require('fs').readFileSync(process.env.HOLDER, 'utf8'));
  const listed = await (await fetch(`http://${holder.address}/v1/projects`, { headers: { Authorization: `Bearer ${holderToken()}`, 'cozy-project-storage': '3' } })).json();
  check('gone_leaves_the_index', !listed.projects.some((project) => project.id === gone.id), listed.projects.map((project) => project.id));
  await shot(page, 'gone');

  // A reload of it says the same, and stays.
  await page.reload();
  await page.locator('.gate h2', { hasText: said }).waitFor();
  await stays(page, 'gone_reload_stays', `/p/${gone.code}`);
  await shot(page, 'gone-reload');

  // Another project's tab, Calcifer killed and back again, then reloaded: its canvas, in place, with no gate before or after the reload.
  await page.goto(`${base}/p/${kept.code}`);
  await page.locator('.react-flow__node').first().waitFor();
  if (process.env.RESTART) {
    const restarted = restart();
    await until('it said offline again', () => page.locator('.pill.warn').allTextContents(), (lines) => lines.some((line) => line.startsWith('Calcifer offline')));
    await restarted;
    await until('back again', () => page.locator('.pill.warn').allTextContents(), (lines) => !lines.some((line) => line.includes('offline')));
    await stays(page, 'restart_again_stays', `/p/${kept.code}`);
    check('restart_again_draws_no_gate', (await gates(page)).length === 0 && (await page.locator('.unread, .gate').count()) === 0, await gates(page));
    await page.reload();
    await page.locator('.react-flow__node').first().waitFor();
    await stays(page, 'reload_after_restart_stays', `/p/${kept.code}`);
    check('reload_after_restart_draws_no_gate', (await gates(page)).length === 0 && (await page.locator('.unread, .gate').count()) === 0, await gates(page));
    await shot(page, 'reloaded');
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  await removeGeneratedProject(kept.path, 'web-stays-');
  console.log(JSON.stringify({ ok: !failures.length, failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  seen.died = String(error?.stack ?? error);
  await lastLook();
  console.log(JSON.stringify({ ok: false, failures: [...failures, 'died'], seen }, null, 2));
  process.exit(2);
}
