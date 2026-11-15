// cl-980 (Paul, 11-13): `/` is the project page with no project open: every project of Calcifer's index in the strip along the top, most
// recently opened first, each opening its own; an empty canvas that says, in a line, to pick one or press +; and the + makes a project in
// place, through the one create `cozy new` uses (`POST /v1/projects {path}`): its folder suggested as `<last project's parent>/untitled-N`
// and edited where it is, encrypted unless asked not to, its recovery secret shown once, then opened. A folder with anything in it is said
// in one line beside the field. Going from one project to another never blanks the screen: the frame stays, and some canvas is drawn in
// every frame. Screenshots at 1280, 1600 and 2560 wide, paper and dark. The gate a page Calcifer refuses draws is access.js's, with a real
// second OS account: a browser of this one is always Calcifer's own page (cl-985).
//   node home.js: opened by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where its screenshots go. Needs the project
// encryption driver (e2e/cli/install-vault-driver.sh). Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } = require('fs');
const { dirname, join } = require('path');
const { addBoxes, calcifer, launch, lastLook, lookAt, newProject, noStall, projectFolder, removeGeneratedProject, signIn, STEP_MS, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok === true ? true : (detail ?? false);
  if (ok !== true) failures.push(name);
}
const HOME_LINE = 'No project open. Pick one above, or press +.';
const PASSPHRASE = 'synthetic home passphrase';
const holder = () => JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
const index = async () => {
  const { address, token } = holder();
  return (await (await fetch(`http://${address}/v1/projects?limit=1000`, { headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } })).json()).projects;
};
const vaultOf = (folder, ...words) => JSON.parse(execFileSync(process.env.COZY, ['--json', 'vault', ...words], { cwd: folder, encoding: 'utf8', timeout: 180000 }));
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const CODE = /\/p\/([0-9A-HJKMNP-TV-Z]{9})$/;

