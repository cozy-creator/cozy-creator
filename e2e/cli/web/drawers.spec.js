// drawers (cl-1096). Paul, 11-16, on an encrypted project's unlock screen, clicking "30 more": "the ability of these menus to open or close
// should not depend upon the main screen, ideally". And: "When I push '+' that should open the new project modal. ... I think the list of
// projects should be sorted by most recently used (I think the projects in the top panel shoudl be most recently used as well)." Then: "rename
// 'files' to 'assets'", "rename library -> Installations", "'account' should be at the bottom ... in the hamburger menu", and of "Cozy updated
// to 0.26-dev" in the header: "make them temporary toast notifications in the bottom-left, like everything else".
// Projects opened in the page lead the index and the tabs, most recent first; the Projects list is a search and every project in that order,
// with no form. On the locked project, "N more" opens the list, Machines opens, and Runs and Assets open saying the project is locked; a
// machine's tab lists what it's working on, says the project is locked with no Prefer, and nothing asks the project and is refused (cl-1099). The
// menu says Installations and Assets, Account last. A new Cozy version reloads the page by itself, no note and nothing in the bar (#3673). And of the bar's +: "have the + (for
// new projects) connected to the project list": it's the strip's last item, after "N more", clear of every tab. Each at 1440×900 and
// 1920×1080, paper and dark, kept to look at: the bar with and without "N more", the lock screen with the list open and with a machine's
// list open, the list, the menu.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { randomBytes } = require('crypto');
const { test, expect, themed } = require('./fixtures');

test.use({ projectOptions: { encrypted: true } });

const LOOKS = [
  { width: 1440, height: 900, theme: 'paper' },
  { width: 1440, height: 900, theme: 'dark' },
  { width: 1920, height: 1080, theme: 'paper' },
  { width: 1920, height: 1080, theme: 'dark' },
];

