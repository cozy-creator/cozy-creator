// A package's own UI (crates/calcifer/docs/PACKAGE-UI.md), in a real browser on a real Calcifer. Local code a project names: a component draws
// one field, saved through setFields and read back by the file, `cozy status` and a reload; one draws the whole face; one that throws,
// one from a later API and one that isn't there leave the declared box, with the canvas up. What's served: a module's type and CSP, and that a
// wrong hash, a climb out, a file that's no module and a foreign origin get nothing. A release: Calcifer on a Hub this cell stands up: not
// installed, it doesn't load; installed, it loads under the wheel's digest; a Hub whose file isn't what it says doesn't load. The Hub's own API
// is held by every other cell.
//   node package_ui.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so no run and no restart; $RESTART, $HUB_RESTART: shell commands that kill this Calcifer and bring it back, the second on the Hub
// @HUB@ names. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { exec, execFile, execFileSync } = require('child_process');
const { createHash } = require('crypto');
const { cpSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } = require('fs');
const http = require('http');
const { homedir, tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectJson, calcifer, signIn, STEP_MS, boxOf, closeBox, noStall, openBox, renderOf, until, boxLog, lastLook, lookAt, watchBoxes, yamlOf } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
const FIRST_RUN_MS = 300_000;
const COZY = process.env.COZY;
const shell = (command) => new Promise((done, failed) => exec(command, (error) => (error ? failed(error) : done())));
const quiet = { stdio: ['ignore', 'pipe', 'pipe'], timeout: FIRST_RUN_MS };
// For what asks the stand-in Hub, which is this process: blocked in execFileSync it answers no one, and the install waits on it for good.
const cozyHere = (args, options) => new Promise((done, fail) => {
  execFile(COZY, args, options, (error, stdout, stderr) => (error ? fail(Object.assign(error, { said: `${stdout}${stderr}` })) : done(stdout)));
});
const element = (page, name) => page.locator('.react-flow__node').filter({ has: page.locator(`.nm:text-is("${name}")`) });