(async () => {
  const base = calcifer();
  const stamp = Date.now();
  const folders = { plain: projectFolder(`web-home-${stamp}-plain`), enc: projectFolder(`web-home-${stamp}-enc`), full: projectFolder(`web-home-${stamp}-full`) };
  const a = await newProject(projectFolder(`web-home-${stamp}-a`));
  const b = await newProject(projectFolder(`web-home-${stamp}-b`));
  addBoxes(b.path, [{ name: 'only_b', fields: { text: 'project b' }, at: { x: 120, y: 120 } }]);
  mkdirSync(folders.full, { recursive: true });
  writeFileSync(join(folders.full, 'notes.txt'), 'already here\n');
  seen.project = { id: a.id, folder: a.path };
  const browser = await launch(playwright);
  const errors = [];
  const open = async (width, height, theme = 'paper') => {
    const context = await browser.newContext({ viewport: { width, height } });
    await context.addInitScript((theme) => { try { localStorage.setItem('cozy.theme', theme); } catch {} }, theme);
    const page = await context.newPage();
    page.setDefaultTimeout(STEP_MS);
    page.on('pageerror', (error) => errors.push(error.message));
    lookAt(page);
    return page;
  };
  const shot = (page, name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());
  const plus = (page) => page.getByRole('button', { name: 'New project', exact: true });
  const panel = (page) => page.getByRole('region', { name: 'New project', exact: true });
  const folderField = (page) => panel(page).getByLabel("New project's folder", { exact: true });
  const askFor = async (page, folder, { encrypted }) => {
    await plus(page).click();
    await folderField(page).fill(folder);
    if (encrypted) {
      await panel(page).getByLabel('Passphrase', { exact: true }).fill(PASSPHRASE);
      await panel(page).getByLabel('Confirm passphrase', { exact: true }).fill(PASSPHRASE);
    } else {
      await panel(page).getByLabel('Create without encryption', { exact: true }).check();
    }
    await panel(page).getByRole('button', { name: 'Create project', exact: true }).click();
  };

  // `/`: the strip of every project, most recently opened first, over an empty canvas that says what to do.
  const page = await open(1600, 1000);
  await signIn(page);
  await page.locator('.home-line').waitFor();
  check('home_line', (await page.locator('.home-line').innerText()) === HOME_LINE, await page.locator('.home-line').innerText());
  const tabs = await page.locator('nav.ptabs a.ptab').evaluateAll((links) => links.map((link) => ({ name: link.textContent, href: link.getAttribute('href') })));
  const at = (project) => tabs.findIndex((tab) => tab.href === `/p/${project.code}`);
  check('picker_lists_both', at(a) >= 0 && at(b) >= 0, tabs);
  // Most recently opened first: the strip in the index's order, and the index by when each was last opened.
  const listed = await index();
  const rank = (project) => listed.findIndex((row) => row.code === project.code);
  check('index_newest_first', listed.every((row, i) => i === 0 || listed[i - 1].used_at_ms >= row.used_at_ms), listed.map((row) => [row.name, row.used_at_ms]));
  check('picker_newest_first', at(a) >= 0 && at(b) >= 0 && (at(a) < at(b)) === (rank(a) < rank(b)), { tabs, listed: listed.map((row) => row.code) });
  check('home_stays', new URL(page.url()).pathname === '/');
  await shot(page, 'home');

  // A tab opens its project.
  await page.click(`nav.ptabs a.ptab[href="/p/${a.code}"]`);
  await page.waitForURL(`${base}/p/${a.code}`);
  await page.locator('.react-flow__node').first().waitFor();
  check('picker_opens', (await page.locator('main.gate, .unread').count()) === 0);

  // From one project to another: the frame stays, and every frame draws a canvas, never a blank screen.
  await page.evaluate(() => {
    window.__frames = [];
    window.__sampling = true;
    const look = () => {
      window.__frames.push({ top: Boolean(document.querySelector('header.top')), canvas: Boolean(document.querySelector('.react-flow')), gate: Boolean(document.querySelector('main.gate')) });
      if (window.__sampling) requestAnimationFrame(look);
    };
    requestAnimationFrame(look);
  });
  await page.click(`nav.ptabs a.ptab[href="/p/${b.code}"]`);
  await page.locator('.card[data-asset="only_b"]').waitFor({ state: 'attached' });
  await until('the next project checked', () => page.locator('main.gate').count(), (count) => count === 0);
  const frames = await page.evaluate(() => { window.__sampling = false; return window.__frames; });
  const blank = frames.filter((frame) => !frame.top || !frame.canvas || frame.gate);
  seen.switch_frames = frames.length;
  check('switch_never_blank', frames.length > 0 && blank.length === 0, { frames: frames.length, blank: blank.slice(0, 5) });

  // +: the form in place at once, its folder suggested by the most recently opened project, and the tab it'll be shown in the strip.
  await plus(page).click();
  await folderField(page).waitFor();
  const latest = (await index())[0];
  const suggested = await folderField(page).inputValue();
  check('folder_suggested', new RegExp(`^${escape(dirname(latest.path))}[\\\\/]untitled-\\d+$`).test(suggested), { suggested, latest: latest.path });
  check('plus_at_home', new URL(page.url()).pathname === '/');
  const pending = await page.locator('nav.ptabs span.ptab.on').innerText().catch(() => null);
  check('new_tab_shows_at_once', pending === suggested.split(/[\\/]/).pop(), pending);
  check('encrypted_by_default', !(await panel(page).getByLabel('Create without encryption', { exact: true }).isChecked()));
  await shot(page, 'new');
  // The Projects drawer makes one the same way.
  await page.goto(`${base}/projects`);
  const drawerFolder = await until('the drawer suggests a folder', () => page.locator('.drawer input[aria-label="New project\'s folder"]').inputValue(), Boolean);
  check('drawer_same_form', drawerFolder === suggested, { drawerFolder, suggested });
  await page.goto(`${base}/`);
  await page.locator('.home-line').waitFor();

  // A folder with anything in it: one line beside the field, and nothing else.
  await askFor(page, folders.full, { encrypted: true });
  const beside = panel(page).locator('.said-beside .beside');
  await beside.waitFor();
  const said = await beside.innerText();
  check('not_empty_said_beside', /isn't empty/.test(said) && said.includes(realpathSync(folders.full)), said);
  check('not_empty_no_dialog', (await page.locator('[role=dialog], [role=alertdialog], dialog[open]').count()) === 0 && (await panel(page).count()) === 1 && new URL(page.url()).pathname === '/');
  check('not_empty_untouched', readFileSync(join(folders.full, 'notes.txt'), 'utf8') === 'already here\n' && !existsSync(join(folders.full, 'project.db')));
  await shot(page, 'not-empty');
  await page.keyboard.press('Escape');
  await page.locator('.home-line').waitFor();

  // + without encryption: made, and opened.
  await askFor(page, folders.plain, { encrypted: false });
  await page.waitForURL(CODE);
  await page.locator('.react-flow').waitFor();
  const plainCode = CODE.exec(page.url())[1];
  const plainRow = (await index()).find((row) => row.code === plainCode);
  check('plus_unencrypted_opens', Boolean(plainRow) && plainRow.path === realpathSync(folders.plain) && existsSync(join(folders.plain, 'project.db')) && (await page.locator('main.gate, .unread').count()) === 0, plainRow);

  // + encrypted: the recovery secret once, saved, then opened.
  await askFor(page, folders.enc, { encrypted: true });
  const recovery = page.locator('[aria-label="Save recovery secret"]');
  await recovery.locator('pre').waitFor();
  check('recovery_says_save_it', (await recovery.innerText()).includes('Save this somewhere safe'));
  await shot(page, 'recovery');
  await page.getByLabel('I saved this somewhere away from this computer').check();
  await recovery.getByRole('button', { name: 'Done', exact: true }).click();
  await page.waitForURL(CODE);
  await page.locator('.react-flow').waitFor();
  await until('the encrypted project readable', () => page.locator('main.gate').count(), (count) => count === 0);
  check('plus_encrypted_opens', !existsSync(join(folders.enc, 'project.db')) && vaultOf(folders.enc, 'status').encrypted === true);
  check('recovery_shown_once', (await page.locator('[aria-label="Save recovery secret"]').count()) === 0);
  await page.context().close();

  // How it all looks, at each width, on paper and dark: the home, the form, the line beside the field.
  for (const [width, height] of [[1280, 800], [1600, 1000], [2560, 1440]]) {
    for (const theme of ['paper', 'dark']) {
      const look = await open(width, height, theme);
      await signIn(look);
      await look.locator('.home-line').waitFor();
      await shot(look, `home-${width}-${theme}`);
      await plus(look).click();
      await folderField(look).waitFor();
      await shot(look, `new-${width}-${theme}`);
      await folderField(look).fill(folders.full);
      await panel(look).getByLabel('Passphrase', { exact: true }).fill(PASSPHRASE);
      await panel(look).getByLabel('Confirm passphrase', { exact: true }).fill(PASSPHRASE);
      await panel(look).getByRole('button', { name: 'Create project', exact: true }).click();
      await panel(look).locator('.said-beside .beside').waitFor();
      await shot(look, `not-empty-${width}-${theme}`);
      await look.context().close();
    }
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  vaultOf(folders.enc, 'lock');
  await until('the encrypted project locked', () => vaultOf(folders.enc, 'status').state, (state) => state === 'locked');
  for (const folder of [a.path, b.path, folders.plain, folders.enc, folders.full]) await removeGeneratedProject(folder, 'web-home-');
  console.log(JSON.stringify({ ok: !failures.length, failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  seen.died = String(error?.stack ?? error);
  await lastLook();
  console.log(JSON.stringify({ ok: false, failures: [...failures, 'died'], seen }, null, 2));
  process.exit(2);
}
