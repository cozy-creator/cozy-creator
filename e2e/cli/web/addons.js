// Existing model and adapter parameters on real nodes: project API assignment, chips, their geometry, removal and strength controls behind More.
// The sidebar's checkpoint explorer and contextual picker are retired; this file records those removed UI flows explicitly instead of recreating them.
// Current authoring import/export preserves words and `|`; model names come from `GET /v1/models` and `GET /v1/tools`.
//   node addons.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $RENDER: a machine with a GPU, on which a box with a LoRA and a fine-tune
// really renders, and what its run was asked and applied is read back. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { projectFromPage, projectUrl, calcifer, signIn, boxOf, closeBox, STEP_MS, importYaml, lastLook, launch, lookAt, noStall, renderOf, settled, until, yamlOf } = require('./page');

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

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-addons-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  // The project as `cozy export` writes it (cl-803: its state is project.db).
  const yaml = () => yamlOf(folder);
  const box = (name) => boxOf(page, name);

  const tools = (await api('/v1/tools')).body.tools.filter((tool) => tool.model?.family);
  const models = (await api('/v1/models?limit=100')).body.models;
  const loras = (family) => models.filter((model) => model.kind === 'lora' && model.family === family);
  const fines = (family) => models.filter((model) => model.kind === 'weights' && model.family === family);
  // A tool with three LoRAs and two fine-tunes; a second tool must keep the same media type.
  const mine = tools.find((tool) => loras(tool.model.family).length >= 3 && fines(tool.model.family).length >= 2);
  const other = tools.find((tool) => tool.id !== mine?.id && tool.makes === mine?.makes);
  seen.mine = mine && { id: mine.id, ...mine.model };
  seen.other = other && { id: other.id, ...other.model };
  // A test Hub may hold no LoRAs: that says nothing of the page, and the suite prints it.
  if (!mine) {
    seen.skipped = `no model-parameter fixture: ${tools.length} tools with a family, ${models.length} models`;
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
    tool: ${(other ?? mine).id}
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

  // A hand-written one is a chip.
  await chips('hand').first().waitFor();
  check('hand_written_chips', (await chips('hand').count()) === 2 && (await box('hand').locator('.add-on.fine-tune').count()) === 1, await chips('hand').allInnerTexts());
  await chips('flowed').first().waitFor();
  check('one_line_chip', (await chips('flowed').allInnerTexts()).join() === 'wushu', await chips('flowed').allInnerTexts());

  // Sidebar checkpoint discovery and its drag/drop surface were intentionally removed. Assign existing model/adapter parameters through
  // the supported project API, then exercise their real node controls. This does not stand in for a removed picker.
  seen.retired_sidebar_ui = [
    'checkpoint explorer grouping, names, search and compatible-target highlights',
    'checkpoint/LoRA dragging, duplicate-drop notices and pre-write optimistic chips',
    'mouse/touch scrolling and dragging from the checkpoint list',
    'checkpoint and LoRA drops onto empty canvas',
    'contextual left-bar model/LoRA picks, click-to-add and restoration on close',
  ];
  check('sidebar_has_only_nodes_and_tools', JSON.stringify(await page.locator('.tools h4').allTextContents()) === JSON.stringify(['Nodes', 'Tools']) && await page.locator('.tools input, .tools .browse, .tools .box-shelf-head').count() === 0);
  const madeHere = async () => ((await api('/v1/runs?limit=100')).body?.runs ?? []).filter((run) => run.project_id === id).length;
  const fetched = async (names) => {
    const answer = await fetch(`${base}/v1/machines/local/models`, { headers: { Authorization: `Bearer ${held?.token}` } });
    return ((await answer.json().catch(() => null))?.models ?? []).filter((one) => names.includes(one.model));
  };
  const cardRect = (name) => box(name).locator('.card').boundingBox();
  const size = await cardRect('shot');
  const setModel = (fields) => api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields }) });
  const attached = await setModel({ lora: [{ ref: loraA.model, strength: 1 }, { ref: loraB.model, strength: 1 }], models: { [mine.model.slot]: { ref: fineA.model } } });
  check('model_and_adapter_parameters_are_accepted', attached.status === 200, attached);
  await until('assigned model and adapters in the node', () => chips('shot').count(), (n) => n === 3);
  const replaced = await setModel({ models: { [mine.model.slot]: { ref: fineB.model } } });
  await until('replacement model in the node', () => box('shot').locator('.add-on.fine-tune').innerText(), (text) => text.includes(word(fineB)));
  check('model_parameter_replaces_one_fine_tune', replaced.status === 200 && await box('shot').locator('.add-on.fine-tune').count() === 1);
  check('words_and_blocks_stay', kept().length === 0, kept());
  const fields = (await state()).assets.find((asset) => asset.name === 'shot').fields;
  check('project_reads_model_and_adapter_parameters', fields.lora?.length === 2 && fields.lora.every((one) => one.strength === 1) && fields.models?.[mine.model.slot]?.ref === fineB.model, fields);
  await settled(page);
  const sized = await cardRect('shot');
  check('chip_leaves_its_size', Math.abs(sized.width - size.width) < 0.5 && Math.abs(sized.height - size.height) < 0.5, { size, sized });
  check('no_run_and_no_weights', (await madeHere()) === 0 && (await fetched([loraA.model, loraB.model, fineA.model, fineB.model])).length === 0);
  await shot('node-model-parameters');

  // Chips follow.
  const list = (name) => box(name).locator('.add-ons');
  // The list takes no pointer, only its chips do (ce29dad9b): a hover is on the chip whose × it presses.
  const chipOf = (name, off) => box(name).locator('.add-on', { has: page.locator(`[aria-label="${off}"]`) });
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
  await chipOf('shot', `Take ${word(loraB)} off shot`).hover();
  await box('shot').locator(`[aria-label="Take ${word(loraB)} off shot"]`).click();
  // Its tool's chip is first in the stack, and no LoRA.
  await until('one LoRA left', () => box('shot').locator('.add-on:not(.fine-tune):not(.tool)').count(), (n) => n === 1, 2000);
  check('x_before_the_write', yaml().includes(loraB.model));
  again();
  await page.unroute(`**/v1/projects/${id}/assets/shot`);
  await until('it off the file', () => Promise.resolve(yaml()), (text) => !text.includes(loraB.model));

  // Strength is behind More, and not on the chip.
  await box('shot').locator('.media').first().dblclick({ position: { x: 20, y: 20 } });
  await page.locator('.card.here[data-asset="shot"]').waitFor();
  if (!(await box('shot').locator('.edit-more').count())) await box('shot').locator('.more-toggle').click();
  const slider = box('shot').locator(`input[aria-label="Strength of ${word(loraA)}"]`);
  await slider.focus();
  for (let i = 0; i < 4; i++) await page.keyboard.press('ArrowRight');
  await until('its strength in the file', () => Promise.resolve(yaml()), (text) => /strength: 1\.2/.test(text.split(`ref: ${loraA.model}`)[1] ?? ''));
  await closeBox(page);
  check('chip_says_no_strength', !/\d\.\d/.test((await chips('shot').first().innerText()).replace(word(fineB), '')) );

  // Hand-written: one off, the rest of it as it was.
  await chipOf('hand', `Take ${word(loraC)} off hand`).hover();
  await box('hand').locator(`[aria-label="Take ${word(loraC)} off hand"]`).click();
  await until('off the file', () => Promise.resolve(yaml()), (text) => !text.includes(loraC.model));
  check('hand_written_rest_stays', yaml().includes(`${mine.model.slot}: {ref: ${fineB.model}}`) || yaml().split('hand:')[1].includes(fineB.model), yaml());
  check('words_outlast_it', kept().length === 0, kept());

  await chipOf('flowed', 'Take wushu off flowed').hover();
  await box('flowed').locator('[aria-label="Take wushu off flowed"]').click();
  await until('off the one line', () => Promise.resolve(yaml()), (text) => !text.includes('wushu'));
  check('one_line_goes', (await chips('flowed').count()) === 0 && yaml().includes('instructions: One line.'), yaml());

  // Drawn at the canvas's scale, as all of a box is (cl-890): the same size in canvas pixels at every zoom, and never wider than their box.
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
      return { zoom, font: parseFloat(getComputedStyle(chip).fontSize), wide: chip.getBoundingClientRect().width / zoom };
    });
    if (measure) sizes.push(measure);
  }
  seen.sizes = sizes;
  // Past 20% a chip is only a mark (AddOns.css).
  const worded = sizes.filter((one) => one.zoom >= 0.2);
  check('chip_scales_with_the_canvas', new Set(worded.map((one) => one.zoom)).size > 1 && worded.every((one) => one.font === worded[0].font && Math.abs(one.wide - worded[0].wide) < 0.5 && one.wide <= 320), sizes);

  // A compatible tool changes without changing the node's media type or its explicit adapter parameters.
  if (other) {
    const changed = await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ tool: other.id }) });
    const now = (await state()).assets.find((asset) => asset.name === 'shot');
    check('a_compatible_tool_keeps_parameters', changed.status === 200 && now.kind === mine.makes && now.fields?.lora?.some((one) => one.ref === loraA.model) && now.fields?.models?.[mine.model.slot]?.ref === fineB.model, now);
  } else seen.unavailable_tool_swap = 'no second tool of the same media type';

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
    const assigned = await api(`/v1/projects/${id}/assets/still`, { method: 'PATCH', body: JSON.stringify({ fields: { lora: [{ ref: lora.model, strength: 1 }], models: { [still.model.slot]: { ref: fine.model } } } }) });
    check('render_model_parameters_accepted', assigned.status === 200, assigned);
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
      await page.evaluate((to) => { history.pushState(null, '', to); dispatchEvent(new PopStateEvent('popstate')); }, await projectUrl(id, '/assets/still'));
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