test('drawers', { tag: '@vault' }, async ({ page, project, api, cli, removeAfter, signIn, newContext, guards }) => {
  const shots = join(process.env.OUT, 'web', 'drawers');
  mkdirSync(shots, { recursive: true });
  const tabs = page.getByRole('navigation', { name: 'Projects' });
  const drawer = page.locator('aside.drawer');
  const menu = page.locator('details.main-menu');
  const pick = async (name) => {
    if ((await menu.getAttribute('open')) === null) await menu.locator('summary').click();
    await menu.locator('nav').getByRole('link', { name, exact: true }).click();
  };
  const titled = (title) => drawer.locator('.drawer-head b', { hasText: title });
  /** Everywhere's appearance (cl-1097): a locked project has no other. Put back as Paper after. */
  const looks = (theme) => api.call('PUT', '/v1/appearance', { theme });
  const plus = tabs.getByRole('button', { name: 'New project', exact: true });
  /** The strip's + is its last item, and no tab reaches it. */
  const plusEnds = async (tag) => {
    const items = tabs.locator(':scope > *');
    expect.soft(await items.last().getAttribute('aria-label'), `${tag}: the + ends the strip`).toBe('New project');
    const at = await plus.boundingBox();
    expect.soft(at, `${tag}: the + in the project strip`).not.toBeNull();
    if (!at) return;
    const rights = await tabs.locator('a.ptab').evaluateAll((all) => all.map((tab) => tab.getBoundingClientRect().right));
    expect.soft(rights.every((right) => right <= at.x + 0.5), `${tag}: no tab over the + (${rights} vs ${at.x})`).toBe(true);
  };

  // The encrypted project is locked before its folder goes, whatever happened: a mounted view keeps it.
  try {
    await test.step('the bar: the + ends the project strip, without "N more"', async () => {
      for (const { width, height, theme } of LOOKS) {
        const tag = `${width}x${height}-${theme}`;
        await page.setViewportSize({ width, height });
        await project.look(theme);
        await page.reload();
        await expect(page.locator('.stage')).toBeVisible();
        await themed(page, theme);
        await plusEnds(tag);
        await page.screenshot({ path: join(shots, `header-${tag}.png`), clip: { x: 0, y: 0, width, height: 120 } });
      }
      await page.setViewportSize({ width: 1600, height: 1000 });
    });

    // Enough of them that the tabs don't all fit, made through the API (each made is used now).
    const extras = [];
    for (let i = 0; i < 12; i++) {
      const folder = join(process.env.OUT, 'web', 'projects', `web-drawers-${randomBytes(3).toString('hex')}-${i}`);
      removeAfter(folder);
      extras.push(await api.post('/v1/projects', { path: folder, unencrypted: true }));
    }
    const plain = extras[0];
    const mine = [project.path, ...extras.map((one) => one.path)];
    /** The index's order of this cell's projects, by folder (a locked one's row is a handle). */
    const indexed = async () => (await api.get('/v1/projects?limit=1000')).projects.map((row) => row.path).filter((path) => mine.includes(path));
    const nameOf = (path) => path.split(/[\\/]/).pop();

    await test.step('projects opened in the page lead the index and the tabs, most recent first', async () => {
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();
      // Shown, it's used: the page tells Calcifer what it shows just after it draws.
      await expect.poll(async () => (await indexed())[0], { message: 'opened in the page, it leads the index' }).toBe(project.path);
      await page.goto(`${api.origin}/p/${plain.code}`);
      await expect(page.locator('.stage')).toBeVisible();
      await expect.poll(async () => (await indexed()).slice(0, 2), { message: 'the index: the one opened last, then the one before' }).toEqual([plain.path, project.path]);
      const order = await indexed();
      expect.soft(order.slice(2), 'then the rest, last made first').toEqual(extras.slice(1).map((one) => one.path).reverse());
      const shown = (await tabs.locator('a.ptab:not(.more)').allInnerTexts()).filter((name) => mine.some((path) => nameOf(path) === name));
      expect.soft(shown.length > 1, `tabs of this cell's projects: ${shown}`).toBe(true);
      expect.soft(shown, 'the tabs in the same order').toEqual(order.map(nameOf).slice(0, shown.length));
      await expect.soft(tabs.locator('a.ptab.more'), 'the rest behind "N more"').toBeVisible();
      await plusEnds('with "N more"');
    });

    await test.step('the list: every project most recently used first, a search, no form', async () => {
      await tabs.locator('a.ptab.on').click();
      await expect(titled('Projects')).toBeVisible();
      await expect.soft(drawer.getByRole('textbox', { name: 'Search projects' }), 'a search').toBeVisible();
      await expect.soft(drawer.locator('form, input[aria-label="New project\'s folder"], input[aria-label="New project\'s name"]'), 'no creation form').toHaveCount(0);
      const listed = (await drawer.locator('ul.list a').allInnerTexts()).filter((name) => mine.some((path) => nameOf(path) === name));
      expect.soft(listed, 'in the index order').toEqual((await indexed()).map(nameOf));
      await page.keyboard.press('Escape');
      await expect(drawer).toHaveCount(0);
    });

    await test.step('locked: N more, Machines, Runs and Assets each open', async () => {
      // cl-1099: nothing on the page asks the locked project for what it holds; one refused (409) is said by name.
      const refused = [];
      const heard = (response) => { if (response.status() === 409) refused.push(`${response.request().method()} ${new URL(response.url()).pathname}`); };
      page.on('response', heard);
      const said = await cli('--json', 'vault', '--directory', project.path, 'lock');
      expect(said.code, `cozy vault lock: ${said.stderr.slice(-300)}`).toBe(0);
      await page.goto(project.url());
      await expect(page.getByRole('heading', { name: 'Encrypted project' })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Unlock project' })).toBeVisible();
      // Locked, it keeps its place at the top (its lock was a use too).
      expect.soft((await indexed()).slice(0, 2).sort(), 'the locked one keeps its place').toEqual([plain.path, project.path].sort());

      await tabs.locator('a.ptab.more').click();
      await expect(titled('Projects'), '"N more" opens the list').toBeVisible();
      await expect.soft(drawer.locator('ul.list a').first()).toBeVisible();
      await expect.soft(page.getByRole('button', { name: 'Unlock project' }), 'the unlock beside it').toBeVisible();
      await page.keyboard.press('Escape');
      await expect(drawer, 'Esc closes it').toHaveCount(0);

      await pick('Machines');
      await expect.soft(titled('Machines'), 'Machines opens').toBeVisible();
      for (const [item, title] of [['Runs', 'Runs'], ['Assets', 'Assets']]) {
        await pick(item);
        await expect.soft(titled(title), `${item} opens`).toBeVisible();
        await expect.soft(drawer.locator('.drawer-body'), `${item} says it's locked, and nothing else`).toHaveText('This project is locked. Unlock it to see this.');
      }
      await page.keyboard.press('Escape');
      await expect(drawer).toHaveCount(0);

      // cl-1099: a machine's tab lists what it's working on, and of the locked project says only how to render there.
      await page.getByRole('navigation', { name: 'Machines', exact: true }).locator('.mtab[data-machine="local"]').click();
      const working = page.getByRole('dialog', { name: /^What .+ is doing$/ });
      await expect(working, 'a click on local lists what it is doing').toBeVisible();
      await expect.soft(working.locator('footer'), 'it says the project is locked').toContainText('This project is locked. Unlock it to render here.');
      await expect.soft(working.getByRole('checkbox'), 'with no Prefer to press').toHaveCount(0);
      await page.keyboard.press('Escape');
      await expect(working).toHaveCount(0);
      page.off('response', heard);
      expect.soft(refused, 'nothing asked of the locked project and refused').toEqual([]);
    });

    await test.step('the menu: Installations and Assets, Account last', async () => {
      await page.goto(`${api.origin}/p/${plain.code}`);
      await expect(page.locator('.stage')).toBeVisible();
      await menu.locator('summary').click();
      const items = (await menu.locator('nav > *').allInnerTexts()).map((text) => text.trim()).filter(Boolean);
      expect.soft(items.slice(-2), `the menu: ${items}`).toEqual(['Account', 'Settings']);
      expect.soft(items, 'Installations, not Library').toContain('Installations');
      expect.soft(items, 'Assets, not Files').toContain('Assets');
      expect.soft(items).not.toContain('Library');
      expect.soft(items).not.toContain('Files');
      await pick('Assets');
      await expect(titled('Assets'), 'the Assets drawer').toBeVisible();
      expect.soft(new URL(page.url()).pathname, 'at assets').toBe(`/p/${plain.code}/assets`);
      await pick('Installations');
      await expect(titled('Installations'), 'the Installations drawer').toBeVisible();
      await page.keyboard.press('Escape');
    });

    await test.step('a new Cozy version reloads the page by itself: no note, nothing in the bar (#3673)', async () => {
      // A browser of its own: tabs of one browser share one event stream (its leader's), and this page's must hear it itself. Calcifer
      // back as another build is its stream's hello saying another version, heard once the page connects again (as web/update's cell
      // stages it, which proves when it may reload). Paul, 11-17: "remove this notification ... auto-refresh the webui".
      const other = await (await newContext()).newPage();
      await other.addInitScript(() => {
        const real = window.fetch.bind(window);
        window.fetch = async (input, init = {}) => {
          const url = new URL(input instanceof Request ? input.url : String(input), location.href);
          if (url.pathname !== '/v1/events') return real(input, init);
          const dropped = new AbortController();
          window.__dropStream = () => { dropped.abort(new DOMException('Calcifer stopped', 'AbortError')); return true; };
          const response = await real(input, { ...init, signal: init.signal ? AbortSignal.any([init.signal, dropped.signal]) : dropped.signal });
          if (!response.body) return response;
          const reader = response.body.getReader();
          const [decoder, encoder] = [new TextDecoder(), new TextEncoder()];
          const body = new ReadableStream({
            async pull(stream) {
              const { done, value } = await reader.read();
              if (done) return stream.close();
              const text = decoder.decode(value, { stream: true });
              stream.enqueue(encoder.encode(window.__calciferVersion ? text.replace(/("type":"hello"[^\n]*?"version":")[^"]*"/, `$1${window.__calciferVersion}"`) : text));
            },
            cancel(reason) { return reader.cancel(reason); },
          });
          return new Response(body, { status: response.status, statusText: response.statusText, headers: response.headers });
        };
      });
      // Every note it shows and anything in the bar, kept across the reload.
      const seen = [];
      let inBar = false;
      await other.exposeFunction('__cozySaw', (note, bar) => { if (note && !seen.includes(note)) seen.push(note); inBar ||= bar; });
      await other.addInitScript(() => {
        new MutationObserver(() => {
          for (const note of document.querySelectorAll('.notes .note')) window.__cozySaw?.(note.querySelector('.line')?.textContent ?? '', false);
          if (/updated|Reload/.test(document.querySelector('header.top')?.textContent ?? '')) window.__cozySaw?.('', true);
        }).observe(document, { subtree: true, childList: true, characterData: true });
      });
      await signIn(other, `${api.origin}/p/${plain.code}`);
      await expect(other.locator('.stage')).toBeVisible();
      await other.evaluate(() => { window.__notReloaded = true; });
      expect(await other.evaluate(() => { window.__calciferVersion = '9.9.9-newer'; return window.__dropStream?.() ?? false; }), 'its stream ended').toBe(true);
      await expect.poll(() => other.evaluate(() => !window.__notReloaded).catch(() => false), { message: 'it reloads by itself', timeout: 15_000 }).toBe(true);
      await expect(other.locator('.stage')).toBeVisible();
      expect.soft(new URL(other.url()).pathname, 'and stays where it was').toBe(`/p/${plain.code}`);
      expect.soft(seen.filter((said) => /updated|Reload/i.test(said)), 'no note of it').toEqual([]);
      expect.soft(inBar, 'nothing in the bar').toBe(false);
      await other.context().close();
    });

    await test.step('screenshots: the lock screen with the list open, the list, the tabs, the menu', async () => {
      for (const { width, height, theme } of LOOKS) {
        const tag = `${width}x${height}-${theme}`;
        await page.setViewportSize({ width, height });
        await looks(theme);
        await page.goto(`${project.url()}/projects`);
        await expect(titled('Projects')).toBeVisible();
        await expect(page.getByRole('button', { name: 'Unlock project' })).toBeVisible();
        await page.screenshot({ path: join(shots, `locked-list-${tag}.png`) });
        await page.keyboard.press('Escape');
        await expect(titled('Projects')).toHaveCount(0);
        await page.getByRole('navigation', { name: 'Machines', exact: true }).locator('.mtab[data-machine="local"]').click();
        await expect(page.getByRole('dialog', { name: /^What .+ is doing$/ })).toBeVisible();
        await page.screenshot({ path: join(shots, `locked-machine-${tag}.png`) });
        await page.keyboard.press('Escape');
        await page.goto(`${api.origin}/p/${plain.code}`);
        await expect(page.locator('.stage')).toBeVisible();
        await plusEnds(tag);
        await page.screenshot({ path: join(shots, `header-more-${tag}.png`), clip: { x: 0, y: 0, width, height: 120 } });
        await tabs.locator('a.ptab.on').click();
        await expect(titled('Projects')).toBeVisible();
        await page.screenshot({ path: join(shots, `list-${tag}.png`) });
        await page.keyboard.press('Escape');
        await menu.locator('summary').click();
        await page.screenshot({ path: join(shots, `menu-${tag}.png`) });
        await page.keyboard.press('Escape');
      }
    });
  } finally {
    await looks('paper');
    if ((await api.get(project.at('/vault'))).state !== 'locked') await cli('--json', 'vault', '--directory', project.path, 'lock');
  }
  guards.over();
});
