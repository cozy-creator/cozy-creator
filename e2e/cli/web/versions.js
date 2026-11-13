// Retained asset states in a real browser on a real Calcifer: imported v1, edits append alternatives, and the original right-hand previews select the current image.
// `<name>` follows that selection while `<name.2>` remains fixed. There is no linear counter/arrow UI and editing an older state prunes nothing.
// Real CLI/API/browser writes retain rename, deletion/Undo, reclaim, typed input/refusal and render identity guarantees.
//   node versions.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: not on this computer's machine, which the rename during a
// render needs, as it renders the free `waits.py`. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { implementationRow, previewVersion, approveVersion, projectFromPage, projectUrl, calcifer, carry: carryTo, signIn, MP4, STEP_MS, boxOf, lastLook, launch, lookAt, noStall, photo, png, renderOf, settled, fitCanvas, until, upload, rowsOf, addBoxes, storedOf } = require('./page');

const fixture = join(__dirname, '..', 'project', 'waits.py');
const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
let last = () => null;
async function died(why) {
  failures.push(`${during}: ${why}`);
  seen.file = last();
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
  const refused = [];
  seen.refused = refused;
  page.on('response', (answer) => {
    const url = new URL(answer.url());
    if (answer.status() < 400 || !url.pathname.startsWith('/v1/') || answer.request().method() === 'HEAD') return;
    const row = { status: answer.status(), method: answer.request().method(), url: url.pathname, during };
    refused.push(row);
    answer.json().then(({ error }) => Object.assign(row, { code: error?.code, message: error?.message })).catch(() => {});
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = (path, body) => api(path, { method: 'POST', body: JSON.stringify(body) });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-versions-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const state = async () => (await api(at)).body;
  const asset = async (name) => (await state()).assets.find((row) => row.name === name);
  const folder = (await state()).path;
  seen.project = { id, folder };
  /** Undo of the newest edit of `name` that stands, as the line `cozy delete` prints does. */
  const undoOf = async (name) => post(`${at}/history/undo`, { entry: (await api(`${at}/history`)).body?.entries?.find((entry) => !entry.undone && entry.names?.some((held) => held.toLowerCase() === name.toLowerCase()))?.id });
  last = () => storedOf(folder);
  const box = (name) => boxOf(page, name);

  const pictures = mkdtempSync(join(tmpdir(), 'web-versions-'));
  const picture = (shade, width, height) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade, width, height));
    return path;
  };
  const imported = async (name, shade) => post(`${at}/imports`, { source: await upload(held?.token, picture(shade), id), name, exact: true, at: { x: 100 + shade * 4, y: 100 } });
  const joined = async (name, shade, width, height) => post(`${at}/assets/${name}/versions`, { source: await upload(held?.token, picture(shade, width, height), id) });
  // A camera's JPEG (cl-888).
  const importedPhoto = async (name) => {
    const path = join(pictures, `${name}.jpg`);
    writeFileSync(path, photo());
    return post(`${at}/imports`, { source: await upload(held?.token, path, id), name, exact: true, at: { x: 260, y: 100 } });
  };
  /** A box's pictures as drawn: how many loaded, and what it says instead of one. */
  const drawn = (where) => where.evaluate((card) => ({
    up: [...card.querySelectorAll('.media img')].filter((img) => img.complete && img.naturalWidth > 0).length,
    notes: [...card.querySelectorAll('.media-note')].map((note) => note.textContent),
  }));

  // Mentions, in boxes another writer made: its words kept whatever comes.
  const words = 'Two lines,\nkept as they are.';
  await imported('Abr', 10);
  // Its second square, so a slide along its row crosses a take of another shape.
  await joined('Abr', 20, 200, 200);
  await joined('Abr', 30);
  await importedPhoto('other');
  addBoxes(folder, [
    { name: 'words', fields: { text: words } },
    { name: 'fixed', tool: `${fixture}/wait`, fields: { instructions: '<abr.2> beside <ABR.1>, and <Abr>' } },
    { name: 'edits', tool: `${fixture}/wait`, fields: { instructions: '<self.1> again' } },
    { name: 'bare', tool: `${fixture}/wait`, fields: { instructions: '<self> again' } },
    { name: 'one', tool: `${fixture}/wait`, fields: { instructions: '<two.1>' } },
    { name: 'two', tool: `${fixture}/wait`, fields: { instructions: '<one.1>' } },
  ]);
  // Square, as its tool makes with nothing it mentions in view, a box can land past the edge of the view, where it isn't drawn.
  await until('fixed placed', async () => (await api(`${at}/canvas`)).body?.cards?.fixed, Boolean);
  await page.keyboard.press('f');
  await settled(page);
  await box('fixed').waitFor();
  const fixed = await until('its reads', () => asset('fixed'), (row) => row?.reads?.length === 3);
  check('versions_are_reads', fixed.reads.map((read) => `${read.name}.${read.version ?? ''}`).sort().join() === 'Abr.,Abr.1,Abr.2', fixed.reads);
  check('no_cycle_of_versions', !(await state()).warnings?.some((line) => /made from it/.test(line.message)), (await state()).warnings);
  check('bare_self_warns_with_the_fix', (await state()).warnings?.some((line) => line.name === 'bare' && line.message.includes('<self.1>')), (await state()).warnings);
  await post(`${at}/approve`, { name: 'Abr', version: '1' });
  await until('first version is current before reference comparison', () => asset('Abr'), (row) => row.version === 1);
  const was = await asset('fixed');
  await post(`${at}/approve`, { name: 'Abr', version: '3' });
  const moved = await until('what reads it moves', () => asset('fixed'), (row) => row.inputs !== was.inputs);
  // Only the unnumbered reference moves with the selected state.
  const keyOf = (row, version) => row.reads.find((read) => read.name === 'Abr' && read.version === version)?.key;
  const sentBefore = [undefined, 1, 2].map((version) => keyOf(was, version));
  const sentAfter = [undefined, 1, 2].map((version) => keyOf(moved, version));
  check('only_the_moving_read_moved', sentBefore.every(Boolean) && sentAfter.every(Boolean) && sentBefore[0] !== sentAfter[0] && sentBefore[1] === sentAfter[1] && sentBefore[2] === sentAfter[2], { sentBefore, sentAfter });

  // Names.
  for (const [to, code] of [['abr.3', 'project_name'], ['self', 'project_name']]) {
    const refused = await post(`${at}/rename`, { from: 'other', to });
    check(`rename_to_${to}_refused`, refused.status === 400 && refused.body.error.code === code && refused.body.error.message.includes('use'), refused);
  }
  const made = await post(`${at}/assets`, { tool: '', name: 'b.7', fields: { text: 'x' } });
  check('a_new_name_ending_so_is_changed', made.body.name === 'b_7', made);
  check('a_typed_name_ending_so_is_refused', (await post(`${at}/assets`, { tool: '', name: 'b.8', fields: { text: 'x' }, exact: true })).status === 400);

  // A camera's JPEG has its thumbnail, not "No preview" (cl-888: FFmpeg took it for an image2 sequence, which Calcifer refuses).
  await box('other').waitFor();
  const photoAtRest = await until('the photo drawn', () => drawn(box('other').locator('.card')), (now) => now.up > 0 || now.notes.length > 0, 5 * STEP_MS).catch((why) => String(why));
  check('a_photo_has_its_thumbnail', photoAtRest.up > 0 && !photoAtRest.notes?.length, photoAtRest);

  // A rename rewrites what says it, and only that.
  await post(`${at}/rename`, { from: 'Abr', to: 'Ada' });
  await until('the mentions', async () => (await asset('fixed')).fields.instructions, (text) => text?.includes('<Ada.2> beside <Ada.1>, and <Ada>'));
  check('rename_keeps_words', (await asset('words')).fields.text === words, (await asset('words')).fields);
  check('rename_keeps_versions', (await asset('Ada')).versions === 3 && (await asset('Ada')).version === 3, await asset('Ada'));

  // Deleting a version something reads: it stays read, a render fails clearly, and Undo brings it back.
  await box('Ada').waitFor();
  const before = (await asset('fixed')).inputs;
  const gone = await api(`${at}/assets/Ada/versions/2`, { method: 'DELETE' });
  check('delete_names_what_reads_it', JSON.stringify(gone.body.read_by) === '["fixed"]', gone.body);
  check('a_deleted_version_puts_nothing_out_of_date', (await asset('fixed')).inputs === before);
  await until('it says so', () => asset('fixed'), (row) => row.missing?.includes('Ada.2'));
  const reclaimed = await post(`${at}/reclaim`, { yes: true });
  check('reclaim_keeps_a_named_version', reclaimed.status === 200 && rowsOf(folder, 'SELECT count(*) FROM versions WHERE discarded = 1')[0][0] > 0 && (await api(`${at}/assets/Ada`)).body.discarded?.includes(2), [reclaimed, (await api(`${at}/assets/Ada`)).body.discarded]);
  const back = await undoOf('Ada');
  check('undo_brings_it_back', back.status === 200 && !(await until('it reads again', () => asset('fixed'), (row) => !row.missing?.includes('Ada.2'))).missing?.length, back);

  // Original right-hand state previews remain, and selecting one writes current state without deleting alternatives.
  const shut = async () => {
    await page.keyboard.press('Escape');
    await page.goto(await projectUrl(id));
    await page.locator('.react-flow__viewport').waitFor();
    await settled(page);
  };
  const open = async (name) => {
    await page.goto(await projectUrl(id, `/assets/${name}`));
    await page.locator(`.card.here[data-asset="${name}"]`).first().waitFor();
  };
  const current = (name) => asset(name).then((row) => row.version);
  const shaOf = (row, number) => row.prefixes?.find((version) => version.number === number)?.main?.sha256 ?? (number === 1 ? row.sha256 : undefined);
  const history = (name) => box(name).locator('.image-history.here-side');
  const previews = (name) => history(name).locator('.take[data-take]');
  // The take shown. None shown is a state its check judges (null), not a minute's wait that ends the cell.
  const selected = (name) => history(name).locator('.take[aria-pressed="true"][data-take]').first().getAttribute('data-take', { timeout: 5000 }).catch(() => null);
  const noLinearControls = (name) => box(name).getByLabel('Current version', { exact: true }).count().then(async (count) => count + await box(name).getByRole('button', { name: /^(Previous|Next) version$/ }).count());
  const noDescription = (name) => box(name).locator('.desc-field, [aria-label="Description"]').count();
  const geometry = (name) => box(name).evaluate((node) => {
    const media = node.querySelector('.media-pixels').getBoundingClientRect();
    const viewer = node.querySelector('.focus-viewer').getBoundingClientRect();
    const controls = node.querySelector('.image-controls')?.getBoundingClientRect();
    const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
    return { width: media.width, height: media.height, zoom, viewer: { x: viewer.x, y: viewer.y, width: viewer.width, height: viewer.height }, controls: controls ? { x: controls.x, y: controls.y, width: controls.width, height: controls.height } : null };
  });
  const stableRoomAndZoom = (before, after) => Math.abs(before.zoom - after.zoom) < 0.001 && ['x', 'y', 'width', 'height'].every((field) => Math.abs(before.viewer[field] - after.viewer[field]) < 2);
  const displayedAspect = (name) => box(name).locator('.focus-viewer .media-pixels img').evaluateAll((images) => {
    const image = [...images].reverse().find((image) => image.complete && image.naturalWidth && image.getBoundingClientRect().width && Number(getComputedStyle(image).opacity) > 0);
    return image ? image.naturalWidth / image.naturalHeight : 0;
  });

  await open('Ada');
  await approveVersion(page, 'Ada', 3);
  check('retained_states_have_original_right_previews', await history('Ada').isVisible() && await previews('Ada').count() === 3 && await selected('Ada') === '3', await box('Ada').innerText());
  check('linear_counter_and_arrow_controls_are_removed', await noLinearControls('Ada') === 0);
  check('image_has_no_description_editor', await noDescription('Ada') === 0, await box('Ada').innerText());
  await settled(page);
  const wideGeometry = await geometry('Ada');
  await previewVersion(page, 'Ada', 2);
  await settled(page);
  const squareGeometry = await geometry('Ada');
  check('browsing_states_keeps_the_focus_room_and_wander_zoom', stableRoomAndZoom(wideGeometry, squareGeometry) && Math.abs(wideGeometry.width - squareGeometry.width) > 20, { wideGeometry, squareGeometry });
  check('thumbnail_preview_does_not_change_approval', await current('Ada') === 3);
  await previewVersion(page, 'Ada', 3);
  const beforeBrowse = (await asset('Ada')).prefixes.map((version) => [version.number, version.main.sha256]);
  const readerBefore = await asset('fixed');
  await previewVersion(page, 'Ada', 1);
  check('preview_keeps_bare_reference_and_approval_unchanged', await current('Ada') === 3 && (await asset('fixed')).inputs === readerBefore.inputs);
  await approveVersion(page, 'Ada', 1);
  await until('selected version is durable', () => current('Ada'), (number) => number === 1);
  const readerAfter = await until('bare reference follows selected version', () => asset('fixed'), (row) => row.inputs !== readerBefore.inputs);
  const readKey = (row, number) => row.reads.find((read) => read.name === 'Ada' && read.version === number)?.key;
  check('plain_name_follows_approval_explicit_versions_stay_fixed', readKey(readerBefore) !== readKey(readerAfter) && readKey(readerBefore, 1) === readKey(readerAfter, 1) && readKey(readerBefore, 2) === readKey(readerAfter, 2), { before: readerBefore.reads, after: readerAfter.reads });
  await page.reload();
  await history('Ada').waitFor();
  check('selected_version_survives_reload', await selected('Ada') === '1' && await current('Ada') === 1);
  check('browsing_preserves_all_versions', JSON.stringify((await asset('Ada')).prefixes.map((version) => [version.number, version.main.sha256])) === JSON.stringify(beforeBrowse));
  const warmedAda = await asset('Ada');
  const aspectOf = (number) => {
    const main = warmedAda.prefixes.find((version) => version.number === number).main;
    return main.width / main.height;
  };
  for (const number of [2, 1]) {
    await previewVersion(page, 'Ada', number);
    await until(`image ${number} decoded for warm switching`, () => displayedAspect('Ada'), (ratio) => Math.abs(ratio / aspectOf(number) - 1) < 0.02);
  }
  check('preview_selection_is_not_a_new_edit', await selected('Ada') === '1' && (await asset('Ada')).versions === 3);
  const flip = (number) => page.evaluate(({ number, wanted }) => {
    const card = () => document.querySelector('.card.here[data-asset="Ada"]');
    const began = performance.now();
    card().querySelector(`.image-history .take[data-take="${number}"]`).click();
    let emptyFrames = 0;
    return new Promise((done) => {
      const look = () => {
        const images = [...card().querySelectorAll('.focus-viewer .media-pixels img')].filter((image) => {
          if (!image.complete || !image.naturalWidth || !image.getBoundingClientRect().width) return false;
          for (let element = image; element && !element.classList.contains('card'); element = element.parentElement) {
            const style = getComputedStyle(element);
            if (style.visibility !== 'visible' || Number(style.opacity) === 0) return false;
          }
          return true;
        });
        if (!images.length) emptyFrames++;
        if (images.some((image) => Math.abs((image.naturalWidth / image.naturalHeight) / wanted - 1) < 0.02)) return done({ first: performance.now() - began, emptyFrames });
        if (performance.now() - began > 10_000) return done({ first: null, emptyFrames });
        requestAnimationFrame(look);
      };
      requestAnimationFrame(look);
    });
  }, { number, wanted: aspectOf(number) });
  const flips = [];
  for (const number of [2, 1, 2, 1]) {
    flips.push(await flip(number));
    await until('thumbnail click previews its state', () => selected('Ada'), (selected) => selected === String(number));
  }
  check('decoded_state_switching_has_no_empty_flash', flips.every((look) => look.first !== null && look.first < 400 && look.emptyFrames === 0), flips);
  check('warm_thumbnail_clicks_do_not_change_approval', await current('Ada') === 1);

  // A longer state history keeps the original scrollable previews, with both mouse and touch scrolling. Sixteen 100px previews are
  // longer than Focus's full-height column at this window, so there's always something to scroll (at nine they can just fit).
  const MANY = 16;
  await imported('many', 50);
  for (let shade = 51; shade < 50 + MANY; shade++) await joined('many', shade);
  await open('many');
  await approveVersion(page, 'many', MANY);
  check('all_alternative_states_keep_previews', await previews('many').count() === MANY && await selected('many') === String(MANY));
  const side = history('many');
  await settled(page);
  const sideBox = await side.boundingBox();
  const scrolled = () => side.evaluate((element) => element.scrollTop);
  const room = await side.evaluate((element) => element.scrollHeight - element.clientHeight);
  const across = sideBox.x + sideBox.width / 2;
  await page.mouse.move(across, sideBox.y + sideBox.height - 12);
  await page.mouse.down();
  for (let step = 1; step <= 8; step++) await page.mouse.move(across, sideBox.y + sideBox.height - 12 - step * 18);
  const scrolling = { marked: await page.locator('.scroll-marker.on').count(), ghosts: await page.locator('.carry-ghost').count() };
  await page.mouse.up();
  const byMouse = await scrolled();
  await page.waitForTimeout(500);
  const coasted = await scrolled();
  const looks = await side.evaluate((element) => ({ bar: element.offsetWidth - element.clientWidth, mask: getComputedStyle(element).maskImage || getComputedStyle(element).webkitMaskImage, top: element.style.getPropertyValue('--fade-top') }));
  check('right_previews_scroll_with_a_mouse', byMouse > 40 && coasted >= byMouse && looks.bar === 0 && looks.top === '24px' && /gradient/.test(looks.mask), { room, byMouse, coasted, looks });
  const cdp = await context.newCDPSession(page);
  await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 1 });
  const beforeFinger = await scrolled();
  const touchFrom = { x: across, y: sideBox.y + sideBox.height - 12 };
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [touchFrom] });
  for (let step = 1; step <= 6; step++) await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x: touchFrom.x, y: touchFrom.y - step * 18 }] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await page.waitForTimeout(500);
  const byFinger = await scrolled();
  await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: false });
  await cdp.detach();
  check('right_previews_scroll_with_a_finger', byFinger > beforeFinger + 40, { beforeFinger, byFinger });
  // And with the wheel, as ever (Paul): back up the column.
  await page.mouse.move(across, sideBox.y + sideBox.height / 2);
  await page.mouse.wheel(0, -240);
  const byWheel = await until('the wheel scrolled it', scrolled, (top) => top < byFinger - 100, 5000).catch(() => scrolled());
  check('right_previews_scroll_with_the_wheel', byWheel < byFinger - 100, { byFinger, byWheel });
  const unmarked = await until('scroll marker gone', () => page.locator('.scroll-marker.on').count(), (count) => count === 0, 4000).then(() => true, (why) => String(why));
  check('scroll_marker_does_not_carry_or_remove_a_state', scrolling.marked === 1 && scrolling.ghosts === 0 && unmarked === true && (await asset('many')).versions === MANY, { scrolling, unmarked });
  await approveVersion(page, 'many', 5);
  await until('fifth version selected', () => current('many'), (number) => number === 5);
  check('preview_selects_fifth_retained_state', await selected('many') === '5');
  await shut();

  // A category click only opens implementations; clicking an implementation also creates nothing.
  const namesBeforePalette = (await state()).assets.map((row) => row.name).sort();
  // Of Edit Image's tools, the first that edits from words: BiRefNet, listed first on this Hub (f2ccc98ef), takes no prompt.
  await implementationRow(page, 'edit_image');
  const listedTools = (await api(`/v1/tools?project=${id}`)).body?.tools ?? [];
  const takesWords = (toolId) => listedTools.find((one) => one.id === toolId)?.fields?.some((field) => field.type === 'prompt');
  const editIds = await page.locator('.tools [data-implementation][data-tool-operation="edit_image"]').evaluateAll((rows) => rows.map((row) => row.dataset.tool));
  const toolRow = await implementationRow(page, 'edit_image', editIds.find(takesWords));
  await toolRow.click();
  await page.waitForTimeout(250);
  check('clicking_a_tool_creates_no_node', JSON.stringify((await state()).assets.map((row) => row.name).sort()) === JSON.stringify(namesBeforePalette));
  const tool = { id: await toolRow.getAttribute('data-tool') };
  const carryToolOnto = async (name) => {
    await shut();
    await fitCanvas(page);
    const row = await implementationRow(page, 'edit_image', tool.id);
    const target = await box(name).locator('.card').boundingBox();
    await carryTo(page, row, { x: target.x + target.width / 2, y: target.y + target.height / 2 });
  };
  const wordsBefore = (await asset('words')).fields.text;
  await carryToolOnto('words');
  await page.waitForTimeout(250);
  check('image_tool_cannot_change_text_output_type', (await asset('words')).kind === 'text' && !(await asset('words')).tool && (await asset('words')).fields.text === wordsBefore && JSON.stringify((await state()).assets.map((row) => row.name).sort()) === JSON.stringify(namesBeforePalette));

  // A source image is v1. Attaching a tool exposes its settings, but Edit is an explicit action.
  const original = await asset('other');
  const originalSha = original.sha256;
  check('imported_image_starts_at_v1', original.versions === 1 && original.version === 1 && shaOf(original, 1) === originalSha, original);
  await carryToolOnto('other');
  await until('image tool attached', () => asset('other'), (row) => row.tool === tool.id);
  await open('other');
  check('source_is_version_one_after_tool_attachment', await selected('other') === '1' && await previews('other').count() === 1 && shaOf(await asset('other'), 1) === originalSha);
  check('attachment_starts_in_image_view', await box('other').getByRole('button', { name: 'Edit image', exact: true }).count() === 1 && await box('other').locator('.in-frame .mention-text').count() === 0);
  check('tool_attachment_does_not_add_a_description_box', await noDescription('other') === 0);
  await box('other').getByRole('button', { name: 'Edit image', exact: true }).click();
  const prompt = box('other').locator('.in-frame .mention-text').first();
  await prompt.waitFor();
  await prompt.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' beside <Ada.2> ');
  // Behind the words, every picture they use, the backdrop's spread (cl-949).
  const reference = box('other').locator('.in-frame .backdrop');
  await until('all referenced image previews load behind editor', () => reference.locator('img').evaluateAll((images) => images.length === 2 && images.every((image) => image.complete && image.naturalWidth > 0)), Boolean);
  const referenceGeometry = () => reference.evaluate((element) => {
    const frame = element.getBoundingClientRect();
    return [...element.querySelectorAll('img')].map((image) => {
      const rect = image.getBoundingClientRect();
      let opacity = 1;
      for (let node = image; node && !node.classList.contains('card'); node = node.parentElement) opacity *= Number(getComputedStyle(node).opacity);
      return { width: rect.width, height: rect.height, share: (rect.width * rect.height) / (frame.width * frame.height), naturalWidth: image.naturalWidth, naturalHeight: image.naturalHeight, fit: getComputedStyle(image).objectFit, opacity };
    });
  });
  // A new set of pictures fades in (240 ms, FocusParameters.css): read once it has.
  const faded = () => reference.evaluate((element) => Promise.all(element.getAnimations({ subtree: true }).map((one) => one.finished))).catch(() => null);
  await faded();
  const beforeShape = await referenceGeometry();
  // Full-bleed, as large as they can (Paul 11-12, cl-949): each covers its share, together the frame.
  const fills = (shape) => shape.length === 2 && shape.every((image) => image.opacity > 0 && image.opacity < 1 && image.fit === 'cover' && image.share >= 0.12) && shape.reduce((sum, image) => sum + image.share, 0) >= 0.93;
  check('edit_mode_shows_all_faded_references_full_bleed', fills(beforeShape), beforeShape);
  const ratioBefore = (await asset('other')).fields?.aspect_ratio;
  check('output_ratio_uses_the_corner_not_a_new_dropdown', await box('other').getByRole('combobox', { name: 'Output aspect ratio', exact: true }).count() === 0);
  const corner = await box('other').getByRole('slider', { name: 'Its size', exact: true }).boundingBox();
  await page.mouse.move(corner.x + corner.width / 2, corner.y + corner.height / 2);
  await page.mouse.down();
  await page.mouse.move(corner.x + corner.width / 2 + 110, corner.y + corner.height / 2 - 50, { steps: 8 });
  await page.mouse.up();
  await until('output aspect ratio changed in edit mode', () => asset('other'), (row) => Boolean(row.fields?.aspect_ratio) && row.fields.aspect_ratio !== ratioBefore);
  await faded();
  const afterShape = await referenceGeometry();
  check('output_ratio_keeps_all_references_filling_the_frame', fills(afterShape), { beforeShape, afterShape });
  await previewVersion(page, 'other', 1);
  check('view_mode_leaves_editor_and_keeps_selection', await box('other').locator('.in-frame .mention-text').count() === 0 && await current('other') === 1);
  await box('other').getByRole('button', { name: 'Remove image tool', exact: true }).click();
  await until('image tool removed', () => asset('other'), (row) => !row.tool);
  check('tool_removal_preserves_image_and_versions', (await asset('other')).kind === 'image' && shaOf(await asset('other'), 1) === originalSha && await selected('other') === '1' && await previews('other').count() === 1);
  await undoOf('other');
  await until('undo restores tool', () => asset('other'), (row) => row.tool === tool.id);

  // Real local rendering appends numbered versions; a box with an approved take keeps showing it as one lands (cl-928, Paul 11-12),
  // and an imported picture's first take is approved. Older explicit references remain readable.
  if ((await api('/v1/status')).body.local_usable && !process.env.ALONE) {
    const render = (names, key) => api(`${at}/runs`, { method: 'POST', body: JSON.stringify({ names, on: ['local'] }), headers: { 'Idempotency-Key': key } });
    await api(`${at}/assets/other`, { method: 'PATCH', body: JSON.stringify({ tool: `${fixture}/wait`, fields: { instructions: '<self.1> in a new light.', aspect_ratio: '1:1' } }) });
    await until('local image tool ready', () => asset('other'), (row) => row.tool === `${fixture}/wait`);
    await open('other');
    const renderIt = await renderOf(page, 'other');
    // Render sends what the page holds: the 1:1 written over the API reaches it first.
    const shapeSaid = await until('the page holds 1:1', () => box('other').locator('.ratio-rest').getAttribute('data-output-aspect').catch(() => null), (label) => (label ?? '').includes('1:1'), 10_000).catch(() => box('other').locator('.ratio-rest').getAttribute('data-output-aspect').catch(() => null));
    await renderIt.click();
    const second = await until('first modification is v2', () => asset('other'), (row) => row.versions === 2 && !row.making?.length, 5 * STEP_MS);
    check('first_modification_is_v2_beside_the_approved', second.version === 1 && second.approved === 1 && second.pinned && shaOf(second, 1) === originalSha && shaOf(second, 2) !== originalSha, second);
    const made = second.prefixes.find((row) => row.number === 2).main;
    check('new_output_has_requested_ratio', Math.abs(made.width / made.height - 1) < 0.02, { width: made.width, height: made.height, shapeSaid, aspect_ratio: second.fields?.aspect_ratio });
    await open('other');
    await until('the new take has a retained preview', () => previews('other').count(), (count) => count === 2);
    check('new_output_adds_a_retained_preview', await selected('other') === '1');
    await approveVersion(page, 'other', 1);
    await settled(page);
    const sourceGeometry = await geometry('other');
    await approveVersion(page, 'other', 2);
    await settled(page);
    const editedGeometry = await geometry('other');
    check('variant_browsing_keeps_focus_room_zoom_and_tool_controls_stable', stableRoomAndZoom(sourceGeometry, editedGeometry) && ['x', 'y', 'width', 'height'].every((field) => Math.abs(sourceGeometry.controls[field] - editedGeometry.controls[field]) < 2), { sourceGeometry, editedGeometry });
    await approveVersion(page, 'other', 1);
    await (await renderOf(page, 'other')).click();
    const third = await until('editing an older version appends v3', () => asset('other'), (row) => row.versions === 3 && !row.making?.length, 5 * STEP_MS);
    check('editing_old_version_keeps_forward_history', third.version === 1 && third.approved === 1 && third.prefixes.map((row) => row.number).join() === '1,2,3' && shaOf(third, 1) === originalSha && shaOf(third, 2) === shaOf(second, 2), third.prefixes);
    const inputOf = async (row, number) => (await api(`${at}/runs/${row.prefixes.find((version) => version.number === number).run}?detail=full`)).body?.request?.input;
    const editedFrom = await inputOf(third, 3);
    check('edit_reads_the_selected_source_version', editedFrom?.instructions?.references?.length === 1 && editedFrom.instructions.references[0].file === originalSha, editedFrom);
    addBoxes(folder, [{ name: 'current_reader', tool: `${fixture}/wait`, fields: { instructions: '<other> now, with <other.1> kept.' } }]);
    const followed = await until('current and explicit references read', () => asset('current_reader'), (row) => row.reads?.length === 2);
    await approveVersion(page, 'other', 2);
    const refChanged = await until('reader tracks selection', () => asset('current_reader'), (row) => row.inputs !== followed.inputs);
    const key = (row, version) => row.reads.find((read) => read.name === 'other' && read.version === version)?.key;
    check('selected_output_is_the_plain_reference', key(followed) !== key(refChanged) && key(followed, 1) === key(refChanged, 1), { before: followed.reads, after: refChanged.reads });
    await render(['current_reader'], `selected-reference-${Date.now()}`);
    const readerOutput = await until('selected references reach a real function', () => asset('current_reader'), (row) => row.versions === 1 && !row.making?.length, 5 * STEP_MS);
    const sent = await inputOf(readerOutput, 1);
    const referenceFile = (name) => sent?.instructions?.references?.find((reference) => reference.key.toLowerCase() === name)?.file;
    check('bare_and_explicit_references_reach_the_function', sent?.instructions?.references?.length === 2 && referenceFile('other') === shaOf(second, 2) && referenceFile('other.1') === originalSha, sent);
    await page.reload();
    await history('other').waitFor();
    check('rendered_version_selection_survives_reload', await current('other') === 2 && await selected('other') === '2' && await previews('other').count() === 3);

    // Selecting a state must not make its image tool disappear from Render all.
    const { versions: beforeAll, approved: approvedBefore } = await asset('other');
    await page.getByRole('button', { name: /^Render all / }).click();
    const renderedAll = await until('Render all includes the selected image', () => asset('other'), (row) => row.versions === beforeAll + 1 && !row.making?.length, 5 * STEP_MS);
    check('render_all_includes_selected_image_tools', renderedAll.version === approvedBefore && renderedAll.approved === approvedBefore && shaOf(renderedAll, 1) === originalSha, renderedAll);
    await until('Render all settles before rename', () => state(), (row) => !row.busy, 5 * STEP_MS);

    // Rename while actual work is running: the existing versions and references follow the same asset.
    const beforeRename = (await asset('other')).versions;
    await api(`${at}/assets/other`, { method: 'PATCH', body: JSON.stringify({ fields: { seconds: 30 } }) });
    await render(['other'], `rename-${Date.now()}`);
    await until('render running before rename', () => asset('other'), (row) => row.making?.length > 0, 2 * STEP_MS);
    const rename = await post(`${at}/rename`, { from: 'other', to: 'modified' });
    check('rename_during_render_is_accepted', rename.status === 200, rename);
    const renamed = await until('render lands under renamed asset', () => asset('modified'), (row) => row.versions === beforeRename + 1 && !row.making?.length, 5 * STEP_MS);
    check('rename_preserves_old_versions_and_its_approved', renamed.version === approvedBefore && renamed.approved === approvedBefore && shaOf(renamed, 1) === originalSha && !((await state()).orphans ?? []).some((row) => row.name === 'other'), renamed);
    check('rename_rewrites_bare_and_explicit_references', (await asset('current_reader')).fields.instructions.includes('<modified>') && (await asset('current_reader')).fields.instructions.includes('<modified.1>'));

    // Canceling actual work adds no empty version to the holder.
    await post(`${at}/assets`, { name: 'canceled_edit', exact: true, kind: 'image', tool: `${fixture}/wait`, fields: { instructions: 'Do not retain a canceled image.', seconds: 30 } });
    await render(['canceled_edit'], `canceled-edit-${Date.now()}`);
    const running = await until('image run is active before cancellation', () => asset('canceled_edit'), (row) => row.making?.length === 1, 2 * STEP_MS);
    const canceled = await post(`${at}/runs/${running.making[0]}/cancel`, {});
    // A run still going answers 202, asked to stop (api.rs `cancel`); 200 is for one already over.
    check('image_render_can_be_canceled', canceled.status === 202 && canceled.body?.cancel_requested === true, canceled);
    const canceledImage = await until('canceled image run leaves no work', () => asset('canceled_edit'), (row) => !row.making?.length, 2 * STEP_MS);
    check('canceled_render_adds_no_version', canceledImage.versions === 0 && !canceledImage.sha256, canceledImage);

    // Existing refusal boundaries: no unmade/self-without-version input is silently substituted.
    await render(['bare'], `bare-${Date.now()}`);
    const bare = await until('bare self refused', () => asset('bare'), (row) => row?.failure, 2 * STEP_MS);
    check('bare_self_is_refused_with_the_fix', bare.failure.code === 'input_missing' && bare.failure.message.includes('<self.1>') && !bare.versions, bare.failure);
    await api(`${at}/assets/Ada/versions/2`, { method: 'DELETE' });
    await render(['fixed'], `fixed-${Date.now()}`);
    const refused = await until('deleted explicit version refused', () => asset('fixed'), (row) => row?.failure, 2 * STEP_MS);
    check('a_deleted_version_fails_clearly', refused.failure.code === 'input_missing' && refused.failure.message.includes('was deleted'), refused.failure);
    await undoOf('Ada');

    // A video tool still receives the selected video file in its required slot.
    const clipAt = join(pictures, 'clip.mp4');
    writeFileSync(clipAt, MP4);
    await post(`${at}/imports`, { source: await upload(held?.token, clipAt, id), name: 'clip', exact: true, at: { x: 100, y: 1100 } });
    await api(`${at}/assets/clip`, { method: 'PATCH', body: JSON.stringify({ tool: `${fixture}/keep` }) });
    const clip = await until('video source enters its slot', () => asset('clip'), (row) => row?.fields?.video === '<self.1>');
    check('video_tool_still_reads_the_source', clip.kind === 'video' && clip.versions === 1, clip);
  } else if (!process.env.ALONE) check('renders_on_this_computer', false, 'local is not usable, so none of the render checks ran');
  else seen.skipped_render = 'no free tool runs here';

  await shut();
  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
