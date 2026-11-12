// Focus is a viewport workspace, independent of Wander's camera. Real imported image states
// exercise full-fit viewing, retained previews, explicit model configuration and return gestures.
// A weightless local image render, where available, proves editing an older state prunes nothing.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { boxOf, calcifer, fitCanvas, lastLook, launch, lookAt, noStall, png, projectFromPage, settled, signIn, STEP_MS, until, upload } = require('./page');

const NAME = 'focus_image';
const failures = [];
const seen = {};
let during = 'setup';
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(error) {
  failures.push(`${during}: ${error instanceof Error ? error.message : error}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(message.text())) errors.push(message.text());
  });
  const shot = (state) => process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${state}.png` }) : Promise.resolve();
  await signIn(page);
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-focused-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const project = await projectFromPage(page);
  const id = project.id;
  const session = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${session.token}`, 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const at = `/v1/projects/${id}`;
  const state = () => api(at);
  const asset = async () => (await state()).assets.find((item) => item.name === NAME);
  const images = mkdtempSync(join(tmpdir(), 'web-focused-'));
  const picture = (number, width, height) => {
    const path = join(images, `${number}.png`);
    writeFileSync(path, png(20 + number * 15, width, height));
    return upload(session.token, path);
  };
  const first = await picture(1, 960, 240);
  await api(`${at}/imports`, 'POST', { source: first, name: NAME, exact: true, at: { x: 750, y: 350, w: 300 } });
  await api(`${at}/assets/${NAME}/versions`, 'POST', { source: await picture(2, 240, 960) });
  for (let number = 3; number <= 10; number++) await api(`${at}/assets/${NAME}/versions`, 'POST', { source: await picture(number, 480, 320) });
  await api(`${at}/approve`, 'POST', { name: NAME, version: '1' });
  const metadata = await api(`/v1/tools?project=${id}&palette=true`);
  const tool = metadata.tools.find((item) => item.makes === 'image' && item.model && item.fields.some((field) => field.type === 'prompt'));
  if (!tool) throw new Error('the image tool metadata needed by the Focus fixture is unavailable');
  const promptField = tool.fields.find((field) => field.type === 'prompt').field;
  // The words last run (cl-909): New begins from them.
  await api(`${at}/assets/${NAME}`, 'PATCH', { tool: tool.id, fields: { [promptField]: '<self.1> make her smile' } });
  await until('ten image states are retained', asset, (item) => item?.versions === 10 && item.version === 1);
  await boxOf(page, NAME).waitFor();
  await fitCanvas(page);
  const tools = page.locator('aside.tools');
  if (await tools.locator('[aria-label="Show the tools"]').count()) await tools.getByLabel('Show the tools', { exact: true }).click();
  await settled(page);

  const focus = page.locator(`[data-focused-asset="${NAME}"]`);
  const card = focus.locator('.card.focus-card');
  const history = card.locator('.image-history.here-side');
  const camera = () => page.evaluate(() => {
    const matrix = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
    return { x: matrix.e, y: matrix.f, zoom: matrix.a };
  });
  const palette = () => page.evaluate(() => ({ shut: document.querySelector('aside.tools')?.classList.contains('shut'), saved: localStorage.getItem('cozy.tools-shut') }));
  const sameCamera = (before, after) => Math.abs(before.x - after.x) < 0.5 && Math.abs(before.y - after.y) < 0.5 && Math.abs(before.zoom - after.zoom) < 0.0001;
  const sameRect = (before, after) => ['x', 'y', 'width', 'height'].every((key) => Math.abs(before[key] - after[key]) < 2);
  const focusByClick = async () => {
    const media = boxOf(page, NAME).locator('.media');
    await media.waitFor();
    const point = await media.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      const left = Math.max(rect.left, document.querySelector('aside.tools')?.getBoundingClientRect().right ?? 0);
      const right = Math.min(rect.right, innerWidth - 8);
      const top = Math.max(rect.top, 64);
      const bottom = Math.min(rect.bottom, innerHeight - 12);
      if (right <= left || bottom <= top) throw new Error('the Wander image has no visible click target');
      return { x: (left + right) / 2, y: (top + bottom) / 2 };
    });
    await page.mouse.click(point.x, point.y);
    await focus.waitFor();
    await card.waitFor();
  };
  // A tap looks at a state; a double tap makes it current (630309269, explicit variant approval).
  const choose = async (number) => {
    const take = history.locator(`.take[data-take="${number}"]`);
    await take.scrollIntoViewIfNeeded();
    await take.dblclick();
    await until(`state ${number} is current`, asset, (item) => item.version === number);
    await until(`state ${number} is selected in the previews`, () => history.locator('.take.on[data-take]').getAttribute('data-take'), (current) => current === String(number));
  };
  const measure = () => card.evaluate((element) => {
    const rect = (node) => {
      const value = node.getBoundingClientRect();
      return { x: value.x, y: value.y, width: value.width, height: value.height, right: value.right, bottom: value.bottom };
    };
    const viewer = element.querySelector('.focus-viewer');
    const media = viewer.querySelector(':scope > .media');
    const loaded = [...media.querySelectorAll('.media-pixels img')].some((image) => image.complete && image.naturalWidth > 0);
    return { viewer: rect(viewer), media: rect(media), controls: rect(element.querySelector('.image-controls')), loaded };
  });
  const fits = (look, ratio) => look.loaded && look.media.width > 20 && look.media.height > 20
    && look.media.x >= look.viewer.x - 2 && look.media.y >= look.viewer.y - 2
    && look.media.right <= look.viewer.right + 2 && look.media.bottom <= look.viewer.bottom + 2
    && Math.abs((look.media.width / look.media.height) / ratio - 1) < 0.03
    && Math.min(Math.abs(look.media.width - look.viewer.width), Math.abs(look.media.height - look.viewer.height)) < 3;
  const expectFit = (ratio) => until('the full image fits the Focus viewer', measure, (look) => fits(look, ratio));

  const wander = await camera();
  const expanded = await palette();
  await focusByClick();
  const wide = await expectFit(4);
  check('node_click_opens_one_focus_workspace', await page.locator('[data-focused-asset]').count() === 1 && await card.count() === 1);
  check('focus_preserves_wander_camera', sameCamera(wander, await camera()), { before: wander, after: await camera() });
  check('focus_keeps_temporary_palette_collapse_out_of_saved_settings', (await palette()).saved === expanded.saved);
  check('right_hand_original_previews_are_available', await history.isVisible() && await history.locator('.take[data-take]').count() === 10);
  check('focus_has_no_linear_counter_or_edit_button', await focus.locator('.count:visible').count() === 0 && await focus.getByRole('button', { name: /^(Previous version|Next version|Edit image)$/ }).count() === 0);
  await shot('wide');
  await choose(2);
  const tall = await expectFit(0.25);
  check('wide_and_tall_states_fit_the_same_viewport_room', sameRect(wide.viewer, tall.viewer) && fits(wide, 4) && fits(tall, 0.25), { wide, tall });
  check('tool_controls_stay_pinned_when_state_ratio_changes', sameRect(wide.controls, tall.controls), { wide: wide.controls, tall: tall.controls });
  check('state_selection_does_not_zoom_wander', sameCamera(wander, await camera()));
  await shot('tall');

  await choose(10);
  const side = await history.boundingBox();
  const beforeScroll = await history.evaluate((element) => element.scrollTop);
  await page.mouse.move(side.x + 30, side.y + 35);
  await page.mouse.wheel(0, 400);
  await until('right previews scroll', () => history.evaluate((element) => element.scrollTop), (top) => top > beforeScroll);
  check('scrolling_previews_keeps_focus_and_wander_camera', await focus.isVisible() && sameCamera(wander, await camera()));
  await choose(1);
  await expectFit(4);
  check('models_button_is_explicit_and_not_an_edit_button', await focus.getByRole('button', { name: 'Models and LoRAs', exact: true }).isVisible() && await focus.getByRole('button', { name: 'Edit image', exact: true }).count() === 0);
  const toolBeforePicker = (await asset()).tool;
  await focus.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
  const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
  await picker.waitFor();
  check('model_dialog_does_not_add_a_per_popup_nsfw_switch', await picker.getByLabel(/Show NSFW/).count() === 0);
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  check('model_dialog_preserves_focus_tool_and_states', await focus.isVisible() && (await asset()).tool === toolBeforePicker && (await asset()).versions === 10);

  // New keeps the words last run, word for word, from the state shown: state 2 picked, `<self.1>` becomes `<self.2>` (cl-909, Paul 11-06).
  await choose(2);
  const newImage = history.locator('.take.add[aria-label="Its next take"]');
  await newImage.scrollIntoViewIfNeeded();
  await newImage.click();
  const prompt = focus.locator('.in-frame .mention-text').first();
  await prompt.waitFor();
  await until('New image keeps the words, from the state shown', () => prompt.innerText(), (text) => /<self\.2>/i.test(text) && text.includes('make her smile')).catch(() => {});
  const kept = await until('the words from the state shown are saved', asset, (item) => item.fields?.[promptField] === '<self.2> make her smile').catch(asset);
  check('new_keeps_the_words_last_run_from_the_state_shown', kept.fields?.[promptField] === '<self.2> make her smile', { saved: kept.fields?.[promptField], shown: await prompt.innerText() });
  check('new_image_is_explicit_and_does_not_discard_variants', (await asset()).versions === 10 && await history.locator('.take[data-take]').count() === 10);

  const local = (await api('/v1/status')).local_usable && !process.env.ALONE;
  if (local) {
    const before = (await asset()).prefixes.map((version) => [version.number, version.main.sha256]);
    const waiting = `${join(__dirname, '..', 'project', 'waits.py')}/wait`;
    await api(`${at}/assets/${NAME}`, 'PATCH', { tool: waiting, fields: { instructions: '<self.1> ', seconds: 0, aspect_ratio: '1:1' } });
    await until('weightless image tool attached', asset, (item) => item.tool === waiting);
    await newImage.click();
    await focus.getByRole('button', { name: 'Render', exact: true }).click();
    const edited = await until('editing an older state appends another alternative', asset, (item) => item.versions === 11 && !item.making?.length, 300_000);
    check('generation_from_old_state_retains_every_alternative', edited.version === 11 && JSON.stringify(edited.prefixes.slice(0, 10).map((version) => [version.number, version.main.sha256])) === JSON.stringify(before), edited.prefixes);
    await until('new alternative has a preview', () => history.locator('.take[data-take]').count(), (count) => count === 11);
    await choose(2);
    await expectFit(0.25);
    check('old_tall_state_stays_selectable_after_generation', (await asset()).version === 2 && (await asset()).versions === 11);
  } else seen.skipped_render = 'no free local execution in this cell';

  await choose(2);
  await expectFit(0.25);
  await focus.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await focus.waitFor({ state: 'detached' });
  await settled(page);
  check('back_restores_the_expanded_palette_and_wander_camera', (await palette()).shut === expanded.shut && (await palette()).saved === expanded.saved && sameCamera(wander, await camera()));
  if (!(await palette()).shut) await tools.getByLabel('Hide the tools', { exact: true }).click();
  const collapsed = await palette();
  const beforeGestures = await camera();
  await focusByClick();
  await expectFit(0.25);
  const panRoom = await card.locator('.focus-viewer').boundingBox();
  await page.mouse.move(panRoom.x + 5, panRoom.y + 5);
  await page.mouse.down();
  await page.mouse.move(panRoom.x + 20, panRoom.y + 14);
  await page.mouse.up();
  await focus.waitFor({ state: 'detached' });
  await settled(page);
  check('panning_the_viewer_exits_focus_without_a_camera_jump', sameCamera(beforeGestures, await camera()) && (await palette()).shut === collapsed.shut && (await palette()).saved === collapsed.saved);
  await focusByClick();
  await expectFit(0.25);
  const wheelRoom = await card.locator('.focus-viewer').boundingBox();
  await page.mouse.move(wheelRoom.x + 5, wheelRoom.y + 5);
  await page.mouse.wheel(0, 120);
  await focus.waitFor({ state: 'detached' });
  await settled(page);
  check('wheel_in_the_viewer_exits_focus_without_zooming_wander', sameCamera(beforeGestures, await camera()) && (await palette()).shut === collapsed.shut && (await palette()).saved === collapsed.saved);
  check('no_focus_workspace_or_carry_ghost_remains', await page.locator('[data-focused-asset], .carry-ghost').count() === 0);
  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