/** The folder at `root` held by Calcifer as a tree (`api::tree`): each file PUT by its sha256, then the document. Its digest. */
async function sentTree(base, token, root) {
  const walk = (dir) => readdirSync(join(root, dir), { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? (entry.name === '__pycache__' ? [] : walk(join(dir, entry.name))) : [join(dir, entry.name)]);
  const put = async (bytes) => {
    const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
    const answer = await fetch(`${base}/v1/files/${digest}`, { method: 'PUT', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/octet-stream' }, body: bytes });
    if (!answer.ok) throw new Error(`PUT ${digest}: ${answer.status} ${await answer.text()}`);
    return digest;
  };
  const entries = [];
  for (const path of walk('').sort()) {
    const bytes = readFileSync(join(root, path));
    entries.push({ blob: { length: bytes.length, sha256: (await put(bytes)).slice(7) }, kind: 'file', path: path.split(/[\\/]/).join('/') });
  }
  return put(Buffer.from(JSON.stringify({ entries })));
}

/** A GET as a browser's script loader sends one: no credential, the path as it's written. */
function raw(base, path, headers = {}) {
  const { hostname, port } = new URL(base);
  return new Promise((done, failed) => {
    const asked = http.request({ hostname, port, path, headers }, (answer) => {
      const chunks = [];
      answer.on('data', (chunk) => chunks.push(chunk));
      answer.on('end', () => done({ status: answer.statusCode, headers: answer.headers, body: Buffer.concat(chunks).toString() }));
    });
    asked.on('error', failed);
    asked.end();
  });
}

/** A Hub with one package: its real wheel, lock and interface, and a file that isn't what it says once it's told to lie. */
function standIn(name, release, wheelName, wheel, pyproject, lock, describedInterface) {
  const digest = createHash('sha256').update(wheel).digest('hex');
  const hub = { lying: false, asked: [], digest, origin: '' };
  const summary = { org: 'e2e', name, created_at: '2026-11-04T00:00:00Z', latest_release: release };
  const bytes = (text) => ({ canonical_bytes: Buffer.from(text).toString('base64') });
  // More than Browse's first page, so reading on is real.
  const shelf = [summary, ...Array.from({ length: 60 }, (_, n) => ({ ...summary, name: `shelf-${n}` }))];
  hub.server = http.createServer((request, response) => {
    const { pathname, searchParams } = new URL(request.url, 'http://hub');
    hub.asked.push(`${request.method} ${pathname}`);
    const json = (status, body) => {
      response.writeHead(status, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify(body));
    };
    request.resume();
    request.on('end', () => {
      const at = `/v1/packages/e2e/${name}`;
      if (hub.silent && pathname === at) return;
      if (hub.down) return json(503, { error: { kind: 'unavailable', code: 'down', message: 'this Hub is down' } });
      if (pathname === '/v1/packages') {
        const words = (searchParams.get('q') ?? '').split(/\s+/);
        const found = shelf.filter((row) => words.every((word) => `${row.org}/${row.name}`.includes(word)));
        return json(200, { packages: found.slice(0, Number(searchParams.get('limit'))), search: { total: found.length } });
      }
      // One of the defaults is this package again: a list reads its card and nothing else.
      if (pathname === at || pathname === '/v1/packages/_/minimax-h3') return json(200, { package: summary, releases: [{ release }] });
      if (pathname.endsWith(`/releases/${release}`)) return json(200, { release: { release }, package_interface: describedInterface, summary: 'One field, shaded.' });
      if (pathname === `${at}/download`) {
        return json(200, {
          release,
          python_version: '3.12',
          downloads: [{ url: `${hub.origin}/wheels/${wheelName}`, digest: `sha256:${digest}`, path: wheelName, length: wheel.length }],
          package_interface: bytes(JSON.stringify(describedInterface)),
          pyproject: bytes(pyproject),
          uv_lock: bytes(lock),
        });
      }
      if (pathname === `/wheels/${wheelName}`) {
        const sent = hub.lying ? Buffer.alloc(wheel.length, 7) : wheel;
        const range = /^bytes=(\d+)-(\d*)$/.exec(request.headers.range ?? '');
        const from = range ? Number(range[1]) : 0;
        const to = range && range[2] ? Math.min(Number(range[2]) + 1, sent.length) : sent.length;
        const headers = { 'Content-Type': 'application/octet-stream', 'Content-Length': to - from, 'Accept-Ranges': 'bytes' };
        if (range) headers['Content-Range'] = `bytes ${from}-${to - 1}/${sent.length}`;
        response.writeHead(range ? 206 : 200, headers);
        return response.end(sent.subarray(from, to));
      }
      return json(404, { error: { kind: 'not_found', code: 'absent', message: `no ${pathname} here` } });
    });
  });
  return new Promise((done) => hub.server.listen(0, '127.0.0.1', () => {
    hub.origin = `http://127.0.0.1:${hub.server.address().port}`;
    done(hub);
  }));
}

(async () => {
  const base = calcifer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  await watchBoxes(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  const uiCalls = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => {
    // React says what a boundary caught, and a browser says what it couldn't import: both are what this cell provokes.
    if (m.type() === 'error' && !/status of 4\d\d|status of 5\d\d|net::ERR_|thrown on purpose|dynamically imported module/.test(m.text())) errors.push(`console: ${m.text()}`);
  });
  page.on('request', (asked) => {
    const { pathname } = new URL(asked.url());
    if (pathname.startsWith('/v1/tools/') && pathname.includes('/ui')) uiCalls.push(pathname);
  });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  const documentAnswer = await signIn(page);
  const pagePolicy = documentAnswer.headers()['content-security-policy'];
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-package-ui-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  // The project as the YAML export API writes it (cl-803: its state is project.db).
  const yaml = () => yamlOf(folder);
  const status = () => projectJson(folder);
  const held = (name) => status().assets.find((asset) => asset.name === name);
  const box = (name) => boxOf(page, name);
  const away = async () => {
    await closeBox(page);
    await page.mouse.move(2, 500);
  };
  const open = async (name) => {
    await openBox(page, name);
    await (await renderOf(page, name)).waitFor();
  };
  // Behind its More, opened if it isn't. With nothing behind it there's no More, and all of it shows.
  const more = async (name) => {
    await open(name);
    if (!(await box(name).locator('.more-toggle').count())) return;
    if (!(await box(name).locator('.edit-more').count())) await box(name).locator('.more-toggle').click();
    await box(name).locator('.edit-more').waitFor();
  };
  const toolUi = (tool, project = id) => api(`/v1/tools/${encodeURIComponent(tool)}/ui${project ? `?project=${project}` : ''}`);
  let nth = 0;
  const dropTool = async (name, tool) => {
    nth += 1;
    const at = { x: ((nth - 1) % 4) * 340, y: Math.floor((nth - 1) / 4) * 360, w: 300 };
    const made = await api(`/v1/projects/${id}/assets`, { method: 'POST', headers: { 'Idempotency-Key': `package-ui-${name}` }, body: JSON.stringify({ tool, name, at }) });
    check(`dropped_${name}`, made.status === 201, made);
    // Drawn in on a box, the canvas draws only what's near it: a new one is seen at rest, all of it in view.
    await away();
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box(name).waitFor();
  };
  const fit = async () => {
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
  };

  if (local) {
    // Local code the project names, as a person keeps it beside the project. A run reads it with this machine's Runtime, so a box doesn't wait on PyPI's.
    const boxed = join(folder, 'boxed');
    cpSync(join(__dirname, '..', 'project', 'boxed'), boxed, { recursive: true });
    const primed = () => execFileSync(COZY, ['--json', 'run', './boxed/plain', 'instructions=primed', '--on', 'local'], { ...quiet, cwd: folder });
    primed();
    await page.reload();
    await page.waitForSelector('.top');
    await dropTool('shaded', './boxed/one_field');
    await dropTool('faced', './boxed/whole_face');
    await dropTool('thrown', './boxed/throws');
    await dropTool('later', './boxed/later');
    await dropTool('gone', './boxed/gone');
    await dropTool('plain', './boxed/plain');
    await fit();
    await until('the local tools listed', async () => (await api(`/v1/tools?project=${id}`)).body.tools.filter((tool) => tool.local).length, (n) => n === 6, FIRST_RUN_MS);

    // One field: its swatches where the declared range would be, behind More as that would be, a relative import behind them, and the rest of the box as declared.
    await more('shaded');
    const shade = box('shaded').locator('[data-boxed=shade]');
    await shade.waitFor({ timeout: FIRST_RUN_MS });
    check('draws_one_field', (await shade.getAttribute('data-asset')) === 'shaded');
    check('relative_import_loads', (await shade.getAttribute('data-steps')) === '0,0.25,0.5,0.75,1', await shade.getAttribute('data-steps'));
    check('limits_reach_it', (await shade.locator('button').count()) === 5);
    check('mentions_reach_it', (await shade.getAttribute('data-mentions')).split(',').includes('faced'), await shade.getAttribute('data-mentions'));
    check('tokens_reach_it', (await shade.evaluate((node) => getComputedStyle(node).color)) === (await page.evaluate(() => { const probe = document.createElement('i'); probe.style.color = 'var(--ink)'; document.body.append(probe); const color = getComputedStyle(probe).color; probe.remove(); return color; })));
    check('declared_range_replaced', (await box('shaded').locator('input[type=range]').count()) === 0);
    check('other_fields_stay_declared', (await box('shaded').locator('.mention-text[data-field=instructions]').count()) === 1);
    await shot('one-field');

    // Saved through setFields, read back three ways.
    await shade.locator('button[aria-label="Shade 0.75"]').click();
    await until('it stood', () => shade.getAttribute('data-saved'), (saved) => saved === 'true');
    check('saved_in_the_file', /shade: 0\.75/.test(yaml()), yaml());
    check('status_agrees', held('shaded').fields.shade === 0.75, held('shaded'));
    await page.reload();
    await page.waitForSelector('.react-flow__node');
    await fit();
    await more('shaded');
    await box('shaded').locator('[data-boxed=shade]').waitFor();
    check('reload_reads_it_back', (await box('shaded').locator('button[aria-label="Shade 0.75"]').getAttribute('aria-pressed')) === 'true');
    check('a_component_is_called_for_by_its_id', uiCalls.some((path) => path.endsWith('/ui') && path.includes(encodeURIComponent('./boxed/one_field'))), uiCalls);

    // The whole face: its own text, with Render still the app's.
    await open('faced');
    const whole = box('faced').locator('[data-boxed=whole]');
    await whole.waitFor();
    check('whole_face_replaces_the_controls', (await box('faced').locator('.mention-text[data-field=instructions], input[type=range]').count()) === 0);
    await whole.click();
    await page.keyboard.type('Its own words');
    // Anywhere else on it takes the keys away, as a person clicks: its name would rename it.
    await box('faced').locator('.here-under').click({ position: { x: 4, y: 4 } });
    await until('its words in the file', () => Promise.resolve(yaml()), (text) => text.includes('Its own words'));
    check('whole_face_saves', held('faced').fields.instructions === 'Its own words', held('faced').fields);

    // What doesn't draw leaves the declared box and the canvas.
    const fallsBack = async (name) => {
      await more(name);
      await box(name).locator('.tool-ui-failed').waitFor();
      check(`${name}_falls_back`, (await box(name).locator('[data-boxed]').count()) === 0 && (await box(name).locator('input[type=range]').count()) === 1 && (await box(name).locator('.mention-text[data-field=instructions]').count()) === 1, await box(name).innerText());
    };
    await page.evaluate(() => { window.notReloaded = true; });
    await fallsBack('thrown');
    await shot('thrown');
    await fallsBack('later');
    await fallsBack('gone');
    await away();
    await fit();
    check('canvas_stays_up', (await page.evaluate(() => window.notReloaded === true)) && (await page.locator('.react-flow__node').count()) === 6, await page.locator('.react-flow__node').count());
    await more('shaded');
    await box('shaded').locator('[data-boxed=shade]').waitFor();
    check('others_still_draw', true);
    const missing = await toolUi('./boxed/gone');
    check('gone_says_so', missing.status === 404 && missing.body.error.code === 'tool_ui_missing', missing);

    // A function with none: nothing is asked, and nothing's wrong.
    await open('plain');
    check('plain_asks_nothing', !uiCalls.some((path) => path.includes(encodeURIComponent('./boxed/plain'))), uiCalls);
    check('plain_is_quiet', (await box('plain').locator('.tool-ui-failed').count()) === 0);
    const none = await toolUi('./boxed/plain');
    check('plain_ships_none', none.status === 404 && none.body.error.code === 'tool_ui_none', none);

    // What's served.
    const found = await toolUi('./boxed/one_field');
    check('found', found.status === 200 && /^\/v1\/tools\/\.%2Fboxed%2Fone_field\/ui\/[0-9a-f]{64}\/ui\/shade\.js$/.test(found.body.module), found);
    const module_ = found.body.module;
    const [, hash] = module_.match(/\/ui\/([0-9a-f]{64})\//);
    const served = await raw(base, module_);
    check('served_with_no_session', served.status === 200 && served.body.includes('data-boxed'), served.status);
    check('served_as_javascript', served.headers['content-type'] === 'application/javascript', served.headers);
    check('served_under_the_page_csp', served.headers['content-security-policy'] === pagePolicy, served.headers);
    check('served_immutable', /immutable/.test(served.headers['cache-control'] ?? ''), served.headers);
    check('served_nosniff', served.headers['x-content-type-options'] === 'nosniff', served.headers);
    check('sibling_served', (await raw(base, module_.replace('shade.js', 'steps.js'))).status === 200);
    const nothing = async (what, path, headers) => {
      const answer = await raw(base, path, headers);
      check(what, answer.status === 404, { status: answer.status, body: answer.body.slice(0, 200) });
    };
    await nothing('wrong_hash_gets_nothing', module_.replace(hash, 'a'.repeat(64)));
    await nothing('file_no_module_gets_nothing', module_.replace('shade.js', 'notes.txt'));
    await nothing('package_source_gets_nothing', module_.replace('ui/shade.js', '__init__.py'));
    await nothing('climb_gets_nothing', module_.replace('ui/shade.js', '../__init__.py'));
    await nothing('escaped_climb_gets_nothing', module_.replace('ui/shade.js', '%2e%2e%2f__init__.py'));
    const { host } = new URL(base);
    check('foreign_origin_refused', (await raw(base, module_, { Origin: 'http://evil.example' })).status === 403);
    check('foreign_host_refused', (await raw(base, module_, { Host: `rebind.example:${host.split(':')[1]}` })).status === 403);
    const noSession = await raw(base, `/v1/tools/${encodeURIComponent('./boxed/one_field')}/ui?project=${id}`);
    check('finding_one_needs_a_session', noSession.status === 401, noSession.status);
    const notNamed = await toolUi('./boxed/one_field', '');
    check('local_code_needs_its_project', notNamed.status === 404 && notNamed.body.error.code === 'tool_ui_not_installed', notNamed);

    // A hash nothing holds, through the page's own import: the box stays the declared one, and only a real answer draws it again.
    const faked = (url) => decodeURIComponent(url.pathname) === '/v1/tools/./boxed/one_field/ui';
    await page.route(faked, (route) => route.fulfill({ json: { module: module_.replace(hash, 'b'.repeat(64)) } }));
    await page.reload();
    await page.waitForSelector('.react-flow__node');
    await fit();
    await more('shaded');
    await box('shaded').locator('.tool-ui-failed').waitFor();
    check('hash_nothing_holds_does_not_load', (await box('shaded').locator('[data-boxed]').count()) === 0 && (await box('shaded').locator('input[type=range]').count()) === 1);
    await page.unroute(faked);

    // An edit is another hash: the old one still names the old bytes, and never the new.
    writeFileSync(join(boxed, 'e2e_boxed', 'ui', 'steps.js'), `${readFileSync(join(boxed, 'e2e_boxed', 'ui', 'steps.js'), 'utf8')}// Edited.\n`);
    primed();
    const edited = await toolUi('./boxed/one_field');
    const [, again] = edited.body.module.match(/\/ui\/([0-9a-f]{64})\//);
    check('an_edit_is_another_hash', again !== hash, edited);
    const oldSteps = await raw(base, module_.replace('shade.js', 'steps.js'));
    const newSteps = await raw(base, edited.body.module.replace('shade.js', 'steps.js'));
    check('each_hash_names_its_bytes', !oldSteps.body.includes('Edited') && newSteps.body.includes('Edited'), { old: oldSteps.body, new: newSteps.body });
  }

  // A release, through a Hub this cell stands up.
  seen.release_checked = Boolean(local && process.env.HUB_RESTART && process.env.RESTART);
  if (seen.release_checked) {
    const name = 'e2e-boxed';
    const package_ = `e2e/${name}`;
    // Described by content, as any client sends local code (cl-819): its files, then its tree, each PUT by its digest.
    const described = (await api(`/v1/targets?tree=${encodeURIComponent(await sentTree(base, token, join(folder, 'boxed')))}`)).body?.interface;
    check('interface_names_its_ui', described?.entrypoints?.some((entry) => entry.tool?.ui === 'ui/shade.js'), described?.entrypoints?.map((entry) => entry.tool?.ui));
    // Locked and built beside the local code, not in it: what a publish does, with a real wheel's real layout.
    const dist = mkdtempSync(join(tmpdir(), 'package-ui-'));
    const dir = join(dist, 'src');
    cpSync(join(__dirname, '..', 'project', 'boxed'), dir, { recursive: true });
    const uvs = join(homedir(), '.cozy', 'tools', 'uv');
    const uvAt = join(uvs, readdirSync(uvs)[0], 'uv');
    execFileSync(uvAt, ['lock', '--no-progress'], { ...quiet, encoding: 'utf8', cwd: dir });
    execFileSync(uvAt, ['build', '--wheel', '--no-progress', '--out-dir', dist], { ...quiet, cwd: dir });
    const wheelName = readdirSync(dist).find((file) => file.endsWith('.whl'));
    const hub = await standIn(name, '0.1.0', wheelName, readFileSync(join(dist, wheelName)), readFileSync(join(dir, 'pyproject.toml'), 'utf8'), readFileSync(join(dir, 'uv.lock'), 'utf8'), described);
    seen.hub = { origin: hub.origin, asked: hub.asked };
    await shell(process.env.HUB_RESTART.replaceAll('@HUB@', hub.origin)).catch(() => {});
    await until('Calcifer back on that Hub', () => api('/v1/status').catch(() => null), Boolean);
    // Whatever happens, Calcifer goes back as it was: the suite goes on with it.
    const env = { ...process.env, COZY_HUB_URL: hub.origin };
    try {
      // Not installed: on the Hub's list with its `ui`, and it doesn't load.
      const released = `${package_}/one_field`;
      const listed = await until('the release listed', async () => {
        const { tools, note } = (await api(`/v1/tools?on=local&q=${name}`)).body;
        return { tool: tools.find((tool) => tool.id === released), note };
      }, (now) => now.tool);
      check('release_lists_its_ui', listed.tool.ui === 'ui/shade.js' && !listed.tool.local, listed);
      // At rest, a default by its card with nothing installed, and the rest only by searching. With no Hub, the card kept here stands, and one never read says why.
      const resting = (await api('/v1/tools?on=local')).body;
      const isDefault = (tool) => tool.package === 'minimax-h3';
      check('a_default_is_listed', resting.tools.some(isDefault), resting.tools.map((tool) => tool.id));
      check('the_rest_is_found_by_searching', !resting.tools.some((tool) => tool.id === released), resting.tools.map((tool) => tool.id));
      hub.down = true;
      const noHub = (await api('/v1/tools?on=local')).body;
      hub.down = false;
      check('a_default_outlasts_its_hub', noHub.tools.some(isDefault), noHub.tools.map((tool) => tool.id));
      check('a_default_never_read_says_why', noHub.note?.startsWith("only what's installed"), noHub);
      await page.reload();
      await page.waitForSelector('.react-flow__node');
      await dropTool('from_the_hub', released);
      await fit();
      const before = await toolUi(released);
      check('not_installed_does_not_load', before.status === 404 && before.body.error.code === 'tool_ui_not_installed', before);
      // Browse: a package of the Hub, on your list with nothing installed, there after a reload, and off it again. A box of it goes on.
      const listRow = page.locator(`.tools [data-tool="${released}"]`);
      await away();
      await page.locator('.tools button:has-text("Browse")').click();
      const found = page.locator('.browse .browsed', { hasText: name });
      await found.waitFor();
      check('browse_says_what_it_does', (await found.innerText()).includes('One field, shaded.'), await found.innerText());
      check('browse_has_a_first_page', (await page.locator('.browse .browsed').count()) === 50);
      await page.locator('.browse button:has-text("More")').click();
      await page.locator('.browse .browsed', { hasText: 'shelf-59' }).waitFor();
      check('browse_reads_on', (await page.locator('.browse .browsed').count()) === 61 && (await page.locator('.browse button:has-text("More")').count()) === 0);
      await page.locator('.tools input[aria-label="Search the tools"]').fill('shelf-59');
      await until('browse searched', () => page.locator('.browse .browsed').count(), (rows) => rows === 1);
      await shot('browse');
      await page.locator('.tools input[aria-label="Search the tools"]').fill('');
      await found.waitFor();
      await found.locator('button:has-text("Add")').click();
      await page.locator('.tools button:has-text("Your tools")').click();
      await listRow.waitFor();
      await page.reload();
      await listRow.waitFor();
      check('added_is_there_after_a_reload', true);
      const unheldStill = await toolUi(released);
      check('adding_installs_nothing', unheldStill.status === 404 && unheldStill.body.error.code === 'tool_ui_not_installed' && !hub.asked.some((line) => line.includes('/download') || line.includes('/wheels/')), { unheldStill, asked: hub.asked });
      await shot('your-list');
      await away();
      await listRow.hover();
      await listRow.locator('.tool-off').click();
      await listRow.waitFor({ state: 'detached' });
      await until('off the list on the wire', async () => (await api('/v1/tools?on=local')).body.tools.some((tool) => tool.id === released), (held) => !held);
      await page.reload();
      await page.waitForSelector('.tools .tool');
      check('removed_stays_off', (await listRow.count()) === 0);
      const offWire = (await api('/v1/tools/list', { method: 'POST', body: JSON.stringify({ remove: ['minimax-h3/one_field'] }) })).body;
      check('a_default_comes_off', offWire.without?.includes('minimax-h3/one_field'), offWire);
      check('a_default_is_off', !(await api('/v1/tools?on=local')).body.tools.some((tool) => tool.id === 'minimax-h3/one_field'));
      await api('/v1/tools/list', { method: 'POST', body: JSON.stringify({ add: 'minimax-h3' }) });
      check('a_default_comes_back', (await api('/v1/tools?on=local')).body.tools.some((tool) => tool.id === 'minimax-h3/one_field'));
      await more('from_the_hub');
      await page.waitForTimeout(1000);
      check('a_box_of_a_removed_tool_opens', (await box('from_the_hub').locator('input[type=range]').count()) === 1, await box('from_the_hub').innerText());
      check('not_installed_is_declared_and_quiet', (await box('from_the_hub').locator('[data-boxed]').count()) === 0 && (await box('from_the_hub').locator('.tool-ui-failed').count()) === 0 && (await box('from_the_hub').locator('input[type=range]').count()) === 1, { said: await box('from_the_hub').innerText(), boxes: await boxLog(page) });
      check('not_installed_read_no_wheel', !hub.asked.some((line) => line.includes('/wheels/')), hub.asked);

      // Installed: drawn, from the release, named by the digest of its wheel.
      // A Hub that takes the request and never answers: the install says so, rather than waiting with nothing in any log.
      // The one retry rule (cl-840), on an install a person waits on. Silence may end: asked again, each attempt a line, and given up
      // within about a minute in all. A 503 may recover: asked again by the backoff, then one line naming the Hub's own words. A 404 is
      // the Hub's refusal: failed at once, never asked again.
      const install = (name) => {
        const began = Date.now();
        return cozyHere(['--json', 'package', 'install', name, '--on', 'local'], { ...quiet, env, cwd: folder })
          .then(() => 'it installed', (error) => error.said).then((said) => ({ said, took: Date.now() - began }));
      };
      hub.silent = true;
      const unanswered = await install(package_);
      hub.silent = false;
      check('a_silent_hub_says_so', unanswered.said.includes('hub_slow'), unanswered);
      check('a_silent_hub_is_asked_again_within_a_minute', /attempt 2\/5 in \d/.test(unanswered.said) && !unanswered.said.includes('attempt 4/5') && unanswered.took < 100_000, unanswered);
      hub.down = true;
      const down = await install(package_);
      hub.down = false;
      check('a_503_is_asked_again_then_named', /attempt 5\/5 in \d/.test(down.said) && down.said.includes('this Hub is down') && down.took < 100_000, down);
      const asked = hub.asked.length;
      const absent = await install('e2e/never-published');
      check('a_404_fails_at_once', absent.said !== 'it installed' && !absent.said.includes('attempt') && new Set(hub.asked.slice(asked).filter((line) => line.includes('never-published'))).size === hub.asked.slice(asked).filter((line) => line.includes('never-published')).length && absent.took < 20_000, { ...absent, asks: hub.asked.slice(asked) });
      await cozyHere(['--json', 'package', 'install', package_, '--on', 'local'], { ...quiet, env, cwd: folder });
      const wire = await until('its component', () => toolUi(released), (answer) => answer.status !== 404, FIRST_RUN_MS);
      check('installed_loads', wire.status === 200 && wire.body.module.includes(`/ui/${hub.digest}/`), { wire, digest: hub.digest });
      await page.reload();
      await page.waitForSelector('.react-flow__node');
      await fit();
      await more('from_the_hub');
      await box('from_the_hub').locator('[data-boxed=shade]').waitFor();
      check('release_draws', true);
      await shot('release');

      // A Hub whose file isn't what it says: what isn't held yet doesn't load, and is never served.
      hub.lying = true;
      const whole = `${package_}/whole_face`;
      const lied = await toolUi(whole);
      check('hash_mismatch_refused', lied.status === 503 && lied.body.error.code === 'tool_ui_hash_mismatch', lied);
      const unheld = await raw(base, `/v1/tools/${encodeURIComponent(whole)}/ui/${hub.digest}/ui/whole.js`);
      check('hash_mismatch_serves_nothing', unheld.status === 404, unheld.status);
      await dropTool('lied_about', whole);
      await page.reload();
      await page.waitForSelector('.react-flow__node');
      await fit();
      await more('lied_about');
      await box('lied_about').locator('.tool-ui-failed').waitFor();
      check('hash_mismatch_is_declared', (await box('lied_about').locator('[data-boxed]').count()) === 0 && (await box('lied_about').locator('input[type=range]').count()) === 1, await box('lied_about').innerText());
      await shot('mismatch');

      await cozyHere(['--json', 'package', 'uninstall', package_, '--on', 'local'], { ...quiet, env, cwd: folder });
    } finally {
      await shell(process.env.RESTART).catch(() => {});
      await until('Calcifer back as it was', () => api('/v1/status').catch(() => null), Boolean);
    }
    hub.server.close();
  }

  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  process.exit(2);
}
