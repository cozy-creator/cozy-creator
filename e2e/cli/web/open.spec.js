// open (cl-973 core cell 1): the project's address, as `cozy open` opens it, opens a tab as Calcifer's own page (cl-985): nothing minted
// or sent, no cookie, nothing of Calcifer's kept in Web Storage, its stream asked with no Authorization, through a reload and in another
// browser of this account alike. / opens no project by itself and says what to do; the bar's + makes one there (its folder suggested and
// edited in place, encrypted unless asked, its tab shown at once), which opens at its address, unencrypted. Alone on its Calcifer (`@solo`:
// an index of its own projects only, so every tab fits the strip): the strip lists them in the index's order, a tab opens its own, going
// from one to another never blanks the screen, and + and the Projects drawer both suggest `<first project's parent>/untitled-N`; Calcifer
// killed and back leaves the tab where it is with its canvas and no gate at any moment, a new Calcifer answering its reads, and so again on
// another project's tab, through a reload. The gate a page Calcifer refuses draws is web/access.js's, with a real second OS account.
const { randomBytes } = require('crypto');
const { existsSync, readFileSync, realpathSync } = require('fs');
const { dirname, join } = require('path');
const { test, expect, recordAddress, stays, canvasVisible } = require('./fixtures');

const NO_SESSION = 'No project open here';
const HOME_LINE = 'No project open. Pick one above, or press +.';

const streaming = (request) => request.method() === 'GET' && new URL(request.url()).pathname === '/v1/events';
/** Open at `project`: its tab, the current one, over the canvas. */
const signedIn = (page, project) => page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: project.name, exact: true });
/** The gate a page Calcifer won't answer draws (cl-980's one state). */
const gate = (page) => page.getByRole('heading', { name: NO_SESSION });
/** How many gates saying `words` this document drew. */
const drawn = async (page, words) => (await page.evaluate(() => window.__gates)).filter((text) => text.includes(words)).length;
const plus = (page) => page.getByRole('button', { name: 'New project', exact: true });
const panel = (page) => page.getByRole('region', { name: 'New project', exact: true });
const folderField = (page) => panel(page).getByLabel("New project's folder", { exact: true });
const index = async (api) => (await api.get('/v1/projects?limit=1000')).projects;
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
/** Each request a context's pages make to Calcifer that mints a credential (`/v1/session…`) or sends one (an Authorization header), its
 *  headers read as it's made and again as its answer comes, with no awaited read: `headerValue` waits for headers some requests never
 *  give (run 43733704423: 180 s on Windows, 8 of 12). Called, it stops listening and says what it heard. */
function credentials(context, origin) {
  const minted = new Set();
  const sent = new Set();
  const look = (request) => {
    if (!request.url().startsWith(origin)) return;
    const said = `${request.method()} ${new URL(request.url()).pathname}`;
    if (said.split(' ')[1].startsWith('/v1/session')) minted.add(said);
    if (request.headers().authorization) sent.add(said);
  };
  const answered = (response) => look(response.request());
  context.on('request', look);
  context.on('response', answered);
  return () => {
    context.off('request', look);
    context.off('response', answered);
    return { minted: [...minted], sent: [...sent] };
  };
}
/** What this tab keeps in Web Storage that holds `secret`. */
const keeps = (page, secret) => page.evaluate((secret) => [localStorage, sessionStorage].flatMap((store) => Object.keys(store).filter((key) => key.includes(secret) || (store.getItem(key) ?? '').includes(secret))), secret);

