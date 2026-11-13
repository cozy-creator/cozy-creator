// instant (cl-973 core cell 17), local-first (Paul, 11-05): a tool box is drawn in the frame it's dropped with its POST held, and what's
// typed in it and its move show at once and land after it, in order; an answer lost after it landed is sent again under the same
// Idempotency-Key and makes one asset and one history entry; a rename refused because another writer took the name meanwhile rolls back
// with one line saying why; and a delete with project.db held by another program shows, says in the top bar that it waits, and saves once
// it's let go.
const { spawn } = require('child_process');
const { join } = require('path');
const { createInterface } = require('readline');
const { test, expect, box, canvasIdle, focus, middle, openPoint } = require('./fixtures');

/** Each `method` call to `path` waits here, unsent, until `release`. */
async function hold(page, method, path) {
  const calls = [];
  let open;
  const held = new Promise((done) => (open = done));
  const matches = (url) => url.pathname === path;
  const handler = async (route) => {
    if (route.request().method() !== method) return route.fallback();
    calls.push(route.request());
    await held;
    await route.fallback();
  };
  await page.route(matches, handler);
  return { calls, release: async () => { open(); await page.unroute(matches, handler); } };
}

/** The first `method` call to `path` goes to Calcifer and its answer is lost, as a dropped connection loses one after the write landed. */
async function loseOne(page, method, path) {
  const keys = [];
  const matches = (url) => url.pathname === path;
  const handler = async (route) => {
    if (route.request().method() !== method) return route.fallback();
    keys.push(route.request().headers()['idempotency-key'] ?? null);
    if (keys.length > 1) return route.fallback();
    await route.fetch();
    await route.abort('connectionreset');
  };
  await page.route(matches, handler);
  return { keys, done: () => page.unroute(matches, handler) };
}

/** project.db's write lock, held by another program (an open `BEGIN IMMEDIATE`, as a SQLite browser holds one) until `release`. */
async function holdDatabase(file) {
  const script = 'import sqlite3, sys\nc = sqlite3.connect(sys.argv[1], isolation_level=None)\nc.execute("BEGIN IMMEDIATE")\nprint("acquired", flush=True)\nsys.stdin.buffer.read(1)\nc.execute("ROLLBACK")';
  const child = spawn(process.platform === 'win32' ? 'python' : 'python3', ['-c', script, file], { stdio: ['pipe', 'pipe', 'pipe'] });
  let said = '';
  child.stderr.on('data', (bytes) => { said = (said + bytes).slice(-4000); });
  const ended = new Promise((done) => child.once('close', (code) => done(code)));
  await new Promise((done, fail) => {
    createInterface({ input: child.stdout }).on('line', (line) => { if (line === 'acquired') done(); });
    ended.then((code) => fail(new Error(`the SQLite holder ended before it held the lock: exit ${code}: ${said}`)));
    child.once('error', fail);
  });
  let released;
  return {
    release: () => {
      released ??= (async () => {
        child.stdin.end();
        const code = await ended;
        if (code !== 0) throw new Error(`the SQLite holder: exit ${code}: ${said}`);
      })();
      return released;
    },
  };
}

/** Carried as a hand carries one out of the palette: across it a little first, which tells a carry from a scroll, then let go at `to`. */
async function carry(page, from, to) {
  const { x, y } = await middle(from);
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 12, y, { steps: 3 });
  await page.mouse.move(to.x, to.y, { steps: 8 });
  await page.mouse.up();
}

/** In the page: the boxes on the canvas as the next drop is let go, and in the frame after it. */
const armDrop = (page) => page.evaluate(() => {
  const names = () => [...document.querySelectorAll('.react-flow__node .card[data-asset]')].map((card) => card.dataset.asset);
  const clock = (window.__dropped = {});
  document.querySelector('.stage').addEventListener('letgo', () => {
    clock.before = names();
    requestAnimationFrame(() => { clock.after = names(); });
  }, { capture: true, once: true });
});

