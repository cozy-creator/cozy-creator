// Assets as a person works them (web-ui.md, Tools and nodes), in a real browser on a real Calcifer. The shell: a name, Render all, a menu; tools
// on the left, each dragged out to make a box. A box is its picture or its text at rest. Text with chips, the file still plain; a name dragged in; a fresh box as its prompt, or its prompt
// and one beat, with all else behind More; controls from what it holds; reshaping; beats; conflicts; deleting and Undo. Where this computer runs models, Paul's example, real: drop a picture, drop a tool, drag the
// picture in, type, render, approve, delete the picture: what's approved stands, rendering it again fails naming what's gone, the rest goes on,
// Undo brings it back. Takes side by side, one dragged out, a new take from here, Render all.
//   node nodes.js <sign-in link from `cozy open --no-open`>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { existsSync, readFileSync, readdirSync, renameSync, writeFileSync } = require('fs');
const { join } = require('path');
const { MP4, boxLog, boxOf, closeBox, openBox, openCanvas, STEP_MS, drop, lastLook, launch, lookAt, png, noStall, until, watchBoxes } = require('./page');

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
const shots = join(__dirname, '..', 'project', 'shots.py');
const drawer = join(__dirname, '..', 'project', 'draw.py');

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
  const link = process.argv[2];
  const base = new URL(link).origin;
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  await watchBoxes(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  const refused = [];
  seen.refused = refused;
  // WebKit hands the page's error event its spec's own note that a ResizeObserver put a delivery off a frame, which nothing has thrown.
  page.on('pageerror', (error) => { if (!(process.env.ENGINE === 'webkit' && /^ResizeObserver loop completed with undelivered notifications/.test(error.message))) errors.push(`pageerror: ${error.message}`); });
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  page.on('response', (answer) => {
    const url = new URL(answer.url());
    // HEAD on a file asks whether it's held, and no is an answer: what then doesn't get sent shows on a PUT or at the import.
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

  await page.goto(link);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;

  // Answers read as the project opens are kept out until the file below names local code: what they list isn't the last word on it (Firefox, 42002036309).
  let letGo;
  const lateList = new Promise((done) => { letGo = done; });
  const listRead = /\/v1\/tools\?.*project=/;
  await page.route(listRead, async (route) => {
    const answer = await route.fetch();
    await lateList;
    await route.fulfill({ response: answer }).catch(() => {});
  });

  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-assets-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  const yamlAt = join(folder, 'project.yaml');
  const yaml = () => readFileSync(yamlAt, 'utf8');
  const of = async (name) => (await state()).assets.find((held) => held.name === name);
  const rows = async () => (await api(`/v1/projects/${id}/canvas`)).body.cards;
  const box = (name) => boxOf(page, name);
  // Shut, and off every box.
  const away = async () => {
    await closeBox(page);
    await page.mouse.move(2, 500);
  };
  const open = (name) => openBox(page, name);
  // The X is a shut box's, under the pointer.
  const deleteBox = async (name) => {
    await away();
    await box(name).locator('.nm').hover();
    await box(name).locator('.head [aria-label=Delete]').click();
  };
  // Behind its More, opened if it isn't.
  const more = async (name) => {
    await open(name);
    if (!(await box(name).locator('.edit-more').count())) await box(name).locator('.more-toggle').click();
    await box(name).locator('.edit-more').waitFor();
  };
  await page.evaluate(() => { window.notReloaded = true; });

  // A file a person wrote: every comment and `|` of it outlasts all of this.
  const head = '# Mine, and kept.\nversion: 3\n\nassets:\n  # A note on what follows.\n  kept_words:\n    # Inside it.\n    description: |\n      Written\n      by hand.\n';
  // Local code is a tool once something in the project names it. A real render needs no GPU this way.
  writeFileSync(yamlAt, local ? `${head}  named_here:\n    tool: ${drawer}/draw\n` : head);
  await box('kept_words').waitFor();
  letGo();
  await page.unroute(listRead);

  // The shell: a name, Render all, and a menu holding the rest. No dock, no edges, no path, no zoom, no machine picker.
  check('no_dock', (await page.locator('.dock').count()) === 0);
  check('no_edges', (await page.locator('.react-flow__edge, .ge').count()) === 0);
  check('top_is_minimal', (await page.locator('.top .path, .top select, .top .sum:not(.ood)').count()) === 0, await page.textContent('.top'));
  await page.click('.main-menu summary');
  const menu = await page.locator('.main-menu nav a').allTextContents();
  check('menu_holds_the_rest', ['Runs', 'Machines', 'Library', 'Account'].every((word) => menu.some((held) => held.startsWith(word))), menu);
  await page.mouse.click(600, 900);
  check('menu_shuts', !(await page.locator('.main-menu nav').isVisible()));

  // Tools on the left: searchable, folding away with the canvas where it was.
  const listed = await until('tools or why none', async () => ({ tools: await page.locator('.tools .tool').count(), note: await page.locator('.tools p').allTextContents() }), (now) => now.tools > 0 || now.note.length > 0);
  seen.tools_listed = listed;
  const resting = (await api(`/v1/tools?project=${id}`)).body;
  check('defaults_always_listed', ['cozy', 'qwen-image-2', 'minimax-h3'].every((name) => resting.tools.some((tool) => tool.package === name)), { note: resting.note, tools: resting.tools.map((tool) => tool.id) });
  const viewport = () => page.evaluate(() => getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  const before = await viewport();
  await page.click('.tools [aria-label="Hide the tools"]');
  await page.waitForSelector('.tools.shut');
  check('tools_fold', (await page.locator('.stage').evaluate((stage) => stage.getBoundingClientRect().left)) < 60);
  check('folding_keeps_the_canvas', (await viewport()) === before && (await box('kept_words').count()) === 1);
  await page.click('.tools [aria-label="Show the tools"]');
  await page.fill('.tools input', 'no-tool-is-called-this-at-all');
  await until('nothing found', () => page.locator('.tools .tool').count(), (n) => n === 0);
  await page.fill('.tools input', '');

  // A tool dragged out of the list drops a box at that point: what it is comes from what it makes. The Edit is always there, and costs nothing.
  const dragOut = async (row, x, y) => {
    const namesBefore = (await state()).assets.map((held) => held.name);
    await row.dragTo(page.locator('.stage'), { targetPosition: { x, y } });
    return until('a box from the tool', async () => (await state()).assets.find((held) => !namesBefore.includes(held.name)), Boolean);
  };
  await until('the list back', () => page.locator('.tools .tool').count(), (n) => n > 0);
  const firstTool = page.locator('.tools .tool:not([data-kind=text])').first();
  const label = await firstTool.locator('b').textContent();
  const kind = await firstTool.getAttribute('data-kind');
  const made = await dragOut(firstTool, 1300, 900);
  seen.tool_dragged = made.name;
  check('tool_named_from_its_label', made.name.toLowerCase().startsWith(label.toLowerCase().slice(0, 4)), { label, name: made.name });
  check('tool_kind_from_its_output', made.kind === kind, { kind, made: made.kind });
  check('tool_written_as_a_function', made.tool?.includes('/'), made.tool);
  const home = await until('its row', async () => (await rows())[made.name], Boolean);
  const place = await page.evaluate(({ x, y }) => {
    const m = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
    return { x: (x - m.e) / m.a, y: (y - m.f) / m.a };
  }, { x: 1300, y: 900 });
  check('box_at_the_drop', Math.abs(home.y - (place.y - 16)) < 3, { home, place });
  await api(`/v1/projects/${id}/assets/${encodeURIComponent(made.name)}`, { method: 'DELETE' });
  await until('that box gone', () => box(made.name).count(), (n) => n === 0);
  check('list_keeps_comments', yaml().startsWith(head), yaml());

  // Local code is listed, and says so: the box Paul's example uses comes from here, dragged like any tool, and called what a person calls it.
  const fromTheList = page.locator('.tools section:has(h4:has-text("Editable")) .tool').first();
  const dropBox = async (name, x, y) => {
    await until('nothing being rendered, so it can be named', () => state(), (now) => !now.busy, FIRST_RUN_MS);
    const fresh = await dragOut(fromTheList, x, y);
    await open(fresh.name);
    await box(fresh.name).locator('.nm').dblclick();
    await page.waitForSelector('.nm-field');
    await page.keyboard.type(name);
    await page.keyboard.press('Enter');
    await until(`named ${name}`, () => of(name), Boolean);
    await away();
    return fresh;
  };
  let tool = 'no-such-package-here';
  if (local) {
    await fromTheList.waitFor({ timeout: FIRST_RUN_MS });
    const wire = (await api(`/v1/tools?project=${id}`)).body.tools.find((held) => held.local);
    tool = wire.id;
    check('local_tool_says_so', wire.local && !wire.label.includes('(local)') && wire.id.startsWith(drawer), wire);
    check('local_tools_are_editable', (await page.locator('.tools section:has(h4:has-text("Editable")) .tool').count()) > 0);
    // A retry of a new asset makes no `_2`, on the wire.
    const body = JSON.stringify({ tool, at: { x: 0, y: 0, w: 300 }, name: 'Twice' });
    const first = await api(`/v1/projects/${id}/assets`, { method: 'POST', headers: { 'Idempotency-Key': 'nodes-twice' }, body });
    const second = await api(`/v1/projects/${id}/assets`, { method: 'POST', headers: { 'Idempotency-Key': 'nodes-twice' }, body });
    check('retry_makes_no_second', first.status === 201 && second.status === 201 && second.body.name === first.body.name && !yaml().includes('Twice_2'), { first, second });
    check('new_asset_has_its_tool', /Twice:\n {4}tool: /.test(yaml()), linesOf(yaml(), 'Twice'));
    await api(`/v1/projects/${id}/assets/Twice`, { method: 'DELETE' });
  }

  // Two pictures dropped, and a box to make something of them.
  await drop(page, [{ name: 'Source-A.png', type: 'image/png', bytes: png(30) }, { name: 'Source-B.png', type: 'image/png', bytes: png(90) }], 700, 240);
  await box('source-a').waitFor();
  await box('source-b').waitFor();
  await until('both in the file', () => Promise.resolve(yaml()), (text) => text.includes('source-a') && text.includes('source-b'));
  if (local) {
    await dropBox('Sheet', 500, 600);
    check('dragged_box_is_that_tool', (await of('Sheet')).tool === tool && (await of('Sheet')).kind === 'image', await of('Sheet'));
    await api(`/v1/projects/${id}/assets/named_here`, { method: 'DELETE' });
  } else replaceFile(yamlAt, `${yaml().trimEnd()}\n  Sheet:\n    tool: ${tool}\n`);
  await box('Sheet').waitFor();
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  await page.mouse.click(1500, 1000);
  await away();
  await page.waitForTimeout(100);

  // At rest a box is its picture or its text and a name, with nothing to press and no state in words.
  check('at_rest_only_its_x', (await box('source-a').locator('button:visible').count()) === 0, await box('source-a').innerText());
  check('at_rest_no_words', (await box('Sheet').locator('.status').count()) === 0 && (await box('source-a').locator('.status').count()) === 0);
  await open('Sheet');
  check('hover_shows_render', (await box('Sheet').locator('button[aria-label=Render]').count()) === 1);
  await shot('box');

  // Typing: `<` offers names, a chip is plain text in the file, and nothing is written until it's let go.
  const sheetText = box('Sheet').locator('.mention-text[data-field=instructions]');
  const before2 = yaml();
  await sheetText.click();
  await page.keyboard.type('A sheet of ');
  await page.keyboard.type('<sour');
  await page.waitForSelector('.name-menu');
  const offered = await page.locator('.name-menu li').allTextContents();
  check('less_than_offers_names', offered.includes('source-a') && offered.includes('source-b'), offered);
  await shot('offered');
  await page.keyboard.press('Enter');
  await until('a chip', () => sheetText.locator('.chip').count(), (n) => n === 1);
  await page.keyboard.type('and ');
  // Everything saves itself (Paul, 11-05): a pause in typing writes it, a chip as its name.
  await until('a pause saves it', () => Promise.resolve(linesOf(yaml(), 'Sheet') ?? ''), (lines) => lines.includes('A sheet of <source-a> and'));
  check('a_pause_saves_it', yaml() !== before2);

  // Any element dragged onto the box: its name goes in, where the caret was, and it goes back where it was with no move saved.
  await away();
  const rowsBefore = await rows();
  const from = await box('source-b').locator('.nm').boundingBox();
  const to = await box('Sheet').locator('.media').boundingBox();
  await page.mouse.move(from.x + 4, from.y + 4);
  await page.mouse.down();
  await page.mouse.move(from.x + 12, from.y + 4);
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 8 });
  await page.waitForTimeout(100);
  await shot('dragging');
  await page.mouse.up();
  const said = await until('its name in the file', () => Promise.resolve(linesOf(yaml(), 'Sheet')), (lines) => lines?.includes('<source-b>'));
  check('file_holds_plain_text', said.includes('A sheet of <source-a> and <source-b>') && /instructions: \|/.test(said), said);
  check('drop_is_no_move', (await rows())['source-b'].x === rowsBefore['source-b'].x && (await rows())['source-b'].y === rowsBefore['source-b'].y, await rows());
  await until('it went back', () => box('source-b').locator('.nm').boundingBox(), (now) => Math.abs(now.x - from.x) < 3 && Math.abs(now.y - from.y) < 3);
  await open('Sheet');
  await until('chips', () => sheetText.locator('.chip').count(), (n) => n === 2);
  const thumbs = await until('the pictures it mentions, small', () => box('Sheet').locator('.mention-thumb').evaluateAll((all) => all.map((one) => one.getBoundingClientRect().width)), (widths) => widths.length === 2 && widths.every((width) => width > 8), 5000).catch((why) => String(why));
  check('mentions_shown_small', Array.isArray(thumbs), thumbs);
  check('wire_reads_what_it_mentions', (await of('Sheet')).reads?.map((read) => read.name).sort().join() === 'source-a,source-b' && (await of('Sheet')).reads[0].at.includes('instructions'), (await of('Sheet')).reads);
  check('wire_fields_flat', (await of('Sheet')).fields?.instructions?.includes('<source-a>') && !('tool' in (await of('Sheet')).fields), (await of('Sheet')).fields);
  check('wire_etag', /^"[0-9a-f]{64}"$/.test((await of('Sheet')).etag ?? ''), (await of('Sheet')).etag);

  // No box lies on another. Dropped on one that can't take it, it slides to the nearest clear place, the other stays, and where it ended is what's saved.
  // Words can't take a picture: a picture box would take it as its version (27c5db3f5).
  const canvasAt = join(folder, 'project.canvas.yaml');
  const lies = (a, b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
  const apart = async (a, b) => !lies(await box(a).boundingBox(), await box(b).boundingBox());
  await away();
  const stood = await rows();
  const carried = await box('source-b').locator('.nm').boundingBox();
  const onto = await box('kept_words').boundingBox();
  await page.mouse.move(carried.x + 4, carried.y + 4);
  await page.mouse.down();
  await page.mouse.move(carried.x + 12, carried.y + 4);
  await page.mouse.move(onto.x + onto.width / 2, onto.y + onto.height / 2, { steps: 8 });
  await page.waitForTimeout(100);
  check('a_box_that_cant_take_it_doesnt_offer', (await box('kept_words').locator('.card.hot').count()) === 0 && (await page.locator('.stage[data-offering]').count()) === 0);
  await page.mouse.up();
  await until('it slid clear', () => apart('kept_words', 'source-b'), Boolean);
  check('a_box_cant_lie_on_a_box', true);
  check('the_other_box_stays', (await rows()).kept_words.x === stood.kept_words.x && (await rows()).kept_words.y === stood.kept_words.y, await rows());
  await until('where it ended, saved', rows, (now) => now['source-b'].x !== stood['source-b'].x || now['source-b'].y !== stood['source-b'].y);

  // Another writer's `at`, on a box, settles the same way; so does a file that arrives with two on one spot, as a converted project does, with no input changing.
  const held = (await rows())['source-a'];
  execFileSync(COZY, ['project', '-p', folder, 'set', 'source-b', `at=${held.x},${held.y}`], { stdio: 'ignore' });
  await page.waitForTimeout(300);
  await until('an `at` on a box comes off it', () => apart('source-a', 'source-b'), Boolean);
  check('an_at_settles', true);
  const asItStood = (await state()).assets.map(({ name, inputs, standing }) => [name, inputs, standing]);
  const piled = readFileSync(canvasAt, 'utf8').replace(/^(\s+source-b: )\{[^}]*\}$/m, `$1{x: ${held.x}, y: ${held.y}}`);
  replaceFile(canvasAt, piled);
  await page.reload();
  await box('source-a').waitFor();
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  await until('what lay on another came apart on opening', () => apart('source-a', 'source-b'), Boolean);
  await until('and that was saved', rows, (now) => now['source-b'].x !== held.x || now['source-b'].y !== held.y);
  check('coming_apart_changes_no_input', JSON.stringify((await state()).assets.map(({ name, inputs, standing }) => [name, inputs, standing])) === JSON.stringify(asItStood));
  await page.evaluate(() => { window.notReloaded = true; });

  // A description, on anything, as text whatever it says.
  await open('source-a');
  await box('source-a').locator('.desc-line').click();
  await page.keyboard.type('A sea <b>bold</b> <img src=x onerror=window.pwned=1> and more');
  await page.mouse.click(1500, 1000);
  await until('its description in the file', () => Promise.resolve(linesOf(yaml(), 'source-a')), (lines) => lines?.includes('description: '));
  await open('source-a');
  check('text_is_only_text', (await page.evaluate(() => window.pwned === undefined && !document.querySelector('.card b') && !document.querySelector('.card img[src="x"]'))));
  check('description_on_the_wire', (await of('source-a')).fields?.description?.includes('A sea'), (await of('source-a')).fields);
  check('a_description_makes_it_no_tool', !(await of('source-a')).tool && (await of('source-a')).sha256?.startsWith('sha256:'), await of('source-a'));

  // Controls from what it holds: its words and its beats shown, whatever the alphabet says of `style` and `summary`, and what else it holds behind More, saying so. Each writes one field and moves nothing else.
  replaceFile(yamlAt, `${yaml().trimEnd()}\n  knobs:\n    # Stays.\n    summary: |\n      <source-b> walks.\n    tool: no-such-package-here/shoot  # how\n    aspect_ratio: "3:2"\n    megapixels: 1\n    style: wash\n    turbo: true\n    beats:\n      - duration: 4\n        description: |\n          First.\n        camera: low\n`);
  await box('knobs').waitFor();
  await open('knobs');
  const knobs = box('knobs');
  const field = async (re) => (linesOf(yaml(), 'knobs') ?? '').match(re)?.[1];
  check('summary_is_its_prompt', (await knobs.locator('.in-frame .mention-text').getAttribute('data-field')) === 'summary');
  check('words_and_beats_shown', (await knobs.locator('.mention-text').count()) === 2 && (await knobs.locator('.beat').count()) === 1 && (await knobs.locator('.edit-more, input[aria-label=megapixels]').count()) === 0, await knobs.innerText());
  const marked = await knobs.locator('.more-marks').textContent();
  check('what_it_holds_says_so', ['megapixels 1', 'turbo on', 'style'].every((word) => marked.includes(word)), marked);
  await more('knobs');
  check('its_style_behind_more', (await knobs.locator('.mention-text[data-field=style]').textContent()).includes('wash'));
  check('a_shape_is_a_choice', (await knobs.locator('select[aria-label="Shape"]').count()) === 1);
  await knobs.locator('input[aria-label=megapixels]').focus();
  await page.keyboard.press('ArrowRight');
  await until('the slider written', () => field(/megapixels: (\S+)/), (value) => value === '2');
  // Nothing has moved the pointer since it opened More. On Windows (41917007085) the box closed here by itself: this says when, and what moved.
  await page.waitForTimeout(300);
  check('a_box_in_use_stays_open', (await knobs.locator('.edit-more').count()) === 1, await boxLog(page));
  check('the_canvas_cannot_scroll', (await page.locator('.react-flow').evaluate((el) => getComputedStyle(el).overflow)) === 'clip');
  await more('knobs');
  await knobs.locator('label:has-text("turbo") input').click();
  await until('the toggle written', () => field(/turbo: (\S+)/), (value) => value === 'false');
  check('a_control_keeps_the_comment', (await field(/(# Stays\.)/)) === '# Stays.' && (await field(/(# how)/)) === '# how', linesOf(yaml(), 'knobs'));
  check('text_stays_block', (linesOf(yaml(), 'knobs') ?? '').includes('summary: |'), linesOf(yaml(), 'knobs'));

  // Its shape is a choice in the box, and writes what it makes. Width is only layout, from its corner on the canvas.
  await more('knobs');
  const widthBefore = (await rows()).knobs.w;
  await knobs.locator('select[aria-label="Shape"]').selectOption('3:4');
  await until('the ratio written', () => field(/aspect_ratio: (\S+)/), (value) => value === '"3:4"');
  check('reshape_writes_the_shape', true);
  await away();
  await knobs.locator('.nm').hover();
  const corner = await knobs.locator('.corner').boundingBox();
  await page.mouse.move(corner.x + 6, corner.y + 6);
  await page.mouse.down();
  await page.mouse.move(corner.x + 60, corner.y + 6, { steps: 6 });
  await page.mouse.up();
  await until('the width written', rows, (now) => now.knobs.w && now.knobs.w !== widthBefore);
  // Pulled wider, it takes the nearest shape its tool makes (2a50453b1), and its width with it.
  check('corner_takes_its_shape', (await field(/aspect_ratio: (\S+)/)) !== '"3:4"', await field(/aspect_ratio: (\S+)/));
  seen.width_before_reshape = widthBefore;

  // Beats: one text and a duration slider each; add, remove, down to the one that always stays; what else a beat holds stays.
  await open('knobs');
  check('beat_one', (await knobs.locator('.beat').count()) === 1 && (await knobs.locator('.beat input[type=range]').inputValue()) === '4');
  check('one_beat_stays', (await knobs.locator('[aria-label^="Remove beat"]').count()) === 0);
  await knobs.locator('.add-beat').click();
  await until('two beats', () => knobs.locator('.beat').count(), (n) => n === 2);
  await knobs.locator('.beat .mention-text').nth(1).click();
  await page.keyboard.type('Second.');
  await knobs.locator('.beat input[type=range]').nth(1).focus();
  await page.keyboard.press('ArrowRight');
  const beats = await until('both beats in the file', () => Promise.resolve(linesOf(yaml(), 'knobs') ?? ''), (lines) => lines.includes('Second.') && lines.includes('duration: 5'));
  check('a_beat_keeps_what_it_holds', beats.includes('camera: low'), beats);
  await open('knobs');
  await knobs.locator('[aria-label="Remove beat 2"]').click();
  await until('one beat again', () => Promise.resolve(linesOf(yaml(), 'knobs') ?? ''), (lines) => !lines.includes('Second.'));
  check('down_to_the_one_that_stays', (await knobs.locator('.beat').count()) === 1 && (await knobs.locator('[aria-label^="Remove beat"]').count()) === 0);
  await api(`/v1/projects/${id}/assets/knobs`, { method: 'DELETE' });
  await until('knobs gone', () => box('knobs').count(), (n) => n === 0);

  // Conflicts are per asset: another's edit doesn't count, its own does, and neither side is lost.
  await open('Sheet');
  await sheetText.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' mine');
  replaceFile(yamlAt, yaml().replace('      Written\n', '      Rewritten\n'));
  // Read again: the canvas is drawn in on Sheet, so what else is on it may not be drawn.
  await until('its etag read again', async () => (await of('kept_words'))?.fields?.description ?? '', (text) => text.includes('Rewritten'));
  await until('another\'s edit is no conflict', () => Promise.resolve(linesOf(yaml(), 'Sheet')), (lines) => lines?.includes('mine'));
  await sheetText.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' again');
  replaceFile(yamlAt, yaml().replace(' mine', ' theirs'));
  await box('Sheet').locator('.status.ood:has-text("changed under this")').waitFor();
  await shot('conflict');
  check('a_conflict_writes_nothing', linesOf(yaml(), 'Sheet').includes(' theirs') && !linesOf(yaml(), 'Sheet').includes(' again'), linesOf(yaml(), 'Sheet'));
  await box('Sheet').locator('button:has-text("Keep mine")').click();
  await until('mine kept', () => Promise.resolve(linesOf(yaml(), 'Sheet')), (lines) => lines?.includes(' again'));
  check('kept_mine', true);

  // A caret never rests in a name, where what's typed goes nowhere: a click on one puts it just after.
  await open('Sheet');
  await sheetText.locator('.chip').first().click();
  await page.waitForTimeout(50);
  await page.keyboard.type(' typed');
  await page.mouse.click(1500, 1000);
  await until('typed after the name', () => Promise.resolve(linesOf(yaml(), 'Sheet')), (lines) => lines?.includes('<source-a> typed'));
  check('typed_after_a_name', true);

  // Right click: rename, delete, and what it's made with.
  await away();
  await box('source-b').locator('.nm').click({ button: 'right' });
  await page.waitForSelector('.asset-menu');
  check('file_menu', (await page.locator('.asset-menu > button').allTextContents()).join() === 'Rename,Delete', await page.locator('.asset-menu > button').allTextContents());
  await page.keyboard.press('Escape');
  await open('Sheet');
  await box('Sheet').locator('[aria-label="More for this box"]').click();
  await page.waitForSelector('.asset-menu');
  const items = await page.locator('.asset-menu > button').allTextContents();
  check('box_menu', ['Change the tool', 'Rename', 'Delete'].every((item) => items.includes(item)), items);
  await page.keyboard.press('Escape');

  // A fresh box, from the list: one prompt, or a prompt and one beat that's in the file from the start. All else is behind More, which stays as it's left, says what's set there, and saves what `cozy project status` reads.
  const statusOf = (name) => JSON.parse(execFileSync(COZY, ['--json', '--full', 'project', '-p', folder, 'status'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] })).assets.find((held) => held.name === name);
  if (local) {
    const fixture = join(__dirname, '..', 'project', 'tool.py');
    replaceFile(yamlAt, `${yaml().trimEnd()}\n  fixture:\n    tool: ${fixture}/told\n`);
    await box('fixture').waitFor();
    // Read with this machine's Runtime, as a run reads it: PyPI's can't, and this may be the first run of it here.
    execFileSync(COZY, ['--json', 'run', `${fixture}/still`, 'prompt=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
    const wire = await until('its functions listed', async () => (await api(`/v1/tools?project=${id}`)).body.tools.filter((held) => held.local && held.id.startsWith(fixture)), (held) => held.length > 0, FIRST_RUN_MS);
    const labelOf = (name) => wire.find((held) => held.function === name).label;
    const reload = async () => {
      await page.reload();
      await page.waitForSelector('.react-flow__node');
      await page.evaluate(() => { window.notReloaded = true; });
      await page.keyboard.press('f');
      await page.waitForTimeout(400);
    };
    await reload();
    const place = async (name) => {
      await away();
      await page.locator('.tools li.tool', { has: page.locator(`b:text-is("${labelOf(name)}")`) }).click();
      await box(`${name}_1`).waitFor();
      await open(`${name}_1`);
    };

    await place('still');
    const still = box('still_1');
    await until('one prompt', () => still.locator('.mention-text').count(), (n) => n === 1);
    check('fresh_box_is_its_prompt', (await still.locator('.in-frame .mention-text[data-field=instructions]').count()) === 1 && (await still.locator('.here-all input, .here-all select, .beat, .edit-more').count()) === 0, await still.innerText());
    check('fresh_box_marks_nothing', (await still.locator('.more-marks').count()) === 0, await still.innerText());
    await shot('fresh-prompt');
    await more('still_1');
    await still.locator('input[aria-label=Count]').waitFor();
    await still.locator('select[aria-label=Style]').selectOption({ label: 'noise' });
    await until('the choice in the status', async () => statusOf('still_1').fields?.style, (style) => style === 'noise');
    await still.locator('.more-toggle').click();
    await still.locator('.edit-more').waitFor({ state: 'detached' });
    await until('marked', () => still.locator('.more-marks').count(), (n) => n === 1);
    check('a_set_value_is_marked', (await still.locator('.more-marks').textContent()).includes('Style noise'), await still.locator('.more-marks').textContent());
    await shot('fresh-marked');
    await open('still_1');
    check('shut_stays_shut', (await still.locator('.edit-more').count()) === 0);
    await more('still_1');
    await open('still_1');
    check('open_stays_open', (await still.locator('.edit-more').count()) === 1);
    await reload();
    await open('still_1');
    check('open_outlasts_a_reload', (await still.locator('.edit-more').count()) === 1);
    await still.locator('select[aria-label=Style]').selectOption({ label: 'flat' });
    await until('its default in the status', async () => statusOf('still_1').fields?.style, (style) => style === 'flat');
    await still.locator('.more-toggle').click();
    await still.locator('.edit-more').waitFor({ state: 'detached' });
    check('its_default_marks_nothing', (await still.locator('.more-marks').count()) === 0, await still.innerText());

    await place('told');
    const told = box('told_1');
    await until('a prompt and a beat', () => told.locator('.beat').count(), (n) => n === 1);
    check('fresh_shot_is_its_prompt_and_a_beat', (await told.locator('.in-frame .mention-text[data-field=instructions]').count()) === 1 && (await told.locator('.mention-text').count()) === 2 && (await told.locator('input[type=range]').count()) === 1 && (await told.locator('.edit-more').count()) === 0, await told.innerText());
    check('fresh_beat_cannot_go', (await told.locator('[aria-label^="Remove beat"]').count()) === 0);
    await until('its beat in the status', async () => statusOf('told_1').fields?.beats?.length, (n) => n === 1);
    check('its_beat_has_a_length', statusOf('told_1').fields.beats[0].duration === Number(await told.locator('.beat input[type=range]').inputValue()), statusOf('told_1').fields);
    await shot('fresh-beat');
    await told.locator('.add-beat').click();
    await until('two beats', () => told.locator('.beat').count(), (n) => n === 2);
    check('either_can_go_now', (await told.locator('[aria-label^="Remove beat"]').count()) === 2);
    await told.locator('.beat .mention-text').nth(1).click();
    await page.keyboard.type('The second.');
    await told.locator('.beat input[type=range]').nth(1).focus();
    await page.keyboard.press('ArrowRight');
    await until('both in the status', async () => statusOf('told_1').fields?.beats, (beats) => beats?.length === 2 && beats[1].description?.includes('The second.') && beats[1].duration === Number(beats[0].duration) + 1);
    await told.locator('[aria-label="Remove beat 2"]').click();
    await until('one in the status', async () => statusOf('told_1').fields?.beats?.length, (n) => n === 1);
    check('the_last_beat_stays', (await told.locator('.beat').count()) === 1 && (await told.locator('[aria-label^="Remove beat"]').count()) === 0, await told.innerText());
    for (const name of ['still_1', 'told_1', 'fixture']) {
      await api(`/v1/projects/${id}/assets/${name}`, { method: 'DELETE' });
      await until(`${name} gone`, () => box(name).count(), (n) => n === 0);
    }
    // The list again, as what the project names now makes it.
    await reload();
  }

  // Only where this computer runs models: real renders.
  if (local) {
    const versions = async (name) => (await of(name))?.prefixes?.length ?? 0;
    const render = async (name, count) => {
      await open(name);
      // Render is with its words, and it may be showing what it made.
      if (await box(name).locator('.take.add[aria-label="Its words"]:not(.on)').count()) await box(name).locator('.take.add[aria-label="Its words"]').click();
      await box(name).locator('button[aria-label=Render]').click();
      await until(`${name}'s ${count}`, () => versions(name), (n) => n === count, FIRST_RUN_MS);
    };
    await render('Sheet', 1);
    await open('Sheet');
    await until('its picture, unreloaded', () => box('Sheet').locator('.media img').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n > 0);
    check('rendered_with_no_reload', await page.evaluate(() => window.notReloaded === true));
    await render('Sheet', 2);
    await open('Sheet');
    await until('its count', () => box('Sheet').locator('.count').textContent(), (text) => text === '2/2');

    // Scrolled through: looking changes only this page.
    const fileBefore = yaml();
    // What just landed can still move the row from under a press (Firefox, 42006624081): a person presses again.
    await until('looking at 1', async () => {
      const count = await box('Sheet').locator('.count').textContent();
      if (count !== '1/2') await box('Sheet').locator('.take[aria-label="version 1"]').click();
      return count;
    }, (text) => text === '1/2');
    check('looking_is_only_this_page', yaml() === fileBefore);

    // Approve: it becomes that file, and flips back.
    await box('Sheet').locator('button:has-text("Approve")').click();
    await until('approved in the file', () => Promise.resolve(linesOf(yaml(), 'Sheet')), (lines) => /approved: "?[0-9a-f]{6,}/.test(lines));
    const approved = await of('Sheet');
    check('approved_is_that_version', approved.approved === 1 && approved.pinned, approved.approved);
    await open('Sheet');
    await box('Sheet').locator('.take.add[aria-label="Its words"]').waitFor();
    check('approved_shows_its_file', (await box('Sheet').locator('.in-frame').count()) === 0);
    check('no_word_says_pin', !/\bpin|accept/i.test(await box('Sheet').innerText()), await box('Sheet').innerText());
    await box('Sheet').locator('button:has-text("Unapprove")').click();
    await until('unapproved', () => of('Sheet'), (held) => !held.pinned);
    await box('Sheet').locator('button:text-is("Approve")').waitFor();
    await box('Sheet').locator('button:has-text("Approve")').click();
    await until('approved again', () => of('Sheet'), (held) => held.pinned);

    // One discarded: off the strip at once, Undo brings it back, and what's approved going says so and stands on the newest.
    const strip = () => box('Sheet').locator('.take[aria-label^="version"]').count();
    const asked = await strip();
    await box('Sheet').locator('.take-at').first().hover();
    await box('Sheet').locator('.take-at .take-off').first().click();
    check('a_take_goes_at_once', (await strip()) === asked - 1, await strip());
    check('and_the_box_stays_open', (await page.locator('.card.here[data-asset="Sheet"]').count()) === 1);
    await until('and from the project', async () => (await of('Sheet')).prefixes?.length, (n) => n === asked - 1);
    await page.locator('.note:has-text("Discarded") button:has-text("Undo")').first().click();
    await until('it came back', async () => (await of('Sheet')).prefixes?.length, (n) => n === asked);
    check('a_take_comes_back', (await strip()) === asked, await strip());

    // One dragged out: its own asset, the same file, what it says it is, and no new bytes.
    await api(`/v1/projects/${id}/assets/Sheet`, { method: 'PATCH', body: JSON.stringify({ fields: { description: 'The sheet of two.' } }) });
    await until('described', () => of('Sheet'), (held) => held.fields?.description?.includes('sheet of two'));
    const filesBefore = readdirSync(join(folder, 'files')).length;
    await open('Sheet');
    // The drag carries what the page holds, and that came by the stream, a moment behind the answer above.
    await until('the page has its words', () => box('Sheet').locator('text=The sheet of two.').count(), (n) => n > 0);
    // Picked up from its big tile, the take it shows.
    await box('Sheet').locator('.take[aria-label="version 2"]').click();
    await box('Sheet').locator('.take-out').dragTo(page.locator('.stage'), { targetPosition: await openCanvas(page) });
    const out = await until('taken out', () => of('Sheet_2'), Boolean);
    check('taken_out_is_the_same_file', out.sha256 === (await of('Sheet')).prefixes[1].main.sha256 && !out.tool, out);
    check('taken_out_no_new_bytes', readdirSync(join(folder, 'files')).length === filesBefore);
    await until('it keeps what it says', () => of('Sheet_2'), (held) => held.fields?.description?.includes('sheet of two'));
    await shot('versions');

    // Something else to go on while one fails.
    await away();
    await dropBox('Other', 1100, 600);
    await render('Other', 1);

    // What a package shows beside its first text: both on the box as it opens, empty reading N/A, and what's typed is the file's, as the CLI reads it. An empty one is written nowhere.
    const shaped = join(__dirname, '..', 'project', 'shaped.py');
    // Read with this machine's Runtime, as tool.py is above.
    execFileSync(COZY, ['--json', 'run', `${shaped}/picture`, 'prompt=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
    replaceFile(yamlAt, `${yaml().trimEnd()}\n  sounded:\n    tool: ${shaped}/shot\n`);
    await box('sounded').waitFor();
    await open('sounded');
    const sounded = box('sounded');
    const scape = sounded.locator('.mention-text[data-field=soundscape]');
    await scape.waitFor();
    check('shown_by_its_node', (await sounded.locator('.mention-text[data-field=music]').count()) === 1 && (await sounded.locator('.edit-more').count()) === 0, await sounded.innerText());
    check('empty_reads_na', (await scape.getAttribute('data-placeholder')) === 'N/A (silent)', await scape.getAttribute('data-placeholder'));
    check('a_hidden_input_isnt_drawn', (await sounded.locator('[data-field=steps], [data-field=plate], [data-field=references]').count()) === 0, await sounded.innerText());
    await scape.click();
    await page.keyboard.type('Rain on steel.');
    await sounded.locator('.mention-text[data-field=music]').click();
    await until('the soundscape as the CLI reads it', async () => statusOf('sounded').fields?.soundscape, (said) => said === 'Rain on steel.');
    check('an_empty_one_isnt_written', !('music' in statusOf('sounded').fields), statusOf('sounded').fields);
    await page.keyboard.type('Low strings.');
    await sounded.locator('.mention-text[data-field=music]').blur();
    await until('the music as the CLI reads it', async () => statusOf('sounded').fields?.music, (said) => said === 'Low strings.');
    await shot('sounded');

    // What a box mentions goes to the list of what's mentioned. A function's optional frames are the file's to set: filled from mentions, the first
    // picture took the canvas over the shot's aspect_ratio, and the second forced how it ends.
    replaceFile(yamlAt, `${yaml().trimEnd()}\n  mentioning:\n    tool: ${shaped}/shot\n    summary: <source-a> meets <source-b>.\n    aspect_ratio: "16:9"\n`);
    // Drawn in on a box, the canvas draws only what's near it: a new one is seen at rest, all of it in view.
    await away();
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('mentioning').waitFor();
    execFileSync(COZY, ['project', '-p', folder, 'render', 'mentioning', '--on', 'local', '--plain'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
    // A project's status counts an asset's versions; one asset's status lists them.
    const madeBy = JSON.parse(execFileSync(COZY, ['--json', '--full', 'project', '-p', folder, 'status', 'mentioning'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] })).versions.at(-1).runs.at(-1);
    const sent = JSON.parse(execFileSync(COZY, ['--json', '--full', 'run', 'show', String(madeBy), '--once'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] })).request.input;
    check('a_mention_is_no_keyframe', !('first_frame' in sent) && !('last_frame' in sent), sent);
    check('a_mention_is_a_reference', sent.references?.length === 2 && sent.aspect_ratio === '16:9', sent);

    // Text: a name for some words. It's on the list before any function, its box is only its words, with nothing to render, and a mention of it is a chip like any other.
    const textRow = page.locator('aside.tools li.tool[data-kind=text]');
    check('text_is_on_the_list', (await textRow.count()) === 1, await page.locator('aside.tools').innerText());
    await away();
    await textRow.click();
    await box('text').waitFor();
    await open('text');
    const wording = box('text');
    await wording.locator('.mention-text[data-field=text]').waitFor();
    await wording.locator('.mention-text[data-field=text]').click();
    await page.keyboard.type('Painted by hand.');
    await wording.locator('.mention-text[data-field=text]').blur();
    await until('its words as the CLI reads them', async () => statusOf('text').fields?.text, (said) => said === 'Painted by hand.');
    check('text_has_no_render', (await wording.locator('button:text-is("Render"), .takes, button:text-is("Approve")').count()) === 0, await wording.innerText());
    check('text_has_a_kind', statusOf('text').kind === 'text', statusOf('text'));

    // Caption: what the package says lands in the field as an edit, and nothing's written until it's kept. Local code a project names can answer.
    const asker = join(__dirname, '..', 'project', 'tool.py');
    replaceFile(yamlAt, `${yaml().trimEnd()}\n  asker:\n    tool: ${asker}/told\n`);
    await away();
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('asker').waitFor();
    // A toggle shows its function's own default, on, with nothing in the file, and off is what's written.
    await more('asker');
    const bright = box('asker').locator('label:has-text("Bright") input');
    await bright.waitFor();
    check('a_toggle_shows_its_default', await bright.isChecked(), linesOf(yaml(), 'asker'));
    check('a_default_isnt_written', !(linesOf(yaml(), 'asker') ?? '').includes('bright'), linesOf(yaml(), 'asker'));
    await bright.click();
    await until('off is written', () => linesOf(yaml(), 'asker') ?? '', (lines) => lines.includes('bright: false'));
    await open('Other');
    // A caption is of what's been made: it's with the take, which is what a box with takes opens on.
    await box('Other').locator('button:text-is("Caption")').click();
    const typed = box('Other').locator('.desc-open .mention-text');
    await typed.waitFor({ timeout: FIRST_RUN_MS });
    await until('its description in the file', async () => (await of('Other')).fields?.description ?? '', (text) => text.includes('THE-PACKAGE-ASKS'));
    await api(`/v1/projects/${id}/assets/asker`, { method: 'DELETE' });
    await until('asker gone', () => box('asker').count(), (n) => n === 0);

    // X on what it was made from. What's approved shows nothing wrong, no dialog stops it, and Undo waits.
    const heldFile = (await of('Sheet')).sha256;
    const filesThen = readdirSync(join(folder, 'files')).length;
    const stood = (await rows())['source-a'];
    const deleting = page.waitForResponse((answer) => answer.request().method() === 'DELETE' && answer.url().endsWith('/assets/source-a'));
    await deleteBox('source-a');
    const deleted = await (await deleting).json();
    await until('gone at once', () => box('source-a').count(), (n) => n === 0);
    check('no_dialog', (await page.locator('dialog, [role=dialog]').count()) === 0);
    const noted = await until('its note', () => page.locator('.note .line').allTextContents(), (lines) => lines.some((line) => line.includes('source-a')));
    // `mentioning` reads it too, and isn't approved: only it is named.
    check('approved_reader_says_nothing', !noted.some((line) => line.includes('Sheet')), noted);
    check('unapproved_reader_named', noted.some((line) => line.includes('mentioning reads this')), noted);
    check('a_deleted_file_stays', readdirSync(join(folder, 'files')).length === filesThen);
    await until('out of the file', () => Promise.resolve(linesOf(yaml(), 'source-a')), (lines) => lines === null);
    await away();
    await page.waitForTimeout(300);
    check('approved_stands', (await of('Sheet')).sha256 === heldFile && (await of('Sheet')).pinned, await of('Sheet'));
    check('approved_shows_nothing_wrong', (await box('Sheet').locator('.dot.s-ood, .dot.s-failed').count()) === 0 && (await of('Sheet')).standing === 'current', await of('Sheet'));
    await open('Sheet');
    check('approved_face_has_no_warning', (await box('Sheet').locator('.status.ood, .status.bad, .warned').count()) === 0, await box('Sheet').innerText());
    check('wire_missing', (await of('Sheet')).missing?.join() === 'source-a', (await of('Sheet')).missing);
    await box('Sheet').locator('.take.add[aria-label="Its words"]').click();
    await box('Sheet').locator('.chip.missing:has-text("source-a")').waitFor();
    check('missing_chip_on_edit', true);
    await shot('missing');

    // Rendering it again fails, naming what's gone, and the rest goes on.
    await box('Sheet').locator('button[aria-label=Render]').click();
    const failed = await until('it says why', () => of('Sheet'), (held) => held.failure);
    check('fails_naming_it', failed.failure.code === 'input_missing' && failed.failure.message.includes('<source-a> is no longer in the project'), failed.failure);
    // Render leaves the box: the failure is a toast, and an approved box's is on its words, its picture standing.
    await until('a toast says why', () => page.locator('.note .line').allTextContents(), (lines) => lines.some((line) => line.includes("Sheet didn't render") && line.includes('no longer in the project')));
    await open('Sheet');
    if (await box('Sheet').locator('.take.add[aria-label="Its words"]:not(.on)').count()) await box('Sheet').locator('.take.add[aria-label="Its words"]').click();
    await box('Sheet').locator('.status.bad:has-text("no longer in the project")').waitFor();
    check('its_next_steps_as_text', (await box('Sheet').locator('.status.bad .next-line').count()) > 0, await box('Sheet').locator('.status.bad').innerText());
    // The same failure again is said again, naming its box once.
    await page.locator('.note [aria-label=Dismiss]').evaluateAll((all) => all.forEach((one) => one.click()));
    await box('Sheet').locator('button[aria-label=Render]').click();
    const again = await until('it says so again', () => page.locator('.note .line').allTextContents(), (lines) => lines.some((line) => line.includes("Sheet didn't render")));
    check('failed_again_said_again', again.every((line) => !line.includes('Sheet: ')), again);
    check('failed_runs_nothing', (await versions('Sheet')) === 2 && (await of('Sheet')).sha256 === heldFile);
    await render('Other', 2);
    check('the_rest_goes_on', true);
    await open('Sheet');
    await box('Sheet').locator('.take.approved').click();
    await away();
    await page.waitForTimeout(200);
    check('back_on_its_file_nothing_wrong', (await box('Sheet').locator('.dot.s-failed').count()) === 0);

    // Undo: back with its row, and nothing is missing. Its note has gone by now (10 s, Paul 11-05), so this is what its Undo sends; the button's own below.
    const back = await api(`/v1/projects/${id}/assets/source-a`, { method: 'PUT', body: JSON.stringify({ text: deleted.text, at: stood }) });
    check('put_back', back.status === 200, back);
    await box('source-a').waitFor();
    await until('back in the file', () => Promise.resolve(linesOf(yaml(), 'source-a')), Boolean);
    check('undo_keeps_its_description', (await of('source-a')).fields?.description?.includes('A sea'), (await of('source-a')).fields);
    check('undo_has_its_row', Boolean((await rows())['source-a']), await rows());
    await until('nothing missing', () => of('Sheet'), (held) => !held.missing && !held.failure);
    check('undo_brings_back_the_input', true);

    // A deleted asset's versions come back with it.
    const takes = await versions('Other');
    // Only Shift selects: a click opens.
    await away();
    await box('Other').locator('.nm').click({ modifiers: ['Shift'] });
    await page.keyboard.press('Delete');
    await until('the Delete key', () => box('Other').count(), (n) => n === 0);
    await until('no versions showing', () => of('Other'), (held) => !held);
    check('orphan_is_listed', (await state()).orphans?.some((orphan) => orphan.name === 'Other'), (await state()).orphans);
    await page.locator('.note:has-text("Other") button:has-text("Undo")').click();
    await box('Other').waitFor();
    check('undo_brings_its_versions', (await versions('Other')) === takes, await versions('Other'));

    // What hasn't been approved when its source goes is worth a plain note, with no dialog, and Undo stays.
    await open('Other');
    // A box with takes opens on its take: + gives its words.
    if (await box('Other').locator('.take.add[aria-label="Its words"]:not(.on)').count()) await box('Other').locator('.take.add[aria-label="Its words"]').click();
    await box('Other').locator('.mention-text[data-field=instructions]').click();
    await page.keyboard.type('From <source-b>');
    await page.mouse.click(1500, 1000);
    await until('it reads it', () => of('Other'), (held) => held.reads?.some((read) => read.name === 'source-b'));
    // The note says what the page has heard, a moment behind the file.
    await page.waitForTimeout(1000);
    await deleteBox('source-b');
    const warned = await until('a plain note', () => page.locator('.note .line').allTextContents(), (lines) => lines.some((line) => line.includes('source-b')));
    // `mentioning` reads it too: both are named.
    check('note_when_not_approved', warned.some((line) => line.includes('Other') && line.includes('mentioning') && line.includes("aren't approved yet")), warned);
    await page.locator('.note:has-text("source-b") button:has-text("Undo")').click();
    await box('source-b').waitFor();

    // Render on what's approved: another to look at, and the approval stays.
    const nowHeld = (await of('Sheet')).sha256;
    await open('Sheet');
    await box('Sheet').locator('.take.add[aria-label="Its words"]').click();
    await box('Sheet').locator('button[aria-label=Render]').click();
    await until('another to look at', () => versions('Sheet'), (n) => n === 3, FIRST_RUN_MS);
    check('approved_stays', (await of('Sheet')).pinned && (await of('Sheet')).sha256 === nowHeld, await of('Sheet'));

    // Render all: its count, what's approved left alone.
    await until('Render all says how many', () => page.locator('.render-all button').textContent(), (text) => /^Render all \d+$/.test(text));
    const counted = Number((await page.locator('.render-all button').textContent()).match(/\d+/)[0]);
    const unapproved = (await state()).assets.filter((held) => held.tool && !held.pinned).length;
    check('render_all_counts', counted === unapproved, { counted, unapproved });
    const [sheetTakes, otherTakes] = [await versions('Sheet'), await versions('Other')];
    await page.click('.render-all button');
    await until('Other made again', () => versions('Other'), (n) => n === otherTakes + 1, FIRST_RUN_MS);
    check('render_all_leaves_approved', (await versions('Sheet')) === sheetTakes);

    // A new take from here: scrubbed to a time, its words as they were, and the function told to keep up to there.
    // One file dropped asks its name first: Enter takes the one offered.
    await away();
    await drop(page, [{ name: 'Look.mp4', type: 'video/mp4', bytes: MP4 }], 400, 900);
    await page.waitForSelector('.nm-field');
    await page.keyboard.press('Enter');
    await box('look').waitFor();
    await until('look in the file', () => Promise.resolve(linesOf(yaml(), 'look')), Boolean);
    replaceFile(yamlAt, `${yaml().trimEnd()}\n  Clip:\n    summary: |\n      On <look>.\n    tool: ${shots}/shoot\n`);
    // By the project: it can land past the edge of the view, and what's off it isn't drawn.
    await until('Clip in the project', () => of('Clip'), Boolean);
    await render('Clip', 1);
    await until('its length measured', () => of('Clip'), (held) => held.prefixes?.[0]?.main?.duration_ms > 0);
    await open('Clip');
    const clipFile = linesOf(yaml(), 'Clip');
    await more('Clip');
    await box('Clip').locator('.from-here .keeps').fill('12');
    await box('Clip').locator('button:has-text("New take from here")').click();
    await until('a new take', () => versions('Clip'), (n) => n === 2, FIRST_RUN_MS);
    const given = (await api(`/v1/projects/${id}/assets/Clip`)).body.versions.at(-1).files.given.path;
    // Listed as its run ends, it lands in files/ a moment later (Firefox, 42110239536).
    await until('what it was given, in files/', () => Promise.resolve(existsSync(join(folder, given))), Boolean);
    const told = JSON.parse(readFileSync(join(folder, given), 'utf8'));
    check('kept_up_to_there', told.keep === 0.5, told);
    check('words_as_they_were', linesOf(yaml(), 'Clip') === clipFile, linesOf(yaml(), 'Clip'));
    await open('Clip');
    await box('Clip').locator('.take[aria-label="take 2"]').waitFor();
    check('takes_are_called_takes', true);
    await shot('takes');

    // The terminal sees the same project.
    const status = JSON.parse(execFileSync(COZY, ['--json', '--full', 'project', '-p', folder, 'status'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }));
    check('cli_sees_it', ['Sheet', 'Sheet_2', 'Other', 'source-a', 'source-b', 'Clip'].every((name) => status.assets.some((held) => held.name === name)), status.assets.map((held) => held.name));
  } else {
    // No machine: Render offers one, under Machines, and waits.
    await open('Sheet');
    await box('Sheet').locator('button[aria-label=Render]').click();
    await until('Machines offered', () => Promise.resolve(page.url()), (url) => url.includes('/machines'));
    check('no_machine_offers_one', true);
    await page.goBack();
    await open('Sheet');
    await box('Sheet').locator('.status:has-text("Waits for a machine")').waitFor();
    check('render_waits_for_a_machine', true);
  }

  // X and Delete work anywhere, and Undo with them, whatever can run.
  await deleteBox('source-b');
  await until('gone', () => box('source-b').count(), (n) => n === 0);
  await page.locator('.note:has-text("source-b") button:has-text("Undo")').first().click();
  await box('source-b').waitFor();
  // The box is back as it's asked for, ahead of the file.
  check('delete_and_undo', Boolean(await until('back in the file', () => Promise.resolve(linesOf(yaml(), 'source-b')), Boolean)));

  check('comments_and_text_kept', yaml().includes('# Mine, and kept.') && yaml().includes('  # A note on what follows.') && yaml().includes('    # Inside it.') && yaml().includes('      by hand.'), yaml());
  const hashes = [...yaml().matchAll(/approved: "?([0-9a-f]{12})/g)].map((m) => m[1]);
  const shown = await page.evaluate(() => document.body.innerText + [...document.querySelectorAll('[title],[aria-label]')].map((node) => node.title + node.ariaLabel).join(' '));
  check('hash_not_shown', hashes.every((hash) => !shown.includes(hash)), hashes.filter((hash) => shown.includes(hash)));
  check('no_word_says_node', !/\bnodes?\b/i.test(shown), shown.match(/[\s\S]{0,40}\bnodes?\b[\s\S]{0,40}/i)?.[0]);
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  await new Promise((done) => setTimeout(done, 300));
  // Only the conflict it asked for: anything else a project's route refused is a fault, however the page took it.
  // Nor a read of what the stream said changed, gone by then: that's how the page hears it's gone.
  const unasked = refused.filter((refusal) => refusal.url.includes('/v1/projects/') && !(refusal.code === 'asset_changed' && refusal.url.endsWith('/assets/Sheet')) && !(refusal.code === 'asset_unknown' && refusal.method === 'GET'));
  check('nothing_refused_unasked', unasked.length === 0, unasked);
  check('no_page_errors', errors.length === 0, { errors, refused });
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
