// offline (cl-973 core cell 23, solo): Calcifer away, the page stays the person's (Paul, 11-06: "the UI should always remain responsive, even
// if calcifer is offline, and then we sync the changes in when calcifer comes back online"). Stopped, every edit shows at once, through a tab
// away and back, counted in the top bar, with nothing over the canvas and nothing in project.db; started, all of it lands in order with no
// reload (a picture dropped on a box as its next version too, whatever its bytes waited for), and history keeps Redo. Killed with -9 while
// words are typed, the page says so and comes back with every character, and nothing to sign in again (cl-985).
// With no Hub, the page loads, the palette says it lists only what's installed, a local tool renders, and Rent says why it lists nothing.
const { execFileSync } = require('child_process');
const { createHash, randomBytes } = require('crypto');
const { dirname, join } = require('path');
const { test, expect, canvasIdle, fixtureTool, focus, middle, openPoint, png, canvasCard, canvasNames, canvasPoint, canvasVisible, canvasText, canvasLabel, hoverBox } = require('./fixtures');

const PLACES = { shot: { x: 0, y: 0 }, named: { x: 0, y: 420 }, spare: { x: 360, y: 420 }, words: { x: 720, y: 420 }, still: { x: 1080, y: 420 } };
const STILL = `${fixtureTool('tool.py')}/still`;
const LATE = png(77);
const JOINED = png(55);
const FIRST = 'A lighthouse at dusk.';
const SECOND = 'A lighthouse at dawn.';
const BEFORE = 'Typed through a restart:';
const AFTER = ' one two three four five six seven eight';
const AT_ONCE = { timeout: 1_000 };
const PYTHON = process.platform === 'win32' ? 'python' : 'python3';

/** What project.db holds, read past Calcifer (read only): its revision, each box's fields, and each box's place on the canvas. */
function held(folder) {
  const script = `import json, sqlite3, sys
db = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
seq = db.execute("SELECT value FROM meta WHERE key = 'seq'").fetchone()
boxes = {}
for name, field, value in db.execute("SELECT b.name, f.field, f.value FROM boxes b LEFT JOIN fields f ON f.box = b.key ORDER BY b.ord, f.ord"):
    boxes.setdefault(name, {})
    if field is not None:
        boxes[name][field] = json.loads(value)
canvas = {name: [round(x), round(y)] for name, x, y in db.execute("SELECT name, x, y FROM canvas")}
print(json.dumps({"seq": seq and seq[0], "boxes": boxes, "canvas": canvas}))`;
  return JSON.parse(execFileSync(PYTHON, ['-c', script, join(folder, 'project.db')], { encoding: 'utf8' }));
}

/** Where `name` is drawn on the canvas: its node's flow position. */
const placeOf = async (page, name) => {
  const card = await canvasCard(page, name), at = card?.rendered?.transform ?? card?.position;
  return at ? [Math.round(at.x), Math.round(at.y)] : null;
};

/** The top bar's word on Calcifer and what waits for it, and everything it says. */
const status = (page) => page.locator('header.top').getByRole('status');
const bar = (page) => page.locator('header.top .pill');

/** Every box in view, a step out from fit. */
async function fitAll(page) {
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
  await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
  await canvasIdle(page);
}

/** `name` dragged by its name, as a hand does. */
async function drag(page, name, dx, dy) {
  const { x, y } = await middle((await hoverBox(page, name)).locator('.nm'));
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 8, y + 2);
  await page.mouse.move(x + dx, y + dy, { steps: 8 });
  await page.mouse.up();
  await canvasIdle(page);
}

/** A PNG dropped on the canvas at `at`, as the browser delivers a file dragged in. */
async function drop(page, name, bytes, { x, y }) {
  const transfer = await page.evaluateHandle(({ name, b64 }) => {
    const data = new DataTransfer();
    data.items.add(new File([Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))], name, { type: 'image/png' }));
    return data;
  }, { name, b64: bytes.toString('base64') });
  for (const type of ['dragenter', 'dragover', 'drop']) await page.dispatchEvent('.stage', type, { dataTransfer: transfer, clientX: x, clientY: y });
}

