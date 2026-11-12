// One Models & LoRAs picker for every tool, and H3's fields unfolded (cl-934, Paul 11-12: "for minimax h3, we should display the 'more' by
// default ... We also don't need to display the fine-tune as its own section ... for images ... 'models and loras' at the bottom, next to the
// tool ... I want this type of UX to be uniform"), in a real browser on a real Calcifer. H3's Shot in Focus: every field shown, no More, Less
// or Advanced; no fine-tune or LoRA rows of its own; Models & LoRAs at the bottom of its fields, its picker listing H3's Base Model once
// (its segments' slots as one, written `base_model`), a LoRA picked there set on the box; its tool named once, "<Package> · <Tool>", under its media, nothing of it
// beside the title. Qwen's Image's Models & LoRAs opens the same picker. Screenshots of each.
// The isolated Hub holds H3's default model (a stand-in) and an H3 LoRA of its family (e2e/isolated.sh, `loras`).
//   node models.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, until } = require('./page');

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

/** A tool as the page names it (shown.ts toolName). */
const named = (tool) => (tool.package_title && tool.package_title !== tool.label ? `${tool.package_title} · ${tool.label}` : tool.label);

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
  // What Calcifer refused, and why: a write the page keeps as a draft says so only in a note.
  const refused = [];
  page.on('response', (answer) => {
    if (answer.status() < 400 || answer.request().method() === 'GET') return;
    const refusal = { status: answer.status(), method: answer.request().method(), url: new URL(answer.url()).pathname, during };
    refused.push(refusal);
    answer.json().then(({ error }) => Object.assign(refusal, { code: error?.code, message: error?.message })).catch(() => {});
  });

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-models-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const fieldsOf = async (name) => (await api(at)).body.assets.find((asset) => asset.name === name)?.fields ?? {};

  // H3's Shot and Qwen's Image, each a box; the Shot's segments' base model holding a fine-tune, as Paul's did (run 46907).
  const tools = [...((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? []), ...((await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? [])];
  const shotTool = tools.find((tool) => tool.id === 'minimax-h3/generate');
  const imageTool = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && (tool.models?.length || tool.model));
  if (!shotTool || !imageTool) throw new Error(`the Hub lists no H3 Shot or Qwen Image: ${JSON.stringify(tools.map((tool) => tool.id))}`);
  const bases = (shotTool.models ?? []).filter((slot) => slot.slot.endsWith('base_model'));
  seen.slots = (shotTool.models ?? []).map((slot) => ({ slot: slot.slot, family: slot.family, kind: slot.kind }));
  if (!bases.length) throw new Error(`H3's Shot declares no base_model slot: ${JSON.stringify(seen.slots)}`);
  const tune = 'someone/h3-tune@0.1.0';
  const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: shotTool.id, kind: 'video', name: 'scene', exact: true, at: { x: 80, y: 80 },
    fields: { summary: 'A harbour at dusk.', beats: [{ description: 'Boats come in.', duration: 5 }], models: { base_model: { ref: tune } } } }) });
  if (made.status >= 300) throw new Error(`the Shot box: ${JSON.stringify(made)}`);
  const still = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: imageTool.id, kind: 'image', name: 'still', exact: true, fields: { instructions: 'A lighthouse.' }, at: { x: 700, y: 80 } }) });
  if (still.status >= 300) throw new Error(`the Image box: ${JSON.stringify(still)}`);
  await boxOf(page, 'scene').waitFor();
  await boxOf(page, 'still').waitFor();
  await fitCanvas(page);

  // H3 in Focus: every field at once.
  await openBox(page, 'scene');
  const scene = page.locator('.card.here[data-asset="scene"]').first();
  await scene.locator('.beats-edit[data-field="beats"]').waitFor();
  const declared = new Set(shotTool.fields.map((field) => field.field));
  const wanted = shotTool.fields.filter((field) => ['summary', 'beats', 'soundscape', 'music', 'first_frame', 'last_frame', 'carried_frames'].includes(field.field));
  const shown = {};
  for (const field of wanted) shown[field.field] = await scene.locator(`[data-field="${field.field}"], [aria-label="${field.label}"]`).count();
  seen.shown = shown;
  check('every_field_shown', wanted.length >= 5 && Object.values(shown).every((n) => n > 0), { shown, declared: [...declared] });
  check('no_more_less_or_advanced', (await scene.locator('.more-toggle, .edit-more, .advanced, .advanced-toggle').count()) === 0, await scene.locator('.more-toggle, .edit-more, .advanced').allInnerTexts());
  // No fine-tune or LoRA rows of its own: what's set is listed under Models & LoRAs, as a picture's is.
  check('no_fine_tune_section', (await scene.locator('.add-on-row, .add-ons, .tool-head').count()) === 0 && !/Fine-tune/.test(await scene.innerText()), await scene.innerText());
  // Its tool named once, as the canvas names it, and nothing of it beside the title (Paul, 11-12).
  const word = named(shotTool);
  const said = await page.locator('.focus-viewport').evaluate((root, word) => ({
    times: [...root.querySelectorAll('*')].filter((el) => el.getClientRects().length && ![...el.children].some((child) => child.textContent.includes(word)) && el.textContent.trim() === word).length,
    heading: root.querySelector('.focus-heading')?.textContent ?? '',
  }), word);
  seen.tool_said = { word, ...said };
  check('the_tool_named_once', said.times === 1, seen.tool_said);
  check('nothing_of_the_tool_beside_the_title', !said.heading.includes(shotTool.package_title) && !said.heading.includes(shotTool.label), said.heading);
  const models = scene.locator('.edit-foot .models-loras');
  const button = models.locator('button[aria-label="Models and LoRAs"]');
  await button.waitFor();
  const placed = await scene.evaluate((card) => {
    const pick = card.querySelector('.edit-foot .models-loras button[aria-label="Models and LoRAs"]');
    const controls = [...card.querySelectorAll('.edit-shown [data-field], .edit-shown [aria-label], .run')];
    const box = pick.getBoundingClientRect();
    return {
      buttons: card.querySelectorAll('button[aria-label="Models and LoRAs"]').length,
      words: pick.textContent.trim(),
      after_every_field: controls.every((el) => Boolean(el.compareDocumentPosition(pick) & Node.DOCUMENT_POSITION_FOLLOWING)),
      in_sight: box.bottom <= innerHeight && box.top >= innerHeight / 2,
    };
  });
  seen.placed = placed;
  check('models_and_loras_once_at_the_bottom_of_the_fields', placed.buttons === 1 && placed.words === 'Models & LoRAs' && placed.after_every_field && placed.in_sight, placed);
  const listed = () => models.locator('.active-models .active-model-name').evaluateAll((els) => els.map((el) => el.title));
  check('the_fine_tune_listed_under_it_once', JSON.stringify(await listed()) === JSON.stringify([tune]), await listed());
  await shot('h3-unfolded');

  // Its picker: H3's Base Model once, for every segment; a LoRA picked there is on the box.
  await button.click();
  const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
  await picker.waitFor();
  const choices = await picker.locator('[data-model-slot]').allInnerTexts();
  seen.h3_choices = choices;
  check('h3_lists_its_base_model_once', choices.filter((said) => /^Base Model$/.test(said.trim())).length === 1, choices);
  const shape = () => picker.evaluate((dialog) => ({ cls: dialog.className, parts: [...dialog.querySelectorAll('header, h2, .model-picker-body, .model-picker-slots')].map((el) => el.tagName + '.' + el.className) }));
  const h3Shape = await shape();
  await shot('h3-picker');
  await picker.locator('[data-model-slot]', { hasText: /^Base Model$/ }).click();
  await picker.getByRole('button', { name: 'LoRAs', exact: true }).click();
  const option = picker.locator('.model-picker-lora-option').first();
  await option.waitFor().catch(() => {});
  const offered = await picker.locator('.model-picker-lora-option').allInnerTexts();
  seen.offered = offered;
  if (!offered.length) throw new Error(`H3's Base Model offers no LoRA (seeded by isolated.sh's loras?): ${await picker.innerText()}`);
  // Wide enough, and nothing cut off sideways: a name wraps, a description takes at most two lines and ends in an ellipsis, all of it on
  // hover; on a narrow window too (Paul, 11-12: "it's not wide enough. All of the information is getting cropped off to the side").
  const fits = () => picker.evaluate((dialog) => {
    const body = dialog.querySelector('.model-picker-body');
    const box = dialog.getBoundingClientRect();
    const options = [...dialog.querySelectorAll('.model-picker-lora-option')].map((option) => {
      const label = option.querySelector(':scope > span > span');
      const about = option.querySelector('small');
      const line = Number.parseFloat(getComputedStyle(about).lineHeight) || Number.parseFloat(getComputedStyle(about).fontSize) * 1.4;
      return {
        name: label.textContent, about: about.textContent, title: about.title,
        name_whole: label.scrollWidth <= label.clientWidth + 1 && option.scrollWidth <= option.clientWidth + 1,
        lines: Math.round(about.clientHeight / line), cut: about.scrollHeight > about.clientHeight + 1,
      };
    });
    return { width: box.width, inside: box.left >= 0 && box.right <= innerWidth + 0.5, sideways: body.scrollWidth > body.clientWidth + 1, options };
  });
  const wide = await fits();
  seen.picker_wide = wide;
  const whole = (one) => one.inside && !one.sideways && one.options.every((option) => option.name_whole && option.lines <= 2 && option.title === option.about);
  check('the_picker_is_wide_and_cuts_nothing_off', wide.width >= 700 && whole(wide), wide);
  await page.setViewportSize({ width: 420, height: 900 });
  await page.waitForTimeout(300);
  const narrow = await fits();
  seen.picker_narrow = narrow;
  check('on_a_narrow_window_it_fits_and_wraps', whole(narrow) && narrow.options.some((option) => option.lines === 2), narrow);
  await shot('picker-narrow');
  await page.setViewportSize({ width: 1600, height: 1100 });
  await page.waitForTimeout(300);
  await option.click();
  // One Base Model, written by its parameter, which the binder takes to every segment's slot (choices.rs `slots`).
  const after = await until('the LoRA on its Base Model', () => fieldsOf('scene'), (fields) => fields.models?.base_model?.lora?.length === 1).catch(() => fieldsOf('scene'));
  seen.models = after.models;
  seen.refused = refused;
  seen.notes = await page.locator('.note').allInnerTexts().catch(() => []);
  const lora = after.models?.base_model?.lora?.[0]?.ref;
  check('a_lora_picked_is_on_the_box', Boolean(lora) && after.models.base_model.ref === tune && Object.keys(after.models).length === 1, after.models);
  await shot('h3-lora-picked');
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  await picker.waitFor({ state: 'detached' });
  const now = await until('the LoRA listed under Models & LoRAs', listed, (refs) => refs.includes(lora)).catch(listed);
  check('the_lora_listed_under_models_and_loras', now.length === 2 && now.includes(tune) && now.includes(lora), now);
  await shot('h3-after');
  await closeBox(page);

  // A picture's Models & LoRAs opens the same picker.
  await openBox(page, 'still');
  const picture = page.locator('.card.here[data-asset="still"]').first();
  const pictureSaid = await picture.locator('.image-controls .image-tool-name').innerText();
  check('a_picture_names_its_tool_the_same_way', pictureSaid === named(imageTool), { pictureSaid, wanted: named(imageTool) });
  check('a_picture_has_one_models_and_loras', (await picture.locator('button[aria-label="Models and LoRAs"]').count()) === 1);
  await picture.locator('button[aria-label="Models and LoRAs"]').click();
  await picker.waitFor();
  const imageShape = await shape();
  const imageSlots = new Set((imageTool.models?.length ? imageTool.models : [imageTool.model]).map((slot) => slot.slot.split('.models.').at(-1))).size;
  seen.shapes = { h3: h3Shape, image: imageShape };
  check('an_image_opens_the_same_picker', JSON.stringify(h3Shape) === JSON.stringify(imageShape) && (await picker.locator('[data-model-slot]').count()) >= 1 && (await picker.locator('[data-model-slot]').count()) <= imageSlots, { h3Shape, imageShape, imageSlots });
  await shot('image-picker');
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  await closeBox(page);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
