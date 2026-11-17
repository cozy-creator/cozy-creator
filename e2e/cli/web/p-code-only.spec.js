// p-code-only (cl-1171). Paul, 2026-11-17: "http://127.0.0.1:8818/p/29NZZ3R45 — these are used universally right? like we only use these
// urls, and not the old hex-code urls … /projects/<hex-code>? I want to ensure that those old ones are completely gone and not referenced or
// used anywhere in the codebase rather than merely hidden and sitting latent somewhere". And 11-16, of `/projects/<id>`: "where did this
// dumb other url come from?"
// A project's only address is `/p/<code>`. `cozy open` prints it. Every link the page draws is one of a listed project's: the tabs, the
// Projects list, its assets, its runs, a run's drawer. Every place the page goes is one too: a take's landed note's View, an unlock.
// `/projects/<id>` and `/projects/<id>/assets/<name>` answer 404 as any unknown path does; the Projects drawer at `/projects` stays.
// `/p/` takes a code alone: `/p/<id>` opens nothing.
// A locked encrypted project is listed by its id and code, never a `locked-` handle. Its tab goes to `/p/<code>` and it unlocks there.
// So does one whose keybag holds no code (locked before codes were kept there), by the code derived from its id, which its unlock keeps.
// Each look at 1440×900 and 1920×1080, paper and dark, is kept to look at: the tabs and the list with it locked, its lock screen, it unlocked.
// Solo: the tabs are every project's on this Calcifer, so a cell beside it would lead them.
const { execFile } = require('child_process');
const { randomBytes, randomUUID } = require('crypto');
const { appendFileSync, mkdirSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, fixtureTool, png } = require('./fixtures');

test.use({ projectOptions: { encrypted: true } });

const STILL = `${fixtureTool('tool.py')}/still`;
const LOOKS = [
  { width: 1440, height: 900, theme: 'paper' },
  { width: 1440, height: 900, theme: 'dark' },
  { width: 1920, height: 1080, theme: 'paper' },
  { width: 1920, height: 1080, theme: 'dark' },
];

/** `cozy <words>` in `cwd`, as a person's shell runs it there: {code, stdout, stderr}. */
const cozyIn = (cwd, ...words) => new Promise((done) => {
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  execFile(process.env.COZY, words, { cwd, timeout: 120_000 }, (error, stdout, stderr) => done({ code: error ? error.code ?? 1 : 0, stdout, stderr }));
});