test('open', { tag: '@engines' }, async ({ context, newContext, project, api, signIn, guards, removeAfter }) => {
  await context.addInitScript(recordAddress);
  const page = await context.newPage();
  const asked = credentials(context, api.origin);

  await test.step("the project's address opens this tab as Calcifer's own page, with nothing minted or sent", async () => {
    const streamed = page.waitForRequest(streaming);
    await signIn(page, project.url());
    await expect(signedIn(page, project)).toHaveAttribute('aria-current', 'page');
    const stream = await streamed;
    expect.soft(stream.headers().authorization ?? null, "the page's stream carries no credential").toBeNull();
    const answer = await stream.response();
    expect.soft(answer?.status(), 'GET /v1/events').toBe(200);
    expect.soft(answer?.headers()['content-type']).toMatch(/^text\/event-stream/);
    expect.soft(await context.cookies(), 'cookies').toEqual([]);
    expect.soft(await keeps(page, api.token), "Web Storage keeps nothing of Calcifer's").toEqual([]);
    await expect.soft(gate(page)).toHaveCount(0);
  });

  await test.step('a reload opens it the same', async () => {
    const streamed = page.waitForRequest(streaming);
    await page.reload();
    await expect(signedIn(page, project)).toHaveAttribute('aria-current', 'page');
    expect.soft((await streamed).headers().authorization ?? null, 'still no credential').toBeNull();
    await expect.soft(gate(page)).toHaveCount(0);
    expect.soft(await drawn(page, NO_SESSION), 'no gate at any moment').toBe(0);
  });

  await test.step('another browser of this account opens it the same', async () => {
    const elsewhere = await newContext();
    await elsewhere.addInitScript(recordAddress);
    const theirs = credentials(elsewhere, api.origin);
    const other = await elsewhere.newPage();
    await signIn(other, project.url());
    await expect(signedIn(other, project)).toHaveAttribute('aria-current', 'page');
    await expect.soft(gate(other)).toHaveCount(0);
    expect.soft(await drawn(other, NO_SESSION), 'no gate at any moment').toBe(0);
    expect.soft(theirs(), 'nothing minted or sent').toEqual({ minted: [], sent: [] });
  });

  await test.step('what that reads sees a credential when a page sends one', async () => {
    const elsewhere = await newContext();
    const listened = credentials(elsewhere, api.origin);
    const other = await elsewhere.newPage();
    await signIn(other, `${api.origin}/`);
    // Refused on purpose, as the browser says in its console: a bearer that isn't Calcifer's.
    guards.allow(/status of 401\b.*\/v1\/status\?sent$/);
    expect(await other.evaluate(() => fetch('/v1/status?sent', { headers: { Authorization: 'Bearer 00' } }).then((answer) => answer.status)), 'a wrong bearer').toBe(401);
    expect(listened().sent, 'the request that sent it').toContain('GET /v1/status');
  });

  await test.step('/ opens no project by itself and says what to do', async () => {
    await page.goto(`${api.origin}/`);
    await expect(page.locator('.home-line')).toBeVisible();
    expect.soft(await page.locator('.home-line').innerText()).toBe(HOME_LINE);
    expect.soft(await stays(page), 'it stays at /').toEqual({ path: '/', moves: [] });
  });

  await test.step("New project, from the bar's +", async () => {
    const name = `web-open-made-${randomBytes(3).toString('hex')}`;
    const folder = join(process.env.OUT, 'web', 'projects', name);
    // Its folder suggested in place, edited there: one of this suite's, which it removes after.
    await plus(page).click();
    await expect(folderField(page), 'a folder suggested').toHaveValue(/untitled-\d+$/);
    const suggested = await folderField(page).inputValue();
    expect.soft(new URL(page.url()).pathname, 'made at /').toBe('/');
    expect.soft(await page.locator('nav.ptabs span.ptab.on').innerText(), 'its tab shown at once').toBe(suggested.split(/[\\/]/).pop());
    await expect.soft(panel(page).getByLabel('Create without encryption', { exact: true }), 'encrypted unless asked').not.toBeChecked();
    await folderField(page).fill(folder);
    await panel(page).getByLabel('Create without encryption', { exact: true }).check();
    const made = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/projects');
    await panel(page).getByRole('button', { name: 'Create project', exact: true }).click();
    const answer = await made;
    expect(answer.status(), 'POST /v1/projects').toBe(200);
    const opened = await answer.json();
    removeAfter(opened.path);
    await expect(page, 'it opens at its address').toHaveURL(`${api.origin}/p/${opened.code}`);
    expect.soft(opened.name, 'named for its folder').toBe(name);
    await expect(signedIn(page, opened)).toHaveAttribute('aria-current', 'page');
    await expect.soft(page.locator('main.gate, .unread'), 'and opens').toHaveCount(0);
    const listed = (await index(api)).find((row) => row.id === opened.id);
    expect.soft(listed, 'the API lists it, at its folder').toMatchObject({ name, code: opened.code, path: realpathSync(folder) });
    expect.soft(existsSync(join(folder, 'project.db')), 'its project.db in its folder').toBe(true);
    expect.soft(await api.get(`/v1/projects/${encodeURIComponent(opened.id)}/vault`), 'unencrypted').toMatchObject({ encrypted: false });
  });

  expect.soft(asked(), 'nothing minted or sent, all along').toEqual({ minted: [], sent: [] });
});

