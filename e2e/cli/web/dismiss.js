// A press that began inside never dismisses (cl-923, Paul 11-12: "when I highlight something, or highlight text, and then release my click,
// that should NOT count as a click off of a page"), in a real browser on a real Calcifer. For each surface a click off closes (a box open in
// Focus, a box's right-click menu, the main menu, the zoom menu, a machine's activity): words selected inside, or a press inside, let go
// outside, and it's still open, the selection kept; pressed and let go outside, and it closes.
//   node dismiss.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { projectFolder, projectFromPage, calcifer, signIn, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, openBox, openCanvas, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

/** What's on a focused item rather than its background (FocusedViewport.tsx). */
const OWN = '.media, .tl, .focus-heading, .run-face, .here-side, .here-under, .image-controls, .focus-image-zoom, button, input, textarea, select, [contenteditable]';

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-dismiss-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };

  // A box of words: Qwen's Image, its prompt written, nothing made yet.
  const searched = (await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? [];
  const qwen = searched.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  if (!qwen) throw new Error(`the Hub lists no Qwen Image tool: ${JSON.stringify(searched.map((tool) => tool.id))}`);
  const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: qwen.id, kind: 'image', name: 'words', exact: true, fields: { instructions: 'a lighthouse at dusk, waves breaking white on black rocks, gulls over the lamp' }, at: { x: 100, y: 100 } }) });
  if (made.status >= 300) throw new Error(`the box: ${JSON.stringify(made)}`);
  await boxOf(page, 'words').waitFor();
  await fitCanvas(page);

  /** A press at `from`, carried to `to` and let go there. */
  const drag = async (from, to) => {
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(to.x, to.y, { steps: 16 });
    await page.mouse.up();
    await page.waitForTimeout(300);
  };
  const selected = () => page.evaluate(() => document.getSelection()?.toString() ?? '');
  /** Where a press lands on `locator`: just inside its first line. */
  const inside = async (locator) => {
    const box = await locator.boundingBox();
    return { x: box.x + 8, y: box.y + Math.min(12, box.height / 2) };
  };
  /** A surface's two cases: a selection or a press from inside let go outside keeps it; pressed and let go outside, it closes. */
  const surface = async (tag, { open, shown, from, outside, words }) => {
    await open();
    await shown.first().waitFor();
    const start = await from();
    const away = await outside();
    await drag(start, away);
    const kept = (await shown.count()) > 0;
    const said = words ? await selected() : null;
    const focused = await page.evaluate(() => document.activeElement?.closest('[contenteditable], input, textarea') ? document.activeElement.className : null);
    seen[tag] = { start, away, kept, selected: said, focused };
    check(`${tag}_stays_open_when_a_press_from_inside_is_let_go_outside`, kept && (!words || said.length > 0), seen[tag]);
    await shot(`${tag}-kept`);
    await page.mouse.click(away.x, away.y);
    const closed = await until(`${tag} closed`, () => shown.count(), (n) => n === 0, 5_000).then(() => true, () => false);
    check(`${tag}_closes_on_a_press_and_release_outside`, closed, await shown.count());
  };

  // A box open in Focus: words selected in its prompt, let go on its background.
  const focus = page.locator('.focus-viewport');
  const focusedPrompt = page.locator('.focus-viewport .mention-text').first();
  await surface('focus', {
    open: async () => {
      await openBox(page, 'words');
      await focusedPrompt.waitFor();
    },
    shown: focus,
    from: () => inside(focusedPrompt),
    outside: () => page.evaluate((own) => {
      const viewport = document.querySelector('.focus-viewport').getBoundingClientRect();
      for (let y = viewport.bottom - 12; y > viewport.top + 40; y -= 16) for (let x = viewport.right - 12; x > viewport.left + 12; x -= 16) {
        const there = document.elementFromPoint(x, y);
        if (there?.closest('.focus-viewport') && !there.closest(own)) return { x, y };
      }
      throw new Error('no background on the focused box');
    }, OWN),
    words: true,
  });

  // Its right-click menu: a press on one of its items let go on open canvas.
  const menu = page.locator('.asset-menu[role=menu]');
  await surface('box_menu', {
    open: async () => {
      const box = await boxOf(page, 'words').locator('.card').boundingBox();
      await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' });
    },
    shown: menu,
    from: () => inside(menu.locator('button').first()),
    outside: () => openCanvas(page),
  });

  // The main menu.
  const main = page.locator('details.main-menu[open]');
  await surface('main_menu', {
    open: () => page.locator('details.main-menu > summary').click(),
    shown: main,
    // On the menu but on none of its links, which a press would carry off as a link.
    from: () => page.evaluate(() => {
      const nav = document.querySelector('details.main-menu nav').getBoundingClientRect();
      for (let y = nav.top + 1; y < nav.bottom; y += 2) for (let x = nav.left + 1; x < nav.right; x += 4) {
        const there = document.elementFromPoint(x, y);
        if (there?.closest('details.main-menu') && !there.closest('a, button, summary')) return { x, y };
      }
      throw new Error('no room on the main menu off its links');
    }),
    outside: () => openCanvas(page),
  });

  // The zoom menu: a press on one of its items let go on open canvas.
  const zoom = page.locator('.zoom-menu[role=menu]');
  await surface('zoom_menu', {
    open: () => page.locator('.zoom-at').click(),
    shown: zoom,
    from: () => inside(zoom.locator('[role=menuitem]').first()),
    outside: () => openCanvas(page),
  });

  // What a machine is doing, under its chip: words selected in it, let go on open canvas.
  const activity = page.locator('.activity[role=dialog]');
  await surface('machine_activity', {
    open: () => page.locator('.mtab').first().click(),
    shown: activity,
    from: () => inside(activity.locator('.said').first()),
    outside: () => openCanvas(page),
    words: true,
  });

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