test('p-code-only', { tag: '@solo' }, async ({ page, project, api, cli, removeAfter, guards }) => {
  test.setTimeout(300_000);
  const shots = join(process.env.OUT, 'web', 'p-code-only');
  mkdirSync(shots, { recursive: true });
  const looks = (theme) => api.call('PUT', '/v1/appearance', { theme });
  const tabs = page.getByRole('navigation', { name: 'Projects' });
  const drawer = page.locator('aside.drawer');
  const vault = async () => (await api.get(project.at('/vault'))).state;
  /** Every code the index has listed in this cell: one changes as the cell goes (its last step). */
  const listed = new Set();
  const listing = async () => {
    const rows = (await api.get('/v1/projects?limit=1000')).projects;
    for (const row of rows) listed.add(row.code);
    return rows;
  };
  const folder = join(process.env.OUT, 'web', 'projects', `web-p-code-only-${randomBytes(3).toString('hex')}`);
  removeAfter(folder);
  const plain = await api.post('/v1/projects', { path: folder, unencrypted: true });
  // Refused on purpose: what the page asks of a locked project before it has read that it's locked.
  guards.allow(new RegExp(`status of 409\\b.*/v1/projects/${project.id}\\b`));

  /** Why a path the page drew or went to isn't a project's only address: a `/projects/<…>` one, or a `/p/<x>` no listed project has. */
  const wrong = (path, codes) => {
    if (/^\/projects\/./.test(path)) return `${path}: a /projects/<id> address`;
    const code = path.match(/^\/p\/([^/]+)/)?.[1];
    return code !== undefined && !codes.has(decodeURIComponent(code)) ? `${path}: no listed project's code` : null;
  };
  /** Every place the page went, as its address bar said it. */
  const went = [];
  page.on('framenavigated', (frame) => { if (frame === page.mainFrame()) went.push(new URL(frame.url()).pathname); });
  /** Every link drawn now, each a listed project's `/p/<code>` where it names a project: the links, by path. */
  const linksAt = async (where) => {
    const codes = new Set((await listing()).map((row) => row.code));
    expect.soft(codes.has(''), `${where}: every listed project has a code`).toBe(false);
    const hrefs = await page.locator('a[href]').evaluateAll((all) => all.map((link) => link.getAttribute('href')));
    const paths = hrefs.map((href) => new URL(href, page.url()));
    expect.soft(paths.filter((url) => url.origin !== new URL(page.url()).origin).map(String), `${where}: every link this page's own`).toEqual([]);
    expect.soft(paths.map((url) => wrong(url.pathname, codes)).filter(Boolean), `${where}: every project link a listed project's /p/<code>`).toEqual([]);
    return paths.map((url) => url.pathname);
  };
  const shoot = async (name, { width, height, theme }) => {
    await page.setViewportSize({ width, height });
    await themed(page, theme);
    await page.screenshot({ path: join(shots, `${name}-${width}x${height}-${theme}.png`) });
  };

  // The encrypted project is locked before its folder goes, whatever happened: a mounted view keeps it.
  try {
    await test.step('cozy open prints /p/<code>, for a plain project and an encrypted one', async () => {
      for (const [one, cwd] of [[plain, plain.path], [project, project.path]]) {
        const said = await cozyIn(cwd, '--json', 'open');
        expect.soft(said.code, `cozy --json open in ${cwd}: ${said.stderr.slice(-300)}`).toBe(0);
        const url = said.code === 0 ? new URL(JSON.parse(said.stdout).url) : null;
        expect.soft(url && `${url.pathname}${url.search}${url.hash}`, `cozy open in ${one.name}`).toBe(`/p/${one.code}`);
      }
    });

    await test.step('/projects/<id> answers 404 as any unknown path does; the Projects drawer and /p/<code> answer the page; /p/<id> opens nothing', async () => {
      await project.import(png(120), { name: 'picture', at: { x: 400, y: 0 } });
      for (const path of [`/projects/${project.id}`, `/projects/${plain.id}`, `/projects/${project.id}/assets/picture`, `/projects/${plain.id}/runs`, `/projects/${project.code}`]) {
        expect.soft((await api.raw('GET', path)).status, `GET ${path}`).toBe(404);
      }
      for (const path of ['/projects', `/p/${project.code}`, `/p/${project.code}/assets/picture`, `/p/${plain.code}/projects`]) {
        expect.soft((await api.raw('GET', path)).status, `GET ${path}`).toBe(200);
      }
      // `/p/` takes a code alone: a project's id there opens nothing and goes nowhere.
      const tab = await page.context().newPage();
      await tab.goto(`${api.origin}/p/${plain.id}`);
      await expect.soft(tab.getByText("There's no project with this code on this computer."), '/p/<id> is no address').toBeVisible({ timeout: 30_000 });
      await expect.soft(tab, 'and goes nowhere').toHaveURL(`${api.origin}/p/${plain.id}`);
      await tab.close();
    });

    await test.step('every link the page draws: the tabs, the Projects list, its assets, its runs, a run, a landed take', async () => {
      await project.add({ name: 'shot', tool: STILL, kind: 'image', fields: { instructions: 'a still' }, at: { x: 0, y: 0 } });
      await page.goto(`${api.origin}/p/${plain.code}`);
      await expect(page.locator('.stage')).toBeVisible();
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();

      const tabbed = await linksAt('the tabs');
      expect.soft(tabbed, 'a tab to each').toEqual(expect.arrayContaining([`/p/${project.code}`, `/p/${plain.code}`]));
      await page.goto(project.url('projects'));
      await expect(drawer.locator('ul.list a').first()).toBeVisible();
      expect.soft(await linksAt('the Projects list'), 'the list links each').toEqual(expect.arrayContaining([`/p/${project.code}`, `/p/${plain.code}`]));
      await page.goto(project.url('assets'));
      await expect(drawer.locator('a[href$="/assets/picture"]'), 'the picture is listed').toBeVisible({ timeout: 30_000 });
      expect.soft(await linksAt('the Assets list'), 'its picture at its address').toContain(`/p/${project.code}/assets/picture`);

      // The take lands where the page isn't looking at it: its note's View opens it at its address.
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();
      await api.call('POST', project.at('/runs'), { names: ['shot'], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
      const note = page.locator('[data-note]').filter({ hasText: /of shot is ready/ });
      await expect(note, 'the landed take is said').toBeVisible({ timeout: 180_000 });
      await note.getByRole('button', { name: 'View', exact: true }).click();
      await expect(page, 'View opens it at its address').toHaveURL(`${api.origin}/p/${project.code}/assets/shot`);
      await linksAt('a box opened');

      await page.goto(project.url('runs'));
      const run = drawer.locator('a[href*="/runs/"]').first();
      await expect(run, 'its run is listed').toBeVisible({ timeout: 30_000 });
      const runs = await linksAt('the Runs list');
      expect.soft(runs.filter((path) => path.includes('/runs/')).every((path) => path.startsWith(`/p/${project.code}/runs/`)), `its runs under its address: ${runs}`).toBe(true);
      await run.click();
      await expect(page).toHaveURL(new RegExp(`/p/${project.code}/runs/\\d+$`));
      await expect(drawer.locator('.drawer-head')).toBeVisible();
      await linksAt('a run');
    });

    await test.step('locked: listed by its id and code, its tab goes to /p/<code>, and it unlocks there', async () => {
      await page.goto(`${api.origin}/p/${plain.code}`);
      await expect(page.locator('.stage')).toBeVisible();
      const locked = await cli('--json', 'vault', '--directory', project.path, 'lock');
      expect(locked.code, `cozy vault lock: ${locked.stderr.slice(-300)}`).toBe(0);
      await expect.poll(vault, { message: 'locked', timeout: 60_000 }).toBe('locked');
      const rows = await listing();
      expect.soft(rows.filter((row) => row.id.startsWith('locked-')).map((row) => row.path), 'no row under a locked handle').toEqual([]);
      expect.soft(rows.find((row) => row.path === project.path), 'it is listed by its id and code').toMatchObject({ id: project.id, code: project.code });

      const tab = tabs.getByRole('link', { name: project.name, exact: true });
      await expect.poll(() => tab.getAttribute('href'), { message: 'its tab goes to its address' }).toBe(`/p/${project.code}`);
      await page.goto(`${api.origin}/p/${plain.code}/projects`);
      await expect(drawer.getByRole('link', { name: project.name, exact: true }), 'the list links it at its address').toHaveAttribute('href', `/p/${project.code}`);
      await linksAt('locked: the tabs and the list');
      for (const look of LOOKS) {
        await looks(look.theme);
        await shoot('list-locked', look);
      }
      await page.keyboard.press('Escape');
      await expect(drawer).toHaveCount(0);

      await tab.click();
      await expect(page, 'the tab opens it at its address').toHaveURL(`${api.origin}/p/${project.code}`);
      await expect(page.getByText('This project is locked.', { exact: true }), 'it says it is locked').toBeVisible({ timeout: 30_000 });
      await linksAt('locked: its lock screen');
      for (const look of LOOKS) {
        await looks(look.theme);
        await shoot('locked', look);
      }
      await page.getByLabel('Passphrase', { exact: true }).fill(project.passphrase);
      await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
      await expect.poll(vault, { message: 'Calcifer has it unlocked', timeout: 60_000 }).toBe('unlocked');
      await expect(page.locator('.stage'), 'its canvas shows').toBeVisible({ timeout: 30_000 });
      await expect(page, 'unlocked where it was').toHaveURL(`${api.origin}/p/${project.code}`);
      await expect(page.getByText(/couldn't open|can't be opened/i), 'never "can\'t be opened"').toHaveCount(0);
      await linksAt('unlocked');
      for (const look of LOOKS) {
        await looks(look.theme);
        await shoot('unlocked', look);
      }
      await looks('paper');
    });

    await test.step('locked with a keybag that holds no code: the code derived from its id, kept as it unlocks', async () => {
      await page.goto(`${api.origin}/p/${plain.code}`);
      await expect(page.locator('.stage')).toBeVisible();
      const locked = await cli('--json', 'vault', '--directory', project.path, 'lock');
      expect(locked.code, `cozy vault lock: ${locked.stderr.slice(-300)}`).toBe(0);
      await expect.poll(vault, { message: 'locked', timeout: 60_000 }).toBe('locked');
      // As a keybag from before codes were kept there (cl-984, 11-15): the code is no part of what its envelopes authenticate.
      const keys = join(project.path, 'project.vault.keys.json');
      const keybag = JSON.parse(readFileSync(keys, 'utf8'));
      expect(keybag.code, 'its keybag keeps its code').toBe(project.code);
      delete keybag.code;
      writeFileSync(keys, JSON.stringify(keybag, null, 2));

      const row = (await listing()).find((one) => one.path === project.path);
      expect(row?.id, 'listed by its id').toBe(project.id);
      expect(row?.code, 'and a code').toMatch(/^[0-9A-HJKMNP-TV-Z]{9}$/);
      const code = row.code;
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      const tab = tabs.getByRole('link', { name: project.name, exact: true });
      await expect.poll(() => tab.getAttribute('href'), { message: 'its tab goes to that code' }).toBe(`/p/${code}`);
      await tab.click();
      await expect(page.getByText('This project is locked.', { exact: true }), 'it says it is locked').toBeVisible({ timeout: 30_000 });
      await page.getByLabel('Passphrase', { exact: true }).fill(project.passphrase);
      await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
      await expect.poll(vault, { message: 'Calcifer has it unlocked', timeout: 60_000 }).toBe('unlocked');
      await expect(page.locator('.stage'), 'its canvas shows').toBeVisible({ timeout: 30_000 });
      await expect(page, 'unlocked where it was').toHaveURL(`${api.origin}/p/${code}`);
      expect.soft((await project.state()).code, 'its unlock keeps that code').toBe(code);
      expect.soft(JSON.parse(readFileSync(keys, 'utf8')).code, 'and its keybag keeps it').toBe(code);
      const said = await cli('--json', 'open');
      expect.soft(said.code === 0 && new URL(JSON.parse(said.stdout).url).pathname, 'cozy open prints it').toBe(`/p/${code}`);
      await linksAt('unlocked by its derived code');
    });

    await listing();
    expect.soft([...new Set(went)].map((path) => wrong(path, listed)).filter(Boolean), `every place the page went is a project's /p/<code>: ${[...new Set(went)]}`).toEqual([]);
  } finally {
    await looks('paper');
    if ((await vault()) !== 'locked') await cli('--json', 'vault', '--directory', project.path, 'lock');
  }
  guards.over();
});
