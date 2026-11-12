// A box's corner pulled to an odd shape snaps to a ratio its tool makes (cl-936, Paul 11-12: "when we free-form select an aspect ratio, it
// should probably auto-snap to one of our more sane aspect ratios, and one we can actually support, rather than arbitrary A:B values in the
// thousands"), in a real browser on a real Calcifer. Qwen's Image takes any ratio from 1:16 to 16:1: its corner pulled to 1237:689, on the
// canvas and in Focus, lands on 16:9, and that's what it says.
//   node aspect.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, settled, until } = require('./page');

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
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-aspect-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const fieldsOf = async (name) => (await api(at)).body?.assets?.find((row) => row.name === name)?.fields ?? {};
  seen.project = { id, folder: (await api(at)).body.path };

  // Qwen's Image: any ratio within its bounds, nothing made yet.
  const tools = (await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? [];
  const qwen = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image');
  const shape = qwen?.fields.find((field) => field.type === 'shape');
  seen.shape = shape && { field: shape.field, ratios: shape.ratios, min: shape.min, max: shape.max };
  if (!shape) throw new Error(`no Qwen Image tool with a shape: ${JSON.stringify(tools.map((tool) => tool.id))}`);
  const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: qwen.id, kind: 'image', name: 'still', exact: true, fields: { instructions: 'a lighthouse at dusk' }, at: { x: 200, y: 200 } }) });
  if (made.status >= 300) throw new Error(`the box: ${JSON.stringify(made)}`);
  await boxOf(page, 'still').waitFor();
  await fitCanvas(page);
  await settled(page);

  /** Its corner pulled until what it shows is 1237:689: its width out by `by` pixels, its height to match. */
  const pull = async (scope, by) => {
    const corner = scope.locator('.corner').first();
    const media = await scope.locator('.media').first().boundingBox();
    const grip = await corner.boundingBox();
    const from = { x: grip.x + grip.width / 2, y: grip.y + grip.height / 2 };
    const width = media.width + by;
    const height = (width * 689) / 1237;
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + by, from.y + (height - media.height), { steps: 20 });
    const live = await page.locator('.ratio-live, [data-output-aspect]').first().innerText().catch(() => null);
    await page.mouse.up();
    return { media, live };
  };
  const snapped = (fields) => fields[shape.field] === '16:9';

  // On the canvas.
  const onCanvas = await pull(boxOf(page, 'still'), 140);
  const canvasFields = await until('its shape saved', () => fieldsOf('still'), (fields) => typeof fields[shape.field] === 'string').catch(() => fieldsOf('still'));
  seen.canvas = { ...onCanvas, saved: canvasFields[shape.field] };
  check('on_the_canvas_1237_689_lands_on_16_9', snapped(canvasFields), seen.canvas);
  check('on_the_canvas_it_said_16_9_as_it_was_pulled', onCanvas.live === null || onCanvas.live.trim() === '16:9', onCanvas.live);
  await shot('canvas');

  // In Focus, from 1:1.
  await api(`${at}/assets/still`, { method: 'PATCH', body: JSON.stringify({ fields: { [shape.field]: '1:1' } }) });
  await until('back to 1:1', () => fieldsOf('still'), (fields) => fields[shape.field] === '1:1');
  await openBox(page, 'still');
  const focus = page.locator('.focus-viewport');
  await focus.locator('.corner').first().waitFor();
  const inFocus = await pull(focus, 160);
  const focusFields = await until('its shape saved', () => fieldsOf('still'), (fields) => fields[shape.field] !== '1:1').catch(() => fieldsOf('still'));
  await page.waitForTimeout(300);
  const label = await focus.locator('[data-output-aspect]').first().innerText().catch(() => null);
  seen.focus = { ...inFocus, saved: focusFields[shape.field], label };
  check('in_focus_1237_689_lands_on_16_9', snapped(focusFields), seen.focus);
  check('in_focus_it_says_16_9', label === null || label.trim().startsWith('16:9'), label);
  await shot('focus');
  await closeBox(page);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