test('open, alone on its Calcifer', { tag: '@solo' }, async ({ context, page, project, api, guards, removeAfter, calciferControl }) => {
  await context.addInitScript(recordAddress);
  // Two projects, each with a box: the cell's, and another.
  const b = await api.post('/v1/projects', { path: join(process.env.OUT, 'web', 'projects', `web-open-b-${randomBytes(3).toString('hex')}`), unencrypted: true });
  removeAfter(b.path);
  await project.add({ name: 'only_a', fields: { text: 'project a' }, at: { x: 120, y: 120 } });
  await api.post(`/v1/projects/${encodeURIComponent(b.id)}/assets`, { name: 'only_b', tool: '', exact: true, fields: { text: 'project b' }, at: { x: 120, y: 120 } });
  const tab = (row) => page.locator(`nav.ptabs a.ptab[href="/p/${row.code}"]`);

  await test.step("/: every project in the strip, in the index's order, over a canvas that says what to do", async () => {
    await page.goto(`${api.origin}/`);
    await expect(page.locator('.home-line')).toBeVisible();
    expect.soft(await page.locator('.home-line').innerText()).toBe(HOME_LINE);
    // Read once the strip has its list: it comes after the line does.
    let tabs = [];
    const at = (row) => tabs.indexOf(`/p/${row.code}`);
    await expect.configure({ soft: true }).poll(async () => {
      tabs = await page.locator('nav.ptabs a.ptab').evaluateAll((links) => links.map((link) => link.getAttribute('href')));
      return at(project) >= 0 && at(b) >= 0;
    }, { message: 'the strip lists both' }).toBe(true);
    const listed = await index(api);
    const rank = (row) => listed.findIndex((one) => one.code === row.code);
    expect.soft(listed.every((row, i) => i === 0 || listed[i - 1].used_at_ms >= row.used_at_ms), 'the index, most recently opened first').toBe(true);
    expect.soft(at(project) >= 0 && at(b) >= 0 && (at(project) < at(b)) === (rank(project) < rank(b)), `the strip in the index's order: ${tabs.join(' ')}`).toBe(true);
    expect.soft(new URL(page.url()).pathname, 'it stays at /').toBe('/');
  });

  await test.step('a tab opens its project, and going to another never blanks the screen', async () => {
    await tab(project).click();
    await expect(page).toHaveURL(project.url());
    await expect.poll(() => canvasVisible(page, 'only_a')).toBe(true);
    await expect.soft(page.locator('main.gate, .unread'), 'opened').toHaveCount(0);
    // Each frame from here: the bar, the canvas with its paper under it, and no gate. The WebGL picture over the paper is the project's own
    // and transparent, made again for each project; one frame without it paints the same paper.
    await page.evaluate(() => {
      window.__frames = [];
      window.__sampling = true;
      const look = () => {
        window.__frames.push({ top: Boolean(document.querySelector('header.top')), canvas: Boolean(document.querySelector('.stage .canvas-root') && document.querySelector('.stage :is(.paper-world, .paper-landscape)')), gate: Boolean(document.querySelector('main.gate')) });
        if (window.__sampling) requestAnimationFrame(look);
      };
      requestAnimationFrame(look);
    });
    await tab(b).click();
    await expect.poll(() => canvasVisible(page, 'only_b')).toBe(true);
    await expect(page.locator('main.gate')).toHaveCount(0);
    const frames = await page.evaluate(() => { window.__sampling = false; return window.__frames; });
    const blank = frames.filter((frame) => !frame.top || !frame.canvas || frame.gate);
    expect.soft(frames.length > 0 && blank.length === 0, `never blank: ${frames.length} frames, ${JSON.stringify(blank.slice(0, 5))}`).toBe(true);
  });

  await test.step("+ and the Projects drawer suggest the first project's parent's next untitled-N", async () => {
    await plus(page).click();
    await expect(folderField(page)).toBeVisible();
    const [first] = await index(api);
    const suggested = await folderField(page).inputValue();
    expect.soft(suggested, `beside ${first.path}`).toMatch(new RegExp(`^${escape(dirname(first.path))}[\\\\/]untitled-\\d+$`));
    expect.soft(new URL(page.url()).pathname, 'at /').toBe('/');
    await page.goto(`${api.origin}/projects`);
    const drawer = page.locator(`.drawer input[aria-label="New project's folder"]`);
    let offered;
    await expect.poll(async () => (offered = await drawer.inputValue()), { message: 'the drawer suggests a folder' }).not.toBe('');
    expect.soft(offered, 'the drawer, the same').toBe(suggested);
  });

  // Calcifer away: what the page asks of it fails as the browser says in its console, from each kill until the page is back with it.
  const refused = new RegExp(`^console error: Failed to load resource: net::ERR_[A-Z_]+ ${api.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}/`);
  let away = false;
  guards.allow({ test: (line) => away && refused.test(line) });
  /** What the top bar says from here, each pill once. */
  const barSays = () => page.evaluate(() => {
    const said = (window.barSaid = []);
    new MutationObserver(() => {
      for (const pill of document.querySelectorAll('header.top .pill')) if (!said.includes(pill.textContent)) said.push(pill.textContent);
    }).observe(document.body, { subtree: true, childList: true, characterData: true });
  });

  const pid = () => JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).pid;
  /** Killed with -9 and back: the bar says Calcifer was offline, then clears. */
  const killed = async () => {
    await barSays();
    const was = pid();
    away = true;
    await calciferControl.kill();
    await expect(page.locator('header.top .pill'), 'the top bar clears').toHaveCount(0, { timeout: 60_000 });
    away = false;
    expect.soft(await page.evaluate(() => window.barSaid), 'it said Calcifer was offline').toContainEqual(expect.stringMatching(/^Calcifer offline/));
    expect.soft(pid(), 'a new Calcifer').not.toBe(was);
  };
  /** Where `row`'s tab stands, its canvas kept, and no gate drawn at any moment of this document. */
  const kept = async (row, only) => {
    expect.soft(await stays(page), 'it stays where it is').toEqual({ path: `/p/${row.code}`, moves: [] });
    expect.soft(await drawn(page, NO_SESSION), 'no gate at any moment').toBe(0);
    await expect.soft(page.locator('.unread, main.gate'), 'none now').toHaveCount(0);
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, only), { message: 'and its canvas' }).toBe(true);
  };

  await test.step('Calcifer killed and back: the tab goes on where it is, and the new one answers its reads', async () => {
    await page.goto(project.url());
    await expect.poll(() => canvasVisible(page, 'only_a')).toBe(true);
    await killed();
    await kept(project, 'only_a');
    // The page's own read, as it asks: no Authorization.
    const read = await page.evaluate((id) => fetch(`/v1/projects/${encodeURIComponent(id)}?assets=false`, { headers: { 'cozy-project-storage': '3' } }).then((answer) => answer.status), project.id);
    expect.soft(read, "the new Calcifer answers the page's read").toBe(200);
  });

  await test.step("another project's tab, Calcifer killed and back again, then reloaded: its canvas in place, no gate before or after", async () => {
    await page.goto(`${api.origin}/p/${b.code}`);
    await expect.poll(() => canvasVisible(page, 'only_b')).toBe(true);
    await killed();
    await kept(b, 'only_b');
    await page.reload();
    await expect.poll(() => canvasVisible(page, 'only_b')).toBe(true);
    await kept(b, 'only_b');
  });
});
