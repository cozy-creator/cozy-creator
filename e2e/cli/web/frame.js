// One editing view for a picture and a video (cl-938, Paul 11-12: "when you're editing an image, you type the description of what you want
// directly into the image box, and set the aspect ratio of the image, I want that same sort of UX for videos as well, but we have more than
// one box ... those should fit inside of the video-panel as sections, and we'll still display the reference images in the background"), in a
// real browser on a real Calcifer. H3's Shot in Focus: Summary and its Beats are sections inside the frame, each opened by its head (Summary
// too, which can't be removed), + Beat adds one; behind them, faded and full height, the pictures the section being written mentions, all of
// them with none written in; its corner pulled toward 9:16 lands on a ratio H3 makes; under the frame its tool with Models & LoRAs, then
// Render and the fields. Writing its next take (New) is the same, its take faded behind. Qwen's Image writes in the same frame. Screenshots of each.
//   node frame.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, until, upload } = require('./page');

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
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-frame-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const fieldsOf = async (name) => (await api(at)).body.assets.find((asset) => asset.name === name)?.fields ?? {};
  const folder = mkdtempSync(join(tmpdir(), 'web-frame-'));

  // Two pictures, H3's Shot reading one in its summary and the other in its second beat, and Qwen's Image reading the first.
  const tools = [...((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? []), ...((await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? [])];
  const shotTool = tools.find((tool) => tool.id === 'minimax-h3/generate');
  const imageTool = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  if (!shotTool || !imageTool) throw new Error(`the Hub lists no H3 Shot or Qwen Image: ${JSON.stringify(tools.map((tool) => tool.id))}`);
  writeFileSync(join(folder, 'amy.png'), png(20));
  writeFileSync(join(folder, 'boat.png'), png(200));
  writeFileSync(join(folder, 'take.mp4'), MP4);
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'amy.png')), name: 'amy', exact: true, at: { x: 100, y: 600 } });
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'boat.png')), name: 'boat', exact: true, at: { x: 400, y: 600 } });
  const beats = [{ description: 'Waves roll in.', duration: 5 }, { description: '<boat> sails past the lighthouse.', duration: 5 }];
  await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'scene', exact: true, fields: { summary: '<amy> watches the harbour at dusk.', beats }, at: { x: 100, y: 100 } });
  await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'again', exact: true, fields: { summary: '<amy> on the pier.', beats: [{ description: 'She waves.', duration: 5 }] }, at: { x: 700, y: 100 } });
  await post(`${at}/assets/again/versions`, { source: await upload(held?.token, join(folder, 'take.mp4')) });
  await post(`${at}/assets`, { tool: imageTool.id, kind: 'image', name: 'still', exact: true, fields: { instructions: '<amy> in a red coat.' }, at: { x: 1300, y: 100 } });
  for (const name of ['amy', 'boat', 'scene', 'again', 'still']) await boxOf(page, name).waitFor();
  await fitCanvas(page);

  /** The frame of the open box, and what it holds. */
  const card = (name) => page.locator(`.card.here[data-asset="${name}"]`).first();
  const looked = (name) => card(name).evaluate((node) => {
    const media = node.querySelector('.media');
    const frame = media?.getBoundingClientRect();
    const run = node.querySelector('.here-under .run')?.getBoundingClientRect();
    const models = node.querySelector(':scope > .tool-line button[aria-label="Models and LoRAs"]')?.getBoundingClientRect();
    const tool = node.querySelector(':scope > .image-controls .image-tool-name')?.getBoundingClientRect();
    const fields = [...node.querySelectorAll('.here-under .edit-shown [data-field], .here-under .edit-shown [aria-label]')].map((el) => el.getBoundingClientRect());
    return {
      sections: [...(media?.querySelectorAll('.in-frame .frame-sections .beat-toggle') ?? [])].map((toggle) => ({ head: toggle.querySelector('b')?.textContent, open: toggle.getAttribute('aria-expanded') === 'true' })),
      summary_removable: Boolean(media?.querySelector('.frame-first [aria-label^="Remove"]')),
      in_frame: { summary: Boolean(media?.querySelector('.in-frame [data-field="summary"]')), beats: Boolean(media?.querySelector('.in-frame .beats-edit[data-field="beats"]')) },
      behind: [...(media?.querySelectorAll('.backdrop .backdrop-picture') ?? [])].map((el) => el.dataset.mention),
      full_height: [...(media?.querySelectorAll('.backdrop .backdrop-picture') ?? [])].every((el) => Math.abs(el.getBoundingClientRect().height - frame.height) < 2),
      ratio: frame ? frame.width / frame.height : null,
      order: Boolean(frame && tool && run && models) && tool.top >= frame.bottom - 1 && Math.abs(models.top + models.height / 2 - (tool.top + tool.height / 2)) < models.height && run.top >= Math.max(tool.bottom, models.bottom) - 1 && fields.every((rect) => rect.top >= run.top - 1),
      inside: node.querySelectorAll('.here-under .active-models, .here-under button[aria-label="Models and LoRAs"]').length,
    };
  });
  const write = async (name, place) => {
    await card(name).locator(`.in-frame .mention-text[data-field="${place}"]`).click();
    await page.waitForTimeout(400);
  };

  // An empty Shot in Focus: Summary and its Beats in the frame, the tool, Render, the fields and Models & LoRAs under it.
  await openBox(page, 'scene');
  await card('scene').locator('.in-frame .frame-sections').waitFor();
  const empty = await looked('scene');
  seen.empty = empty;
  check('summary_and_beats_are_sections_in_the_frame', empty.in_frame.summary && empty.in_frame.beats && empty.sections[0]?.head === 'Summary' && empty.sections.filter((one) => /^Beat \d/.test(one.head ?? '')).length === 2, empty);
  check('summary_is_a_section_that_cannot_go', !empty.summary_removable, empty);
  check('one_section_open_at_a_time', empty.sections.filter((one) => one.open).length === 1, empty.sections);
  check('the_frame_is_16_9', Math.abs(empty.ratio - 16 / 9) < 0.03, empty.ratio);
  check('tool_and_models_then_render_then_fields', empty.order && empty.inside === 0, empty);
  check('with_none_written_in_all_it_mentions_is_behind', JSON.stringify([...empty.behind].sort()) === JSON.stringify(['amy', 'boat']) && empty.full_height, empty.behind);
  await shot('empty-16-9');

  // Written in: what that section mentions, full height.
  await write('scene', 'summary');
  const inSummary = await looked('scene');
  seen.in_summary = inSummary.behind;
  check('in_summary_its_picture_is_behind', JSON.stringify(inSummary.behind) === JSON.stringify(['amy']) && inSummary.full_height, inSummary);
  await shot('writing-summary');
  await card('scene').locator('.beat-toggle', { hasText: 'Beat 2' }).click();
  await write('scene', 'beats.1.description');
  const inBeat = await looked('scene');
  seen.in_beat = inBeat.behind;
  check('in_beat_2_its_picture_is_behind', JSON.stringify(inBeat.behind) === JSON.stringify(['boat']) && inBeat.full_height, inBeat);
  check('opening_a_beat_shuts_summary', inBeat.sections.find((one) => one.head === 'Summary')?.open === false && inBeat.sections.find((one) => one.head === 'Beat 2')?.open === true, inBeat.sections);
  await shot('writing-beat-2');

  // + Beat adds a beat, and only a beat.
  await card('scene').locator('.in-frame .beats-last .add-beat').click();
  const added = await until('a third beat', () => fieldsOf('scene'), (fields) => fields.beats?.length === 3).catch(() => fieldsOf('scene'));
  const three = await looked('scene');
  check('plus_beat_adds_a_beat', added.beats?.length === 3 && three.sections.filter((one) => one.head === 'Summary').length === 1 && three.sections.filter((one) => /^Beat \d/.test(one.head ?? '')).length === 3, { beats: added.beats?.length, sections: three.sections });
  await shot('a-beat-added');

  // Its corner pulled toward 9:16 lands on a ratio H3 makes.
  const shape = shotTool.fields.find((field) => field.type === 'shape');
  seen.h3_ratios = shape?.ratios ?? null;
  const media = await card('scene').locator('.media').first().boundingBox();
  const grip = await card('scene').locator('.media .corner').first().boundingBox();
  if (grip) {
    await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
    await page.mouse.down();
    await page.mouse.move(grip.x + grip.width / 2 - media.width * 0.7, grip.y + grip.height / 2 + media.height * 0.4, { steps: 20 });
    await page.mouse.up();
    await page.waitForTimeout(600);
  }
  const pulled = await fieldsOf('scene');
  const ratios = (shape?.ratios ?? []).map(String);
  seen.pulled = { corner: Boolean(grip), saved: pulled[shape?.field ?? 'aspect_ratio'] ?? null };
  check('a_pulled_corner_lands_on_a_ratio_h3_makes', Boolean(grip) && (seen.pulled.saved === null || !ratios.length || ratios.includes(seen.pulled.saved)), seen.pulled);
  await shot('pulled-toward-9-16');
  await closeBox(page);

  // Writing its next take: the same frame, its take faded behind, the picture written of over it.
  await openBox(page, 'again');
  const next = card('again').locator('.here-side .take.add');
  await next.waitFor();
  await next.click();
  await card('again').locator('.in-frame .frame-sections').waitFor();
  await write('again', 'summary');
  const again = await looked('again');
  seen.again = again;
  const take = await card('again').evaluate((node) => {
    const pixels = node.querySelector('.media .media-pixels');
    return { writing: Boolean(node.querySelector('.media.writing')), faded: pixels ? Number(getComputedStyle(pixels).opacity) : null };
  });
  seen.again_take = take;
  check('a_next_take_is_written_in_the_same_frame', again.in_frame.summary && again.in_frame.beats && take.writing && JSON.stringify(again.behind) === JSON.stringify(['amy']) && again.order, { again, take });
  await shot('next-take-with-a-reference');
  await closeBox(page);

  // A picture writes in the same frame: its one prompt, the picture it mentions behind it.
  await openBox(page, 'still');
  await card('still').locator('.in-frame .mention-text[data-field="instructions"]').waitFor();
  const still = await card('still').evaluate((node) => ({
    sections: node.querySelectorAll('.in-frame .frame-sections').length,
    behind: [...node.querySelectorAll('.media .backdrop .backdrop-picture')].map((el) => el.dataset.mention),
    tool: node.querySelector(':scope > .image-controls .image-tool-name')?.textContent ?? null,
  }));
  seen.still = still;
  check('a_picture_writes_in_the_same_frame', still.sections === 0 && JSON.stringify(still.behind) === JSON.stringify(['amy']) && Boolean(still.tool), still);
  await shot('picture-writing');
  await closeBox(page);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