/** `words` typed over a box's words in Focus. Focus's region and the words' textbox, still open. */
async function typeOver(page, name, words, options) {
  const region = await focus(page, name);
  const text = region.getByRole('textbox', { name: 'Text' });
  await text.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type(words, options);
  return { region, text };
}

async function leave(page, region) {
  await page.keyboard.press('Escape');
  await expect(region).toHaveCount(0);
  await canvasIdle(page);
}

test('offline', { tag: '@solo' }, async ({ context, project, api, signIn, guards, calciferControl, removeAfter }) => {
  // Another project, to go to and come back from while Calcifer's away: made first, so the page lists its tab.
  const other = await api.post('/v1/projects', { path: join(dirname(project.path), `web-offline-other-${randomBytes(4).toString('hex')}`), unencrypted: true });
  removeAfter(other.path);
  // A picture with two takes, two more, a box of words and one of a warmed local tool, none rendering.
  await project.import(png(10), { name: 'shot', at: PLACES.shot });
  await api.post(project.at('/assets/shot/versions'), { source: await project.upload(png(20)) });
  await project.import(png(30), { name: 'named', at: PLACES.named });
  await project.import(png(40), { name: 'spare', at: PLACES.spare });
  await project.add({ name: 'words', fields: { text: 'What Calcifer held.' }, at: PLACES.words });
  await project.add({ name: 'still', tool: STILL, kind: 'image', fields: { instructions: 'a lighthouse' }, at: PLACES.still });
  // Looked at already: a take first shown is marked seen, one more change to wait while Calcifer's away.
  for (const asset of await project.assets()) for (const number of asset.unseen ?? []) await api.post(project.at(`/assets/${encodeURIComponent(asset.name)}/versions/${number}/seen`));
  const was = await project.asset('shot');
  const approve = was.pinned && was.approved === 2 ? 1 : 2;

  const page = await context.newPage();
  await signIn(page, project.url());
  await expect(page.locator('.stage')).toBeVisible();
  const tab = (row) => page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: row.name, exact: true });
  const reach = async (row) => {
    await tab(row).click();
    await expect(tab(row)).toHaveAttribute('aria-current', 'page');
    await expect(page.locator('.stage')).toBeVisible();
  };
  await fitAll(page);
  for (const name of Object.keys(PLACES)) await expect.poll(() => canvasVisible(page, name), {  }).toBe(true);
  await expect.poll(() => page.evaluate(() => { const scene = document.querySelector('.canvas-root').canvasInspection; return scene.names().filter(name => scene.card(name)?.rendered?.unseen); })).toEqual([]);
  // Both projects read once, as a person has them open.
  await reach(other);
  await reach(project);
  await expect.poll(() => canvasVisible(page, 'shot'), {  }).toBe(true);
  await expect(bar(page)).toHaveCount(0);
  await page.evaluate(() => { window.unreloaded = true; });
  // Calcifer away: what the page asks of it, its calls and its own files (a frame's sheet, its icons), fails as the browser says in its
  // console. What this cell does on purpose, from each stop until the page is back with it: past that, the same is a failure.
  const origin = api.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const refused = new RegExp(`^console error: Failed to load resource: net::ERR_[A-Z_]+ ${origin}/`);
  let away = false;
  guards.allow({ test: (line) => away && refused.test(line) });
  /** Calcifer back, and the page with it: what failed while it was away was said by then. */
  const back = async () => {
    await expect(bar(page), 'the top bar clears').toHaveCount(0, { timeout: 60_000 });
    away = false;
  };

  let atStop;
  await test.step('Calcifer stopped with SIGTERM, and kept down', async () => {
    away = true;
    await calciferControl.stop();
    await expect(status(page)).toHaveText(/^Calcifer offline/);
    atStop = held(project.path);
  });

  let moved;
  await test.step('typed twice, moved twice, renamed, approved, a picture dropped, one dropped on a box, one deleted and undone, a tab away and back', async () => {
    for (const words of [FIRST, SECOND]) {
      const { region, text } = await typeOver(page, 'words', words);
      await expect.soft(text, 'what is typed shows at once').toHaveText(words, AT_ONCE);
      await leave(page, region);
    }
    await expect.configure({ soft: true }).poll(() => canvasText(page, 'words'), { message: 'its words at rest', ...AT_ONCE }).toContain(SECOND);

    const from = await placeOf(page, 'shot');
    await drag(page, 'shot', 160, 40);
    const once = await placeOf(page, 'shot');
    expect.soft(once, 'the first move shows at once').not.toEqual(from);
    await drag(page, 'shot', 160, -40);
    moved = await placeOf(page, 'shot');
    expect.soft(moved, 'the second move shows at once').not.toEqual(once);

    // A tap on its name at rest renames it.
    const name = await middle((await hoverBox(page, 'named')).locator('.nm'));
    await page.mouse.click(name.x, name.y);
    await expect(page.getByRole('textbox', { name: 'Name' })).toBeFocused();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('renamed');
    await page.keyboard.press('Enter');
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, 'renamed'), { message: 'the rename shows at once', ...AT_ONCE }).toBe(true);
    await expect.configure({ soft: true }).poll(async () => (await canvasNames(page)).includes('named'), { ...AT_ONCE }).toBe(false);

    // A double tap on a take approves it.
    const region = await focus(page, 'shot');
    const take = region.getByRole('button', { name: `version ${approve}`, exact: true });
    await take.dblclick();
    await expect.soft(take, 'the approval shows at once').toHaveAttribute('aria-current', 'true', AT_ONCE);
    await leave(page, region);

    await drop(page, 'late.png', LATE, await openPoint(page));
    await expect(page.getByRole('textbox', { name: 'Name' })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, 'late'), { message: 'the drop shows at once', ...AT_ONCE }).toBe(true);

    // On a picture box it's that box's next version, no box of its own: its step and its turn are taken as it's dropped.
    await drop(page, 'joined.png', JOINED, await canvasPoint(page, 'renamed'));

    // Deleted last, so Undo takes back the delete.
    const head = await middle((await hoverBox(page, 'spare')).locator('.nm'));
    await page.mouse.move(head.x, head.y);
    const trash = await middle((await hoverBox(page, 'spare')).getByRole('button', { name: 'Delete', exact: true }));
    await page.mouse.click(trash.x, trash.y);
    await expect.configure({ soft: true }).poll(async () => (await canvasNames(page)).includes('spare'), { message: 'the delete shows at once', ...AT_ONCE }).toBe(false);
    const clear = await openPoint(page);
    await page.mouse.click(clear.x, clear.y);
    await page.keyboard.press('ControlOrMeta+z');
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, 'spare'), { message: 'its Undo shows at once', ...AT_ONCE }).toBe(true);

    await reach(other);
    await reach(project);
    await expect.poll(() => canvasVisible(page, 'shot'), {  }).toBe(true);
    const names = (await canvasNames(page)).sort();
    expect.soft(names, 'a tab away and back keeps every box').toEqual(['late', 'renamed', 'shot', 'spare', 'still', 'words']);
    expect.soft(await placeOf(page, 'shot'), 'and the move').toEqual(moved);
    await expect.configure({ soft: true }).poll(() => canvasText(page, 'words'), { message: 'and the words',  }).toContain(SECOND);
    await expect.configure({ soft: true }).poll(() => canvasLabel(page, 'shot', 'version'), { message: 'and the approval' }).toBe(`v${approve}`);
    // The person's changes: two saves, the moves as one, the rename, the approval, the file, the file on a box, the delete and its undo.
    // What the page keeps of its own view (what's selected, where the canvas is) is no change of theirs.
    await expect.soft(status(page), 'every edit counted in the top bar, and only those').toHaveText(/^Calcifer offline: 9 changes waiting/);
    await expect.soft(page.getByRole('dialog'), 'nothing over the canvas').toHaveCount(0);
    await expect.soft(page.getByRole('alertdialog')).toHaveCount(0);
    await expect.soft(page.locator('main.gate')).toHaveCount(0);
    expect.soft(held(project.path), 'nothing reached project.db').toEqual(atStop);
    await expect.soft(page.locator('.notes .note .line').filter({ hasText: /^Couldn't/ }), 'nothing refused').toHaveCount(0);
  });

  await test.step('Calcifer started (cozy machine ls)', async () => {
    await calciferControl.start();
    await back();
    const landed = async () => {
      const assets = Object.fromEntries((await project.assets()).map((asset) => [asset.name, asset]));
      const { cards } = await api.get(project.at('/canvas'));
      return {
        words: assets.words?.fields?.text,
        shot: { approved: assets.shot?.approved, pinned: assets.shot?.pinned, at: cards.shot && [Math.round(cards.shot.x), Math.round(cards.shot.y)] },
        renamed: Boolean(assets.renamed),
        named: Boolean(assets.named),
        spare: Boolean(assets.spare),
        late: assets.late?.sha256,
      };
    };
    // In order: the second words over the first, the delete before its undo.
    await expect.poll(landed, { message: 'every edit landed, in order', timeout: 30_000 }).toEqual({
      words: SECOND, shot: { approved: approve, pinned: true, at: moved }, renamed: true, named: false, spare: true,
      late: `sha256:${createHash('sha256').update(LATE).digest('hex')}`,
    });
    const joined = createHash('sha256').update(JOINED).digest('hex');
    await expect.configure({ soft: true }).poll(async () => (await project.asset('renamed'))?.prefixes?.some((version) => version.main?.sha256?.endsWith(joined)) ?? false, { message: 'the picture dropped on renamed landed, its next version' }).toBe(true);
    const stored = held(project.path);
    expect.soft(stored.boxes.words?.text, 'project.db holds the last words').toBe(SECOND);
    expect.soft(JSON.stringify(stored.boxes), 'and never the first').not.toContain(FIRST);
    // Newest first, as it was done: the file dropped, then the one dropped on a box, before the delete, which was undone.
    const { entries } = await api.get(project.at('/history?limit=50'));
    const deleted = entries.findIndex((entry) => entry.words === 'Deleted spare');
    const late = entries.findIndex((entry) => entry.words === 'Added late');
    const version = entries.findIndex((entry) => /^Added \w+ \d+ to renamed$/.test(entry.words));
    expect.soft(entries[deleted]?.undone, 'history: the delete, undone').toBeTruthy();
    expect.soft(late, 'history: the drop before the delete').toBeGreaterThan(deleted);
    expect.soft(version, `history: the drop on a box after the drop, before the delete: ${entries.map((entry) => entry.words).join(' | ')}`).toBeGreaterThan(deleted);
    expect.soft(version).toBeLessThan(late);
    expect.soft(await page.evaluate(() => window.unreloaded), 'no reload').toBe(true);
    await expect.soft(page.locator('.notes .note .line').filter({ hasText: /^Couldn't/ }), 'nothing refused').toHaveCount(0);
    // History keeps Redo: nothing was done after the undo. Done again, by its key, it deletes the box again.
    // The top bar's (a note may offer its own Redo).
    await expect.soft(page.getByRole('button', { name: 'Redo', exact: true }).and(page.getByLabel('Redo', { exact: true })), 'history keeps Redo').toHaveAttribute('title', /^Redo: Deleted spare/);
    const clear = await openPoint(page);
    await page.mouse.click(clear.x, clear.y);
    await page.keyboard.press('ControlOrMeta+Shift+z');
    await expect.configure({ soft: true }).poll(async () => (await canvasNames(page)).includes('spare'), { message: 'Redo shows at once', ...AT_ONCE }).toBe(false);
    await expect.configure({ soft: true }).poll(async () => Boolean(await project.asset('spare')), { message: 'Redo landed' }).toBe(false);
  });

  await test.step('kill -9 while typing: systemd starts it again', async () => {
    await page.evaluate(() => {
      const said = (window.barSaid = []);
      new MutationObserver(() => {
        for (const pill of document.querySelectorAll('header.top .pill')) if (!said.includes(pill.textContent)) said.push(pill.textContent);
      }).observe(document.body, { subtree: true, childList: true, characterData: true });
    });
    const sent = page.waitForRequest((request) => request.method() === 'PATCH' && new URL(request.url()).pathname.endsWith('/assets/words'));
    const { region, text } = await typeOver(page, 'words', BEFORE, { delay: 30 });
    // Its pause sends what's typed, and Calcifer is killed under it as typing goes on.
    await sent;
    away = true;
    const killed = calciferControl.kill();
    await page.keyboard.type(AFTER, { delay: 120 });
    await killed;
    await back();
    await expect.soft(text, 'every character shown').toHaveText(BEFORE + AFTER);
    await leave(page, region);
    expect.soft(await page.evaluate(() => window.barSaid), 'it said Calcifer was offline').toContainEqual(expect.stringMatching(/^Calcifer offline/));
    expect.soft(await page.evaluate(() => window.unreloaded), 'no reload').toBe(true);
    await expect.soft(page.getByRole('heading', { name: 'No project open here' }), 'no gate: the page keeps no session, so nothing to sign in again').toHaveCount(0);
    await expect.poll(async () => (await project.asset('words'))?.fields?.text, { message: 'every character saved, once', timeout: 30_000 }).toBe(BEFORE + AFTER);
    expect.soft(held(project.path).boxes.words?.text, 'in project.db').toBe(BEFORE + AFTER);
  });

  await test.step('no Hub: the page loads, the palette says so, a local tool renders, Rent says why it lists nothing; the Hub again', async () => {
    // Its rental list is the Hub's, asked through Calcifer, which says it can't: as the browser says it in its console, on purpose, while
    // there's no Hub.
    const unlisted = new RegExp(`^console error: Failed to load resource: the server responded with a status of 50[234] .* ${origin}/v1/rental-skus$`);
    let hubless = true;
    guards.allow({ test: (line) => hubless && unlisted.test(line) });
    away = true;
    await calciferControl.hub('http://127.0.0.1:9');
    await back();
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await fitAll(page);
    await expect.configure({ soft: true }).poll(() => canvasVisible(page, 'words'), { message: 'the page loads with no Hub',  }).toBe(true);
    await expect.soft(page.getByRole('complementary', { name: 'Tools' }).getByText(/^only what's installed/), 'the palette says what it lists').toBeVisible();

    const region = await focus(page, 'still');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => {
      const still = await project.asset('still');
      return still?.failure?.message ?? still?.versions;
    }, { message: 'a local tool renders with no Hub', timeout: 120_000 }).toBe(1);
    await expect.soft(region.getByRole('button', { name: 'version 1', exact: true }), 'its take, here').toBeVisible();
    await leave(page, region);

    await page.locator('summary[aria-label="Menu"]').click();
    await page.getByRole('link', { name: 'Machines', exact: true }).click();
    await expect.soft(page.getByRole('alert').filter({ hasText: "Couldn't get the machines for rent from the Hub" }), 'Rent says the Hub listed none').toBeVisible({ timeout: 30_000 });

    away = true;
    await calciferControl.hub();
    await back();
    hubless = false;
    await expect.poll(async () => (await api.get('/v1/tools')).note ?? null, { message: 'the Hub lists its tools again', timeout: 60_000 }).toBeNull();
  });
});
