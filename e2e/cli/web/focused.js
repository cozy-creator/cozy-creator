// Focus animates the canvas into the asset, then uses its own image camera. Explicit exits restore Wander.
// Real imported states exercise full-fit viewing, preview versus approval, image pan/zoom and exits.
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
  // The code URL finishes loading before boot() has redeemed it and mounted the signed-in shell.
  await page.locator('.top').waitFor();
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
  // Only this state needs a full-resolution source larger than the widest thumbnail.
  const first = await picture(1, 4096, 1024);
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
    await page.locator(`[data-focused-asset="${NAME}"]:not([data-entering])`).waitFor();
    await card.waitFor();
  };
  // A tap looks at a state; a double tap makes it current (630309269, explicit variant approval).
  const choose = async (number) => {
    const take = history.locator(`.take[data-take="${number}"]`);
    await take.scrollIntoViewIfNeeded();
    await take.click();

    await until(`state ${number} is selected in the previews`, () => history.locator('.take.on[data-take]').getAttribute('data-take'), (current) => current === String(number));
  };
  const approve = async (number) => {
    const item = await asset();
    const take = history.locator(`.take[data-take="${number}"]`);
    await take.scrollIntoViewIfNeeded();
    if (!item.pinned || item.approved !== number) await take.dblclick();
    else await take.click();
    await until(`state ${number} is approved separately`, asset, (current) => current.version === number && current.approved === number && current.pinned);
  };
  const measure = () => card.evaluate((element) => {
    const rect = (node) => {
      const value = node.getBoundingClientRect();
      return { x: value.x, y: value.y, width: value.width, height: value.height, right: value.right, bottom: value.bottom };
    };
    const viewer = element.querySelector('.focus-viewer');
    const media = viewer.querySelector('.focus-image-transform > .media');
    const loaded = [...media.querySelectorAll('.media-pixels img')].some((image) => image.complete && image.naturalWidth > 0);
    return { viewer: rect(viewer), media: rect(media), controls: rect(element.querySelector('.image-controls')), loaded };
  });
  const fits = (look, ratio) => look.loaded && look.media.width > 20 && look.media.height > 20
    && look.media.x >= look.viewer.x - 2 && look.media.y >= look.viewer.y - 2
    && look.media.right <= look.viewer.right + 2 && look.media.bottom <= look.viewer.bottom + 2
    && Math.abs((look.media.width / look.media.height) / ratio - 1) < 0.03
    && Math.min(Math.abs(look.media.width - look.viewer.width), Math.abs(look.media.height - look.viewer.height)) < 3;
  const expectFit = (ratio) => until('the full image fits the Focus viewer', measure, (look) => fits(look, ratio));
  const imageCamera = () => card.locator('.focus-image-transform').evaluate((element) => {
    const matrix = new DOMMatrix(getComputedStyle(element).transform);
    return { x: matrix.e, y: matrix.f, zoom: matrix.a };
  });
  const decodedOf = (locator) => locator.evaluate((element) => {
    const image = element.querySelector('img');
    return {
      sha: element.dataset.displayedSha?.replace(/^sha256:/, ''),
      requested: Number(element.dataset.requestedWidth),
      resolution: element.dataset.displayedResolution,
      displayed: Number(element.dataset.displayedWidth),
      width: image?.naturalWidth ?? 0,
      loaded: Boolean(image?.complete && image.naturalWidth > 0),
    };
  });
  const decodedImage = () => decodedOf(card.locator('.focus-viewer .buffered-image'));
  const zoomOutToFit = async () => {
    const button = focus.getByRole('button', { name: 'Zoom out image', exact: true });
    for (let step = 0; step < 20 && (await imageCamera()).zoom > 1.0001; step++) {
      const before = (await imageCamera()).zoom;
      await button.click();
      await until('zoom out approaches the fitted minimum', imageCamera, (position) => position.zoom < before);
    }
  };

  const wideSha = (await asset()).prefixes.find((version) => version.number === 1).main.sha256.replace(/^sha256:/, '');
  const wanderBuffer = boxOf(page, NAME).locator('.media-pixels > .buffered-image');
  const wanderFittedPicture = await until('the fitted Wander image is decoded', () => decodedOf(wanderBuffer),
    (picture) => picture.loaded && picture.sha === wideSha && picture.requested > 0 && picture.width >= picture.requested);
  // Wander's supported UI range ends at 400%; Focus separately exercises the original beyond 450%.
  await page.locator('.zoom-at').click();
  await page.getByRole('menuitem', { name: /^Zoom to 100%/ }).click();
  await settled(page);
  for (let step = 0; step < 12 && (await camera()).zoom < 3.999; step++) {
    const before = (await camera()).zoom;
    await page.getByRole('button', { name: 'Zoom in', exact: true }).click();
    await until('Wander zoom approaches its supported maximum', camera, (position) => position.zoom > before);
    await settled(page);
  }
  const wanderZoom = await until('the Wander UI reaches 400 percent', camera, (position) => Math.abs(position.zoom - 4) < 0.001);
  const wanderSharpPicture = await until('Wander decodes enough image pixels for the enlarged canvas node', () => decodedOf(wanderBuffer),
    (picture) => picture.loaded && picture.sha === wideSha && picture.requested > wanderFittedPicture.requested
      && picture.width >= Math.min(4096, picture.requested) && picture.displayed === picture.width
      && (picture.requested <= 2048 || picture.resolution === 'original'));
  check('free_roam_zoom_increases_decoded_image_resolution', wanderSharpPicture.width > wanderFittedPicture.width && Math.abs(wanderZoom.zoom - 4) < 0.001, { fitted: wanderFittedPicture, zoomed: wanderSharpPicture });
  check('free_roam_quality_check_keeps_the_selected_asset_on_the_canvas', await page.locator('[data-focused-asset]').count() === 0 && (await asset()).version === 1);
  await fitCanvas(page);

  const wander = await camera();
  const expanded = await palette();
  await focusByClick();
  const focusedCanvas = await camera();
  const wide = await expectFit(4);
  check('node_click_opens_one_focus_workspace', await page.locator('[data-focused-asset]').count() === 1 && await card.count() === 1);
  check('entry_animates_the_canvas_into_the_selected_asset', !sameCamera(wander, focusedCanvas), { before: wander, after: focusedCanvas });
  check('focus_keeps_temporary_palette_collapse_out_of_saved_settings', (await palette()).saved === expanded.saved);
  const background = await focus.evaluate((element) => ({ color: getComputedStyle(element).backgroundColor, image: getComputedStyle(element).backgroundImage }));
  check('focus_keeps_the_canvas_background_visible', background.image === 'none' && /^(transparent|rgba\(.*[,/]\s*0\))$/.test(background.color), background);
  check('right_hand_original_previews_are_available', await history.isVisible() && await history.locator('.take[data-take]').count() === 10);
  check('focus_has_no_linear_counter_or_arrow_controls', await focus.locator('.count:visible').count() === 0 && await focus.getByRole('button', { name: /^(Previous version|Next version)$/ }).count() === 0);
  const title = focus.getByRole('button', { name: 'Rename node', exact: true });
  check('focus_title_is_large_and_version_is_separate', await title.innerText() === NAME && await title.evaluate((element) => parseFloat(getComputedStyle(element).fontSize) >= 24) && await focus.getByLabel('Image version', { exact: true }).innerText() === 'v1');
  await title.click();
  const titleInput = focus.locator('.focus-heading input[aria-label="Name"]');
  await titleInput.fill(NAME);
  await titleInput.press('Enter');
  await title.waitFor();
  check('title_is_editable_without_leaving_focus', await focus.isVisible() && (await asset()).name === NAME);
  check('seed_and_advanced_fold_controls_are_absent', await focus.locator('[data-field="seed"], .seed, .advanced-toggle').count() === 0);
  await shot('wide');

  const fittedPicture = await until('the fitted large image displays a decoded thumbnail at the requested density', decodedImage,
    (picture) => picture.loaded && picture.sha === wideSha && picture.requested > 0 && picture.requested <= 2048
      && picture.resolution !== 'original' && picture.width >= picture.requested && picture.width <= 2048);
  await page.evaluate(() => {
    const frames = { total: 0, blank: 0, frame: 0 };
    window.__focusedResolutionFrames = frames;
    const sample = () => {
      const image = document.querySelector('.focus-viewer .buffered-image img');
      frames.total++;
      if (!image?.complete || !image.naturalWidth) frames.blank++;
      frames.frame = requestAnimationFrame(sample);
    };
    frames.frame = requestAnimationFrame(sample);
  });
  for (let step = 0; step < 12 && (await imageCamera()).zoom < 4.5; step++) {
    const before = (await imageCamera()).zoom;
    await focus.getByRole('button', { name: 'Zoom in image', exact: true }).click();
    await until('the image camera zooms toward full resolution', imageCamera, (position) => position.zoom > before);
  }
  const originalPicture = await until('zooming past 450 percent displays the actual 4k original', decodedImage,
    (picture) => picture.loaded && picture.sha === wideSha && picture.requested > 2048
      && picture.resolution === 'original' && picture.width === 4096 && picture.displayed === picture.width);
  const resolutionFrames = await page.evaluate(() => {
    const frames = window.__focusedResolutionFrames;
    cancelAnimationFrame(frames.frame);
    delete window.__focusedResolutionFrames;
    return { total: frames.total, blank: frames.blank };
  });
  check('image_resolution_grows_from_fitted_thumbnail_to_decoded_original', originalPicture.width > fittedPicture.width && (await imageCamera()).zoom >= 4.5, { fittedPicture, originalPicture });
  check('resolution_upgrade_keeps_a_decoded_image_on_every_frame', resolutionFrames.total > 0 && resolutionFrames.blank === 0, resolutionFrames);
  check('resolution_upgrade_preserves_focus_and_the_focused_canvas_camera', await focus.isVisible() && sameCamera(focusedCanvas, await camera()));
  await zoomOutToFit();
  await expectFit(4);
  await choose(2);
  const tall = await expectFit(0.25);
  check('wide_and_tall_states_fit_the_same_viewport_room', sameRect(wide.viewer, tall.viewer) && fits(wide, 4) && fits(tall, 0.25), { wide, tall });
  check('tool_controls_stay_pinned_when_state_ratio_changes', sameRect(wide.controls, tall.controls), { wide: wide.controls, tall: tall.controls });
  check('thumbnail_click_previews_without_approving', (await asset()).version === 1 && (await asset()).approved === 1 && await focus.getByLabel('Image version', { exact: true }).innerText() === 'v2');
  check('state_preview_does_not_change_the_focused_canvas_camera', sameCamera(focusedCanvas, await camera()));
  await approve(2);
  check('double_click_approves_the_preview_separately', (await asset()).version === 2 && (await asset()).approved === 2);
  await shot('tall');

  await choose(10);
  const side = await history.boundingBox();
  const beforeScroll = await history.evaluate((element) => element.scrollTop);
  await page.mouse.move(side.x + 30, side.y + 35);
  await page.mouse.wheel(0, 400);
  await until('right previews scroll', () => history.evaluate((element) => element.scrollTop), (top) => top > beforeScroll);
  check('scrolling_previews_keeps_focus_and_the_focused_canvas_camera', await focus.isVisible() && sameCamera(focusedCanvas, await camera()));
  await choose(1);
  await expectFit(4);
  check('previewing_another_state_keeps_approval', (await asset()).version === 2 && (await asset()).approved === 2);
  const edit = focus.getByRole('button', { name: 'Edit image', exact: true });
  const editBox = await edit.boundingBox();
  const imageBox = (await measure()).media;
  check('edit_overlay_is_in_the_lower_right_of_the_image', editBox.x + editBox.width / 2 > imageBox.x + imageBox.width / 2 && editBox.y + editBox.height / 2 > imageBox.y + imageBox.height / 2 && editBox.x + editBox.width <= imageBox.right + 2 && editBox.y + editBox.height <= imageBox.bottom + 2, { editBox, imageBox });
  check('models_button_is_explicit_below_the_image', await focus.getByRole('button', { name: 'Models and LoRAs', exact: true }).isVisible());
  const toolBeforePicker = (await asset()).tool;
  await focus.getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
  const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
  await picker.waitFor();
  check('model_dialog_does_not_add_a_per_popup_nsfw_switch', await picker.getByLabel(/Show NSFW/).count() === 0);
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  check('model_dialog_preserves_focus_tool_and_states', await focus.isVisible() && (await asset()).tool === toolBeforePicker && (await asset()).versions === 10);

  await edit.click();
  const prompt = focus.locator('.in-frame .mention-text').first();
  await prompt.waitFor();
  await until('Edit begins from the displayed preview rather than the approved state', () => prompt.innerText(), (text) => /<self\.1>/i.test(text) && text.includes('make her smile'));
  check('opening_edit_does_not_approve_the_preview', (await asset()).version === 2 && (await asset()).approved === 2);
  await choose(10);

  const newImage = history.locator('.take.add[aria-label="Its next take"]');
  await newImage.scrollIntoViewIfNeeded();
  await newImage.click();
  await prompt.waitFor();
  await until('New image begins from its displayed preview', () => prompt.innerText(), (text) => /<self\.10>/i.test(text) && text.includes('make her smile'));
  await choose(1);
  await newImage.click();
  await prompt.waitFor();
  await until('the source prompt keeps the words and updates its reference', asset, (item) => /<self\.1>/i.test(item.fields?.[promptField] ?? '') && item.fields[promptField].includes('make her smile'));

  check('new_image_is_explicit_and_does_not_discard_variants', (await asset()).versions === 10 && await history.locator('.take[data-take]').count() === 10);

  const local = (await api('/v1/status')).local_usable && !process.env.ALONE;
  if (local) {
    const before = (await asset()).prefixes.map((version) => [version.number, version.main.sha256]);
    const waiting = `${join(__dirname, '..', 'project', 'waits.py')}/wait`;
    await api(`${at}/assets/${NAME}`, 'PATCH', { tool: waiting, fields: { instructions: '<self.1> ', seconds: 3, aspect_ratio: '1:1' } });
    await until('weightless image tool attached', asset, (item) => item.tool === waiting);
    const approvalBefore = await asset();
    await newImage.click();
    await focus.getByRole('button', { name: 'Render', exact: true }).click();
    const running = await until('the image render has a pending state tile', asset, (item) => item.versions === 10 && item.making?.length === 1, 300_000);
    const run = running.making[0];
    const pending = history.locator(`.take.making[title="Run ${run}"]`);
    await pending.scrollIntoViewIfNeeded();
    await pending.click();
    check('pending_tile_preview_does_not_approve_the_render', (await asset()).version === approvalBefore.version && (await asset()).approved === approvalBefore.approved);
    const edited = await until('editing an older state appends another alternative', asset, (item) => item.versions === 11 && !item.making?.length, 300_000);
    check('generation_from_old_state_retains_every_alternative', JSON.stringify(edited.prefixes.slice(0, 10).map((version) => [version.number, version.main.sha256])) === JSON.stringify(before), edited.prefixes);
    const completed = edited.prefixes.find((version) => version.run === run);
    check('completed_pending_run_is_a_retained_numbered_state', Boolean(completed?.main?.sha256), edited.prefixes);
    if (!completed?.main?.sha256) throw new Error(`completed run ${run} has no retained image state`);
    await until('new alternative has a preview', () => history.locator('.take[data-take]').count(), (count) => count === 11);
    await until('clicked pending tile stays previewed after completion', () => history.locator('.take.on[data-take]').getAttribute('data-take'), (number) => number === String(completed.number));
    await until('the completed pending image is actually decoded and displayed', () => card.locator('.focus-viewer .buffered-image[data-displayed-sha]').evaluate((element) => {
      const image = element.querySelector('img');
      return { sha: element.dataset.displayedSha?.replace(/^sha256:/, ''), loaded: Boolean(image?.complete && image.naturalWidth > 0) };
    }), (picture) => picture.loaded && picture.sha === completed.main.sha256.replace(/^sha256:/, ''));
    check('pending_preview_survives_completion_without_changing_approval', (await asset()).version === approvalBefore.version && (await asset()).approved === approvalBefore.approved && await history.locator('.take.on[data-take]').getAttribute('data-take') === String(completed.number));
    await choose(2);
    await expectFit(0.25);
    check('old_tall_state_stays_previewable_after_generation', await history.locator('.take.on[data-take]').getAttribute('data-take') === '2' && (await asset()).versions === 11);
  } else seen.skipped_render = 'no free local execution in this cell';

  await approve(2);
  await expectFit(0.25);
  await focus.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await focus.waitFor({ state: 'detached' });
  await until('Back animates to the original Wander camera', camera, (position) => sameCamera(wander, position));
  await settled(page);
  check('back_restores_the_expanded_palette_and_wander_camera', (await palette()).shut === expanded.shut && (await palette()).saved === expanded.saved && sameCamera(wander, await camera()));
  if (!(await palette()).shut) await tools.getByLabel('Hide the tools', { exact: true }).click();
  const collapsed = await palette();
  const beforeGestures = await camera();
  await focusByClick();
  await expectFit(0.25);
  const gestureCanvas = await camera();
  await focus.getByRole('button', { name: 'Zoom in image', exact: true }).click();
  const zoomed = await until('the Focus image camera zooms in', imageCamera, (position) => position.zoom > 1);
  const wheelRoom = await card.locator('.focus-viewer').boundingBox();
  await page.mouse.move(wheelRoom.x + wheelRoom.width / 2, wheelRoom.y + wheelRoom.height / 2);
  await page.mouse.wheel(0, -120);
  const wheeled = await until('wheel zoom stays inside the Focus image camera', imageCamera, (position) => position.zoom > zoomed.zoom);
  check('wheel_zoom_keeps_focus_and_its_canvas_camera_unchanged', await focus.isVisible() && sameCamera(gestureCanvas, await camera()), { zoomed, wheeled });
  const namesBeforePan = (await state()).assets.map((item) => item.name).sort();
  const panRoom = await card.locator('.focus-viewer').boundingBox();
  await page.mouse.move(panRoom.x + panRoom.width / 2, panRoom.y + panRoom.height / 2);
  await page.mouse.down();
  await page.mouse.move(panRoom.x + panRoom.width / 2 + 40, panRoom.y + panRoom.height / 2 + 80, { steps: 8 });
  await page.mouse.up();
  const panned = await until('the Focus image camera pans', imageCamera, (position) => Math.abs(position.x - wheeled.x) > 1 || Math.abs(position.y - wheeled.y) > 1);
  check('panning_keeps_focus_and_does_not_move_its_canvas_camera', await focus.isVisible() && sameCamera(gestureCanvas, await camera()), { wheeled, panned });
  check('panning_an_image_does_not_drag_out_a_new_asset', await page.locator('.carry-ghost').count() === 0 && JSON.stringify((await state()).assets.map((item) => item.name).sort()) === JSON.stringify(namesBeforePan));
  check('Fit_button_is_removed', await focus.getByRole('button', { name: 'Fit image', exact: true }).count() === 0);
  await zoomOutToFit();
  await page.mouse.move(panRoom.x + panRoom.width / 2, panRoom.y + panRoom.height / 2);
  await page.mouse.wheel(0, 600);
  await until('zoom cannot go below the fitted full image', imageCamera, (position) => Math.abs(position.zoom - 1) < 0.0001 && Math.abs(position.x) < 0.5 && Math.abs(position.y) < 0.5);
  await expectFit(0.25);
  check('fitted_minimum_keeps_focus_open_and_its_canvas_camera_unchanged', await focus.isVisible() && sameCamera(gestureCanvas, await camera()));
  await page.keyboard.press('Escape');
  await focus.waitFor({ state: 'detached' });
  await until('Escape animates to the original Wander camera', camera, (position) => sameCamera(beforeGestures, position));
  await settled(page);
  check('Escape_explicitly_exits_focus_without_a_camera_jump', sameCamera(beforeGestures, await camera()) && (await palette()).shut === collapsed.shut && (await palette()).saved === collapsed.saved);
  await focusByClick();
  await expectFit(0.25);
  const mainImage = (await measure()).media;
  await page.mouse.click(mainImage.x + mainImage.width / 2, mainImage.y + mainImage.height / 2);
  check('clicking_the_main_image_does_not_exit_focus', await focus.isVisible());
  const backgroundPoint = await focus.evaluate((element) => {
    const viewport = element.getBoundingClientRect();
    const heading = element.querySelector('.focus-heading').getBoundingClientRect();
    const body = element.querySelector('.focus-body').getBoundingClientRect();
    return { x: viewport.right - 4, y: (heading.bottom + body.top) / 2 };
  });
  await page.mouse.click(backgroundPoint.x, backgroundPoint.y);
  await focus.waitFor({ state: 'detached' });
  await until('background exit animates to the original Wander camera', camera, (position) => sameCamera(beforeGestures, position));
  await settled(page);
  check('clicking_the_empty_focus_background_exits_without_a_camera_jump', sameCamera(beforeGestures, await camera()) && (await palette()).shut === collapsed.shut && (await palette()).saved === collapsed.saved);

  // An unrendered video tool gets a large editable Beats-first workspace and a New tile.
  const videoTool = metadata.tools.find((item) => item.makes === 'video' && item.id !== 'cozy/edit'
    && item.fields.some((field) => field.type === 'group' && field.field === 'beats')
    && !item.fields.some((field) => field.required && field.type === 'slot' && field.accept === 'video'));
  if (!videoTool) throw new Error('the video tool metadata needed by the Beats fixture is unavailable');
  const VIDEO = 'focus_video';
  await api(`${at}/assets`, 'POST', { name: VIDEO, exact: true, kind: 'video', tool: videoTool.id, fields: { summary: 'A focused video scene.', beats: [{ duration: 5, description: 'A large focused video beat.' }] }, at: { x: 1100, y: 600, w: 360 } });
  await boxOf(page, VIDEO).waitFor();
  await fitCanvas(page);
  const videoWander = await camera();
  const videoTile = await boxOf(page, VIDEO).locator('.media').boundingBox();
  await page.mouse.click(videoTile.x + videoTile.width / 2, videoTile.y + videoTile.height / 2);
  const videoFocus = page.locator(`[data-focused-asset="${VIDEO}"]`);
  await videoFocus.waitFor();
  await page.locator(`[data-focused-asset="${VIDEO}"]:not([data-entering])`).waitFor();
  const videoFocusedCanvas = await camera();
  const videoForm = videoFocus.locator('.focus-parameter-tool > .here-under');
  await videoForm.waitFor();
  const beatPrompt = videoForm.locator('[data-field="beats"] .beat .mention-text').first();
  await beatPrompt.waitFor();
  const formGeometry = await videoForm.boundingBox();
  const videoRoom = await videoFocus.locator('.focus-body').boundingBox();
  check('video_Beats_get_a_prominent_editable_workspace', await videoForm.locator('.focus-parameter-group h3').first().innerText() === 'Beats' && formGeometry.width > videoRoom.width * 0.5 && formGeometry.height > videoRoom.height * 0.6 && await beatPrompt.evaluate((element) => parseFloat(getComputedStyle(element).fontSize) >= 18) && (await beatPrompt.boundingBox()).height >= 100, { formGeometry, videoRoom });
  await beatPrompt.fill('Edited in the large video Beats form.');
  await videoFocus.getByRole('button', { name: 'Rename node', exact: true }).focus();
  await until('video Beats edits are saved', () => state(), (value) => value.assets.find((item) => item.name === VIDEO)?.fields?.beats?.[0]?.description === 'Edited in the large video Beats form.');
  check('video_seed_and_advanced_fold_controls_are_absent', await videoForm.locator('[data-field="seed"], .seed, .advanced-toggle').count() === 0 && await videoForm.getByRole('button', { name: 'Advanced', exact: true }).count() === 0);
  const newVideo = videoFocus.getByRole('button', { name: 'New video', exact: true });
  check('video_has_a_new_state_tile', await newVideo.isVisible());
  await newVideo.click();
  check('new_video_tile_keeps_focus_and_does_not_render_implicitly', await videoFocus.isVisible() && (await state()).assets.find((item) => item.name === VIDEO).versions === 0);
  check('video_parameters_do_not_move_the_focused_canvas_camera', sameCamera(videoFocusedCanvas, await camera()));
  await videoFocus.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await videoFocus.waitFor({ state: 'detached' });
  await until('video exit restores its original Wander camera', camera, (position) => sameCamera(videoWander, position));
  check('no_focus_workspace_or_carry_ghost_remains', await page.locator('[data-focused-asset], .carry-ghost').count() === 0);
  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
