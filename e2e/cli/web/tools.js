// A box drawn from its tool's fields (tools.md, The types), in a real browser on a real Calcifer, with the every-type fixture `project/boxes.py`.
// The wire's `Tool.fields` by type; Focused video parameters without a folded Advanced control; segments up to four, a menu past that, a dropdown for options that say
// what they are; Auto and reset, which takes the key out of the file; seed data kept without seed UI. Groups: rows added, taken out and dragged into a new
// order, their sum, a block that may be absent, and rows two deep. Slots: what doesn't fit is refused as it's carried and at the drop, with why;
// a filled one is on the face, and × unsets it. A video's range: in and out dragged on its frames, written `<walk:in-out>` and shown as
// timecode, in a prompt and on a plate, whose length is set against the beats. Where this computer runs models: previews land as it renders;
// Pick up from here on its own take and from another box's take let go on it, landing on the tool's frames, with nothing written.
//   node tools.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT, $CHROME, $COZY, $SHOT, $ALONE as nodes.js. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { existsSync, readFileSync, renameSync, writeFileSync } = require('fs');
const { join } = require('path');
const { implementationRow, projectFromPage, MP4, boxOf, calcifer, closeBox, openBox, signIn, STEP_MS, drop, lastLook, launch, lookAt, png, noStall, until, watchBoxes, wav, importYaml, yamlOf } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
const FIRST_RUN_MS = 300_000;
const COZY = process.env.COZY;
const BOXES = join(__dirname, '..', 'project', 'boxes.py');

function replaceFile(at, text) {
  writeFileSync(`${at}.part`, text);
  renameSync(`${at}.part`, at);
}

