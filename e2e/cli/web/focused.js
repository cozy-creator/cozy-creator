// Focus animates the canvas into the asset, then uses its own image camera. Explicit exits restore Wander.
// Real imported states exercise full-fit viewing, preview versus approval, image pan/zoom and exits.
// A weightless local image render, where available, proves editing an older state prunes nothing.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { boxOf, calcifer, carry, fitCanvas, implementationRow, lastLook, launch, lookAt, MP4, noStall, openBox, openCanvas, png, projectFromPage, renderOf, settled, signIn, STEP_MS, until, upload } = require('./page');

const NAME = 'focus_image';
const STANDARD_ASPECTS = ['21:9', '16:9', '3:2', '4:3', '5:4', '1:1', '4:5', '3:4', '2:3', '9:16'];
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
  // Signed in once the page is up: going on before then drops its session.
  await page.waitForSelector('.top');


  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-focused-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const project = await projectFromPage(page);
  const id = project.id;
  const session = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${session.token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' },
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
    return upload(session.token, path, id);
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
  const wanderPoint = async () => {
    const media = boxOf(page, NAME).locator('.media');
    await media.waitFor();
    return media.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      const left = Math.max(rect.left, document.querySelector('aside.tools')?.getBoundingClientRect().right ?? 0);
      const right = Math.min(rect.right, innerWidth - 8);
      const top = Math.max(rect.top, 64);
      const bottom = Math.min(rect.bottom, innerHeight - 12);
      if (right <= left || bottom <= top) throw new Error('the Wander image has no visible click target');
      return { x: (left + right) / 2, y: (top + bottom) / 2 };
    });
  };
  const focusByDoubleClick = async () => {
    const point = await wanderPoint();
    await page.mouse.dblclick(point.x, point.y);
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
  const frameFits = (look, ratio) => look.media.width > 20 && look.media.height > 20
    && look.media.x >= look.viewer.x - 2 && look.media.y >= look.viewer.y - 2
    && look.media.right <= look.viewer.right + 2 && look.media.bottom <= look.viewer.bottom + 2
    && Math.abs((look.media.width / look.media.height) / ratio - 1) < 0.03
    && Math.min(Math.abs(look.media.width - look.viewer.width), Math.abs(look.media.height - look.viewer.height)) < 3;
  const fits = (look, ratio) => look.loaded && frameFits(look, ratio);
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
  const selectionPoint = await wanderPoint();
  await page.mouse.click(selectionPoint.x, selectionPoint.y);
  await page.locator(`.react-flow__node.selected:has(.card[data-asset="${NAME}"])`).waitFor();
  check('single_click_selects_without_entering_focus_or_moving_the_camera', await page.locator('[data-focused-asset]').count() === 0 && sameCamera(wander, await camera()));
  await boxOf(page, NAME).locator('.head .nm').click();
  const inlineName = boxOf(page, NAME).locator('.head input[aria-label="Name"]');
  await inlineName.waitFor();
  check('Wander_title_opens_inline_rename_without_focus_or_camera_movement', await page.locator('[data-focused-asset]').count() === 0 && await inlineName.inputValue() === NAME && sameCamera(wander, await camera()));
  await inlineName.fill(NAME);
  await inlineName.press('Enter');
  await inlineName.waitFor({ state: 'detached' });
  await boxOf(page, NAME).locator('.head .nm').dblclick();
  await inlineName.waitFor();
  check('Wander_title_double_click_stays_in_rename', await page.locator('[data-focused-asset]').count() === 0 && sameCamera(wander, await camera()));
  await inlineName.fill(NAME);
  await inlineName.press('Enter');
  await inlineName.waitFor({ state: 'detached' });
  check('Wander_inline_name_commit_preserves_the_asset_and_camera', (await asset()).name === NAME && await page.locator('[data-focused-asset]').count() === 0 && sameCamera(wander, await camera()));
  await focusByDoubleClick();
  const focusedCanvas = await camera();
  const wide = await expectFit(4);
  check('node_double_click_opens_one_focus_workspace', await page.locator('[data-focused-asset]').count() === 1 && await card.count() === 1);
  check('entry_animates_the_canvas_into_the_selected_asset', !sameCamera(wander, focusedCanvas), { before: wander, after: focusedCanvas });
  check('focus_keeps_temporary_palette_collapse_out_of_saved_settings', (await palette()).saved === expanded.saved);
  const background = await focus.evaluate((element) => ({ color: getComputedStyle(element).backgroundColor, image: getComputedStyle(element).backgroundImage }));
  check('focus_keeps_the_canvas_background_visible', background.image === 'none' && /^(transparent|rgba\(.*[,/]\s*0\))$/.test(background.color), background);
  check('right_hand_original_previews_are_available', await history.isVisible() && await history.locator('.take[data-take]').count() === 10);
  check('focus_has_no_linear_counter_or_arrow_controls', await focus.locator('.count:visible').count() === 0 && await focus.getByRole('button', { name: /^(Previous version|Next version)$/ }).count() === 0);
  const title = focus.getByRole('button', { name: 'Rename node', exact: true });
  check('focus_title_is_large_and_version_is_separate', await title.innerText() === NAME && await title.evaluate((element) => parseFloat(getComputedStyle(element).fontSize) >= 24) && await focus.getByLabel('Take', { exact: true }).innerText() === 'v1');
  await title.click();
  const titleInput = focus.locator('.focus-heading input[aria-label="Name"]');
  await titleInput.fill(NAME);
  await titleInput.press('Enter');
  await title.waitFor();
  check('title_is_editable_without_leaving_focus', await focus.isVisible() && (await asset()).name === NAME);
  check('seed_and_advanced_fold_controls_are_absent', await focus.locator('[data-field="seed"], .seed, .advanced-toggle').count() === 0);
  check('image_workspace_has_no_asset_description_box', await page.locator('.desc-field:visible').count() === 0);
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
  check('thumbnail_click_previews_without_approving', (await asset()).version === 1 && (await asset()).approved === 1 && await focus.getByLabel('Take', { exact: true }).innerText() === 'v2');
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

  const newImage = card.locator('.take.add[aria-label="Its next take"]');
  await newImage.scrollIntoViewIfNeeded();
  await newImage.click();
  await prompt.waitFor();
  await until('New image begins from its displayed preview', () => prompt.innerText(), (text) => /<self\.10>/i.test(text) && text.includes('make her smile'));
  await choose(1);
  await newImage.click();
  await prompt.waitFor();
  await until('the source prompt keeps the words and updates its reference', asset, (item) => /<self\.1>/i.test(item.fields?.[promptField] ?? '') && item.fields[promptField].includes('make her smile'));

  check('new_image_is_explicit_and_does_not_discard_variants', (await asset()).versions === 10 && await history.locator('.take[data-take]').count() === 10);

  const beforeReferenceDrop = await asset();
  const namesBeforeReferenceDrop = (await state()).assets.map((item) => item.name).sort();
  const referencedImage = history.locator('.take[data-take="2"]');
  await referencedImage.scrollIntoViewIfNeeded();
  const imageDropTarget = await card.locator('.focus-viewer .media').boundingBox();
  await carry(page, referencedImage, { x: imageDropTarget.x + imageDropTarget.width / 2, y: imageDropTarget.y + imageDropTarget.height - 12 });
  const afterReferenceDrop = await until('dropping an image onto an image tool appends an explicit reference', asset,
    (item) => /<self\.1>/i.test(item.fields?.[promptField] ?? '') && /<(?:self|focus_image)\.2>/i.test(item.fields[promptField]) && item.fields[promptField].includes('make her smile'));
  // The New tile stays active while editing; the source state is the explicit self.1 reference above.
  check('dropping_an_image_into_the_editor_keeps_its_source_and_approval', afterReferenceDrop.version === beforeReferenceDrop.version && afterReferenceDrop.approved === beforeReferenceDrop.approved && afterReferenceDrop.sha256 === beforeReferenceDrop.sha256 && JSON.stringify(afterReferenceDrop.prefixes.map((version) => [version.number, version.main.sha256])) === JSON.stringify(beforeReferenceDrop.prefixes.map((version) => [version.number, version.main.sha256])) && await card.locator('.take.add.on[aria-label="Its next take"]').count() === 1 && await prompt.isVisible());
  check('dropping_an_image_into_the_editor_does_not_create_a_node', JSON.stringify((await state()).assets.map((item) => item.name).sort()) === JSON.stringify(namesBeforeReferenceDrop));

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

    // A bounded AspectRatio snaps to the standard supported ratios while its frame stays free until release.
    await newImage.click();
    const aspectBadge = card.locator('.focus-viewer .media > [data-output-aspect]');
    await aspectBadge.waitFor();
    const beforeResize = await until('the square edit frame is fitted before resizing', measure, (look) => frameFits(look, 1));
    const beforeResizeCamera = await camera();
    const beforeResizeWidth = (await api(`${at}/canvas`)).cards[NAME].w;
    const corner = await card.getByRole('slider', { name: 'Its size', exact: true }).boundingBox();
    const fromCorner = { x: corner.x + corner.width / 2, y: corner.y + corner.height / 2 };
    await page.mouse.move(fromCorner.x, fromCorner.y);
    await page.mouse.down();
    await card.locator('.focus-viewer[data-resizing="true"]').waitFor();
    await page.mouse.move(fromCorner.x - 150, fromCorner.y + 90, { steps: 12 });
    const heldResize = await measure();
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    const stableResize = await measure();
    const requestedShape = await aspectBadge.getAttribute('data-output-aspect');
    const [ratioWidth, ratioHeight] = requestedShape.split(':').map(Number);
    const requestedRatio = ratioWidth / ratioHeight;
    check('aspect_drag_holds_free_bounds_until_release', STANDARD_ASPECTS.includes(requestedShape) && sameRect(beforeResize.viewer, heldResize.viewer) && sameRect(heldResize.media, stableResize.media)
      && Math.abs(heldResize.media.x - beforeResize.media.x) < 2 && Math.abs(heldResize.media.y - beforeResize.media.y) < 2
      && Math.abs(heldResize.media.width - (beforeResize.media.width - 150)) < 4 && Math.abs(heldResize.media.height - heldResize.media.width / requestedRatio) < 4
      && !frameFits(heldResize, requestedRatio), { beforeResize, heldResize, stableResize, requestedShape });
    await shot('aspect-dragging');
    await page.mouse.up();
    await until('aspect dragging ends after release', () => card.locator('.focus-viewer').getAttribute('data-resizing'), (value) => value === null);
    await until('the exact requested ratio is saved', asset, (item) => item.fields?.aspect_ratio === requestedShape);
    const afterResize = await until('the output refits to the exact released ratio', measure, (look) => frameFits(look, requestedRatio)
      && Math.abs((look.media.width / look.media.height) / requestedRatio - 1) < 0.001);
    check('aspect_release_refits_without_changing_the_Wander_width_or_camera', Number.isFinite(beforeResizeWidth) && (await api(`${at}/canvas`)).cards[NAME].w === beforeResizeWidth
      && sameCamera(beforeResizeCamera, await camera()) && Math.abs((await imageCamera()).zoom - 1) < 0.0001, { beforeResizeWidth, afterResize });
    const badgeRect = await aspectBadge.boundingBox();
    check('requested_aspect_badge_stays_inside_the_output_frame', badgeRect.x >= afterResize.media.x - 2 && badgeRect.y >= afterResize.media.y - 2
      && badgeRect.x + badgeRect.width <= afterResize.media.right + 2 && badgeRect.y + badgeRect.height <= afterResize.media.bottom + 2);
    const automatic = focus.getByRole('button', { name: 'Auto aspect ratio', exact: true });
    if (await automatic.count()) await automatic.click();
    await choose(2);
    await expectFit(0.25);
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
  await focusByDoubleClick();
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
  for (let step = 0; step < 12 && (await imageCamera()).zoom < 7; step++) {
    const before = (await imageCamera()).zoom;
    await focus.getByRole('button', { name: 'Zoom in image', exact: true }).click();
    await until('the local image camera reaches 700 percent', imageCamera, (position) => position.zoom > before);
  }
  check('high_image_zoom_keeps_the_focused_canvas_camera_unchanged', (await imageCamera()).zoom >= 7 && sameCamera(gestureCanvas, await camera()));
  await page.keyboard.press('Escape');
  await focus.waitFor({ state: 'detached' });
  await until('Escape animates to the original Wander camera', camera, (position) => sameCamera(beforeGestures, position));
  await settled(page);
  check('Escape_explicitly_exits_focus_without_a_camera_jump', sameCamera(beforeGestures, await camera()) && (await palette()).shut === collapsed.shut && (await palette()).saved === collapsed.saved);
  await focusByDoubleClick();
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

  // Drag a real four-node selection, so one gesture can exercise both React Flow stop callbacks.
  const referenceTools = (await api(`/v1/tools?project=${id}&palette=true`)).tools;
  const reference = referenceTools.map((tool) => ({ tool, prompt: tool.fields.find((field) => field.type === 'prompt' || field.type === 'words') }))
    .find(({ tool, prompt }) => tool.makes === 'image' && prompt?.type === 'prompt' && (!prompt.takes || prompt.takes.includes('image')));
  if (!reference) throw new Error('an image tool whose first prompt accepts image references is unavailable');
  const bulkWords = '<self.2> Keep these words and add the four reference images.';
  await api(`${at}/assets/${NAME}`, 'PATCH', { tool: reference.tool.id, fields: { [reference.prompt.field]: bulkWords } });
  await until('the image reference tool and seed words are saved', asset, (item) => item.tool === reference.tool.id && item.fields?.[reference.prompt.field] === bulkWords);
  await api(`${at}/canvas`, 'PATCH', { cards: { [NAME]: { x: 1000, y: 280, w: 640 } } });
  const bulkNames = Array.from({ length: 4 }, (_, number) => `bulk_reference_${number + 1}`);
  const clientImports = new Set();
  const observeImports = (response) => {
    const path = new URL(response.url()).pathname;
    if (response.request().method() !== 'GET' || !response.ok() || (path !== at && !bulkNames.some((name) => path === `${at}/assets/${name}`))) return;
    void response.json().then((body) => {
      for (const item of [...(body.assets ?? []), ...(body.asset ? [body.asset] : [])]) if (bulkNames.includes(item.name)) clientImports.add(item.name);
    }, () => {});
  };
  page.on('response', observeImports);
  for (let number = 0; number < 4; number++) {
    const name = bulkNames[number];
    await api(`${at}/imports`, 'POST', { source: await picture(11 + number, 480, 320), name, exact: true, at: { x: 160 + (number % 2) * 260, y: 180 + Math.floor(number / 2) * 220, w: 160 } });
  }
  await until('the browser client reads all four imported assets', () => [...clientImports], (names) => bulkNames.every((name) => names.includes(name)));
  page.off('response', observeImports);
  check('bulk_imports_reach_the_client_before_fitting', clientImports.size === 4);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  const sourceNodes = bulkNames.map((name) => page.locator(`.react-flow__node:has(.card[data-asset="${name}"])`));
  await until('all four imported nodes are visible after a synchronized refit', async () => {
    await fitCanvas(page);
    return { visible: await Promise.all(sourceNodes.map((node) => node.isVisible())), camera: await camera() };
  }, (view) => view.visible.every(Boolean));
  const bounds = await Promise.all(sourceNodes.map((node) => node.boundingBox()));
  const enclosing = { left: Math.min(...bounds.map((box) => box.x)) - 12, top: Math.min(...bounds.map((box) => box.y)) - 12,
    right: Math.max(...bounds.map((box) => box.x + box.width)) + 12, bottom: Math.max(...bounds.map((box) => box.y + box.height)) + 12 };
  const pane = await page.locator('.react-flow').boundingBox();
  const navigation = await page.locator('.top').boundingBox();
  const paletteBounds = await tools.boundingBox();
  enclosing.left = Math.max(enclosing.left, pane.x + 8, paletteBounds.x + paletteBounds.width + 8);
  enclosing.top = Math.max(enclosing.top, pane.y + 8, navigation.y + navigation.height + 8);
  enclosing.right = Math.min(enclosing.right, pane.x + pane.width - 8);
  enclosing.bottom = Math.min(enclosing.bottom, pane.y + pane.height - 8);
  await page.mouse.click(enclosing.left, enclosing.top);
  await page.keyboard.down('Shift');
  await page.mouse.move(enclosing.left, enclosing.top);
  await page.mouse.down();
  await page.mouse.move(enclosing.right, enclosing.bottom, { steps: 16 });
  await page.mouse.up();
  await page.keyboard.up('Shift');
  const selected = await until('four image nodes are selected by the pointer rectangle', () => page.locator('.react-flow__node.selected .card[data-asset]').evaluateAll((cards) => cards.map((card) => card.dataset.asset).sort()),
    (names) => JSON.stringify(names) === JSON.stringify([...bulkNames].sort()));
  check('bulk_reference_drop_uses_four_actually_selected_nodes', selected.length === 4);
  const selection = page.locator('.react-flow__nodesselection-rect');
  await selection.waitFor();
  const group = await selection.boundingBox();
  const destination = await boxOf(page, NAME).locator('.media').boundingBox();
  const stamp = (item) => item ? { name: item.name, tool: item.tool, version: item.version, approved: item.approved, pinned: item.pinned, sha: item.sha256, versions: item.versions,
    sources: (item.prefixes ?? []).map((version) => [version.number, version.main?.sha256]) } : null;
  const projectBeforeBulk = await state();
  const targetBeforeBulk = stamp(projectBeforeBulk.assets.find((item) => item.name === NAME));
  const sourcesBeforeBulk = bulkNames.map((name) => stamp(projectBeforeBulk.assets.find((item) => item.name === name)));
  const pendingBulkWrites = new Set();
  let bulkWriteCount = 0;
  let bulkWriteActivity = Date.now();
  const bulkRequest = (request) => {
    if (request.method() !== 'PATCH' || new URL(request.url()).pathname !== `${at}/assets/${NAME}`) return;
    pendingBulkWrites.add(request);
    bulkWriteCount++;
    bulkWriteActivity = Date.now();
  };
  const bulkFinished = (request) => { if (pendingBulkWrites.delete(request)) bulkWriteActivity = Date.now(); };
  page.on('request', bulkRequest);
  page.on('requestfinished', bulkFinished);
  page.on('requestfailed', bulkFinished);
  await page.mouse.move(group.x + group.width / 2, group.y + group.height / 2);
  await page.mouse.down();
  await page.mouse.move(group.x + group.width / 2 + 12, group.y + group.height / 2, { steps: 3 });
  await page.mouse.move(destination.x + destination.width / 2, destination.y + destination.height / 2, { steps: 20 });
  await page.mouse.up();
  await until('the group drop saves all four external image references', asset, (item) => bulkNames.every((name) => item.fields?.[reference.prompt.field]?.includes(`<${name}>`)));
  await until('all group-drop asset writes have finished', () => ({ count: bulkWriteCount, pending: pendingBulkWrites.size, quiet: Date.now() - bulkWriteActivity }),
    (writes) => writes.count > 0 && writes.pending === 0 && writes.quiet >= 500);
  page.off('request', bulkRequest);
  page.off('requestfinished', bulkFinished);
  page.off('requestfailed', bulkFinished);
  const projectAfterBulk = await state();
  const targetAfterBulk = projectAfterBulk.assets.find((item) => item.name === NAME);
  const tags = [...targetAfterBulk.fields[reference.prompt.field].matchAll(/<([^>]+)>/g)].map((match) => match[1]);
  const external = tags.filter((tag) => bulkNames.includes(tag));
  check('one_group_drag_appends_exactly_four_external_references', external.length === 4 && bulkNames.every((name) => external.filter((tag) => tag === name).length === 1) && tags.filter((tag) => tag === 'self.2').length === 1 && tags.length === 5, { tags, bulkWriteCount });
  check('bulk_reference_drop_preserves_the_target_source_and_approval', JSON.stringify(stamp(targetAfterBulk)) === JSON.stringify(targetBeforeBulk));
  check('bulk_reference_drop_keeps_all_source_assets_and_creates_no_nodes', JSON.stringify(bulkNames.map((name) => stamp(projectAfterBulk.assets.find((item) => item.name === name)))) === JSON.stringify(sourcesBeforeBulk)
    && JSON.stringify(projectAfterBulk.assets.map((item) => item.name).sort()) === JSON.stringify(projectBeforeBulk.assets.map((item) => item.name).sort()));
  await shot('bulk-reference-drop');
  if (await page.locator('[data-focused-asset]').count()) {
    await page.getByRole('button', { name: 'Back to canvas', exact: true }).click();
    await page.locator('[data-focused-asset]').waitFor({ state: 'detached' });
    await settled(page);
  }

  // H3 edits Summary before Beats; a real tiny imported clip exercises the separate completed-view face without a GPU.
  const videoTool = metadata.tools.find((item) => item.makes === 'video' && item.id === 'minimax-h3/generate'
    && item.fields.some((field) => field.type === 'group' && field.field === 'beats')
    && !item.fields.some((field) => field.required && field.type === 'slot' && field.accept === 'video'));
  if (!videoTool) throw new Error('the H3 video tool metadata needed by the Summary/Beats fixture is unavailable');
  const VIDEO = 'focus_video';
  await api(`${at}/assets`, 'POST', { name: VIDEO, exact: true, kind: 'video', tool: videoTool.id, fields: { description: 'Stored asset description stays out of the UI.', summary: '<focus_image.1> A focused video scene.', beats: [{ duration: 5, description: '<focus_image.2> A large focused video beat.' }, { duration: 5, description: '<focus_image.1> The second focused video beat.' }] }, at: { x: 1100, y: 600, w: 360 } });
  await boxOf(page, VIDEO).waitFor();
  await fitCanvas(page);
  const videoWander = await camera();
  const videoTile = await boxOf(page, VIDEO).locator('.media').boundingBox();
  await page.mouse.dblclick(videoTile.x + videoTile.width / 2, videoTile.y + videoTile.height / 2);
  const videoFocus = page.locator(`[data-focused-asset="${VIDEO}"]`);
  await videoFocus.waitFor();
  await page.locator(`[data-focused-asset="${VIDEO}"]:not([data-entering])`).waitFor();
  const videoFocusedCanvas = await camera();
  // Empty, it has no New: what it's written in is open (cl-925). Its frame holds it (cl-938): Summary, then its Beats, each a section opened by
  // its head; under it its tool, Render and its other fields.
  check('empty_video_has_no_new', await videoFocus.getByRole('button', { name: 'New video', exact: true }).count() === 0);
  const videoFrame = videoFocus.locator('.card.here .in-frame .frame-sections').first();
  await videoFrame.waitFor();
  const heads = await videoFrame.locator('.beat-toggle b').allInnerTexts();
  await videoFrame.locator('.beat-toggle', { hasText: 'Beat 1' }).click();
  const beatPrompt = videoFrame.locator('.mention-text[data-field="beats.0.description"]');
  await beatPrompt.waitFor();
  const summaryPrompt = videoFrame.locator('.mention-text[data-field="summary"]');
  await summaryPrompt.waitFor({ state: 'attached' });
  const formGeometry = await videoFocus.locator('.card.here .media').first().boundingBox();
  const videoRoom = await videoFocus.locator('.focus-body').boundingBox();
  const beatGeometry = await beatPrompt.boundingBox();
  check('video_summary_precedes_the_prominent_editable_Beats', heads[0] === 'Summary' && heads.slice(1).length === 2 && heads.slice(1).every((head) => /^Beat \d/.test(head)) && formGeometry.width > videoRoom.width * 0.5 && await beatPrompt.evaluate((element) => parseFloat(getComputedStyle(element).fontSize) >= 18) && beatGeometry.height >= 100, { heads, formGeometry, videoRoom, beatGeometry });
  await beatPrompt.fill('<focus_image.2> Edited in the large video Beats form.');
  await videoFocus.getByRole('button', { name: 'Rename node', exact: true }).focus();
  await until('video Beats edits are saved', () => state(), (value) => value.assets.find((item) => item.name === VIDEO)?.fields?.beats?.[0]?.description === '<focus_image.2> Edited in the large video Beats form.');
  // In Focus every field is out, none under More or Advanced (cl-934); H3 has no seed, quality, steps or turbo to set (controls.ts).
  check('video_seed_and_advanced_fold_controls_are_absent', await videoFocus.locator('[data-field="seed"], .seed, .advanced-toggle, .more-toggle').count() === 0 && await videoFocus.getByRole('button', { name: 'Advanced', exact: true }).count() === 0);
  check('H3_quality_steps_and_turbo_controls_are_hidden', await videoFocus.locator('[aria-label="Quality"], [aria-label="Steps"], [aria-label="Turbo"]').count() === 0);
  check('video_workspace_hides_the_stored_asset_description', await page.locator('.desc-field:visible').count() === 0 && (await state()).assets.find((item) => item.name === VIDEO).fields.description === 'Stored asset description stays out of the UI.');

  const clip = join(images, 'completed.mp4');
  writeFileSync(clip, MP4);
  await api(`${at}/assets/${VIDEO}/versions`, 'POST', { source: await upload(session.token, clip, id) });
  const completedVideo = videoFocus.locator('.here-side .take[data-take="1"]');
  const newVideo = videoFocus.getByRole('button', { name: 'New video', exact: true });
  await completedVideo.waitFor();
  await completedVideo.click();
  // It plays from the transport under it (cl-965).
  await videoFocus.locator('.tl-transport button[aria-label="Play"]').click();
  const completedPlayer = videoFocus.locator('.focus-viewer video').first();
  await completedPlayer.waitFor({ state: 'visible' });
  // Hold the one-second fixture on a decoded frame while its view controls are checked.
  await completedPlayer.evaluate((element) => { element.loop = true; });
  await until('the imported completed video can actually play', () => completedPlayer.evaluate((element) => {
    const loaded = element.readyState >= 2 && element.videoWidth > 0;
    if (loaded) element.pause();
    return loaded;
  }), Boolean);
  const completedGeometry = await videoFocus.locator('.focus-viewer').boundingBox();
  check('completed_video_has_a_prominent_view_only_face', completedGeometry.width > videoRoom.width * 0.5 && completedGeometry.height > 120 && !await summaryPrompt.isVisible() && !await beatPrompt.isVisible() && await videoFocus.locator('.mention-text:visible').count() === 0, { completedGeometry, videoRoom });
  // An imported take has no run to say its seams, so no segment is offered to redo (cl-951): web-scrub's rendered take is.
  check('completed_video_has_models_and_no_redo_without_its_seams', await videoFocus.getByRole('button', { name: 'Models and LoRAs', exact: true }).isVisible() && await videoFocus.locator('.tl-take .redo-segment').count() === 0);
  await shot('completed-video');
  // With a take it has New (cl-925).
  check('video_has_a_new_state_tile', await newVideo.isVisible());
  await newVideo.click();
  check('new_video_tile_keeps_focus_and_does_not_render_implicitly', await videoFocus.isVisible() && (await state()).assets.find((item) => item.name === VIDEO).versions === 1);
  // New keeps the frame's sections as they were, the one open still open.
  check('new_video_hides_the_main_player_and_keeps_the_prompt_workspace', !await videoFocus.locator('.focus-viewer').isVisible() && await videoFrame.isVisible() && (await videoFrame.locator('.beat-toggle[aria-expanded="true"]').count()) === 1);
  const beatRows = videoFrame.locator('.beats-edit[data-field="beats"] > .beat');
  const summaryOpen = () => videoFrame.locator('.frame-first .beat-toggle').getAttribute('aria-expanded');
  // Closed panels clip their still-laid-out text; descendant isVisible() does not measure that clipping.
  const readBeatPanels = () => beatRows.evaluateAll((rows) => rows.map((row) => {
    const panel = row.querySelector('.beat-panel');
    return { expanded: row.querySelector('.beat-toggle').getAttribute('aria-expanded'),
      hidden: panel.getAttribute('aria-hidden'), inert: panel.hasAttribute('inert'),
      height: panel.getBoundingClientRect().height, contentHeight: panel.querySelector('.beat-body').scrollHeight,
      promptHeight: row.querySelector('.mention-text').getBoundingClientRect().height,
      animating: panel.getAnimations().some((animation) => animation.playState === 'running') };
  }));
  const beatPanelMatches = (panel, open) => panel.expanded === String(open) && panel.hidden === String(!open) && panel.inert === !open
    && !panel.animating && panel.promptHeight > 100 && (open ? panel.height > 100 && panel.height >= panel.contentHeight - 2 : panel.height <= 1);
  const firstBeatOpen = (panels) => panels.length === 2 && beatPanelMatches(panels[0], true) && beatPanelMatches(panels[1], false);
  const firstBeatToggle = beatRows.first().getByRole('button', { name: 'Beat 1', exact: true });
  const secondBeatToggle = beatRows.nth(1).getByRole('button', { name: 'Beat 2', exact: true });
  // One section open at a time (cl-938): a beat opened shuts Summary and the other beats.
  if ((await firstBeatToggle.getAttribute('aria-expanded')) !== 'true') await firstBeatToggle.click();
  const firstBeatPanels = await until('the first Beat is open and the second is fully clipped', readBeatPanels, firstBeatOpen);
  check('video_editor_expands_the_first_beat_and_collapses_the_rest', firstBeatOpen(firstBeatPanels) && (await summaryOpen()) === 'false', firstBeatPanels);
  await secondBeatToggle.click();
  const secondBeatOpen = (panels) => panels.length === 2 && beatPanelMatches(panels[0], false) && beatPanelMatches(panels[1], true);
  const secondBeatPanels = await until('the second Beat finishes opening and the first finishes clipping', readBeatPanels, secondBeatOpen);
  check('expanding_another_beat_closes_the_previous_one', secondBeatOpen(secondBeatPanels) && (await summaryOpen()) === 'false', secondBeatPanels);
  // Behind the section being written, faded, the picture it mentions, as large as it can be (cl-949: full-bleed).
  for (const [place, head, ratio] of [['summary', 'Summary', 4], ['beats.0.description', 'Beat 1', 0.25]]) {
    const toggle = videoFrame.locator('.beat-toggle').filter({ hasText: head }).first();
    if ((await toggle.getAttribute('aria-expanded')) !== 'true') await toggle.click();
    await videoFrame.locator(`.mention-text[data-field="${place}"]`).focus();
    const behind = videoFocus.locator('.card.here .in-frame > .backdrop .spread-picture img');
    const painted = await until(`the ${place} field displays its own referenced image`, async () => (await behind.count()) === 1 && behind.evaluate((image) => {
      const bounds = image.getBoundingClientRect();
      return { loaded: image.complete && image.naturalWidth > 0, aspect: image.naturalWidth / image.naturalHeight, fit: getComputedStyle(image).objectFit, opacity: Number(getComputedStyle(image.closest('.backdrop')).opacity), width: bounds.width, height: bounds.height };
    }), (image) => image && image.loaded && Math.abs(image.aspect / ratio - 1) < 0.03 && image.width > 20 && image.height > 20 && image.opacity > 0);
    check(`${place}_references_are_faded_and_full_bleed`, painted.fit === 'cover' && painted.opacity > 0 && painted.opacity < 0.6, painted);
  }
  await shot('new-video');
  check('video_parameters_do_not_move_the_focused_canvas_camera', sameCamera(videoFocusedCanvas, await camera()));
  await videoFocus.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await videoFocus.waitFor({ state: 'detached' });
  await until('video exit restores its original Wander camera', camera, (position) => sameCamera(videoWander, position));
  check('no_focus_workspace_or_carry_ghost_remains', await page.locator('[data-focused-asset], .carry-ghost').count() === 0);

  // Palette drops stay in Wander; only an explicit body double-click opens the new holder.
  const canvasTool = await implementationRow(page, 'image');
  const canvasToolId = await canvasTool.getAttribute('data-tool');
  const beforeBasicDrop = new Set((await state()).assets.map((item) => item.name));
  await carry(page, page.locator('.tools [data-palette="nodes"] [data-node="image"]'), await openCanvas(page));
  const newHolder = await until('a basic image holder is created by dragging', state, (value) => value.assets.some((item) => !beforeBasicDrop.has(item.name)));
  const holder = newHolder.assets.find((item) => !beforeBasicDrop.has(item.name));
  check('a_basic_palette_drop_does_not_enter_focus', holder.kind === 'image' && !holder.tool && await page.locator('[data-focused-asset]').count() === 0);
  await fitCanvas(page);
  const holderRect = await boxOf(page, holder.name).locator('.media').boundingBox();
  await canvasTool.scrollIntoViewIfNeeded();
  await carry(page, canvasTool, { x: holderRect.x + holderRect.width / 2, y: holderRect.y + holderRect.height / 2 });
  await until('the dragged implementation attaches to the holder', state, (value) => value.assets.find((item) => item.name === holder.name)?.tool === canvasToolId);
  await settled(page);
  check('a_tool_attachment_does_not_enter_focus', await page.locator('[data-focused-asset]').count() === 0);
  const beforeToolDrop = new Set((await state()).assets.map((item) => item.name));
  await carry(page, canvasTool, await openCanvas(page));
  const toolState = await until('a dragged implementation creates another image holder', state, (value) => value.assets.some((item) => !beforeToolDrop.has(item.name) && item.tool === canvasToolId));
  const newTool = toolState.assets.find((item) => !beforeToolDrop.has(item.name) && item.tool === canvasToolId);
  await settled(page);
  check('a_new_tool_drop_does_not_enter_focus', await page.locator('[data-focused-asset]').count() === 0);
  await openBox(page, newTool.name);
  await renderOf(page, newTool.name);
  check('a_dropped_tool_opens_only_after_explicit_double_click', await page.locator(`[data-focused-asset="${newTool.name}"]`).isVisible());
  await page.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await page.locator('[data-focused-asset]').waitFor({ state: 'detached' });
  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
