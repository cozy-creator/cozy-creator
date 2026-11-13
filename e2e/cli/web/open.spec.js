// open (cl-973 core cell 1): `cozy open`'s link signs a tab in once, under the page's CSP, with no cookie, its stream on its own session,
// through a reload; used again, in a browser of its own, it's refused in words until that browser has a code of its own. With no session the
// page says so once where it stands, says what to run and shows no project (cl-980); signed in, / opens none by itself and says what to do;
// the bar's + makes one there (its folder suggested and edited in place, encrypted unless asked, its tab shown at once), which opens at its
// address, unencrypted. Alone on its Calcifer (`@solo`: an index of its own projects only, so every tab fits the strip): the strip lists
// them in the index's order, a tab opens its own, going from one to another never blanks the screen, and + and the Projects drawer both
// suggest `<first project's parent>/untitled-N`.
const { randomBytes } = require('crypto');
const { existsSync, realpathSync } = require('fs');
const { dirname, join } = require('path');
const { test, expect, recordAddress, stays } = require('./fixtures');

const NO_SESSION = 'No project open here';
const HOME_LINE = 'No project open. Pick one above, or press +.';

const redeeming = (answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/session';
const streaming = (request) => request.method() === 'GET' && new URL(request.url()).pathname === '/v1/events';
/** Signed in at `project`: its tab, the open one, over the canvas. */
const signedIn = (page, project) => page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: project.name, exact: true });
/** No session, however the page found out (cl-980's one state): it says so, and how to open one. */
const gate = (page) => page.getByRole('heading', { name: NO_SESSION });
/** How many gates saying `words` this document drew. */
const drawn = async (page, words) => (await page.evaluate(() => window.__gates)).filter((text) => text.includes(words)).length;
const plus = (page) => page.getByRole('button', { name: 'New project', exact: true });
const panel = (page) => page.getByRole('region', { name: 'New project', exact: true });
const folderField = (page) => panel(page).getByLabel("New project's folder", { exact: true });
const index = async (api) => (await api.get('/v1/projects?limit=1000')).projects;
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

