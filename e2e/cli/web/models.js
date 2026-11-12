// One Models & LoRAs picker for every tool, and H3's fields unfolded (cl-934, Paul 11-12: "for minimax h3, we should display the 'more' by
// default ... We also don't need to display the fine-tune as its own section ... for images ... 'models and loras' at the bottom, next to the
// tool ... I want this type of UX to be uniform"), in a real browser on a real Calcifer. H3's Shot in Focus: every field shown, no More, Less
// or Advanced; no fine-tune or LoRA rows of its own; beneath it its tool with Models & LoRAs beside it, the picker listing H3's Base Model once
// (its segments' slots as one, written `base_model`), a LoRA picked there set on the box; its tool named once, "<Package> · <Tool>", under its media, nothing of it
// beside the title. Qwen's Image has the same component beneath it, and its Models & LoRAs opens the same picker. Screenshots of each.
// Its VAE slot offers the VAEs of its family and its Model slot none of them; a VAE picked is the box's `vae` (se-131).
// A LoRA added on SDXL, Ideogram 4 (words) or Krea 2 (a prompt) puts its trigger in the prompt, never a negative prompt (se-132).
// The isolated Hub holds H3's default model (a stand-in) and an H3 LoRA of its family (e2e/isolated.sh, `loras`), and Qwen's
// default model and a VAE of its family (stand-ins, `vaes`).
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
  /** The theme set as a person sets it, by the menu's Dark or Paper (Shell.tsx ThemeSwitch): the canvas on its dark sheet, not paper in dark
   *  colours (se-132). Clicked in the page, as no press outside closes what's open. */
  const theme = async (dark) => {
    const set = await page.evaluate((dark) => {
      const now = () => (document.documentElement.dataset.theme === 'dark') === dark;
      if (!now()) [...document.querySelectorAll('.main-menu nav button')].find((button) => button.textContent.trim() === (dark ? 'Dark' : 'Paper'))?.click();
      return now();
    }, dark);
    if (!set) throw new Error(`the menu set no ${dark ? 'dark' : 'paper'} theme`);
    await page.waitForTimeout(300);
  };
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
  // Beneath the node, one component for every tool and kind (Paul, 11-12: "just like for the image editing models it is"): its tool,
  // Models & LoRAs beside it, what's set listed under them; nothing of models or LoRAs inside the card.
  const models = scene.locator(':scope > .tool-line');
  const button = models.locator('button[aria-label="Models and LoRAs"]');
  await button.waitFor();
  /** What sits beneath a node, as classes in order, and whether anything of models is inside its card. */
  const beneath = (card) => card.evaluate((node) => {
    const line = node.querySelector(':scope > .tool-line');
    const row = line?.querySelector('.image-version-row');
    const pick = row?.querySelector('button[aria-label="Models and LoRAs"]');
    const tool = row?.querySelector('.image-tool-name');
    return {
      row: [...(row?.children ?? [])].map((el) => el.className),
      tool: tool?.textContent ?? null,
      words: pick?.textContent.trim() ?? null,
      beside: Boolean(pick && tool) && Math.abs(pick.getBoundingClientRect().top + pick.getBoundingClientRect().height / 2 - (tool.getBoundingClientRect().top + tool.getBoundingClientRect().height / 2)) < pick.getBoundingClientRect().height,
      buttons: node.querySelectorAll('button[aria-label="Models and LoRAs"]').length,
      inside: node.querySelectorAll('.here-under .active-models, .here-under button[aria-label="Models and LoRAs"], .media .active-models').length,
    };
  });
  const placed = await beneath(scene);
  seen.placed = placed;
  check('models_and_loras_beside_the_tool_beneath_the_node', placed.buttons === 1 && placed.words === 'Models & LoRAs' && placed.beside && placed.inside === 0 && placed.tool === word, placed);
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
  // 360-orbit: its one trigger is a whole prompt, commas and all (cl-971).
  const option = picker.locator('.model-picker-lora-option', { hasText: /360 Orbit/i }).first();
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

  // cl-961: each LoRA beneath the node has its strength's slider; dragging it stores the strength at once (Paul, 11-12).
  const slider = models.locator('.active-lora input[type="range"]').first();
  await slider.waitFor();
  const track = await slider.boundingBox();
  const was = (await fieldsOf('scene')).models?.base_model?.lora?.[0]?.strength;
  await page.mouse.move(track.x + track.width * 0.5, track.y + track.height / 2);
  await page.mouse.down();
  await page.mouse.move(track.x + track.width * 0.9, track.y + track.height / 2, { steps: 6 });
  await page.mouse.up();
  const dragged = await until('the dragged strength stored', () => fieldsOf('scene'), (fields) => typeof fields.models?.base_model?.lora?.[0]?.strength === 'number' && fields.models.base_model.lora[0].strength !== was).catch(() => fieldsOf('scene'));
  const strength = dragged.models?.base_model?.lora?.[0]?.strength;
  seen.dragged = { was, strength };
  check('dragging_the_slider_stores_the_strength', typeof strength === 'number' && strength > 1.5 && strength <= 2, seen.dragged);
  const loraRow = await models.locator('.active-lora').first().evaluate((el) => {
    const number = el.querySelector('input[type="number"]');
    return { height: el.getBoundingClientRect().height, number: number?.value, whole: Boolean(number) && number.scrollWidth <= number.clientWidth + 1, reset: Boolean(el.querySelector('button[aria-label*="recommended strength"]')) };
  });
  seen.lora_row = loraRow;
  check('a_lora_row_is_tall_its_number_whole_and_resets', loraRow.height >= 28 && loraRow.whole && Number(loraRow.number) === strength && loraRow.reset, loraRow);

  // cl-960 + cl-959: a model slot takes one model. Its picker lists the Hub's family at once; a second pick replaces the first.
  const radios = picker.locator('.model-picker-choices [role="radio"]');
  await button.click();
  await picker.waitFor();
  const opened = Date.now();
  await picker.locator('[data-model-slot]', { hasText: /^Base Model$/ }).click();
  await until('Base Model lists its models', () => radios.count(), (n) => n >= 4).catch(() => 0);
  seen.listed_ms = Date.now() - opened;
  check('a_cold_open_lists_within_2_s', seen.listed_ms <= 2000 && (await radios.count()) >= 4, { ms: seen.listed_ms, choices: await radios.allInnerTexts() });
  const pick = async (label, model) => {
    await radios.filter({ hasText: label }).first().click();
    return until(`${model} picked`, () => fieldsOf('scene'), (fields) => fields.models?.base_model?.ref === model).catch(() => fieldsOf('scene'));
  };
  await pick('sexgod1979 / Pinkcherry', 'sexgod1979/pinkcherry');
  const second = await pick('tenstrip / Eros Max', 'tenstrip/eros-max');
  const others = Object.entries(second.models ?? {}).filter(([key, held]) => key !== 'base_model' && held?.ref);
  seen.second = { models: second.models, checked: await picker.locator('.model-picker-choices [aria-checked="true"]').allInnerTexts() };
  check('a_second_pick_replaces_the_first', second.models?.base_model?.ref === 'tenstrip/eros-max' && others.length === 0 && seen.second.checked.length === 1 && /Eros Max/.test(seen.second.checked[0]), seen.second);
  // Shown at 1440×900, light and dark: the picker and what's beneath the node (Paul's UI rule).
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(300);
  await shot('one-model-light');
  await theme(true);
  await shot('one-model-dark');
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  await picker.waitFor({ state: 'detached' });
  await shot('lora-strength-dark');
  await theme(false);
  await shot('lora-strength-light');
  await page.setViewportSize({ width: 1600, height: 1100 });
  // cl-968: a LoRA added with trigger words puts them after the box's prompt once; any prompt of the box that says them draws them in a token
  // colour, typed or appended; its row beneath the box shows them and puts one at the caret; removed, the words stay and the colour goes.
  const lit = (field) => page.evaluate((field) => {
    const held = [...document.querySelectorAll(`.card.here[data-asset="scene"] [data-field="${field}"]`)];
    const marks = globalThis.CSS?.highlights?.get('lora-trigger');
    return marks ? [...marks].filter((range) => held.some((el) => el.contains(range.startContainer))).map((range) => range.toString()) : [];
  }, field);
  const loraRef = (await fieldsOf('scene')).models?.base_model?.lora?.[0]?.ref ?? '';
  const words = (await api(`/v1/models/${loraRef.split('@')[0]}`)).body?.triggers ?? [];
  const times = (text, word) => String(text ?? '').toLowerCase().split(word.toLowerCase()).length - 1;
  const added = await fieldsOf('scene');
  seen.triggers = { lora: loraRef, words, summary: added.summary };
  check('a_lora_added_puts_its_trigger_after_the_prompt_once', words.length > 0 && words.every((word) => times(added.summary, word) === 1) && String(added.summary).startsWith('A harbour at dusk.'), seen.triggers);
  // cl-971: a trigger is one entry, kept whole: 360-orbit's prompt is one, never split at its commas.
  check('a_whole_prompt_trigger_is_one_entry', /360-orbit/.test(loraRef) && words.length === 1 && words[0].includes(',') && words[0].length > 400, { lora: loraRef, words });
  const litSummary = await until('the trigger drawn in the summary', () => lit('summary'), (marks) => words.every((word) => marks.some((mark) => mark.toLowerCase() === word.toLowerCase()))).catch(() => lit('summary'));
  check('the_appended_trigger_is_drawn', words.every((word) => litSummary.some((mark) => mark.toLowerCase() === word.toLowerCase())), litSummary);
  // Typed in another prompt of the box.
  const music = scene.locator('[data-field="music"]').first();
  await music.click();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.type(`${words[0]} under a slow drum`);
  await scene.locator('[data-field="summary"]').first().click();
  const typed = await until('the typed trigger stored', () => fieldsOf('scene'), (fields) => times(fields.music, words[0]) === 1).catch(() => fieldsOf('scene'));
  const litMusic = await until('the typed trigger drawn', () => lit('music'), (marks) => marks.some((mark) => mark.toLowerCase() === words[0].toLowerCase())).catch(() => lit('music'));
  check('a_trigger_typed_in_another_prompt_is_drawn', times(typed.music, words[0]) === 1 && litMusic.length >= 1, { music: typed.music, litMusic });
  // Its row beneath the box shows its triggers; one clicked goes at the caret of the prompt last typed in.
  const row = models.locator('.active-models .lora-triggers').first();
  seen.trigger_row = await row.allInnerTexts().catch(() => []);
  const soundscape = scene.locator('[data-field="soundscape"]').first();
  await soundscape.click();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.type('Wind,');
  await row.locator('.lora-trigger').first().click();
  const inserted = await until('a clicked trigger at the caret', () => fieldsOf('scene'), (fields) => times(fields.soundscape, words[0]) === 1).catch(() => fieldsOf('scene'));
  check('its_row_shows_its_triggers_and_one_goes_at_the_caret', words.every((word) => seen.trigger_row.join(' ').includes(word) || words.length > 4) && /^Wind,\s*/.test(String(inserted.soundscape)) && times(inserted.soundscape, words[0]) === 1, { row: seen.trigger_row, soundscape: inserted.soundscape });
  // One button per entry; a whole prompt shows at most two lines, all of it in its title.
  const buttons = await row.locator('.lora-trigger').evaluateAll((els) => els.map((el) => ({ title: el.title, lines: Math.round(el.clientHeight / (Number.parseFloat(getComputedStyle(el).lineHeight) || 16)), cut: el.scrollHeight > el.clientHeight + 1 })));
  seen.trigger_buttons = buttons;
  check('a_whole_prompt_is_one_button_of_two_lines', buttons.length === words.length && buttons.every((button, i) => button.title.includes(words[i])) && buttons.every((button) => button.lines <= 2), buttons);
  // Shown at 1440×900 and 1920×1080, light and dark (Paul's UI rule).
  for (const [width, height] of [[1440, 900], [1920, 1080]]) {
    await page.setViewportSize({ width, height });
    for (const look of ['light', 'dark']) {
      await theme(look === 'dark');
      await shot(`triggers-${width}x${height}-${look}`);
    }
  }
  await theme(false);
  await page.setViewportSize({ width: 1600, height: 1100 });
  // The LoRA removed: its words stay as written, no longer drawn.
  await button.click();
  await picker.waitFor();
  await picker.locator('[data-model-slot]', { hasText: /^Base Model$/ }).click();
  await picker.getByRole('button', { name: 'LoRAs', exact: true }).click();
  await picker.locator('.model-picker-adapters button[aria-label^="Remove "]').first().click();
  const removed = await until('the LoRA removed', () => fieldsOf('scene'), (fields) => !(fields.models?.base_model?.lora?.length)).catch(() => fieldsOf('scene'));
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  await picker.waitFor({ state: 'detached' });
  const unlit = await until('no trigger drawn', async () => [...await lit('summary'), ...await lit('music'), ...await lit('soundscape')], (marks) => marks.length === 0).catch(() => lit('summary'));
  seen.trigger_removed = { summary: removed.summary, music: removed.music, unlit };
  check('the_lora_removed_its_words_stay_undrawn', !(removed.models?.base_model?.lora?.length) && words.every((word) => times(removed.summary, word) === 1) && times(removed.music, words[0]) === 1 && unlit.length === 0, seen.trigger_removed);

  // Calcifer refuses a box naming one slot twice with two models, in one line (cl-960).
  const twice = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: shotTool.id, kind: 'video', name: 'twice', exact: true, at: { x: 80, y: 900 },
    fields: { summary: 'Twice.', beats: [{ description: 'Boats.', duration: 5 }], models: { base_model: { ref: 'sexgod1979/pinkcherry' }, [bases[0].slot.includes('.models.') ? bases[0].slot : 'ref2va_segment.models.base_model']: { ref: 'tenstrip/eros-max' } } } }) });
  seen.twice = twice;
  check('two_models_for_one_slot_refused', twice.status >= 400 && twice.body?.error?.code === 'model_slot_twice' && !String(twice.body?.error?.message).includes('\n'), twice);
  await closeBox(page);

  // A picture's Models & LoRAs opens the same picker.
  await openBox(page, 'still');
  const picture = page.locator('.card.here[data-asset="still"]').first();
  const under = await beneath(picture);
  seen.picture_beneath = under;
  check('a_picture_has_the_same_component_beneath_it', JSON.stringify(under.row) === JSON.stringify(placed.row) && under.tool === named(imageTool) && under.buttons === 1 && under.beside && under.inside === 0, { picture: under, shot: placed });
  await picture.locator(':scope > .tool-line button[aria-label="Models and LoRAs"]').click();
  await picker.waitFor();
  const imageShape = await shape();
  const imageSlots = new Set((imageTool.models?.length ? imageTool.models : [imageTool.model]).map((slot) => slot.slot.split('.models.').at(-1))).size;
  seen.shapes = { h3: h3Shape, image: imageShape };
  check('an_image_opens_the_same_picker', JSON.stringify(h3Shape) === JSON.stringify(imageShape) && (await picker.locator('[data-model-slot]').count()) >= 1 && (await picker.locator('[data-model-slot]').count()) <= imageSlots, { h3Shape, imageShape, imageSlots });
  await shot('image-picker');
  // Qwen's VAE: its own slot, offering its default and the VAE of its family, not its fine-tunes; the Model slot offers no VAE.
  const vae = 'madebyollin/texture-fix-vae';
  // A slot's page, once its list holds what `listed` says it should.
  const slotOpen = async (label, listed) => {
    await picker.locator('button[data-model-slot]', { hasText: new RegExp(`^${label}$`) }).click();
    return until(`${label} lists its choices`, () => radios.allInnerTexts(), listed).catch(() => radios.allInnerTexts());
  };
  const back = () => picker.getByRole('button', { name: 'Back to model slots', exact: true }).click();
  seen.image_slots = await picker.locator('button[data-model-slot]').allInnerTexts();
  const vaes = await slotOpen('VAE', (said) => said.some((one) => /Texture Fix VAE/.test(one)));
  seen.vae_choices = vaes;
  check('the_vae_slot_offers_its_familys_vaes', vaes.length === 2 && /Qwen Image 2\.1/.test(vaes[0]) && /madebyollin \/ Texture Fix VAE/.test(vaes[1]), vaes);
  await shot('qwen-vae');
  await radios.filter({ hasText: 'Texture Fix VAE' }).first().click();
  const picked = await until('the VAE on the box', () => fieldsOf('still'), (fields) => fields.models?.vae?.ref === vae).catch(() => fieldsOf('still'));
  seen.vae_models = picked.models;
  check('a_vae_picked_is_the_boxs_vae', picked.models?.vae?.ref === vae && Object.keys(picked.models).length === 1, picked.models);
  await shot('qwen-vae-picked');
  await back();
  const modelChoices = await slotOpen('Model', (said) => said.length >= 1);
  seen.model_choices = modelChoices;
  check('the_model_slot_offers_no_vae', modelChoices.length >= 1 && !modelChoices.some((one) => /Texture Fix VAE/.test(one)), modelChoices);
  await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
  await closeBox(page);

  // se-132: a LoRA's trigger goes to its tool's prompt; a tool whose prompt is words (SDXL's and Ideogram 4's `instructions: str`)
  // takes it in its first words that aren't a negative prompt, even one written first, and a picture those words mention stays crossed
  // out. Krea 2's prompt field takes it as before. Each tool's LoRA is a stand-in of its family with one trigger (isolated.sh `prompts`).
  const toolsFor = async (q) => (await api(`/v1/tools?project=${id}&q=${q}`)).body?.tools ?? [];
  const imageOf = async (pkg) => (await toolsFor(pkg.split('-')[0])).find((tool) => tool.package.split('/').at(-1) === pkg && tool.function === 'image');
  const wordsTools = { sdxl: await imageOf('sdxl'), ideogram: await imageOf('ideogram-4'), krea: await imageOf('krea-2') };
  if (Object.values(wordsTools).some((tool) => !tool)) throw new Error(`the Hub lists no SDXL, Ideogram 4 or Krea 2 Image: ${JSON.stringify(Object.fromEntries(Object.entries(wordsTools).map(([key, tool]) => [key, tool?.id])))}`);
  const typeOf = (tool, field) => tool.fields.find((held) => held.field === field)?.type;
  seen.prompt_types = Object.fromEntries(Object.entries(wordsTools).map(([key, tool]) => [key, typeOf(tool, 'instructions')]));
  check('sdxl_and_ideogram_write_words_krea_a_prompt', seen.prompt_types.sdxl === 'words' && seen.prompt_types.ideogram === 'words' && seen.prompt_types.krea === 'prompt', seen.prompt_types);
  const boxes = [
    { name: 'sdxl', tool: wordsTools.sdxl, said: 'A lighthouse at dusk beside <still>.', trigger: 'ink_wash', x: 80 },
    { name: 'ideogram', tool: wordsTools.ideogram, said: 'A diner sign at night', trigger: 'neon_sign', x: 700 },
    { name: 'krea', tool: wordsTools.krea, said: 'A clay fox in a meadow', trigger: 'claymation', x: 1320 },
  ];
  for (const box of boxes) {
    const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: box.tool.id, kind: 'image', name: box.name, exact: true, fields: { instructions: box.said }, at: { x: box.x, y: 1400 } }) });
    if (made.status >= 300) throw new Error(`the ${box.name} box: ${JSON.stringify(made)}`);
  }
  // Below the others, off screen until the canvas is fitted to them.
  const onPage = (names) => until(`${names.join(', ')} on the canvas`, async () => {
    await fitCanvas(page);
    return Promise.all(names.map((name) => boxOf(page, name).isVisible()));
  }, (shown) => shown.every(Boolean));
  await onPage(boxes.map((box) => box.name));
  const litIn = (name, field) => page.evaluate(({ name, field }) => {
    const held = [...document.querySelectorAll(`.card.here[data-asset="${name}"] [data-field="${field}"]`)];
    const marks = globalThis.CSS?.highlights?.get('lora-trigger');
    return marks ? [...marks].filter((range) => held.some((el) => el.contains(range.startContainer))).map((range) => range.toString()) : [];
  }, { name, field });
  /** Box `name` opened, its Model slot given its family's LoRA from the picker: the box's fields once the LoRA is on it. */
  const addLora = async (name, trigger) => {
    await openBox(page, name);
    const card = page.locator(`.card.here[data-asset="${name}"]`).first();
    await card.locator(':scope > .tool-line button[aria-label="Models and LoRAs"]').click();
    await picker.waitFor();
    await picker.locator('button[data-model-slot="model"]').click();
    await picker.getByRole('button', { name: 'LoRAs', exact: true }).click();
    const option = picker.locator('.model-picker-lora-option', { hasText: new RegExp(trigger.split('_').join(' '), 'i') }).first();
    await option.waitFor();
    await option.click();
    const fields = await until(`${name}'s LoRA on it`, () => fieldsOf(name), (now) => now.models?.model?.lora?.length === 1).catch(() => fieldsOf(name));
    await picker.getByRole('button', { name: 'Close model picker', exact: true }).click();
    await picker.waitFor({ state: 'detached' });
    return fields;
  };
  const unusedOf = async (name) => (await api(at)).body.assets.find((asset) => asset.name === name)?.unused ?? [];
  for (const box of boxes) {
    const fields = await addLora(box.name, box.trigger);
    const marks = await until(`${box.name}'s trigger drawn`, () => litIn(box.name, 'instructions'), (now) => now.some((mark) => mark.toLowerCase() === box.trigger)).catch(() => litIn(box.name, 'instructions'));
    seen[`trigger_${box.name}`] = { instructions: fields.instructions, lora: fields.models?.model?.lora, marks };
    check(`${box.name}_appends_its_trigger_once_and_draws_it`, String(fields.instructions).startsWith(box.said) && times(fields.instructions, box.trigger) === 1 && marks.some((mark) => mark.toLowerCase() === box.trigger), seen[`trigger_${box.name}`]);
    if (box.name === 'sdxl') {
      // `<still>`, a picture, in SDXL's words: Calcifer leaves it out, and the page strikes it through.
      const left = await until('sdxl leaves out <still>', () => unusedOf('sdxl'), (now) => now.some((one) => one.mention === 'still')).catch(() => unusedOf('sdxl'));
      const chipsOf = () => page.locator('.card.here[data-asset="sdxl"] [data-field="instructions"] .chip').evaluateAll((els) => els.map((chip) => ({ mention: chip.dataset.mention, unused: chip.classList.contains('unused'), struck: getComputedStyle(chip).textDecorationLine.includes('line-through'), title: chip.title })));
      const chips = await until('<still> struck through', chipsOf, (now) => now.some((chip) => chip.mention === 'still' && chip.unused)).catch(chipsOf);
      seen.sdxl_picture = { left, chips };
      check('a_picture_in_sdxls_words_is_crossed_out', left.some((one) => one.mention === 'still' && one.at === 'instructions' && /can't use images$/.test(one.why)) && chips.some((chip) => chip.mention === 'still' && chip.unused && chip.struck), seen.sdxl_picture);
      // And it sets no shape: Auto is SDXL's own, not "from <still>".
      const shapeOf = () => page.locator('.card.here[data-asset="sdxl"] [data-output-aspect]').first().getAttribute('data-output-aspect').catch(() => null);
      seen.sdxl_shape = await until('the shape from no crossed-out picture', shapeOf, (label) => label === 'Auto · 1:1').catch(shapeOf);
      check('a_crossed_out_picture_sets_no_shape', seen.sdxl_shape === 'Auto · 1:1', seen.sdxl_shape);
    }
    for (const [width, height] of [[1440, 900], [1920, 1080]]) {
      await page.setViewportSize({ width, height });
      for (const look of ['light', 'dark']) {
        await theme(look === 'dark');
        await shot(`prompt-${box.name}-${width}x${height}-${look}`);
      }
    }
    await theme(false);
    await page.setViewportSize({ width: 1600, height: 1100 });
    await closeBox(page);
  }
  // A tool whose first words are a negative prompt (a fixture: SDXL's description with `negative_prompt` moved first): the trigger
  // still goes to `instructions`.
  const reordered = [];
  await page.route(/\/v1\/tools\?/, async (route) => {
    const answer = await route.fetch();
    const body = await answer.json();
    for (const tool of body.tools ?? []) {
      if (tool.id !== wordsTools.sdxl.id) continue;
      const negative = tool.fields.findIndex((field) => field.field === 'negative_prompt');
      if (negative > 0) tool.fields.unshift(...tool.fields.splice(negative, 1));
      reordered.push(tool.fields.map((field) => `${field.field}:${field.type}`));
    }
    await route.fulfill({ response: answer, json: body });
  });
  await page.reload();
  await page.waitForSelector('.top');
  const neg = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: wordsTools.sdxl.id, kind: 'image', name: 'sdxl_neg', exact: true, fields: { instructions: 'A quiet pier' }, at: { x: 1940, y: 1400 } }) });
  if (neg.status >= 300) throw new Error(`the sdxl_neg box: ${JSON.stringify(neg)}`);
  await onPage(['sdxl_neg']);
  const negFields = await addLora('sdxl_neg', 'ink_wash');
  seen.negative_first = { order: reordered.at(-1), instructions: negFields.instructions, negative_prompt: negFields.negative_prompt };
  check('a_negative_prompt_written_first_never_takes_the_trigger', /^negative_prompt:words,/.test(String(reordered.at(-1))) && negFields.instructions === 'A quiet pier, ink_wash' && !negFields.negative_prompt, seen.negative_first);
  await openBox(page, 'sdxl_neg');
  // Its frame holds its main text, the same rule as the trigger's: `instructions`, not the negative prompt written first.
  const framed = await page.locator('.card.here[data-asset="sdxl_neg"] .in-frame [data-field]').evaluateAll((els) => els.map((el) => el.dataset.field));
  seen.negative_first_frame = framed;
  check('a_negative_prompt_written_first_is_not_the_frames_text', framed.includes('instructions') && !framed.includes('negative_prompt'), framed);
  for (const look of ['light', 'dark']) {
    await page.setViewportSize({ width: 1440, height: 900 });
    await theme(look === 'dark');
    await shot(`prompt-negative-first-1440x900-${look}`);
  }
  await theme(false);
  await page.setViewportSize({ width: 1600, height: 1100 });
  await closeBox(page);
  await page.unroute(/\/v1\/tools\?/);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