/** One asset's lines of a project.yaml. */
function linesOf(text, name) {
  const lines = text.split('\n');
  const at = lines.findIndex((line) => line.startsWith(`  ${name}:`));
  if (at < 0) return null;
  const end = lines.findIndex((line, n) => n > at && /^ {0,2}\S/.test(line));
  return lines.slice(at, end < 0 ? undefined : end).join('\n');
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  await watchBoxes(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  const refused = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  page.on('response', (answer) => {
    const url = new URL(answer.url());
    if (answer.status() < 400 || (answer.request().method() === 'HEAD' && url.pathname.startsWith('/v1/files/'))) return;
    const refusal = { status: answer.status(), method: answer.request().method(), url: url.pathname, during };
    refused.push(refusal);
    answer.json().then(({ error }) => Object.assign(refusal, { code: error?.code, message: error?.message })).catch(() => {});
  });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-tools-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  // The fixture as new code, so this machine has never installed it: what lists it before the install lands is PyPI's Runtime's reading.
  const fixture = join(folder, 'boxes.py');
  writeFileSync(fixture, `${readFileSync(BOXES, 'utf8')}\n# ${id}\n`);
  const yaml = () => yamlOf(folder);
  const of = async (name) => (await state()).assets.find((held) => held.name === name);
  const fieldsOf = async (name) => (await of(name))?.fields ?? {};
  const box = (name) => boxOf(page, name);
  const open = (name) => openBox(page, name);
  const more = async (name) => {
    await open(name);
    const toggle = box(name).locator('.more-toggle');
    if (await toggle.count()) {
      if (!(await box(name).locator('.edit-more').count())) await toggle.click();
      await box(name).locator('.edit-more').waitFor();
    }
  };
  const field = (name, place) => box(name).locator(`[data-field="${place}"]`).first();
  const noted = (words) => page.locator(`.note:has-text("${words}")`).first();

  // A picture, another, a video and a sound, each a box; and the every-type tool, read as a run reads it.
  await drop(page, [{ name: 'Coast.png', type: 'image/png', bytes: png(40) }, { name: 'Amy.png', type: 'image/png', bytes: png(160) }, { name: 'Walk.mp4', type: 'video/mp4', bytes: MP4 }, { name: 'Hum.wav', type: 'audio/wav', bytes: wav() }], 900, 300);
  for (const name of ['coast', 'amy', 'walk', 'hum']) await box(name).waitFor();
  await until('all four in the file', () => Promise.resolve(yaml()), (text) => ['coast', 'amy', 'walk', 'hum'].every((name) => text.includes(`  ${name}:`)));
  // Named first, it's read by PyPI's Runtime, which knows no tool; the install a run makes on this machine's Runtime takes its place.
  importYaml(folder, `${yaml().trimEnd()}\n  shot:\n    tool: ${fixture}/shot\n    plate: <walk>\n`);
  await box('shot').waitFor();
  // No box shows a description field for now (cl-900, Paul 11-12: "remove all of the 'description' boxes since they are confusing"): a
  // picture, a video, a sound, a box with a tool; a stored one stays in the file.
  await api(`/v1/projects/${id}/assets/walk`, { method: 'PATCH', body: JSON.stringify({ fields: { description: 'A walk by the sea' } }) });
  await until('walk described', () => fieldsOf('walk'), (fields) => fields.description === 'A walk by the sea');
  for (const name of ['coast', 'walk', 'hum', 'shot']) {
    await open(name);
    check(`${name}_shows_no_description`, (await box(name).locator('.desc-field, [aria-label="Description"]').count()) === 0, await box(name).innerText());
  }
  check('a_stored_description_stays', (await fieldsOf('walk')).description === 'A walk by the sea', await fieldsOf('walk'));
  await shot('no-description');
  await closeBox(page);
  if (local) execFileSync(COZY, ['--json', 'run', `${fixture}/still`, 'instructions=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
  const tool = await until('the fixture listed', async () => (await api(`/v1/tools?project=${id}`)).body.tools.find((held) => held.local && held.id === `${fixture}/shot`), Boolean, FIRST_RUN_MS);
  // The sidebar names operations only; model implementations stay inside each node's configuration.
  const listed = () => page.evaluate(() => [...document.querySelectorAll('.tools .tool-groups section')].map((section) => ({
    section: section.dataset.palette,
    title: section.querySelector('h4').textContent,
    tools: [...section.querySelectorAll(section.dataset.palette === 'nodes' ? '[data-node]' : '[data-operation]')].map((row) => ({ operation: row.dataset.operation, model: row.dataset.model, kind: row.dataset.kind, label: row.querySelector('b')?.textContent ?? row.textContent.trim() })),
  })));
  await until('the eight tool categories listed', () => listed(), (rows) => rows.find((one) => one.section === 'tools')?.tools.length === 8);
  const atRest = await listed();
  seen.listed = atRest.map((one) => `${one.title}: ${one.tools.map((row) => row.label).join(', ')}`);
  check('nodes_and_tools_are_separate', atRest.length === 2 && atRest[0].title === 'Nodes' && atRest[1].title === 'Tools', atRest);
  check('exactly_four_basic_asset_types', JSON.stringify(atRest.find((one) => one.section === 'nodes')?.tools.map((row) => row.label)) === JSON.stringify(['Image', 'Video', 'Audio', 'Text']), atRest);
  const operations = atRest.find((one) => one.section === 'tools')?.tools ?? [];
  check('timeline_is_a_tool_category', JSON.stringify(operations.map((row) => row.label)) === JSON.stringify(['Image', 'Edit Image', 'Upscale Image', 'Video', 'Edit Video', 'Upscale Video', 'Video Timeline', 'LLM']), operations);
  check('operation_output_types', JSON.stringify(operations.map((row) => row.kind)) === JSON.stringify(['image', 'image', 'image', 'video', 'video', 'video', 'video', 'text']), operations);
  check('sidebar_has_no_model_rows', operations.every((row) => !row.model), operations);
  check('sidebar_has_no_discovery_controls', await page.locator('.tools input, .tools .browse-toggle, .tools .browse, .tools .box-shelf-head, .tools .tool-off').count() === 0 && !(await page.locator('.tools button').allTextContents()).some((word) => /Browse|Checkpoints|Add file/.test(word)));
  const metadata = (await api(`/v1/tools?project=${id}`)).body.tools;
  check('local_output_capabilities_remain', metadata.some((row) => row.id === `${fixture}/shot` && row.makes === 'video') && metadata.some((row) => row.id === `${fixture}/still` && row.makes === 'image'), metadata.filter((row) => row.id.startsWith(fixture)).map((row) => ({ id: row.id, makes: row.makes })));
  const namesBefore = (await state()).assets.map((asset) => asset.name);
  const imageNode = page.locator('.tools [data-palette="nodes"] [data-node="image"]');
  await imageNode.click();
  const imageImplementation = await implementationRow(page, 'image');
  await imageImplementation.click();
  await page.waitForTimeout(200);
  check('clicking_nodes_and_tools_does_not_create', JSON.stringify((await state()).assets.map((asset) => asset.name)) === JSON.stringify(namesBefore));
  const spot = await require('./page').openCanvas(page);
  await require('./page').carry(page, imageNode, spot);
  const choosing = await until('dragged image node for model choices', async () => (await state()).assets.find((asset) => !namesBefore.includes(asset.name)), Boolean);
  await open(choosing.name);
  await implementationRow(page, 'image');
  const implementations = () => page.locator('.tools [data-implementation][data-tool-operation="image"]').evaluateAll((rows) => rows.map((row) => row.dataset.tool));
  const defaults = (tool) => tool?.models?.length ? tool.models.map((slot) => slot.default) : [tool?.model?.default];
  const hasModel = (ids, ref) => ids.some((id) => defaults(metadata.find((tool) => tool.id === id)).some((model) => model?.startsWith(`${ref}@`)));
  const models = await until('Image implementations include the installed local fixture', implementations, (ids) => ids.includes(`${fixture}/still`) && hasModel(ids, 'qwen/qwen-image-2.1') && hasModel(ids, 'stabilityai/sdxl'));
  check('image_category_exposes_real_implementations', hasModel(models, 'qwen/qwen-image-2.1') && hasModel(models, 'stabilityai/sdxl'), models);
  const hasTextModel = metadata.some((one) => one.makes === 'text' && one.id !== 'text');
  check('llm_availability_matches_real_text_tools', await page.locator('.tools [data-operation="llm"]').getAttribute('aria-disabled') === String(!hasTextModel));
  const availability = await page.locator('.tools [data-palette="tools"] [data-operation]').evaluateAll((rows) => rows.map((row) => ({ operation: row.dataset.operation, kind: row.dataset.kind, disabled: row.getAttribute('aria-disabled') === 'true' })));
  check('edit_image_available_for_reference_capable_models', availability.some((row) => row.operation === 'edit_image' && !row.disabled), availability);
  const qwen = metadata.find((tool) => defaults(tool).some((model) => model?.startsWith('qwen/qwen-image-2.1@')));
  const editQwen = await implementationRow(page, 'edit_image', qwen.id);
  const target = await box(choosing.name).locator('.media').boundingBox();
  await require('./page').carry(page, editQwen, { x: target.x + target.width / 2, y: target.y + target.height / 2 });
  await until('dragged tool attaches to image holder', () => of(choosing.name), (asset) => asset.tool === qwen.id);
  check('old_tool_settings_box_is_removed', await box(choosing.name).locator('.tool-settings').count() === 0);
  const versionsBeforePicker = (await of(choosing.name)).versions;
  await box(choosing.name).getByRole('button', { name: 'Models and LoRAs', exact: true }).click();
  const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
  await picker.waitFor();
  const slots = qwen.models?.length ? qwen.models : qwen.model ? [qwen.model] : [];
  check('tool_badge_opens_its_declared_model_slots', slots.length > 0 && await picker.locator('[data-model-slot]').count() === slots.length);
  check('model_picker_uses_global_nsfw_setting', await picker.getByLabel(/Show NSFW/).count() === 0);
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  check('model_picker_keeps_tool_identity_and_versions', (await of(choosing.name)).tool === qwen.id && (await of(choosing.name)).versions === versionsBeforePicker);
  check('model_configuration_keeps_category_structure', JSON.stringify(await listed()) === JSON.stringify(atRest));
  await closeBox(page);
  await api(`/v1/projects/${id}/assets/${encodeURIComponent(choosing.name)}`, { method: 'DELETE' });
  await until('model-choice node removed', () => box(choosing.name).count(), (count) => count === 0);
  // The rest stand beside it, in reach while it's worked on.
  const cards = (await api(`/v1/projects/${id}/canvas`)).body.cards;
  const at = cards.shot;
  await api(`/v1/projects/${id}/canvas`, { method: 'PATCH', body: JSON.stringify({ cards: { coast: { x: at.x + 400, y: at.y }, amy: { x: at.x + 400, y: at.y + 180 }, walk: { x: at.x + 400, y: at.y + 360 }, hum: { x: at.x + 400, y: at.y + 540 } } }) });

  // The wire: each field by its type, and what it is.
  const byName = Object.fromEntries(tool.fields.map((held) => [held.field, held]));
  const beats = byName.beats;
  const member = (group, name) => group?.members?.find((held) => held.field === name);
  seen.wire = tool.fields.map((held) => `${held.field}:${held.type}`);
  check('wire_makes', tool.makes === 'video' && !('kind' in tool) && !('controls' in tool), Object.keys(tool));
  check('wire_types', ['summary:prompt', 'pictures:prompt', 'negative:words', 'beats:group', 'aspect_ratio:shape', 'turbo:toggle', 'bright:toggle', 'megapixels:number', 'quality:choice', 'edit:choice', 'first_frame:slot', 'plate:slot', 'sound:slot', 'clip:slot', 'references:slot', 'steps:number', 'seed:seed'].every((said) => seen.wire.includes(said)), seen.wire);
  check('wire_no_pickup_field', !byName.pickup && tool.pickup?.every === 5 && tool.pickup.offset === 2 && tool.pickup.first === 2, tool.pickup);
  check('wire_prompt_takes', JSON.stringify(byName.pictures.takes) === '["image"]' && !byName.summary.takes, byName.pictures);
  check('wire_auto', byName.bright.auto && byName.megapixels.auto && !byName.turbo.auto, [byName.bright, byName.megapixels]);
  check('wire_slots', byName.first_frame.accept === 'image' && byName.plate.accept === 'video' && byName.plate.spans === 'beats' && byName.sound.accept === 'audio' && byName.clip.described && byName.references.multiple && byName.references.described, [byName.first_frame, byName.plate, byName.references]);
  check('wire_groups', beats.list && beats.min_items === 1 && beats.max_items === 4 && member(beats, 'duration').total && member(beats, 'look')?.type === 'group' && !member(beats, 'look').list && member(beats, 'angles')?.list && member(member(beats, 'angles'), 'framing')?.type === 'prompt', beats);
  check('wire_described_choice', byName.edit.descriptions?.head && byName.edit.words?.head === 'Head swap', byName.edit);
  check('wire_advanced', byName.steps.advanced && !byName.segments.advanced, byName.steps);

  // Focused video tools expose a large parameter form, without an Advanced fold.
  await open('shot');
  const shotBox = box('shot');
  check('face_holds_prompts_groups_and_filled_slots', (await shotBox.locator('.mention-text[data-field=summary]').count()) === 1 && (await shotBox.locator('.mention-text[data-field=pictures]').count()) === 1 && (await shotBox.locator('.edit-shown .beats-edit[data-field=beats]').count()) === 1 && (await shotBox.locator('.edit-shown .slot-control[data-field=plate]').count()) === 1, await shotBox.innerText());
  check('video_parameters_are_prominent_without_a_more_fold', await shotBox.locator('.prominent').count() === 1 && await shotBox.locator('[data-field=first_frame]').count() === 1 && await shotBox.locator('[data-field=negative]').count() === 1);
  await more('shot');
  check('advanced_fold_is_removed', await shotBox.locator('.advanced-toggle').count() === 0 && await shotBox.locator('input[aria-label=Steps]').count() === 1);
  await shot('more');

  // Choices: up to four side by side, more in a menu, and options that say what they are in a dropdown, whatever their number.
  check('four_or_fewer_are_segments', (await shotBox.locator('[role=radiogroup][aria-label=Quality] [role=radio]').count()) === 3);
  check('more_is_a_menu', (await shotBox.locator('select[aria-label=Resolution]').count()) === 1);
  await shotBox.locator('button.dropdown[aria-label=Edit]').click();
  const entries = await shotBox.locator('.dropdown-list [role=option]').allInnerTexts();
  check('a_described_choice_is_a_dropdown', entries.length === 3 && entries.some((said) => said.includes('Head swap') && said.includes('Only the head')), entries);
  await shotBox.locator('.dropdown-list [role=option]:has-text("Head swap")').click();
  await until('head in the file', () => fieldsOf('shot'), (fields) => fields.edit === 'head');
  check('dropdown_shut_shows_its_words', (await shotBox.locator('button.dropdown[aria-label=Edit]').textContent()) === 'Head swap');
  // A reset takes the key out of the file.
  await shotBox.locator('[aria-label="Reset Edit"]').click();
  await until('edit unset', () => fieldsOf('shot'), (fields) => !('edit' in fields));
  await shotBox.locator('[role=radiogroup][aria-label=Quality] [role=radio]:text-is("full")').click();
  await until('quality full', () => fieldsOf('shot'), (fields) => fields.quality === 'full');
  await shotBox.locator('[aria-label="Reset Quality"]').click();
  await until('quality unset', () => fieldsOf('shot'), (fields) => !('quality' in fields));
  check('reset_unsets', !/^\s+quality:/m.test(linesOf(yaml(), 'shot')), linesOf(yaml(), 'shot'));

  // Auto: a toggle's third state, and a number's, unset either way.
  await shotBox.locator('[role=radiogroup][aria-label=Bright] [role=radio]:text-is("On")').click();
  await until('bright on', () => fieldsOf('shot'), (fields) => fields.bright === true);
  await shotBox.locator('[role=radiogroup][aria-label=Bright] [role=radio]:text-is("Auto")').click();
  await until('bright auto', () => fieldsOf('shot'), (fields) => !('bright' in fields));
  check('toggle_auto_is_unset', true);
  check('number_auto_says_so', (await shotBox.locator('label.slide.auto:has(input[aria-label=Megapixels])').count()) === 1);
  await shotBox.locator('input[aria-label=Megapixels]').focus();
  await page.keyboard.press('ArrowRight');
  await until('megapixels set', () => fieldsOf('shot'), (fields) => typeof fields.megapixels === 'number');
  await shotBox.locator('[aria-label="Megapixels: Auto"]').click();
  await until('megapixels auto', () => fieldsOf('shot'), (fields) => !('megapixels' in fields));
  check('number_auto_is_unset', true);

  // Existing seed data remains valid, but the UI deliberately has no seed control.
  await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields: { seed: 17 } }) });
  await until('stored seed remains supported', () => fieldsOf('shot'), (fields) => fields.seed === 17);
  check('seed_input_is_hidden_without_clearing_stored_data', await shotBox.locator('[aria-label=Seed], [data-field=seed], .seed').count() === 0 && (await fieldsOf('shot')).seed === 17);
  await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields: { seed: null } }) });

  // Groups: rows with + and ×, dragged into a new order, their sum; a block that may be absent; rows two deep.
  const beatRows = () => shotBox.locator('.beats-edit[data-field=beats] > .beat');
  check('one_beat_to_start', (await beatRows().count()) === 1 && (await shotBox.locator('[aria-label="Remove beat 1"]').count()) === 0);
  await shotBox.locator('.beats-edit[data-field=beats] > .beats-last .add-beat').click();
  await until('two beats', () => beatRows().count(), (n) => n === 2);
  await shotBox.locator('.mention-text[data-field="beats.0.description"]').click();
  await page.keyboard.type('The first.');
  await shotBox.locator('.mention-text[data-field="beats.1.description"]').click();
  await page.keyboard.type('The second.');
  await until('both beats in the file', () => fieldsOf('shot'), (fields) => fields.beats?.length === 2 && fields.beats[1].description?.includes('The second.'));
  check('total_is_their_sum', (await shotBox.locator('.beats-edit[data-field=beats] > .beats-last .total').textContent()).trim() === '10 s', await shotBox.locator('.beats-last .total').textContent());
  const grip = await shotBox.locator('[aria-label="Move beat 2"]').boundingBox();
  const first = await beatRows().first().boundingBox();
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
  await page.mouse.down();
  await page.mouse.move(grip.x + grip.width / 2, first.y + 4, { steps: 8 });
  await page.mouse.up();
  await until('reordered', () => fieldsOf('shot'), (fields) => fields.beats?.[0]?.description?.includes('The second.'));
  check('rows_drag_to_reorder', true);
  await shotBox.locator('[aria-label="Remove beat 2"]').click();
  await until('one beat left', () => fieldsOf('shot'), (fields) => fields.beats?.length === 1);
  check('a_row_goes', (await beatRows().count()) === 1);
  // A block in a row: absent until it's added, and × takes it out.
  await shotBox.locator('[data-field="beats.0.look"] .add-beat').click();
  await until('a look', () => fieldsOf('shot'), (fields) => fields.beats?.[0]?.look?.lens === 'wide');
  await shotBox.locator('[data-field="beats.0.look"] [role=radio]:text-is("close")').click();
  await until('close', () => fieldsOf('shot'), (fields) => fields.beats?.[0]?.look?.lens === 'close');
  await shotBox.locator('[aria-label="Remove look"]').click();
  await until('no look', () => fieldsOf('shot'), (fields) => fields.beats && !('look' in fields.beats[0]));
  check('a_block_comes_and_goes', true);
  // Two deep: an angle in the beat, with a mention in its own prompt.
  await shotBox.locator('[data-field="beats.0.angles"] .add-beat').click();
  await shotBox.locator('.mention-text[data-field="beats.0.angles.0.framing"]').click();
  await page.keyboard.type('Low on <amy> ');
  await until('two deep in the file', () => fieldsOf('shot'), (fields) => fields.beats?.[0]?.angles?.[0]?.framing?.includes('<amy>') && fields.beats[0].angles[0].hold === 1);
  check('groups_two_deep', true);
  await shot('groups');

  // Slots: what doesn't fit is refused as it's carried over, and at the drop, with why; a filled one is on the face, and × unsets it.
  // A point of a box's picture a person can reach, where a press moves the box: what floats over it, and the box being worked on, may cover the
  // rest, and its × at rest is over its top.
  const reach = (name) => page.evaluate((name) => {
    const node = document.querySelector(`.react-flow__node:has(.card[data-asset="${name}"])`);
    const at = (node.querySelector('.card > .media') ?? node).getBoundingClientRect();
    for (let y = at.top + 4; y < Math.min(at.bottom, innerHeight); y += 8) for (let x = at.left + 4; x < Math.min(at.right, innerWidth); x += 8) {
      const el = document.elementFromPoint(x, y);
      if (el?.closest('.react-flow__node')?.querySelector('.card')?.dataset.asset === name && !el.closest('button')) return { x, y };
    }
    return null;
  }, name);
  const carry = async (from, to, look) => {
    await to.evaluate((el) => el.scrollIntoView({ block: 'nearest' }));
    const start = await reach(from);
    const end = await to.boundingBox();
    await page.mouse.move(start.x, start.y);
    await page.mouse.down();
    await page.mouse.move(start.x + 8, start.y);
    await page.mouse.move(end.x + end.width / 2, end.y + end.height / 2, { steps: 10 });
    await page.waitForTimeout(150);
    const said = look ? await look() : null;
    await page.mouse.up();
    return said;
  };
  await more('shot');
  const firstFrame = field('shot', 'first_frame');
  const whileCarried = await carry('walk', firstFrame, async () => ({ refuses: (await firstFrame.getAttribute('class')).includes('refuses'), why: await firstFrame.locator('.refusal').textContent().catch(() => '') }));
  check('wrong_kind_said_as_carried', whileCarried.refuses && whileCarried.why.includes('takes a picture'), whileCarried);
  await noted('takes a picture').waitFor();
  check('wrong_kind_refused_at_the_drop', !('first_frame' in (await fieldsOf('shot'))), await fieldsOf('shot'));
  // Written by hand, it's marked: a render warns and leaves it out (decision 8).
  await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields: { first_frame: '<walk>' } }) });
  const marked = await until('a wrong kind marked', () => shotBox.locator('.slot-control[data-field=first_frame] .chip.refused').getAttribute('title').catch(() => null), Boolean);
  check('wrong_kind_by_hand_is_marked', marked.includes('leaves it out'), marked);
  await shotBox.locator('[aria-label="Take walk out of First frame"]').click();
  await until('first frame unset', () => fieldsOf('shot'), (fields) => !('first_frame' in fields));
  await more('shot');
  await carry('coast', field('shot', 'first_frame'));
  await until('coast on first frame', () => fieldsOf('shot'), (fields) => fields.first_frame === '<coast>');
  check('a_filled_slot_is_on_the_face', (await shotBox.locator('.edit-shown .slot-control[data-field=first_frame]').count()) === 1);
  await shotBox.locator('[aria-label="Take coast out of First frame"]').click();
  await until('first frame unset', () => fieldsOf('shot'), (fields) => !('first_frame' in fields));
  check('unset_goes_back_under_more', (await shotBox.locator('.edit-shown [data-field=first_frame]').count()) === 0);
  // Typed in: the same rule.
  await more('shot');
  await field('shot', 'sound').locator('input.slot-type').fill('walk');
  await page.keyboard.press('Enter');
  await noted('takes a sound').waitFor();
  await field('shot', 'sound').locator('input.slot-type').fill('<hum>');
  await page.keyboard.press('Enter');
  await until('hum on sound', () => fieldsOf('shot'), (fields) => fields.sound === '<hum>');
  check('a_typed_name_fills_a_slot', true);
  // Several, in order.
  await more('shot');
  for (const name of ['amy', 'coast']) {
    await field('shot', 'references').locator('input.slot-type').fill(name);
    await page.keyboard.press('Enter');
  }
  await until('both references', () => fieldsOf('shot'), (fields) => JSON.stringify(fields.references) === '["<amy>","<coast>"]');
  check('a_slot_of_several', true);
  await shot('slots');

  // Prompts: what a prompt can't send is marked and stays its name; words take only words.
  await open('shot');
  await shotBox.locator('.mention-text[data-field=pictures]').click();
  await page.keyboard.type('With <walk> ');
  await more('shot');
  await shotBox.locator('.mention-text[data-field=negative]').click();
  await page.keyboard.type('Not <coast> ');
  await until('both typed', () => fieldsOf('shot'), (fields) => fields.pictures?.includes('<walk>') && fields.negative?.includes('<coast>'));
  check('a_video_in_pictures_is_marked', (await shotBox.locator('.mention-text[data-field=pictures] .chip.refused').count()) === 1);
  check('a_picture_in_words_is_marked', (await shotBox.locator('.mention-text[data-field=negative] .chip.refused').count()) === 1);

  // A video's range in a prompt: in and out on its frames, written in frames, shown as timecode.
  await shotBox.locator('.mention-text[data-field=summary]').click();
  await page.keyboard.type('Along <walk> ');
  const range = shotBox.locator('.mention-thumbs .clip-range').first();
  await range.waitFor();
  const frames = Number(await range.locator('[aria-label="walk: out"]').getAttribute('aria-valuemax'));
  await range.locator('[aria-label="walk: in"]').focus();
  for (let i = 0; i < 3; i++) await page.keyboard.press('ArrowRight');
  await until('an in point', () => fieldsOf('shot'), (fields) => fields.summary?.includes('<walk:3->'));
  await range.locator('[aria-label="walk: out"]').focus();
  for (let i = 0; i < 2; i++) await page.keyboard.press('ArrowLeft');
  const ranged = await until('an out point', () => fieldsOf('shot'), (fields) => fields.summary?.includes(`<walk:3-${frames - 2}>`));
  check('a_range_is_written_in_frames', ranged.summary.includes(`<walk:3-${frames - 2}>`), ranged.summary);
  const chip = (await shotBox.locator(`.mention-text[data-field=summary] .chip[data-mention="walk:3-${frames - 2}"]`).textContent()) ?? '';
  check('a_range_shows_timecode', /^<Walk> 0:00\.03–0:0\d\.\d\d$/.test(chip), chip);
  // On the plate, the same, and its length against the beats'.
  const plate = field('shot', 'plate');
  check('a_plate_against_its_beats', (await plate.locator('.spans.warn').textContent()).includes('the beats 5 s'), await plate.locator('.spans').textContent());
  await plate.locator('.clip-range [aria-label$=": in"]').focus();
  await page.keyboard.press('ArrowRight');
  await until('the plate ranged', () => fieldsOf('shot'), (fields) => fields.plate === '<walk:1->');
  check('a_plate_takes_a_range', true);
  await shot('ranges');
  await closeBox(page);

  // Rendering, where this computer runs models: previews land as it goes, then Pick up from here.
  if (local) {
    const versions = async (name) => (await of(name))?.prefixes?.length ?? 0;
    const given = async (name) => {
      const path = (await api(`/v1/projects/${id}/assets/${name}`)).body.versions.at(-1).files.given.path;
      await until('what it was given, in files/', () => Promise.resolve(existsSync(join(folder, path))), Boolean);
      return JSON.parse(readFileSync(join(folder, path), 'utf8'));
    };
    const render = async (name, count) => {
      await open(name);
      if (await box(name).locator('.take.add[aria-label="Its words"]:not(.on)').count()) await box(name).locator('.take.add[aria-label="Its words"]').click();
      await box(name).locator('button[aria-label=Render]').click();
      await until(`${name}'s ${count}`, () => versions(name), (n) => n === count, FIRST_RUN_MS);
    };
    await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields: { segments: 2, summary: 'Along the coast.', pictures: null, negative: null, plate: '<walk>' } }) });
    await open('shot');
    await box('shot').locator('button[aria-label=Render]').click();
    const glimpse = await until('a preview on the box', () => box('shot').locator('.glimpse').getAttribute('data-label').catch(() => null), (label) => label?.startsWith('Segment'), FIRST_RUN_MS);
    check('previews_land_on_the_box', glimpse.startsWith('Segment 1 of 2') || glimpse.startsWith('Segment 2 of 2'), glimpse);
    await until('its take', () => versions('shot'), (n) => n === 1, FIRST_RUN_MS);
    const typed = linesOf(yaml(), 'shot');
    const asTyped = await given('shot');
    check('each_type_reaches_the_function', asTyped.summary?.text === 'Along the coast.' && Array.isArray(asTyped.beats) && asTyped.beats[0].angles?.[0]?.framing?.text?.includes('<amy>') && asTyped.plate?.file === 'VideoFile', asTyped);
    check('a_mention_two_deep_is_its_prompts_reference', Object.keys(asTyped.beats[0].angles[0].framing.references ?? {}).join() === 'amy', asTyped.beats[0].angles[0].framing);

    // On its own take: scrubbed to a frame, it lands on the tool's grid, and picks up there.
    await open('shot');
    await box('shot').locator('.from-here').waitFor();
    await box('shot').locator('.from-here .keeps').fill('4');
    check('it_says_where_it_lands', (await box('shot').locator('.from-here .cut-says').textContent()).startsWith('Lands on 0:00.02'), await box('shot').locator('.from-here').innerText());
    await box('shot').locator('button:text-is("Pick up from here")').click();
    await until('a picked-up take', () => versions('shot'), (n) => n === 2, FIRST_RUN_MS);
    const picked = await given('shot');
    check('pickup_reaches_the_function', picked.pickup?.out_frame === 2 && picked.pickup.take?.file?.file === 'VideoFile', picked.pickup);
    const afterPickup = yaml();
    const pickedLines = linesOf(afterPickup, 'shot');
    check('pickup_writes_nothing', pickedLines === typed && !afterPickup.includes('pickup'), { before: typed, after: pickedLines, hasPickup: afterPickup.includes('pickup') });

    // Another box's take let go on it: from that take, at a frame chosen on it.
    importYaml(folder, `${yaml().trimEnd()}\n  other:\n    tool: ${fixture}/shot\n    plate: <walk>\n`);
    await until('other in the project', () => of('other'), Boolean);
    await api(`/v1/projects/${id}/canvas`, { method: 'PATCH', body: JSON.stringify({ cards: { other: { x: at.x - 420, y: at.y } } }) });
    await render('other', 1);
    await open('other');
    await box('other').locator('.take[aria-label="take 1"]').click();
    const onto = await reach('shot');
    await box('other').locator('.take-out').dragTo(page.locator('body'), { targetPosition: onto });
    await box('shot').locator('.from-here .pickup-of:has-text("other\'s take 1")').waitFor();
    check('another_take_offers_pick_up', (await box('shot').locator('.card.here').count()) === 1);
    await box('shot').locator('.from-here .keeps').fill('7');
    await box('shot').locator('button:text-is("Pick up from here")').click();
    await until('picked up from other', () => versions('shot'), (n) => n === 3, FIRST_RUN_MS);
    const fromOther = await given('shot');
    check('pickup_of_another_take', fromOther.pickup?.out_frame === 7 && fromOther.pickup.take?.file?.file === 'VideoFile', fromOther.pickup);
    await shot('picked-up');
  }

  check('sidebar_still_only_nodes_and_tools', JSON.stringify(await page.locator('.tools h4').allTextContents()) === JSON.stringify(['Nodes', 'Tools']) && await page.locator('.tools .browse, .tools .box-shelf-head, .tools .tool-off, .tools input').count() === 0);
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  check('nothing_refused', refused.filter((refusal) => refusal.url.includes('/v1/projects/')).length === 0, refused);
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  process.exit(2);
}