test('open', { tag: '@engines' }, async ({ context, newContext, project, api, signIn, guards, removeAfter }) => {
  // Refused on purpose: Chrome says each in its console.
  guards.allow(/status of 400\b.*\/v1\/session$/, /status of 401\b.*\/v1\/status$/);
  await context.addInitScript(recordAddress);
  const page = await context.newPage();
  // A code as `cozy open` asks the holder for one, at the project's address.
  const { code } = await api.post('/v1/session-codes', {});
  const link = `${project.url()}#code=${encodeURIComponent(code)}`;
  let token;

  await test.step("cozy open's link signs this tab in", async () => {
    const redeemed = page.waitForResponse(redeeming);
    const streamed = page.waitForRequest(streaming);
    await page.goto(link);
    const session = await redeemed;
    expect(session.status(), 'POST /v1/session').toBe(200);
    ({ token } = await session.json());
    await expect(page, 'the code leaves the address').toHaveURL(project.url());
    await expect(signedIn(page, project)).toHaveAttribute('aria-current', 'page');
    const stream = await streamed;
    expect.soft(await stream.headerValue('authorization'), "the page's stream is on its own session").toBe(`Bearer ${token}`);
    const answer = await stream.response();
    expect.soft(answer?.status(), 'GET /v1/events').toBe(200);
    expect.soft(await answer?.headerValue('content-type')).toMatch(/^text\/event-stream/);
    expect.soft(await context.cookies(), 'cookies').toEqual([]);
  });

  await test.step('a reload stays signed in', async () => {
    const streamed = page.waitForRequest(streaming);
    await page.reload();
    await expect(signedIn(page, project)).toHaveAttribute('aria-current', 'page');
    expect.soft(await (await streamed).headerValue('authorization'), 'the same session').toBe(`Bearer ${token}`);
    await expect.soft(gate(page)).toHaveCount(0);
  });

  const other = await (await newContext()).newPage();
  await test.step('the used link, in a browser of its own, is refused in words', async () => {
    const refused = other.waitForResponse(redeeming);
    await other.goto(link);
    const answer = await refused;
    expect.soft(answer.status(), 'a used code').toBe(400);
    expect.soft((await answer.json()).error?.code).toBe('session_code_invalid');
    await expect(gate(other)).toBeVisible();
    await expect.soft(other.getByText('cozy open', { exact: true })).toBeVisible();
    await expect.soft(signedIn(other, project)).toHaveCount(0);
  });

  await test.step('a code of its own signs it in', async () => {
    const session = await signIn(other, project.url());
    expect.soft(session.token, "its own session, not the first tab's").not.toBe(token);
    await expect(signedIn(other, project)).toHaveAttribute('aria-current', 'page');
    await expect.soft(gate(other)).toHaveCount(0);
  });

  await test.step('with no session, it says so once where it stands, and what to run, and shows no project', async () => {
    const stranger = await newContext();
    await stranger.addInitScript(recordAddress);
    const tab = await stranger.newPage();
    await tab.goto(project.url());
    await expect(gate(tab)).toBeVisible();
    expect.soft(await stays(tab), 'it stays where it is').toEqual({ path: `/p/${project.code}`, moves: [] });
    expect.soft(await drawn(tab, NO_SESSION), 'said once').toBe(1);
    await expect.soft(tab.locator('.ptab'), 'no project listed').toHaveCount(0);
    expect.soft(await tab.content(), "nothing of the project's").not.toContain(project.path);
    await tab.goto(`${api.origin}/`);
    await expect(gate(tab)).toBeVisible();
    const said = await tab.locator('main.gate').innerText();
    expect.soft(said.includes('cozy open') && said.includes('cozy new'), `what to run: ${said}`).toBe(true);
    await expect.soft(tab.locator('.ptab'), 'no project listed at /').toHaveCount(0);
    expect.soft(await tab.content(), "nothing of the project's at /").not.toContain(project.path);
  });

  await test.step('signed in, / opens no project by itself and says what to do', async () => {
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
});

test('open, alone on its Calcifer', { tag: '@solo' }, async ({ page, project, api, removeAfter }) => {
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
    const tabs = await page.locator('nav.ptabs a.ptab').evaluateAll((links) => links.map((link) => link.getAttribute('href')));
    const at = (row) => tabs.indexOf(`/p/${row.code}`);
    expect.soft(at(project) >= 0 && at(b) >= 0, `the strip lists both: ${tabs.join(' ')}`).toBe(true);
    const listed = await index(api);
    const rank = (row) => listed.findIndex((one) => one.code === row.code);
    expect.soft(listed.every((row, i) => i === 0 || listed[i - 1].used_at_ms >= row.used_at_ms), 'the index, most recently opened first').toBe(true);
    expect.soft(at(project) >= 0 && at(b) >= 0 && (at(project) < at(b)) === (rank(project) < rank(b)), `the strip in the index's order: ${tabs.join(' ')}`).toBe(true);
    expect.soft(new URL(page.url()).pathname, 'it stays at /').toBe('/');
  });

  await test.step('a tab opens its project, and going to another never blanks the screen', async () => {
    await tab(project).click();
    await expect(page).toHaveURL(project.url());
    await expect(page.locator('.react-flow__node').first()).toBeVisible();
    await expect.soft(page.locator('main.gate, .unread'), 'opened').toHaveCount(0);
    // Each frame from here: the bar, a canvas, and no gate.
    await page.evaluate(() => {
      window.__frames = [];
      window.__sampling = true;
      const look = () => {
        window.__frames.push({ top: Boolean(document.querySelector('header.top')), canvas: Boolean(document.querySelector('.react-flow')), gate: Boolean(document.querySelector('main.gate')) });
        if (window.__sampling) requestAnimationFrame(look);
      };
      requestAnimationFrame(look);
    });
    await tab(b).click();
    await expect(page.locator('.card[data-asset="only_b"]')).toBeAttached();
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
});
