// Checkpoints on boxes (Paul, 11-05), in a real browser on a real Calcifer: the explorer in the left panel lists the Hub's fine-tunes and LoRAs and fetches nothing; one
// carried shows what takes it; dropped where it can't go, a toast says why and nothing changes; dropped where it can, a chip stands under the box at once, follows it, leaves
// its size alone, keeps other boxes clear of it, and stays near one size at any zoom. An X takes it off, strength is behind More, a hand-written `lora:` or `models:` is a chip, and
// the export keeps its words and `|` (its comments go at the import: cl-803). It names no model: what it drops comes from `GET /v1/models` and `GET /v1/tools`.
//   node addons.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $RENDER: a machine with a GPU, on which a box with a LoRA and a fine-tune
// really renders, and what its run was asked and applied is read back. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { calcifer, carry: carryTo, signIn, boxOf, closeBox, STEP_MS, importYaml, lastLook, launch, lookAt, noStall, openCanvas, renderOf, settled, until, yamlOf } = require('./page');

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
  // A console line doesn't say what failed: this does.
  page.on('response', (answer) => { if (answer.status() >= 500) errors.push(`${answer.status()} ${answer.request().method()} ${new URL(answer.url()).pathname}`); });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-addons-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  // The project as `cozy project export` writes it (cl-803: its state is project.db).
  const yaml = () => yamlOf(folder);
  const box = (name) => boxOf(page, name);

  const tools = (await api('/v1/tools')).body.tools.filter((tool) => tool.model?.family);
  const models = (await api('/v1/models?limit=100')).body.models;
  const loras = (family) => models.filter((model) => model.kind === 'lora' && model.family === family);
  const fines = (family) => models.filter((model) => model.kind === 'weights' && model.family === family);
  // A tool with two LoRAs and two fine-tunes, and one of another family.
  const mine = tools.find((tool) => loras(tool.model.family).length >= 3 && fines(tool.model.family).length >= 2);
  const other = tools.find((tool) => tool.model.family !== mine?.model.family);
  seen.mine = mine && { id: mine.id, ...mine.model };
  seen.other = other && { id: other.id, ...other.model };
  // A test Hub may hold no LoRAs: that says nothing of the page, and the suite prints it.
  if (!mine || !other) {
    seen.skipped = `nothing to drop: ${tools.length} tools with a family, ${models.length} models`;
    await browser.close();
    console.log(JSON.stringify({ failures, seen }, null, 2));
    process.exit(0);
  }
  const [loraA, loraB, loraC] = loras(mine.model.family);
  const [fineA, fineB] = fines(mine.model.family);
  const word = (model) => model.model.split('/').pop();

  // A hand's file, imported: its words and `|` outlast all of this, and its comments go at the import (cl-803), as web-nodes reads it.
  const words = ['    instructions: |', '      A small boat,', '      on still water.'];
  importYaml(folder, `# Mine, and kept.
version: 4

assets:
  # What other makes.
  boat:
    tool: ${other.id}
    instructions: |
      A small boat,
      on still water.
  shot:
    tool: ${mine.id}
    instructions: A wave.
  hand:
    tool: ${mine.id}
    instructions: Written by hand.
    # Its look.
    lora:
      - {ref: ${loraC.model}, strength: 0.7}
    models:
      ${mine.model.slot}: {ref: ${fineB.model}}
  flowed:
    tool: ${mine.id}
    instructions: One line.
    lora: [{ref: hf://someone/wushu, strength: 0.8}]  # As a person writes it.
  words:
    text: Words alone.
`);
  await box('hand').waitFor();
  await box('shot').waitFor();
  const kept = () => words.filter((line) => !yaml().includes(line));
  const chips = (name) => box(name).locator('.add-on:not(.tool)');
  const toast = (words) => until(`the toast ${words}`, () => page.locator('.notes, .note-list, [role=status]').allInnerTexts().then((all) => all.join(' | ')), (all) => all.includes(words)).then(() => true, (why) => String(why));

  // A hand-written one is a chip.
  await chips('hand').first().waitFor();
  check('hand_written_chips', (await chips('hand').count()) === 2 && (await box('hand').locator('.add-on.fine-tune').count()) === 1, await chips('hand').allInnerTexts());
  await chips('flowed').first().waitFor();
  check('one_line_chip', (await chips('flowed').allInnerTexts()).join() === 'wushu', await chips('flowed').allInnerTexts());

  // The explorer: it reads, and fetches nothing.
  // Others use this computer: what counts is this project's runs, and what it dropped being held.
  const madeHere = async () => ((await api('/v1/runs?limit=100')).body?.runs ?? []).filter((run) => run.project_id === id).length;
  // Asked from here, not the page: where this computer runs no models (macOS, Windows), `local` answers 503, and that's no page's error.
  const fetched = async (names) => {
    const answer = await fetch(`${base}/v1/machines/local/models`, { headers: { Authorization: `Bearer ${held?.token}` } });
    return ((await answer.json().catch(() => null))?.models ?? []).filter((one) => names.includes(one.model));
  };
  const written = [];
  page.on('request', (request) => { if (request.method() !== 'GET' && request.method() !== 'HEAD' && new URL(request.url()).pathname.startsWith('/v1/') && !request.url().endsWith('/session')) written.push(`${request.method()} ${new URL(request.url()).pathname}`); });
  await page.click('.tools button:has-text("Checkpoints")');
  const row = (model) => page.locator(`[data-checkpoint="${model.model}"]`);
  await row(loraA).waitFor();
  // Grouped by the model each is for, called what its tool's package is (Paul, 11-06), each row its own name less its family's, and what it is under it.
  const groups = await page.locator('.checkpoints section').evaluateAll((all) => all.map((one) => ({ family: one.dataset.family, words: one.querySelector('h4').textContent, rows: [...one.querySelectorAll('[data-checkpoint]')].map((row) => row.dataset.checkpoint) })));
  const ofMine = groups.find((group) => group.family === mine.model.family);
  const mineIs = tools.filter((tool) => tool.model.family === mine.model.family).map((tool) => tool.package_title || tool.label).sort((a, b) => a.length - b.length)[0];
  const familyOf = (ref) => models.find((model) => model.model === ref)?.family;
  check('explorer_groups_by_family', ofMine?.words === mineIs && ofMine.rows.includes(loraA.model) && ofMine.rows.includes(fineA.model) && groups.every((group) => !group.family || group.rows.every((ref) => familyOf(ref) === group.family)), groups);
  const said = { name: await row(loraA).locator('b').innerText(), what: await row(loraA).locator('span').innerText(), fine: await row(fineA).locator('span').innerText() };
  const prefix = new RegExp(`^(${mine.model.family.split('-').map((_, i, all) => all.slice(i).join('[- _]')).join('|')})\\b`, 'i');
  check('rows_say_their_own_name_and_kind', said.what === 'LoRA' && said.fine === 'Fine-tune' && !prefix.test(said.name) && word(loraA).includes(said.name.split(' ').pop()), said);
  await page.fill('input[aria-label="Search the checkpoints"]', word(loraA));
  await until('the search', () => page.locator('[data-checkpoint]').count(), (n) => n >= 1 && n < models.length);
  await row(loraA).waitFor();
  await page.fill('input[aria-label="Search the checkpoints"]', '');
  await row(fineA).waitFor();
  check('what_fits_comes_first', !(await row(loraA).getAttribute('class')).includes('away'));
  // Where boxes sit is saved as they arrive, and says nothing of what a list did.
  const wrote = written.filter((line) => !line.endsWith('/canvas'));
  check('listing_writes_nothing', wrote.length === 0, wrote);
  await shot('explorer');

  const cardRect = (name) => box(name).locator('.card').boundingBox();
  const carry = async (model, name) => {
    await row(model).scrollIntoViewIfNeeded();
    const from = await row(model).boundingBox();
    await page.mouse.move(from.x + 20, from.y + 8);
    await page.mouse.down();
    await page.mouse.move(from.x + 40, from.y + 20, { steps: 4 });
    const at = await cardRect(name);
    await page.mouse.move(at.x + at.width / 2, at.y + at.height / 2, { steps: 12 });
  };

  // Carried: only what takes it shows.
  await carry(loraA, 'boat');
  const takes = await page.locator('.react-flow__node:has(.box-takes) .card').evaluateAll((cards) => cards.map((card) => card.dataset.asset));
  check('carried_shows_what_takes_it', takes.sort().join() === 'flowed,hand,shot', takes);
  await shot('carried');
  const before = yaml();
  await page.mouse.up();
  check('incompatible_says_why', (await toast(`is for ${loraA.family}`)) === true);
  await shot('refused');
  check('incompatible_changes_nothing', yaml() === before && (await chips('boat').count()) === 0);
  await carry(loraA, 'words');
  await page.mouse.up();
  check('no_model_says_why', (await toast('No model makes words')) === true);
  check('no_model_changes_nothing', yaml() === before);

  // The page first: the write is held back, and the chip is there.
  let release;
  const gate = new Promise((done) => { release = done; });
  await page.route(`**/v1/projects/${id}/assets/shot`, async (route) => {
    if (route.request().method() === 'PATCH') await gate;
    await route.fallback();
  });
  const size = await cardRect('shot');
  await carry(loraA, 'shot');
  await page.mouse.up();
  await chips('shot').first().waitFor({ timeout: 2000 });
  check('chip_before_the_write', !yaml().includes(loraA.model));
  release();
  await page.unroute(`**/v1/projects/${id}/assets/shot`);
  await until('the LoRA in the file', () => Promise.resolve(yaml()), (text) => text.includes(`ref: ${loraA.model}`));
  check('default_strength', /strength: 1(\.0)?\n/.test(yaml().split(`ref: ${loraA.model}`)[1]), yaml());
  const sized = await cardRect('shot');
  // To the pixel: the canvas may have moved under it, and a box's edge then lands a float's width off (191.18579 to 191.18582).
  check('chip_leaves_its_size', Math.abs(sized.width - size.width) < 0.5 && Math.abs(sized.height - size.height) < 0.5, { size, sized });
  check('no_more_than_it_said', (await page.locator('.box-takes').count()) === 0);

  await carry(loraB, 'shot');
  await page.mouse.up();
  await until('two chips', () => chips('shot').count(), (n) => n === 2);
  await until('both in the file', () => Promise.resolve(yaml()), (text) => text.includes(`ref: ${loraB.model}`));
  await carry(loraB, 'shot');
  await page.mouse.up();
  check('twice_is_once', (await toast('is already on shot')) === true && (await chips('shot').count()) === 2);

  // A fine-tune replaces, and there's one.
  await carry(fineA, 'shot');
  await page.mouse.up();
  await box('shot').locator('.add-on.fine-tune').waitFor();
  await until('it in the file', () => Promise.resolve(yaml()), (text) => text.includes(`ref: ${fineA.model}`));
  await carry(fineB, 'shot');
  await page.mouse.up();
  await until('the other in the file', () => Promise.resolve(yaml()), (text) => text.includes(`ref: ${fineB.model}`) && text.split('shot:')[1].split('hand:')[0].indexOf(fineA.model) < 0);
  check('fine_tune_replaces', (await box('shot').locator('.add-on.fine-tune').count()) === 1 && (await box('shot').locator('.add-on.fine-tune').innerText()).includes(word(fineB)));
  check('words_and_blocks_stay', kept().length === 0, kept());
  await shot('chips');

  // It's what the project reads.
  const fields = (await state()).assets.find((asset) => asset.name === 'shot').fields;
  check('project_reads_them', fields.lora?.length === 2 && fields.models?.[mine.model.slot]?.ref === fineB.model, fields);
  check('no_run_and_no_weights', (await madeHere()) === 0 && (await fetched([loraA.model, loraB.model, fineA.model, fineB.model])).length === 0);

  // The list is long: dragged up or down it scrolls, by a mouse or a finger, a slim marker showing where it is, and nothing is carried; dragged sideways a
  // row is carried out, by a finger too (Paul, 11-06).
  if (!(await page.locator('.checkpoints').count())) await page.click('.tools button:has-text("Checkpoints")');
  await row(loraB).waitFor();
  await page.keyboard.press('f');
  await settled(page);
  const shelf = page.locator('.browse.checkpoints');
  const scrolledTo = () => shelf.evaluate((el) => el.scrollTop);
  const room = await shelf.evaluate((el) => el.scrollHeight - el.clientHeight);
  const rest = await shelf.boundingBox();
  const middle = rest.x + rest.width / 2;
  const fileThen = yaml();
  await page.mouse.move(middle, rest.y + rest.height - 30);
  await page.mouse.down();
  for (let step = 1; step <= 8; step++) await page.mouse.move(middle + step, rest.y + rest.height - 30 - step * 20);
  const scrolling = { marked: await page.locator('.scroll-marker.on').count(), ghosts: await page.locator('.carry-ghost').count(), lit: await page.locator('.box-takes').count() };
  await page.mouse.up();
  const byMouse = await scrolledTo();
  check('the_list_drags_with_a_mouse', room > 100 && byMouse > 60 && scrolling.marked === 1 && scrolling.ghosts === 0 && scrolling.lit === 0 && yaml() === fileThen, { room, byMouse, scrolling });
  await until('the marker gone', () => page.locator('.scroll-marker.on').count(), (n) => n === 0, 4000);
  await shelf.evaluate((el) => { el.scrollTop = 0; });
  const fingers = await context.newCDPSession(page);
  await fingers.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 1 });
  const touch = (type, points) => fingers.send('Input.dispatchTouchEvent', { type, touchPoints: points });
  await touch('touchStart', [{ x: middle, y: rest.y + rest.height - 30 }]);
  for (let step = 1; step <= 6; step++) await touch('touchMove', [{ x: middle, y: rest.y + rest.height - 30 - step * 20 }]);
  await touch('touchEnd', []);
  const byFinger = await scrolledTo();
  check('the_list_drags_with_a_finger', byFinger > 60 && yaml() === fileThen, byFinger);
  await row(loraB).scrollIntoViewIfNeeded();
  const lifted = await row(loraB).boundingBox();
  const onto = await cardRect('hand');
  const [from, to] = [{ x: lifted.x + lifted.width / 2, y: lifted.y + lifted.height / 2 }, { x: onto.x + onto.width / 2, y: onto.y + onto.height / 2 }];
  await touch('touchStart', [from]);
  for (let step = 1; step <= 3; step++) await touch('touchMove', [{ x: from.x + step * 5, y: from.y }]);
  for (let step = 1; step <= 10; step++) await touch('touchMove', [{ x: from.x + 15 + ((to.x - from.x - 15) * step) / 10, y: from.y + ((to.y - from.y) * step) / 10 }]);
  await touch('touchEnd', []);
  await fingers.send('Emulation.setTouchEmulationEnabled', { enabled: false });
  await fingers.detach();
  const byHand = await until('the LoRA a finger carried', () => Promise.resolve(yaml().split('hand:')[1]?.split('flowed:')[0] ?? ''), (text) => text.includes(loraB.model)).then(() => true, (why) => String(why));
  check('a_finger_carries_one_sideways', byHand === true, yaml());
  // As it was, for what follows.
  await box('hand').locator(`[aria-label="Take ${word(loraB)} off hand"]`).click();
  await until('hand as it was', () => chips('hand').count(), (n) => n === 2);

  // A checkpoint runs alone: let go on open canvas it's a box of the tool that runs it, on it unless it's that tool's own, as a base model is. A LoRA
  // there makes nothing.
  const runner = (model) => tools.find((tool) => tool.model.family === model.family && tool.model.default?.split('@')[0] === model.model) ?? tools.find((tool) => tool.model.family === model.family);
  const ownOf = (model) => runner(model).model.default?.split('@')[0] === model.model;
  const own = fines(mine.model.family).find(ownOf);
  const swapped = fines(mine.model.family).find((model) => !ownOf(model));
  const dropped = async (model) => {
    const before = (await state()).assets.map((asset) => asset.name);
    await row(model).scrollIntoViewIfNeeded();
    await carryTo(page, row(model), await openCanvas(page));
    const made = await until(`a box of ${model.model}`, async () => (await state()).assets.find((asset) => !before.includes(asset.name)), Boolean, 5000).catch(() => null);
    if (made) {
      await closeBox(page);
      await api(`/v1/projects/${id}/assets/${encodeURIComponent(made.name)}`, { method: 'DELETE' });
      await until('it gone', () => box(made.name).count(), (n) => n === 0);
    }
    return made && { tool: made.tool, models: made.fields?.models ?? null };
  };
  if (swapped) {
    const ofFine = await dropped(swapped);
    check('a_checkpoint_on_open_canvas_is_a_box_of_it', ofFine?.tool === runner(swapped).id && ofFine.models?.[runner(swapped).model.slot]?.ref === swapped.model, ofFine);
  }
  if (own) {
    const ofBase = await dropped(own);
    check('a_base_model_on_open_canvas_is_its_tool', ofBase?.tool === runner(own).id && !ofBase.models, ofBase);
  }
  check('a_lora_on_open_canvas_makes_nothing', (await dropped(loraA)) === null);

  // Open, a box whose tool runs a model has the left bar to itself (Paul, 11-06): its name and tool over its model, the default first, and its LoRAs,
  // those on it first; a click swaps or adds, and nothing of the tool's own inner parts is there. Left, the tools are back, searched and scrolled as they were.
  await shelf.evaluate((el) => { el.scrollTop = 120; });
  const listThen = { q: await page.locator('.tools input[aria-label="Search the checkpoints"]').inputValue(), top: await scrolledTo() };
  await box('shot').locator('.media').first().click({ position: { x: 20, y: 20 } });
  await page.locator('.card.here[data-asset="shot"]').waitFor();
  const picks = page.locator('.tools .picks');
  await picks.locator('[data-picks="lora"] li').first().waitFor();
  const pick = (model) => picks.locator(`[data-pick="${model}"]`);
  const ownModel = mine.model.default.split('@')[0];
  const offered = await picks.locator('[data-pick]').evaluateAll((all) => all.map((one) => one.dataset.pick));
  const lit = await picks.locator('.pick.on').getAttribute('data-pick').catch(() => null);
  seen.picks = { offered: offered.length, lit };
  const titled = await page.locator('.tools .box-shelf-head').innerText();
  check('an_open_box_has_the_left_bar', titled.includes('shot') && titled.includes(mine.label) && !(await shelf.isVisible()) && offered[0] === ownModel && (!offered.includes(fineB.model) || lit === fineB.model), { offered, titled, lit });
  check('only_its_familys_picks', offered.every((ref) => ref === ownModel || familyOf(ref) === mine.model.family) && offered.includes(loraC.model) && !offered.includes(loraA.model), offered);
  const slotOf = () => yaml().split('shot:')[1]?.split('hand:')[0] ?? '';
  const weighs = (ref) => models.find((model) => model.model === ref)?.kind === 'weights';
  // Another fine-tune than the one on it and its default, if its family has one besides its Turbo.
  const swapTo = offered.find((ref) => weighs(ref) && ref !== ownModel && ref !== fineB.model);
  if (swapTo) {
    await pick(swapTo).click();
    check('a_click_swaps_its_model', await until('swapped', () => Promise.resolve(slotOf()), (text) => text.includes(swapTo) && !text.includes(fineB.model)).then(() => true, (why) => String(why)));
  } else seen.no_swap = 'its family has only its default and its Turbo';
  await pick(ownModel).click();
  check('its_default_puts_it_back', await until('its own again', () => Promise.resolve(slotOf()), (text) => !/models:/.test(text)).then(() => true, (why) => String(why)));
  await pick(loraC.model).click();
  check('a_click_adds_a_lora', await until('added', () => Promise.resolve(slotOf()), (text) => text.includes(loraC.model)).then(() => true, (why) => String(why)) === true && (await picks.locator('.picks-on .add-on-row').count()) === 3);
  // As it was, for what follows: that LoRA off by its × there, and its fine-tune back.
  await picks.locator(`[aria-label="Take ${word(loraC)} off shot"]`).click();
  await until('the LoRA off', () => Promise.resolve(slotOf()), (text) => !text.includes(loraC.model));
  await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields: { models: { [mine.model.slot]: { ref: fineB.model } } } }) });
  await until('its fine-tune back', () => Promise.resolve(slotOf()), (text) => text.includes(fineB.model));
  await page.keyboard.press('Escape');
  await page.locator('.card.here').waitFor({ state: 'detached' });
  await until('the tools back', () => picks.count(), (n) => n === 0, 3000).catch(() => {});
  const listNow = { q: await page.locator('.tools input[aria-label="Search the checkpoints"]').inputValue(), top: await scrolledTo() };
  check('left_the_tools_are_back', (await picks.count()) === 0 && (await shelf.isVisible()) && listNow.q === listThen.q && Math.abs(listNow.top - listThen.top) < 2, { listThen, listNow });
  await until('its chips as they were', () => chips('shot').count(), (n) => n === 3);
  await settled(page);

  // Chips follow.
  const list = (name) => box(name).locator('.add-ons');
  const moved = await list('shot').boundingBox();
  const head = await box('shot').locator('.nm').boundingBox();
  await page.mouse.move(head.x + 10, head.y + 8);
  await page.mouse.down();
  await page.mouse.move(head.x + 70, head.y + 140, { steps: 8 });
  await page.mouse.up();
  await page.waitForTimeout(300);
  const after = await list('shot').boundingBox();
  const atHead = await box('shot').locator('.nm').boundingBox();
  check('chips_follow', Math.abs((after.x - moved.x) - (atHead.x - head.x)) < 2 && Math.abs((after.y - moved.y) - (atHead.y - head.y)) < 2, { moved, after });

  // Nothing lies on them: another box let go on the chips goes clear of them.
  const name = await box('boat').locator('.nm').boundingBox();
  await page.mouse.move(name.x + 10, name.y + 8);
  await page.mouse.down();
  await page.mouse.move(after.x + 10, after.y + 10, { steps: 10 });
  await page.mouse.up();
  await page.waitForTimeout(400);
  const boat = await cardRect('boat');
  const chipsAt = await list('shot').boundingBox();
  const lies = boat.x < chipsAt.x + chipsAt.width && chipsAt.x < boat.x + boat.width && boat.y < chipsAt.y + chipsAt.height && chipsAt.y < boat.y + boat.height;
  check('nothing_lies_on_chips', !lies, { boat, chipsAt });

  // An X, at once: the page first again.
  let again;
  const held2 = new Promise((done) => { again = done; });
  await page.route(`**/v1/projects/${id}/assets/shot`, async (route) => {
    if (route.request().method() === 'PATCH') await held2;
    await route.fallback();
  });
  await list('shot').hover();
  await box('shot').locator(`[aria-label="Take ${word(loraB)} off shot"]`).click();
  // Its tool's chip is first in the stack, and no LoRA.
  await until('one LoRA left', () => box('shot').locator('.add-on:not(.fine-tune):not(.tool)').count(), (n) => n === 1, 2000);
  check('x_before_the_write', yaml().includes(loraB.model));
  again();
  await page.unroute(`**/v1/projects/${id}/assets/shot`);
  await until('it off the file', () => Promise.resolve(yaml()), (text) => !text.includes(loraB.model));

  // Strength is behind More, and not on the chip.
  await box('shot').locator('.media').first().click({ position: { x: 20, y: 20 } });
  await page.locator('.card.here[data-asset="shot"]').waitFor();
  if (!(await box('shot').locator('.edit-more').count())) await box('shot').locator('.more-toggle').click();
  const slider = box('shot').locator(`input[aria-label="Strength of ${word(loraA)}"]`);
  await slider.focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press('ArrowRight');
  await until('its strength in the file', () => Promise.resolve(yaml()), (text) => /strength: 1\.2/.test(text.split(`ref: ${loraA.model}`)[1] ?? ''));
  await closeBox(page);
  check('chip_says_no_strength', !/\d\.\d/.test((await chips('shot').first().innerText()).replace(word(fineB), '')) );

  // Hand-written: one off, the rest of it as it was.
  await list('hand').hover();
  await box('hand').locator(`[aria-label="Take ${word(loraC)} off hand"]`).click();
  await until('off the file', () => Promise.resolve(yaml()), (text) => !text.includes(loraC.model));
  check('hand_written_rest_stays', yaml().includes(`${mine.model.slot}: {ref: ${fineB.model}}`) || yaml().split('hand:')[1].includes(fineB.model), yaml());
  check('words_outlast_it', kept().length === 0, kept());

  await list('flowed').hover();
  await box('flowed').locator('[aria-label="Take wushu off flowed"]').click();
  await until('off the one line', () => Promise.resolve(yaml()), (text) => !text.includes('wushu'));
  check('one_line_goes', (await chips('flowed').count()) === 0 && yaml().includes('instructions: One line.'), yaml());

  // One size on screen down to a zoom of 0.5, past which they shrink with the canvas (e139a61c6), and never wider than their box.
  const pane = page.locator('.react-flow__pane');
  const sizes = [];
  for (const steps of [0, 1, 3]) {
    // By the page's own zoom keys, a step the same on every OS: a wheel's step is far larger on macOS, and three of them went from 0.9 to 0.05.
    for (let i = 0; i < steps; i++) { await pane.hover({ position: { x: 900, y: 500 } }); await page.keyboard.press('Control+-'); await page.waitForTimeout(200); }
    await page.waitForTimeout(250);
    const measure = await page.evaluate(() => {
      const chip = document.querySelector('.add-on');
      const flow = document.querySelector('.react-flow__viewport');
      if (!chip || !flow) return null;
      const zoom = new DOMMatrix(getComputedStyle(flow).transform).a;
      return { zoom, screen: parseFloat(getComputedStyle(chip).fontSize) * zoom, wide: chip.getBoundingClientRect().width / zoom };
    });
    if (measure) sizes.push(measure);
  }
  seen.sizes = sizes;
  const screens = sizes.filter((one) => one.zoom >= 0.5).map((one) => one.screen);
  const [atHang, far] = [sizes.find((one) => Math.abs(one.zoom - 0.5) < 0.01), sizes.find((one) => one.zoom < 0.45)];
  check('chip_near_one_size', screens.length > 1 && Math.max(...screens) / Math.min(...screens) < 1.2 && sizes.every((one) => one.wide <= 320), sizes);
  check('chip_shrinks_far_out', Boolean(atHang && far) && Math.abs(far.screen / atHang.screen - far.zoom / atHang.zoom) < 0.1, sizes);

  // Its tool changes, and they stay.
  const held3 = yaml();
  await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ tool: other.id }) });
  check('a_new_tool_keeps_them', yaml().includes(loraA.model) && yaml().includes(fineB.model), yaml() === held3);

  // After a reload they're as they were.
  await page.reload();
  await chips('shot').first().waitFor();
  check('reloaded', (await chips('shot').count()) === 2, await chips('shot').allInnerTexts());

  // Where there's a machine: a render passes both on its function's call, and the machine says what it applied. A shared machine goes first to what's already on it: this
  // waits for it to be idle, and gives way if anyone else asks for it. It cancels no run but its own.
  if (process.env.RENDER) {
    const family = process.env.RENDER_FAMILY;
    const still = (await api('/v1/tools')).body.tools.find((tool) => tool.makes === 'image' && tool.model?.family && (!family || tool.model.family === family) && loras(tool.model.family).length && fines(tool.model.family).length);
    const [lora] = loras(still.model.family);
    const fine = fines(still.model.family).find((model) => !still.model.default?.startsWith(`${model.model}@`));
    seen.render = { tool: still.id, local: Boolean(still.local), lora: lora.model, fine: fine.model };
    importYaml(folder, `${yaml().trimEnd()}\n  still:\n    tool: ${still.id}\n    instructions: A red boat on still water, ink on paper.\n`);
    await box('still').waitFor();
    await page.keyboard.press('f');
    await page.waitForTimeout(500);
    if (!(await page.locator('.checkpoints').count())) await page.click('.tools button:has-text("Checkpoints")');
    await row(lora).waitFor();
    await carry(lora, 'still');
    await page.mouse.up();
    await carry(fine, 'still');
    await page.mouse.up();
    await until('both on it', () => Promise.resolve(yaml().split('still:')[1]), (text) => text.includes(lora.model) && text.includes(fine.model));
    await shot('both-on');

    const runs = async () => (await Promise.all(['in_progress', 'queued'].map((status) => api(`/v1/runs?status=${status}&limit=100`)))).flatMap((answer) => answer.body?.runs ?? []).filter((run) => run.machine === process.env.RENDER || !run.machine);
    const others = async () => (await runs()).filter((run) => run.kind === 'call' && run.project_id !== id);
    const pause = (ms) => new Promise((done) => setTimeout(done, ms));
    const idle = async () => {
      for (let held = await others(); held.length; held = await others()) {
        seen.waited_for = [...new Set([...(seen.waited_for ?? []), ...held.map((run) => run.number)])];
        seen.waiting = `${seen.waiting ?? ''}.`;
        await pause(20_000);
      }
    };
    let made;
    for (let tries = 0; !made && tries < 5; tries++) {
      await idle();
      const had = (await state()).assets.find((asset) => asset.name === 'still').versions;
      await page.evaluate((to) => { location.hash = to; }, `/projects/${id}/assets/still`);
      await page.locator('.card.here[data-asset="still"]').waitFor();
      await (await renderOf(page, 'still')).click();
      await closeBox(page);
      seen.asked_at = new Date().toISOString();
      for (;;) {
        await pause(5_000);
        const asset = (await state()).assets.find((held) => held.name === 'still');
        seen.rendering = `${seen.rendering ?? ''}.`;
        if (asset.versions > had) { made = asset; break; }
        if (asset.failure) { made = asset; break; }
        if ((await others()).length) {
          const mine = (await runs()).filter((run) => run.project_id === id);
          for (const run of mine) await api(`/v1/runs/${run.id}/cancel`, { method: 'POST', body: '{}' });
          seen.yielded = [...(seen.yielded ?? []), ...mine.map((run) => run.number)];
          break;
        }
      }
    }
    check('render_landed', made?.versions > 0, made?.failure ?? 'it never had a turn');
    const number = made?.prefixes?.at(-1)?.run;
    const run = (await api(`/v1/runs/${number}?detail=full`)).body;
    const asked = run?.request?.input?.models;
    const applied = run?.measurements?.models;
    seen.render_run = { number, target: run?.target, machine: run?.machine, asked, applied, warnings: run?.warnings };
    const slot = asked && Object.entries(asked).find(([, held]) => held.ref === fine.model || held.lora?.length);
    check('call_was_passed_both', Object.values(asked ?? {}).some((held) => held.ref === fine.model) && Object.values(asked ?? {}).some((held) => held.lora?.some((one) => one.ref === lora.model)), asked ?? slot);
    const word = (name) => name.split('/').pop();
    check('machine_applied_both', (applied ?? []).some((model) => model.repository.includes(word(fine.model))) && (applied ?? []).some((model) => model.adapters?.some((one) => one.model.includes(word(lora.model)))), applied);
    await page.waitForTimeout(1000);
    await shot('rendered');
  }

  check('no_node_on_screen', !/\bnode\b/i.test(await page.evaluate(() => document.body.innerText)));
  check('no_page_errors', errors.length === 0, errors);
  check('no_csp_violations', (await page.evaluate(() => window.violations)).length === 0);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