/** The box the armed drop drew in its frame, after the boxes before it: exactly one. */
async function dropped(page) {
  await expect.poll(() => page.evaluate(() => Boolean(window.__dropped.after)), { message: 'the frame after the drop' }).toBe(true);
  const { before, after } = await page.evaluate(() => window.__dropped);
  const made = after.filter((name) => !before.includes(name));
  expect(made, 'one box drawn in the frame of its drop').toHaveLength(1);
  return made[0];
}

/** Its place on the canvas, as its node is drawn. */
const placeOf = (page, name) => box(page, name).evaluate((card) => {
  const { e, f } = new DOMMatrix(getComputedStyle(card.closest('.react-flow__node')).transform);
  return { x: e, y: f };
});

test('instant', async ({ page, project, api, guards }) => {
  const at = project.at();
  // Caused here on purpose: the answer lost, the rename refused, and the saves refused while project.db is held.
  guards.allow(
    /^(?=.*ERR_CONNECTION_RESET)(?=.*\/v1\/projects\/[^/ ]+\/assets(?![/\w]))/,
    /status of 409\b.*\/v1\/projects\/[^/ ]+\/rename\b/,
    /status of 503\b.*\/v1\/projects\/[^/ ]+\//,
  );
  const writes = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() !== 'GET' && path.startsWith(at)) writes.push(`${request.method()} ${path.slice(at.length)}`);
  });
  const text = page.getByRole('complementary', { name: 'Tools' }).locator('li[data-node="text"]');
  const lines = () => page.locator('.notes .note .line').allInnerTexts();
  let first;
  let second;

  await test.step("a tool box dropped, typed in and moved while its POST is held: each shown at once, landing after it in order", async () => {
    await expect(text).toBeVisible();
    const posts = await hold(page, 'POST', project.at('/assets'));
    await armDrop(page);
    await carry(page, text, await openPoint(page));
    first = await dropped(page);
    await expect.poll(() => posts.calls.length, { message: 'its POST asked, and held' }).toBe(1);
    expect.soft(await project.asset(first), 'nothing saved yet').toBeUndefined();

    const region = await focus(page, first);
    const words = region.getByRole('textbox', { name: 'Text', exact: true });
    await words.click();
    await page.keyboard.type('Typed while it was held');
    await expect.soft(words, 'the words show as typed').toHaveText('Typed while it was held');
    await region.getByRole('button', { name: 'Back to canvas' }).click();
    await canvasIdle(page);

    const was = await box(page, first).boundingBox();
    const head = await middle(box(page, first).locator('.head'));
    await page.mouse.move(head.x, head.y);
    await page.mouse.down();
    await page.mouse.move(head.x + 4, head.y + 2);
    await page.mouse.move(head.x + 120, head.y + 80, { steps: 6 });
    await page.mouse.up();
    const now = await box(page, first).boundingBox();
    expect.soft(Math.abs(now.x - was.x - 120), 'the move shows at once, where it was let go: x, px off').toBeLessThanOrEqual(2);
    expect.soft(Math.abs(now.y - was.y - 80), 'and y, px off').toBeLessThanOrEqual(2);
    const place = await placeOf(page, first);
    expect.soft(posts.calls.length, 'still the one POST, held').toBe(1);
    expect.soft(await project.asset(first), 'still nothing saved').toBeUndefined();

    await posts.release();
    await expect.poll(async () => (await project.asset(first))?.fields?.text, { message: 'its words landed' }).toBe('Typed while it was held');
    await expect.poll(async () => {
      const card = (await api.get(project.at('/canvas'))).cards[first];
      return card && [Math.round(card.x), Math.round(card.y)];
    }, { message: 'its move landed' }).toEqual([Math.round(place.x), Math.round(place.y)]);
    const after = writes.slice(writes.indexOf('POST /assets'));
    const order = [after.indexOf('POST /assets'), after.indexOf(`PATCH /assets/${encodeURIComponent(first)}`), after.indexOf('PATCH /canvas')];
    expect.soft(order.every((i) => i >= 0) && order[0] < order[1] && order[1] < order[2], `the drop, then its words, then its move: ${after.join(', ')}`).toBe(true);
    await expect.soft(box(page, first)).toHaveCount(1);
  });

  await test.step("an answer lost after it lands, and a rename onto a name that's taken", async () => {
    const assets = (await project.assets()).length;
    const lost = await loseOne(page, 'POST', project.at('/assets'));
    await armDrop(page);
    await carry(page, text, await openPoint(page));
    second = await dropped(page);
    await expect.poll(() => lost.keys.length, { message: 'sent again' }).toBeGreaterThanOrEqual(2);
    await lost.done();
    await expect.poll(async () => Boolean(await project.asset(second)), { message: 'it landed' }).toBe(true);
    expect.soft(lost.keys[0], 'an Idempotency-Key').toBeTruthy();
    expect.soft(new Set(lost.keys).size, `every try under one key: ${lost.keys.join(', ')}`).toBe(1);
    expect.soft((await project.assets()).length, 'one asset').toBe(assets + 1);
    const history = (await api.get(project.at('/history?limit=50'))).entries.filter((entry) => entry.names?.includes(second));
    expect.soft(history.map((entry) => entry.words), 'one history entry').toEqual([expect.stringMatching(/^Added /)]);
    await expect.soft(box(page, second)).toHaveCount(1);

    // Another writer takes the name while this rename is on its way: it's refused, and the box is as it was.
    const renames = await hold(page, 'POST', project.at('/rename'));
    const name = await middle(box(page, first).locator('.nm'));
    await page.mouse.click(name.x, name.y);
    const field = page.getByRole('textbox', { name: 'Name', exact: true });
    await field.fill('taken');
    await field.press('Enter');
    await expect.soft(box(page, 'taken'), 'the new name shows at once').toHaveCount(1);
    await expect.soft(box(page, first)).toHaveCount(0);
    await expect.poll(() => renames.calls.length, { message: 'its rename asked, and held' }).toBe(1);
    await project.add({ name: 'taken', kind: 'text', fields: { text: "Another writer's" } });
    const answered = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === project.at('/rename'));
    await renames.release();
    expect.soft((await answered).status(), 'the rename refused').toBe(409);
    await expect.soft(box(page, first), 'rolled back to its name').toContainText('Typed while it was held');
    await expect.soft(box(page, 'taken'), "the other writer's box").toContainText("Another writer's");
    await expect.poll(async () => (await lines()).filter((line) => line.startsWith("Couldn't rename")), { message: 'one line says why' })
      .toEqual([expect.stringMatching(new RegExp(`^Couldn't rename ${first}: .*taken.*already`))]);
    expect.soft((await project.asset(first))?.fields?.text, 'its words where they were').toBe('Typed while it was held');
    expect.soft((await project.asset('taken'))?.fields?.text).toBe("Another writer's");
  });

  await test.step('project.db held by another program: a delete shows, the top bar says it waits, and it saves once let go', async () => {
    const held = await holdDatabase(join(project.path, 'project.db'));
    try {
      const { x, y } = await middle(box(page, second).locator('.media').first());
      await page.mouse.click(x, y);
      await expect(box(page, second)).toHaveAttribute('aria-selected', 'true');
      await page.keyboard.press('Delete');
      await expect.soft(box(page, second), 'the delete shows at once').toHaveCount(0);
      const bar = page.locator('.pill.warn').filter({ hasText: 'project.db is open in another program' });
      // Calcifer waits 10 s on another's write lock before it says so.
      await expect.soft(bar, 'the top bar says what holds it').toHaveCount(1, { timeout: 60_000 });
      await expect.soft(bar).toContainText(/\b[1-9]\d* changes? waiting\b/);
      expect.soft(Boolean(await project.asset(second)), 'nothing written while it is held').toBe(true);
    } finally {
      await held.release();
    }
    await expect.poll(async () => Boolean(await project.asset(second)), { message: 'saved once let go', timeout: 60_000 }).toBe(false);
    await expect.soft(page.locator('.pill.warn').filter({ hasText: 'project.db is open in another program' }), 'the top bar clears').toHaveCount(0, { timeout: 30_000 });
    await expect.soft(box(page, second)).toHaveCount(0);
    expect.soft(Boolean(await project.asset('taken')), 'what else is there stays').toBe(true);
    expect.soft((await lines()).filter((line) => /Couldn't delete/.test(line)), 'nothing refused').toEqual([]);
  });
});
