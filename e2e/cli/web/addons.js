// Existing model and adapter parameters on real nodes: project API assignment; at rest no chips (cl-932), open their list; removal and strength in
// Models & LoRAs, the one picker (cl-934).
// The sidebar's checkpoint explorer and contextual picker are retired; this file records those removed UI flows explicitly instead of recreating them.
// A box's words outlast its model edits; model names come from `GET /v1/models` and `GET /v1/tools`.
//   node addons.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $RENDER: a machine with a GPU, on which a box with a LoRA and a fine-tune
// really renders, and what its run was asked and applied is read back. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { addBoxes, assetsOf, projectFromPage, projectUrl, calcifer, signIn, boxOf, closeBox, STEP_MS, lastLook, launch, lookAt, noStall, openBox, renderOf, settled, storedOf, until } = require('./page');

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
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
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
  // Every box's fields as project.db stores them (cl-803), as one text.
  const stored = () => JSON.stringify(storedOf(folder));
  const fieldsOf = (name) => assetsOf(folder).find((asset) => asset.name === name)?.fields ?? {};
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

  // Boxes another writer made, with their models: the boat's words outlast all of this.
  const words = 'A small boat,\non still water.';
  addBoxes(folder, [
    { name: 'boat', tool: (other ?? mine).id, fields: { instructions: words } },
    { name: 'shot', tool: mine.id, fields: { instructions: 'A wave.' } },
    { name: 'hand', tool: mine.id, fields: { instructions: 'Written by hand.', lora: [{ ref: loraC.model, strength: 0.7 }], models: { [mine.model.slot]: { ref: fineB.model } } } },
    { name: 'flowed', tool: mine.id, fields: { instructions: 'One line.', lora: [{ ref: 'hf://someone/wushu', strength: 0.8 }] } },
    { name: 'words', fields: { text: 'Words alone.' } },
  ]);
  await box('hand').waitFor();
  await box('shot').waitFor();
  const kept = () => (fieldsOf('boat').instructions === words ? [] : [fieldsOf('boat').instructions]);
  // At rest a box has no chips (cl-932, Paul 11-12): its tool and shape under it, nothing else. Open, its models and LoRAs are listed
  // (ModelConfig, each by its ref), and taken off or made stronger in Models & LoRAs (cl-934).
  const here = (name) => page.locator(`.card.here[data-asset="${name}"]`);
  const refs = (name) => here(name).locator('.active-models .active-model-name').evaluateAll((els) => els.map((el) => el.title));
  const restChips = () => page.locator('.react-flow__node .add-on, .react-flow__node .add-ons').count();
  const opened = async (name, run) => {
    await openBox(page, name);
    try { return await run(); } finally { await closeBox(page); }
  };
  check('no_chips_at_rest', (await restChips()) === 0);

  // A hand-written one is listed, open.
  await opened('hand', async () => {
    const listed = await until('hand listed', () => refs('hand'), (now) => now.length === 2).catch(() => refs('hand'));
    check('hand_written_listed', listed.length === 2 && listed.includes(loraC.model), listed);
  });
  await opened('flowed', async () => {
    const listed = await until('flowed listed', () => refs('flowed'), (now) => now.length === 1).catch(() => refs('flowed'));
    check('one_line_listed', listed.join() === 'hf://someone/wushu', listed);
  });

  // Sidebar checkpoint discovery and its drag/drop surface were intentionally removed. Assign existing model/adapter parameters through
  // the supported project API, then exercise their real node controls. This does not stand in for a removed picker.
  seen.retired_sidebar_ui = [
    'checkpoint explorer grouping, names, search and compatible-target highlights',
    'checkpoint/LoRA dragging, duplicate-drop notices and pre-write optimistic chips',
    'mouse/touch scrolling and dragging from the checkpoint list',
    'checkpoint and LoRA drops onto empty canvas',
    'contextual left-bar model/LoRA picks, click-to-add and restoration on close',
    'chips under a box at rest (cl-932): open, its models and LoRAs are listed',
  ];
  check('sidebar_has_only_nodes_and_tools', JSON.stringify(await page.locator('.tools h4').allTextContents()) === JSON.stringify(['Nodes', 'Tools']) && await page.locator('.tools input, .tools .browse, .tools .box-shelf-head').count() === 0);
  const madeHere = async () => ((await api(`/v1/projects/${id}/runs?limit=100`)).body?.runs ?? []).filter((run) => run.project_id === id).length;
  const fetched = async (names) => {
    const answer = await fetch(`${base}/v1/machines/local/models`, { headers: { Authorization: `Bearer ${held?.token}`, 'cozy-project-storage': '3' } });
    return ((await answer.json().catch(() => null))?.models ?? []).filter((one) => names.includes(one.model));
  };
  const cardRect = (name) => box(name).locator('.card').boundingBox();
  const size = await cardRect('shot');
  const setModel = (fields) => api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ fields }) });
  const attached = await setModel({ lora: [{ ref: loraA.model, strength: 1 }, { ref: loraB.model, strength: 1 }], models: { [mine.model.slot]: { ref: fineA.model } } });
  check('model_and_adapter_parameters_are_accepted', attached.status === 200, attached);
  const replaced = await setModel({ models: { [mine.model.slot]: { ref: fineB.model } } });
  await opened('shot', async () => {
    const listed = await until('assigned model and adapters, open', () => refs('shot'), (now) => now.length === 3 && now.includes(fineB.model)).catch(() => refs('shot'));
    check('model_parameter_replaces_one_fine_tune', replaced.status === 200 && listed.length === 3 && listed.includes(fineB.model) && !listed.includes(fineA.model), listed);
  });
  check('words_and_blocks_stay', kept().length === 0, kept());
  const fields = (await state()).assets.find((asset) => asset.name === 'shot').fields;
  check('project_reads_model_and_adapter_parameters', fields.lora?.length === 2 && fields.lora.every((one) => one.strength === 1) && fields.models?.[mine.model.slot]?.ref === fineB.model, fields);
  await settled(page);
  const sized = await cardRect('shot');
  check('parameters_leave_its_size', Math.abs(sized.width - size.width) < 0.5 && Math.abs(sized.height - size.height) < 0.5, { size, sized });
  check('still_no_chips_at_rest', (await restChips()) === 0);
  check('no_run_and_no_weights', (await madeHere()) === 0 && (await fetched([loraA.model, loraB.model, fineA.model, fineB.model])).length === 0);
  await shot('node-model-parameters');

  // Models & LoRAs open on a box's LoRAs: its slot's, or those on its model (Existing LoRAs).
  const picker = page.getByRole('dialog', { name: 'Models and LoRAs', exact: true });
  const openLoras = async (name) => {
    await here(name).locator('button[aria-label="Models and LoRAs"]').first().click();
    await picker.waitFor();
    const existing = picker.getByRole('button', { name: 'Existing LoRAs' });
    if (await existing.count()) await existing.click();
    else {
      await picker.locator('[data-model-slot]').first().click();
      await picker.getByRole('button', { name: 'LoRAs', exact: true }).click();
    }
  };
  const adapter = (model) => picker.locator('.model-picker-adapter', { hasText: new RegExp(word(model).split(/[-_]+/).join('.'), 'i') });
  const shut = () => picker.getByRole('button', { name: 'Close model picker', exact: true }).click();

  // In Models & LoRAs, an X, at once: the page first again.
  let again;
  const held2 = new Promise((done) => { again = done; });
  await page.route(`**/v1/projects/${id}/assets/shot`, async (route) => {
    if (route.request().method() === 'PATCH') await held2;
    await route.fallback();
  });
  await openBox(page, 'shot');
  await openLoras('shot');
  await adapter(loraB).locator('button[title="Remove LoRA"]').click();
  await until('one LoRA left', () => refs('shot'), (now) => !now.includes(loraB.model), 2000);
  check('x_before_the_write', stored().includes(loraB.model));
  again();
  await page.unroute(`**/v1/projects/${id}/assets/shot`);
  await until('it off the file', () => Promise.resolve(stored()), (text) => !text.includes(loraB.model));

  // Its strength, there too.
  const strength = adapter(loraA).locator('input[aria-label^="Strength of"]');
  await strength.fill('1.2');
  await strength.press('Enter');
  const loraOf = (fields, ref) => [...(fields.lora ?? []), ...Object.values(fields.models ?? {}).flatMap((model) => model?.lora ?? [])].find((one) => one.ref === ref);
  await until('its strength in the file', () => Promise.resolve(loraOf(fieldsOf('shot'), loraA.model)?.strength), (strength) => String(strength).startsWith('1.2'));
  await shut();
  await closeBox(page);

  // Hand-written: one off, the rest of it as it was.
  await opened('hand', async () => {
    await openLoras('hand');
    await adapter(loraC).locator('button[title="Remove LoRA"]').click();
    await until('off the file', () => Promise.resolve(stored()), (text) => !text.includes(loraC.model));
    await shut();
  });
  check('hand_written_rest_stays', JSON.stringify(storedOf(folder).hand ?? {}).includes(fineB.model), storedOf(folder).hand);
  check('words_outlast_it', kept().length === 0, kept());

  await opened('flowed', async () => {
    await openLoras('flowed');
    await adapter({ model: 'hf://someone/wushu' }).locator('button[title="Remove LoRA"]').click();
    await until('off the one line', () => Promise.resolve(stored()), (text) => !text.includes('wushu'));
    await shut();
  });
  check('one_line_goes', storedOf(folder).flowed?.instructions === 'One line.', storedOf(folder).flowed);

  // No chips at any zoom (cl-932).
  const pane = page.locator('.react-flow__pane');
  const counts = [];
  for (const steps of [0, 1, 3]) {
    // By the page's own zoom keys, a step the same on every OS: a wheel's step is far larger on macOS, and three of them went from 0.9 to 0.05.
    for (let i = 0; i < steps; i++) { await pane.hover({ position: { x: 900, y: 500 } }); await page.keyboard.press('Control+-'); await page.waitForTimeout(200); }
    await page.waitForTimeout(250);
    counts.push(await restChips());
  }
  check('no_chips_at_any_zoom', counts.every((n) => n === 0), counts);

  // A compatible tool changes without changing the node's media type or its explicit adapter parameters.
  if (other) {
    const changed = await api(`/v1/projects/${id}/assets/shot`, { method: 'PATCH', body: JSON.stringify({ tool: other.id }) });
    const now = (await state()).assets.find((asset) => asset.name === 'shot');
    check('a_compatible_tool_keeps_parameters', changed.status === 200 && now.kind === mine.makes && now.fields?.lora?.some((one) => one.ref === loraA.model) && now.fields?.models?.[mine.model.slot]?.ref === fineB.model, now);
  } else seen.unavailable_tool_swap = 'no second tool of the same media type';

  // After a reload they're as they were.
  await page.reload();
  await box('shot').waitFor();
  await opened('shot', async () => {
    const listed = await until('shot listed after a reload', () => refs('shot'), (now) => now.length === 2).catch(() => refs('shot'));
    check('reloaded', listed.length === 2 && listed.includes(loraA.model) && listed.includes(fineB.model), listed);
  });

  // Where there's a machine: a render passes both on its function's call, and the machine says what it applied. A shared machine goes first to what's already on it: this
  // waits for it to be idle, and gives way if anyone else asks for it. It cancels no run but its own.
  if (process.env.RENDER) {
    const family = process.env.RENDER_FAMILY;
    const still = (await api('/v1/tools')).body.tools.find((tool) => tool.makes === 'image' && tool.model?.family && (!family || tool.model.family === family) && loras(tool.model.family).length && fines(tool.model.family).length);
    const [lora] = loras(still.model.family);
    const fine = fines(still.model.family).find((model) => !still.model.default?.startsWith(`${model.model}@`));
    seen.render = { tool: still.id, local: Boolean(still.local), lora: lora.model, fine: fine.model };
    addBoxes(folder, [{ name: 'still', tool: still.id, fields: { instructions: 'A red boat on still water, ink on paper.' } }]);
    await box('still').waitFor();
    await page.keyboard.press('f');
    await page.waitForTimeout(500);
    const assigned = await api(`/v1/projects/${id}/assets/still`, { method: 'PATCH', body: JSON.stringify({ fields: { lora: [{ ref: lora.model, strength: 1 }], models: { [still.model.slot]: { ref: fine.model } } } }) });
    check('render_model_parameters_accepted', assigned.status === 200, assigned);
    await until('both on it', () => Promise.resolve(JSON.stringify(storedOf(folder).still ?? {})), (text) => text.includes(lora.model) && text.includes(fine.model));
    await shot('both-on');

    const runs = async (scope = '/v1') => (await Promise.all(['in_progress', 'queued'].map((status) => api(`${scope}/runs?status=${status}&limit=100`)))).flatMap((answer) => answer.body?.runs ?? []).filter((run) => run.machine === process.env.RENDER || !run.machine);
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
          const mine = (await runs(`/v1/projects/${id}`)).filter((run) => run.project_id === id);
          for (const run of mine) await api(`/v1/projects/${id}/runs/${run.id}/cancel`, { method: 'POST', body: '{}' });
          seen.yielded = [...(seen.yielded ?? []), ...mine.map((run) => run.number)];
          break;
        }
      }
    }
    check('render_landed', made?.versions > 0, made?.failure ?? 'it never had a turn');
    const number = made?.prefixes?.at(-1)?.run;
    const run = (await api(`/v1/projects/${id}/runs/${number}?detail=full`)).body;
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
