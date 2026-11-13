// One editing view for a picture and a video (cl-938, Paul 11-12: "when you're editing an image, you type the description of what you want
// directly into the image box, and set the aspect ratio of the image, I want that same sort of UX for videos as well, but we have more than
// one box ... those should fit inside of the video-panel as sections, and we'll still display the reference images in the background"), in a
// real browser on a real Calcifer. H3's Shot in Focus: Summary and its Beats are sections inside the frame, each opened by its head (Summary
// too, which can't be removed), + Beat adds one; behind them, faded and full height, the pictures the section being written mentions, all of
// them with none written in (its corner's shapes are aspect.js's); under the frame its tool with Models & LoRAs, then
// Render and the fields. Writing its next take (New, beside Render on the title row) is the same, its take faded behind, "New take" said;
// Render renders and the title row says so; New is a draft again. Qwen's Image writes in the same frame, its Render in the same spot,
// and Cmd/Ctrl+Enter renders. Screenshots of each.
//   node frame.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, projectFromPage, calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, until, upload } = require('./page');

/** Shots this computer renders itself, weightless. */
const SHOTS = join(__dirname, '..', 'project', 'shots.py');

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
  const held = { token: holderToken() };
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-frame-${Date.now()}`));
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
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'amy.png'), id), name: 'amy', exact: true, at: { x: 100, y: 600 } });
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'boat.png'), id), name: 'boat', exact: true, at: { x: 400, y: 600 } });
  const beats = [{ description: 'Waves roll in.', duration: 5 }, { description: '<boat> sails past the lighthouse.', duration: 5 }];
  await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'scene', exact: true, fields: { summary: '<amy> watches the harbour at dusk.', beats }, at: { x: 100, y: 100 } });
  await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'again', exact: true, fields: { summary: '<amy> on the pier.', beats: [{ description: 'She waves.', duration: 5 }] }, at: { x: 700, y: 100 } });
  await post(`${at}/assets/again/versions`, { source: await upload(held?.token, join(folder, 'take.mp4'), id) });
  await post(`${at}/assets`, { tool: imageTool.id, kind: 'image', name: 'still', exact: true, fields: { instructions: '<amy> in a red coat.' }, at: { x: 1300, y: 100 } });
  // A shot this computer renders itself (`project/shots.py`), with a take: what Render and Cmd/Ctrl+Enter start.
  await post(`${at}/assets`, { tool: `${SHOTS}/shoot`, kind: 'video', name: 'local', exact: true, fields: { summary: 'A walk by the sea.', beats: [{ description: 'Waves.', duration: 2 }] }, at: { x: 100, y: 1000 } });
  await post(`${at}/assets/local/versions`, { source: await upload(held?.token, join(folder, 'take.mp4'), id) });
  for (const name of ['amy', 'boat', 'scene', 'again', 'still', 'local']) await boxOf(page, name).waitFor();
  await fitCanvas(page);

  /** The frame of the open box, and what it holds. */
  const card = (name) => page.locator(`.card.here[data-asset="${name}"]`).first();
  const looked = (name) => card(name).evaluate((node) => {
    const media = node.querySelector('.media');
    const frame = media?.getBoundingClientRect();
    const run = node.querySelector(':scope > .focus-actions .run')?.getBoundingClientRect();
    const models = node.querySelector(':scope > .tool-line button[aria-label="Models and LoRAs"]')?.getBoundingClientRect();
    const tool = node.querySelector(':scope > .image-controls .image-tool-name')?.getBoundingClientRect();
    const fields = [...node.querySelectorAll('.here-under .edit-shown [data-field], .here-under .edit-shown [aria-label]')].map((el) => el.getBoundingClientRect());
    return {
      sections: [...(media?.querySelectorAll('.in-frame .frame-sections .beat-toggle') ?? [])].map((toggle) => ({ head: toggle.querySelector('b')?.textContent, open: toggle.getAttribute('aria-expanded') === 'true' })),
      summary_removable: Boolean(media?.querySelector('.frame-first [aria-label^="Remove"]')),
      in_frame: { summary: Boolean(media?.querySelector('.in-frame [data-field="summary"]')), beats: Boolean(media?.querySelector('.in-frame .beats-edit[data-field="beats"]')) },
      behind: [...(media?.querySelectorAll('.backdrop .spread-picture') ?? [])].map((el) => el.dataset.picture),
      full_height: [...(media?.querySelectorAll('.backdrop .spread-picture') ?? [])].every((el) => Math.abs(el.getBoundingClientRect().height - frame.height) < 2),
      ratio: frame ? frame.width / frame.height : null,
      order: Boolean(frame && tool && run && models) && run.bottom <= frame.top + 1 && tool.top >= frame.bottom - 1 && Math.abs(models.top + models.height / 2 - (tool.top + tool.height / 2)) < models.height && fields.every((rect) => rect.top >= Math.max(tool.bottom, models.bottom) - 1),
      render: run ? { top: Math.round(run.top), right: Math.round(run.right) } : null,
      label: document.querySelector('.focus-heading .focus-version-label')?.textContent ?? null,
      in_fields: node.querySelectorAll('.here-under button[aria-label="Render"]').length,
      inside: node.querySelectorAll('.here-under .active-models, .here-under button[aria-label="Models and LoRAs"]').length,
    };
  });
  /** Written in `place`: what's behind the frame follows it, to `behind`, the pictures that section mentions (judged by each check after). */
  const write = async (name, place, behind) => {
    await card(name).locator(`.in-frame .mention-text[data-field="${place}"]`).click();
    await until(`${name}'s backdrop follows ${place}`, async () => (await looked(name)).behind, (now) => JSON.stringify(now) === JSON.stringify(behind), 10_000).catch(() => {});
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
  check('render_on_the_title_row_then_frame_tool_and_models_then_fields', empty.order && empty.inside === 0 && empty.in_fields === 0, empty);
  // Opened, it's on its words (9907b898f: the caret in them once Focus shows), so Summary is being written and its picture is behind.
  check('opened_on_its_summary_its_picture_is_behind', JSON.stringify(empty.behind) === JSON.stringify(['amy']) && empty.full_height, empty.behind);
  // The words left, none is being written: all it mentions is behind (AssetEdit's frame: "with none being written, all it mentions").
  await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
  const none = await until('all it mentions behind', async () => (await looked('scene')).behind, (now) => JSON.stringify([...now].sort()) === JSON.stringify(['amy', 'boat']), 10_000).catch(async () => (await looked('scene')).behind);
  check('with_none_written_in_all_it_mentions_is_behind', JSON.stringify([...none].sort()) === JSON.stringify(['amy', 'boat']) && (await looked('scene')).full_height, none);
  await shot('empty-16-9');

  // Written in: what that section mentions, full height.
  await write('scene', 'summary', ['amy']);
  const inSummary = await looked('scene');
  seen.in_summary = inSummary.behind;
  check('in_summary_its_picture_is_behind', JSON.stringify(inSummary.behind) === JSON.stringify(['amy']) && inSummary.full_height, inSummary);
  await shot('writing-summary');
  await card('scene').locator('.beat-toggle', { hasText: 'Beat 2' }).click();
  await write('scene', 'beats.1.description', ['boat']);
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

  await closeBox(page);

  const allRuns = async () => ((await api(`${at}/runs?limit=100`)).body?.runs ?? []).filter((run) => run.project_id === id && run.kind === 'call').length;
  const header = () => page.locator('.focus-heading .focus-version-label').innerText().catch(() => null);

  // An H3 node with a take (Paul, 11-12: "I cannot start a new take, like if I push the 'new' button nothing happens"): New is a draft,
  // unmistakably, its header saying "New take" and its take faded behind.
  await openBox(page, 'again');
  const next = card('again').locator(':scope > .focus-actions .focus-new');
  await next.waitFor();
  const viewing = { sections: await card('again').locator('.in-frame .frame-sections').count(), label: await header() };
  await next.click();
  await card('again').locator('.in-frame .frame-sections').waitFor();
  await write('again', 'summary', ['amy']);
  const again = await looked('again');
  seen.again = { viewing, again };
  const take = await card('again').evaluate((node) => {
    const pixels = node.querySelector('.media .media-pixels');
    return { writing: Boolean(node.querySelector('.media.writing')), faded: pixels ? Number(getComputedStyle(pixels).opacity) : null };
  });
  seen.again_take = take;
  check('new_on_a_node_with_takes_is_a_draft', viewing.sections === 0 && again.label === 'New take' && again.in_frame.summary && again.in_frame.beats && take.writing && take.faded !== null && take.faded < 0.5 && JSON.stringify(again.behind) === JSON.stringify(['amy']) && again.order, { viewing, again, take });
  await shot('next-take-with-a-reference');
  const h3Render = (await looked('again')).render;
  await closeBox(page);

  // Rendered, the title row says so; New is a draft again, "New take"; Cmd/Ctrl+Enter in a prompt renders. A shot this computer renders,
  // so a run starts here.
  await openBox(page, 'local');
  const localNew = card('local').locator(':scope > .focus-actions .focus-new');
  await localNew.click();
  await card('local').locator('.in-frame .frame-sections').waitFor();
  const before = await allRuns();
  await card('local').locator(':scope > .focus-actions button[aria-label="Render"]').click();
  const rendered = await until('its render started', allRuns, (n) => n > before, 120_000).catch(allRuns);
  const watching = { runs: rendered - before, label: await header(), render: await card('local').locator(':scope > .focus-actions button[aria-label="Render"]').count() };
  seen.watching = watching;
  // Rendered from its words, a video stays on them (cl-907); the title row says it's rendering, not "New take", and Render stays.
  check('render_renders_and_says_so', watching.runs >= 1 && watching.label !== 'New take' && watching.render === 1, watching);
  await shot('rendering-its-take');
  await localNew.click();
  await card('local').locator('.in-frame .frame-sections').waitFor();
  const draft = { label: await header(), sections: await card('local').locator('.in-frame .frame-sections').count() };
  seen.draft_again = draft;
  check('new_while_it_renders_is_a_draft_again', draft.label === 'New take' && draft.sections === 1, draft);
  const keyed = await allRuns();
  await card('local').locator('.in-frame .mention-text[data-field="summary"]').click();
  await page.keyboard.press(process.platform === 'darwin' ? 'Meta+Enter' : 'Control+Enter');
  const after = await until('its render asked by the keys', allRuns, (n) => n > keyed, 120_000).catch(allRuns);
  check('cmd_or_ctrl_enter_renders', after > keyed, { before: keyed, after });
  for (const run of ((await api(`${at}/runs?limit=100`)).body?.runs ?? []).filter((run) => run.project_id === id && !['completed', 'failed', 'canceled'].includes(run.status))) await api(`${at}/runs/${run.id}/cancel`, { method: 'POST', body: '{}' });
  await closeBox(page);

  // A picture writes in the same frame: its one prompt, the picture it mentions behind it; Render in the same spot.
  await openBox(page, 'still');
  await card('still').locator('.in-frame .mention-text[data-field="instructions"]').waitFor();
  const still = await card('still').evaluate((node) => ({
    sections: node.querySelectorAll('.in-frame .frame-sections').length,
    behind: [...node.querySelectorAll('.media .backdrop .spread-picture')].map((el) => el.dataset.picture),
    tool: node.querySelector(':scope > .image-controls .image-tool-name')?.textContent ?? null,
  }));
  seen.still = still;
  check('a_picture_writes_in_the_same_frame', still.sections === 0 && JSON.stringify(still.behind) === JSON.stringify(['amy']) && Boolean(still.tool), still);
  const imageRender = (await looked('still')).render;
  seen.render_at = { h3: h3Render, image: imageRender };
  check('render_is_in_the_same_spot_for_h3_and_an_image', Boolean(h3Render && imageRender) && Math.abs(h3Render.top - imageRender.top) <= 1 && Math.abs(h3Render.right - imageRender.right) <= 1, seen.render_at);
  await shot('picture-writing');
  await closeBox(page);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
