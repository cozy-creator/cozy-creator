// A box's corner pulled to an odd shape snaps to a ratio its tool makes (cl-936, Paul 11-12: "when we free-form select an aspect ratio, it
// should probably auto-snap to one of our more sane aspect ratios, and one we can actually support, rather than arbitrary A:B values in the
// thousands"), in a real browser on a real Calcifer. Qwen's Image takes any ratio from 1:16 to 16:1: its corner pulled to 1237:689, on the
// canvas and in Focus, lands on 16:9, and that's what it says. MiniMax H3 (se-128, Paul 11-12: "The end user should be able to pick a different
// aspect ratio other than 16:9"): any shape within 4:1; Auto says what it makes, 16:9 with nothing set, a first frame's shape, never a mentioned
// picture's; its corner sets one, and × takes it back to Auto.
//   node aspect.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { writeFileSync, mkdtempSync } = require('fs');
const { join } = require('path');
const { tmpdir } = require('os');
const { projectFolder, projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, settled, until, upload } = require('./page');

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
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-aspect-${Date.now()}`));
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
    // What it says while held (Card.tsx: the shape it's nearest, or its badge's label). None said is a failure, not a pass.
    const said = () => page.locator('.ratio-live, [data-output-aspect]').first().innerText().then((text) => text.trim(), () => null);
    const live = await until('it says its shape as it is pulled', said, (text) => text === '16:9', 5000).catch(said);
    await page.mouse.up();
    return { media, live };
  };
  const snapped = (fields) => fields[shape.field] === '16:9';

  // On the canvas.
  const onCanvas = await pull(boxOf(page, 'still'), 140);
  const canvasFields = await until('its shape saved', () => fieldsOf('still'), (fields) => typeof fields[shape.field] === 'string').catch(() => fieldsOf('still'));
  seen.canvas = { ...onCanvas, saved: canvasFields[shape.field] };
  check('on_the_canvas_1237_689_lands_on_16_9', snapped(canvasFields), seen.canvas);
  check('on_the_canvas_it_said_16_9_as_it_was_pulled', onCanvas.live === '16:9', onCanvas.live);
  await shot('canvas');

  // In Focus, from 1:1.
  await api(`${at}/assets/still`, { method: 'PATCH', body: JSON.stringify({ fields: { [shape.field]: '1:1' } }) });
  await until('back to 1:1', () => fieldsOf('still'), (fields) => fields[shape.field] === '1:1');
  await openBox(page, 'still');
  const focus = page.locator('.focus-viewport');
  await focus.locator('.corner').first().waitFor();
  const inFocus = await pull(focus, 160);
  const focusFields = await until('its shape saved', () => fieldsOf('still'), (fields) => fields[shape.field] !== '1:1').catch(() => fieldsOf('still'));
  const badge = () => focus.locator('[data-output-aspect]').first().innerText().then((text) => text.trim(), () => null);
  const label = await until('it says 16:9', badge, (text) => (text ?? '').startsWith('16:9'), 5000).catch(badge);
  seen.focus = { ...inFocus, saved: focusFields[shape.field], label };
  check('in_focus_1237_689_lands_on_16_9', snapped(focusFields), seen.focus);
  check('in_focus_it_says_16_9', (label ?? '').startsWith('16:9'), label);
  await shot('focus');
  await closeBox(page);

  // MiniMax H3: a portrait mentioned in its words leaves Auto at 16:9; a 9:16 first frame makes it 9:16; the corner sets 1:1; × is Auto again.
  const h3 = ((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? []).find((tool) => tool.package === 'minimax-h3' && tool.makes === 'video' && tool.fields.some((field) => field.field === 'first_frame'));
  const h3Shape = h3?.fields.find((field) => field.type === 'shape');
  seen.h3 = h3 && { id: h3.id, release: h3.release, shape: h3Shape && { ratios: h3Shape.ratios, min: h3Shape.min, max: h3Shape.max, auto: h3Shape.auto } };
  // minimax-h3 0.12 takes any shape within 4:1; the Hub's final before it (0.11.1, until the 0.25.0 cut) only 16:9, and its box says so.
  const bounded = Boolean(h3Shape?.auto && h3Shape.min <= 0.25 + 1e-6 && h3Shape.max >= 4 - 1e-6 && !(h3Shape.ratios ?? []).length);
  const only169 = JSON.stringify(h3Shape?.ratios ?? []) === '["16:9"]';
  check('h3_listed_with_its_shape', Boolean(h3Shape) && (bounded || only169), seen.h3);
  if (h3Shape && only169) {
    await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: h3.id, kind: 'video', name: 'shot', exact: true, fields: { beats: [{ duration: 5, description: 'A dancer on a rooftop.' }] }, at: { x: 200, y: 700 } }) });
    await boxOf(page, 'shot').waitFor();
    const said = await until('it says 16:9', () => boxOf(page, 'shot').locator('.ratio-rest').first().innerText().then((text) => text.trim(), () => null), (text) => text === '16:9').catch(() => null);
    check('h3_before_0_12_says_16_9', said === '16:9', said);
  }
  if (h3Shape && bounded) {
    const folder = mkdtempSync(join(tmpdir(), 'web-aspect-'));
    for (const [name, width, height] of [['tall', 288, 512], ['face', 320, 400]]) {
      writeFileSync(join(folder, `${name}.png`), png(90, width, height));
      const imported = await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(folder, `${name}.png`), id), name, exact: true, at: { x: 900 + width, y: 200 } }) });
      if (imported.status >= 300) throw new Error(`import ${name}: ${JSON.stringify(imported)}`);
    }
    const beats = [{ duration: 5, description: '<face> dances on a rooftop.' }];
    const h3Box = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: h3.id, kind: 'video', name: 'shot', exact: true, fields: { beats }, at: { x: 200, y: 700 } }) });
    if (h3Box.status >= 300) throw new Error(`the H3 box: ${JSON.stringify(h3Box)}`);
    await boxOf(page, 'shot').waitFor();
    await fitCanvas(page);
    await settled(page);
    const said = () => boxOf(page, 'shot').locator('.ratio-rest').first().innerText().then((text) => text.trim(), () => null);
    const saying = (want) => until(`it says ${want}`, said, (text) => text === want).catch(said);
    seen.h3_auto = await saying('Auto · 16:9');
    check('h3_auto_is_16_9_and_a_mentioned_portrait_leaves_it', seen.h3_auto === 'Auto · 16:9', seen.h3_auto);
    await api(`${at}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields: { first_frame: '<tall>' } }) });
    seen.h3_first_frame = await saying('Auto · 9:16 from first frame');
    check('h3_auto_takes_its_first_frames_9_16', seen.h3_first_frame === 'Auto · 9:16 from first frame', seen.h3_first_frame);
    await openBox(page, 'shot');
    const focus = page.locator('.focus-viewport');
    await focus.locator('.corner').first().waitFor();
    const corner = await focus.locator('.corner').first().boundingBox();
    const media = await focus.locator('.media').first().boundingBox();
    await page.mouse.move(corner.x + corner.width / 2, corner.y + corner.height / 2);
    await page.mouse.down();
    // Square: as tall as it is wide.
    await page.mouse.move(corner.x + corner.width / 2 + 40, corner.y + corner.height / 2 + (media.width + 40 - media.height), { steps: 20 });
    await page.mouse.up();
    const pulled = await until('its shape saved', () => fieldsOf('shot'), (fields) => typeof fields.aspect_ratio === 'string').catch(() => fieldsOf('shot'));
    const focusSaid = await focus.locator('[data-output-aspect]').first().innerText().then((text) => text.trim(), () => null);
    seen.h3_pulled = { saved: pulled.aspect_ratio, said: focusSaid };
    check('h3_corner_sets_1_1', pulled.aspect_ratio === '1:1' && Boolean(focusSaid?.startsWith('1:1')), seen.h3_pulled);
    await focus.getByRole('button', { name: 'Auto aspect ratio', exact: true }).click();
    const back = await until('back to Auto', () => fieldsOf('shot'), (fields) => fields.aspect_ratio == null).catch(() => fieldsOf('shot'));
    const backSaid = await focus.locator('[data-output-aspect]').first().innerText().then((text) => text.trim(), () => null);
    seen.h3_back = { saved: back.aspect_ratio ?? null, said: backSaid };
    check('h3_x_is_auto_again', back.aspect_ratio == null && backSaid === 'Auto · 9:16 from first frame', seen.h3_back);
    await shot('h3');
    await closeBox(page);
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
