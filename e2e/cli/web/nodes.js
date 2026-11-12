// Assets as a person works them (web-ui.md, Tools and nodes), in a real browser on a real Calcifer. The shell: a name, Render all, a menu; tools
// on the left, each dragged out to make a box. A box is its picture or its text at rest. Text with chips, the file still plain; a name dragged in; a fresh box as its prompt, or its prompt
// and one beat, with all else behind More; controls from what it holds; reshaping; beats; another's edits merged; deleting and Undo. Where this computer runs models, Paul's example, real: drop a picture, drop a tool, drag the
// picture in, type, render, approve, delete the picture: what's approved stands, rendering it again fails naming what's gone, the rest goes on,
// Undo brings it back. Takes side by side, one dragged out, Pick up from here, Render all.
//   node nodes.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { existsSync, readFileSync, readdirSync, renameSync, writeFileSync } = require('fs');
const { join } = require('path');
const { implementationRow, previewVersion, approveVersion, chooseModel, projectFromPage, projectAction, projectJson, calcifer, carry, signIn, MP4, boxLog, boxOf, closeBox, openBox, openCanvas, fitCanvas, STEP_MS, drop, lastLook, launch, lookAt, png, noStall, renderOf, rowsOf, settled, until, watchBoxes, importYaml, yamlOf } = require('./page');

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
  const base = calcifer();
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

  await signIn(page);
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

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-assets-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  const yaml = () => yamlOf(folder);
  const of = async (name) => (await state()).assets.find((held) => held.name === name);
  const rows = async () => (await api(`/v1/projects/${id}/canvas`)).body.cards;
  const box = (name) => boxOf(page, name);
  // Shut, and off every box.
  const away = async () => {
    await closeBox(page);
    await page.mouse.move(2, 500);
  };
  const open = async (name) => {
    await openBox(page, name);
    const held = await of(name);
    if (held?.kind === 'image' && held.tool) await renderOf(page, name);
  };
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

  // An export a person edited and imported: its words and `|` outlast all of this. Its comments go: the project holds values (cl-803).
  const head = '# Mine, and kept.\nversion: 4\n\nassets:\n  # A note on what follows.\n  kept_words:\n    # Inside it.\n    description: |\n      Written\n      by hand.\n';
  // Local code is a tool once something in the project names it. A real render needs no GPU this way.
  importYaml(folder, local ? `${head}  named_here:\n    tool: ${drawer}/draw\n` : head);
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

  // The two lists fold away with the canvas where it was; the sidebar has no discovery or upload controls.
  const listed = await until('tools or why none', async () => ({ tools: await page.locator('.tools .tool').count(), note: await page.locator('.tools p').allTextContents() }), (now) => now.tools > 0 || now.note.length > 0);
  seen.tools_listed = listed;
  // Every package the Hub lists, with nothing installed (cl-896): the isolated Hub's are qwen-image-2, minimax-h3, sdxl and birefnet.
  const resting = await until('the Hub read', async () => (await api(`/v1/tools?project=${id}`)).body, (body) => ['cozy', 'qwen-image-2', 'minimax-h3', 'sdxl', 'birefnet'].every((name) => body.tools.some((tool) => tool.package === name))).catch(() => null);
  check('every_hub_package_listed', Boolean(resting), resting && { note: resting.note, tools: resting.tools.map((tool) => tool.id) });
  const viewport = () => page.evaluate(() => getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  const before = await viewport();
  await page.click('.tools [aria-label="Hide the tools"]');
  await page.waitForSelector('.tools.shut');
  check('tools_fold', (await page.locator('.stage').evaluate((stage) => stage.getBoundingClientRect().left)) < 60);
  check('folding_keeps_the_canvas', (await viewport()) === before && (await box('kept_words').count()) === 1);
  await page.click('.tools [aria-label="Show the tools"]');
  check('sidebar_is_only_nodes_and_tools', JSON.stringify(await page.locator('.tools h4').allTextContents()) === JSON.stringify(['Nodes', 'Tools']) && await page.locator('.tools input, .tools .browse-toggle, .tools .browse, .tools .box-shelf-head').count() === 0 && !(await page.locator('.tools button').allTextContents()).some((word) => /Browse|Checkpoints|Add file/.test(word)));

  // A tool dragged out of the list drops a box at that point: what it is comes from what it makes. The Edit is always there, and costs nothing.
  const dragOut = async (row, x, y) => {
    const namesBefore = (await state()).assets.map((held) => held.name);
    await carry(page, row, { x, y });
    return until('a box from the tool', async () => (await state()).assets.find((held) => !namesBefore.includes(held.name)), Boolean);
  };
  await until('the list back', () => page.locator('.tools .tool').count(), (n) => n > 0);
  const firstTool = await implementationRow(page, 'image');
  const kind = await firstTool.getAttribute('data-kind');
  // A box is called what its tool's published node says, `image_1` for Qwen Image 2, else its package's word, numbered up from there.
  const dragged = await firstTool.getAttribute('data-tool');
  const boxName = (await api(`/v1/tools?project=${id}`)).body.tools.find((held) => held.id === dragged)?.name;
  // Where the drop lands on the canvas, before the box opens and the canvas draws in on it.
  const place = await page.evaluate(({ x, y }) => {
    const m = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
    return { x: (x - m.e) / m.a, y: (y - m.f) / m.a };
  }, { x: 1300, y: 900 });
  const made = await dragOut(firstTool, 1300, 900);
  // A picture tool's box opens as it lands, on its words, the caret in them and its shape Auto, with no name asked. The Edit stays shut.
  if (kind === 'image') {
    const landedOpen = page.locator(`.card.here[data-asset="${made.name}"]`);
    await renderOf(page, made.name);
    const caretIn = await until('the caret in its edit words', () => landedOpen.locator('.in-frame .mention-text').evaluate((el) => el.contains(document.activeElement)).catch(() => false), Boolean).catch(() => false);
    const shapeSaid = await landedOpen.locator('.ratio-rest').innerText().then((text) => text.trim(), () => null);
    check('a_dropped_image_tool_can_enter_edit_mode', caretIn === true && shapeSaid === 'Auto' && (await page.locator('.nm-field').count()) === 0, { caretIn, shapeSaid });
  } else seen.dropped_kind = kind;
  seen.tool_dragged = made.name;
  const word = (name) => name?.replace(/_\d+$/, '');
  check('tool_named_as_its_node_says', Boolean(boxName) && word(made.name) === word(boxName), { tool: dragged, said: boxName, name: made.name });
  check('tool_kind_from_its_output', made.kind === kind, { kind, made: made.kind });
  check('tool_written_as_a_function', made.tool?.includes('/'), made.tool);
  const home = await until('its row', async () => (await rows())[made.name], Boolean);
  check('box_at_the_drop', Math.abs(home.y - (place.y - 16)) < 3, { home, place });
  await api(`/v1/projects/${id}/assets/${encodeURIComponent(made.name)}`, { method: 'DELETE' });
  await until('that box gone', () => box(made.name).count(), (n) => n === 0);
  check('list_keeps_words', /\n  kept_words:\n    description: \|\n      Written\n      by hand\.\n/.test(yaml()), yaml());

  // Basic nodes survive a fresh read without acquiring a model. The editor is an intrinsic timeline, distinct from generative video.
  const basic = page.locator('.tools [data-palette="nodes"] li.tool');
  check('basic_palette_has_exactly_four_types', JSON.stringify(await basic.locator('b').allTextContents()) === JSON.stringify(['Image', 'Video', 'Audio', 'Text']));
  const blanks = {};
  for (const kind of ['image', 'video', 'audio', 'text']) {
    await away();
    const point = await openCanvas(page);
    blanks[kind] = await dragOut(page.locator(`.tools [data-palette="nodes"] li.tool[data-node="${kind}"]`), point.x, point.y);
  }
  await away();
  await page.reload();
  await page.locator('.react-flow').waitFor();
  const afterReload = await state();
  const blank = (kind) => afterReload.assets.find((held) => held.name === blanks[kind].name);
  check('blank_media_types_persist_without_models', ['image', 'video', 'audio'].every((kind) => blank(kind)?.kind === kind && blank(kind)?.fields?.node_kind === kind && !blank(kind)?.tool && !('text' in (blank(kind)?.fields ?? {}))), { image: blank('image'), video: blank('video'), audio: blank('audio') });
  check('text_is_a_plain_asset_holder', blank('text')?.kind === 'text' && !blank('text')?.tool, blank('text'));
  const videoTool = await implementationRow(page, 'video');
  await videoTool.waitFor();
  check('video_operation_outputs_video', await videoTool.getAttribute('data-kind') === 'video');
  const videoToolId = await videoTool.getAttribute('data-tool');
  const toolOnto = async (row, name) => {
    await away();
    await fitCanvas(page);
    await box(name).waitFor();
    await row.scrollIntoViewIfNeeded();
    const target = await box(name).locator('.card').boundingBox();
    await carry(page, row, { x: target.x + target.width / 2, y: target.y + target.height / 2 });
  };
  const allBefore = (await state()).assets.map((held) => held.name).sort();
  for (const kind of ['image', 'audio', 'text']) {
    const name = blanks[kind].name;
    await toolOnto(videoTool, name);
    await until('incompatible drop explained', () => page.locator('.notes, .note-list, [role=status]').allInnerTexts(), (notes) => notes.some((text) => text.includes('makes video: drop it on a video box')));
    const now = await of(name);
    check(`video_rejected_by_${kind}_without_extra_node`, now.tool === blank(kind).tool && now.kind === blank(kind).kind && JSON.stringify((await state()).assets.map((held) => held.name).sort()) === JSON.stringify(allBefore), now);
  }
  await toolOnto(videoTool, blanks.video.name);
  await until('video takes its compatible model', () => of(blanks.video.name), (held) => held.tool === videoToolId);
  check('video_accepts_operation_without_exposing_type_hint', (await of(blanks.video.name)).fields?.node_kind === 'video' && await page.locator('[data-field="node_kind"]').count() === 0);
  check('opening_model_keeps_the_two_sidebar_lists', JSON.stringify(await page.locator('.tools h4').allTextContents()) === JSON.stringify(['Nodes', 'Tools']) && await page.locator('.tools .box-shelf-head, .tools input, .tools .picks').count() === 0);
  await box(blanks.video.name).locator('button[aria-label^="Take "]').first().click();
  await until('video model removed', () => of(blanks.video.name), (held) => !held.tool);
  check('removing_model_keeps_blank_video_type', (await of(blanks.video.name)).kind === 'video' && (await of(blanks.video.name)).fields?.node_kind === 'video');
  const timelineTool = await implementationRow(page, 'timeline', 'cozy/edit');
  await toolOnto(timelineTool, blanks.video.name);
  await until('Timeline attached to Video', () => of(blanks.video.name), (held) => held.tool === 'cozy/edit');
  check('timeline_is_a_tool_on_video', (await of(blanks.video.name)).kind === 'video' && Boolean((await of(blanks.video.name)).timeline));
  await openBox(page, blanks.video.name);
  await box(blanks.video.name).getByRole('button', { name: new RegExp(`^Take .* off ${blanks.video.name}$`) }).click();
  await until('Timeline removed', () => of(blanks.video.name), (held) => !held.tool);
  check('removing_timeline_keeps_video_holder', (await of(blanks.video.name)).kind === 'video');

  await away();
  for (const held of Object.values(blanks)) await api(`/v1/projects/${id}/assets/${encodeURIComponent(held.name)}`, { method: 'DELETE' });

  // Local implementations are chosen inside the Image operation, then named on their node.
  const fromTheList = await implementationRow(page, 'image');
  const dropBox = async (name, x, y) => {
    await until('nothing being rendered, so it can be named', () => state(), (now) => !now.busy, FIRST_RUN_MS);
    const fresh = await dragOut(fromTheList, x, y);
    await chooseModel(page, fresh.name, tool, "image");
    await until("local model selected", () => of(fresh.name), (held) => held.tool === tool);
    await box(fresh.name).getByRole('button', { name: 'Rename node', exact: true }).click();
    const naming = box(fresh.name).locator('.focus-heading input[aria-label="Name"]');
    await naming.fill(name);
    await naming.press('Enter');
    await until(`named ${name}`, () => of(name), Boolean);
    await away();
    return fresh;
  };
  let tool = 'no-such-package-here';
  if (local) {
    await fromTheList.waitFor({ timeout: FIRST_RUN_MS });
    const wire = (await api(`/v1/tools?project=${id}`)).body.tools.find((held) => held.local && held.id === `${drawer}/draw`);
    tool = wire.id;
    check('local_tool_says_so', wire.local && !wire.label.includes('(local)') && wire.id.startsWith(drawer), wire);
    check("local_tool_remains_available_in_metadata", Boolean(wire) && wire.makes === "image", wire);
    // A retry of a new asset makes no `_2`, on the wire. Calcifer keeps a key a day whatever the project (cl-812): each pass, its own.
    const body = JSON.stringify({ tool, at: { x: 0, y: 0, w: 300 }, name: 'Twice' });
    const twice = `nodes-twice-${id}`;
    const first = await api(`/v1/projects/${id}/assets`, { method: 'POST', headers: { 'Idempotency-Key': twice }, body });
    const second = await api(`/v1/projects/${id}/assets`, { method: 'POST', headers: { 'Idempotency-Key': twice }, body });
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
  } else importYaml(folder, `${yaml().trimEnd()}\n  Sheet:\n    tool: ${tool}\n`);
  await box('Sheet').waitFor();
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  await page.mouse.click(1500, 1000);
  await away();
  await page.waitForTimeout(100);

  // A plain imported image keeps its deletion target at rest, with no linear counter/arrow control.
  const atRest = box('source-a').locator('.head button[aria-label="Delete"]');
  check('at_rest_delete_is_always_visible', await atRest.isVisible() && ((await atRest.boundingBox())?.width ?? 0) >= 24, await box('source-a').innerText());
  check('at_rest_image_has_no_linear_version_controls', await box('source-a').getByLabel('Current version', { exact: true }).count() === 0 && await box('source-a').getByRole('button', { name: /^(Previous|Next) version$/ }).count() === 0);
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
  // Held as its words, the chips as names: project.db's value, whatever style the export writes it in.
  const [[stored] = []] = rowsOf(folder, "SELECT f.value FROM fields f JOIN boxes b ON f.box = b.key WHERE b.name = 'Sheet' AND f.field = 'instructions'");
  check('file_holds_plain_text', said.includes('A sheet of <source-a> and <source-b>') && JSON.parse(stored ?? 'null')?.trim() === 'A sheet of <source-a> and <source-b>', { said, stored });
  check('drop_is_no_move', (await rows())['source-b'].x === rowsBefore['source-b'].x && (await rows())['source-b'].y === rowsBefore['source-b'].y, await rows());
  await until('it went back', () => box('source-b').locator('.nm').boundingBox(), (now) => Math.abs(now.x - from.x) < 3 && Math.abs(now.y - from.y) < 3);
  await open('Sheet');
  await until('chips', () => sheetText.locator('.chip').count(), (n) => n === 2);
  // A mention reads as it's written, in its brackets, its first letter up; the file keeps it as typed.
  const chipWords = await sheetText.locator('.chip').allInnerTexts();
  check('chips_read_as_written', chipWords.join() === '<Source-a>,<Source-b>', chipWords);
  // Behind its words, faded, the pictures they mention, side by side in a 2×2.
  const ghosts = await until('the pictures it mentions, behind', () => box('Sheet').locator('.in-frame .ghosts img').evaluateAll((all) => all.map((one) => one.getBoundingClientRect().width)), (widths) => widths.length === 2 && widths.every((width) => width > 8), 5000).catch((why) => String(why));
  check('mentions_ghosted_behind_its_words', Array.isArray(ghosts) && (await box('Sheet').locator('.mention-thumb').count()) === 0, ghosts);
  check('wire_reads_what_it_mentions', (await of('Sheet')).reads?.map((read) => read.name).sort().join() === 'source-a,source-b' && (await of('Sheet')).reads[0].at.includes('instructions'), (await of('Sheet')).reads);
  check('wire_fields_flat', (await of('Sheet')).fields?.instructions?.includes('<source-a>') && !('tool' in (await of('Sheet')).fields), (await of('Sheet')).fields);
  check('wire_etag', /^"[0-9a-f]{64}"$/.test((await of('Sheet')).etag ?? ''), (await of('Sheet')).etag);

  // No box lies on another. Dropped on one that can't take it, it stays where it was let go and the other is pushed aside (Paul, 11-06: "the placement
  // position of the new object should be primary, and then other objects should move around it to make room"), and both are saved.
  // Words can't take a picture: a picture box would take it as its version (27c5db3f5).
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
  // Its middle, on the screen: a little smaller, it keeps it.
  const middleOf = async (name) => {
    const { x, y, width, height } = await box(name).boundingBox();
    return { x: Math.round(x + width / 2), y: Math.round(y + height / 2) };
  };
  const letGoAt = await middleOf('source-b');
  await page.mouse.up();
  await until('they came apart', () => apart('kept_words', 'source-b'), Boolean);
  check('a_box_cant_lie_on_a_box', true);
  const ended = await until('the other pushed aside, saved', rows, (now) => now.kept_words.x !== stood.kept_words.x || now.kept_words.y !== stood.kept_words.y).catch(rows);
  await page.waitForTimeout(400);
  const endedAt = await middleOf('source-b');
  check('the_box_let_go_stays', Math.abs(endedAt.x - letGoAt.x) <= 2 && Math.abs(endedAt.y - letGoAt.y) <= 2, { letGoAt, endedAt });
  check('the_other_box_moves_aside', ended.kept_words.x !== stood.kept_words.x || ended.kept_words.y !== stood.kept_words.y, { stood, ended });
  await until('where it was let go, saved', rows, (now) => now['source-b'].x !== stood['source-b'].x || now['source-b'].y !== stood['source-b'].y);

  // Another writer's `at`, on a box, settles the same way; so does a file that arrives with two on one spot, as a converted project does, with no input changing.
  const held = (await rows())['source-a'];
  projectJson(folder, 'PATCH', '/canvas', { cards: { 'source-b': { x: held.x, y: held.y } } });
  await page.waitForTimeout(300);
  await until('an `at` on a box comes off it', () => apart('source-a', 'source-b'), Boolean);
  check('an_at_settles', true);
  const asItStood = (await state()).assets.map(({ name, inputs, standing }) => [name, inputs, standing]);
  // Another writer's row, onto it: no window had it to settle.
  projectJson(folder, 'PATCH', '/canvas', { cards: { 'source-b': { x: held.x, y: held.y } } });
  await page.reload();
  await box('source-a').waitFor();
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  await until('what lay on another came apart on opening', () => apart('source-a', 'source-b'), Boolean);
  await until('and that was saved', rows, (now) => now['source-b'].x !== held.x || now['source-b'].y !== held.y);
  check('coming_apart_changes_no_input', JSON.stringify((await state()).assets.map(({ name, inputs, standing }) => [name, inputs, standing])) === JSON.stringify(asItStood));
  await page.evaluate(() => { window.notReloaded = true; });

  // Stored image descriptions remain data, but no image description editor is shown.
  await api(`/v1/projects/${id}/assets/source-a`, { method: 'PATCH', body: JSON.stringify({ fields: { description: 'A sea <b>bold</b> <img src=x onerror=window.pwned=1> and more' } }) });
  await open('source-a');
  check('image_description_editor_is_removed', await box('source-a').locator('.desc-field, [aria-label="Description"]').count() === 0, await box('source-a').innerText());
  check('stored_description_is_not_html', await page.evaluate(() => window.pwned === undefined && !document.querySelector('.card img[src="x"]')));
  check('description_data_is_preserved', (await of('source-a')).fields?.description?.includes('A sea'), (await of('source-a')).fields);
  check('a_description_makes_it_no_tool', !(await of('source-a')).tool && (await of('source-a')).sha256?.startsWith('sha256:'), await of('source-a'));

  // A click opens a picture's box at once, with no wait for a second (Paul, 11-06, as Figma does).
  await away();
  const pressed = await box('source-a').locator('.media').boundingBox();
  await page.mouse.click(pressed.x + pressed.width / 2, pressed.y + pressed.height / 2);
  check('a_click_opens_at_once', await page.evaluate(() => Boolean(document.querySelector('.card.here[data-asset="source-a"]'))));
  await away();

  // A box from what it holds, for a tool no list knows: what says the most is its prompt, whatever the alphabet says of `style` and `summary`, its beats shown, and what else it holds behind More, with no line of names beside it. Each writes one field and moves nothing else.
  importYaml(folder, `${yaml().trimEnd()}\n  knobs:\n    # Stays.\n    summary: |\n      <source-b> walks.\n    tool: no-such-package-here/shoot  # how\n    aspect_ratio: "3:2"\n    megapixels: 1\n    style: wash\n    turbo: true\n    beats:\n      - duration: 4\n        description: |\n          First.\n        camera: low\n`);
  // It reads source-b, so it's placed beside it, which may be out of view of source-a open: placed, the canvas at rest and fitted has it in view (as versions.js's moving_words).
  await until('knobs placed', rows, (now) => now.knobs);
  await away();
  await page.keyboard.press('f');
  await box('knobs').waitFor();
  await open('knobs');
  const knobs = box('knobs');
  const field = async (re) => (linesOf(yaml(), 'knobs') ?? '').match(re)?.[1];
  check('summary_is_its_prompt', (await knobs.locator('.in-frame .mention-text').getAttribute('data-field')) === 'summary');
  // Its description has a field of its own, always open.
  check('words_and_beats_shown_without_an_asset_description', (await knobs.locator('.mention-text:not([data-field=description])').count()) === 2 && (await knobs.locator('.desc-field').count()) === 0 && (await knobs.locator('.beat').count()) === 1 && (await knobs.locator('.edit-more, input[aria-label=Megapixels]').count()) === 0, await knobs.innerText());
  check('no_line_beside_more', (await knobs.locator('.more-marks').count()) === 0 && (await knobs.locator('.more-toggle').innerText()).trim() === 'More', await knobs.locator('.more-toggle').innerText());
  await more('knobs');
  check('what_it_holds_behind_more', (await knobs.locator('input[aria-label=Megapixels]').inputValue()) === '1' && (await knobs.locator('label:has-text("turbo") input').isChecked()), await knobs.locator('.edit-more').innerText());
  check('its_style_behind_more', (await knobs.locator('.mention-text[data-field=style]').textContent()).includes('wash'));
  check('no_shape_field', (await knobs.locator('select[aria-label="Shape"], [data-field=aspect_ratio], [aria-label="Aspect ratio"]').count()) === 0, await knobs.innerText());
  await knobs.locator('input[aria-label=Megapixels]').focus();
  await page.keyboard.press('ArrowRight');
  await until('the slider written', () => field(/megapixels: (\S+)/), (value) => value === '2');
  // Nothing has moved the pointer since it opened More. On Windows (41917007085) the box closed here by itself: this says when, and what moved.
  await page.waitForTimeout(300);
  check('a_box_in_use_stays_open', (await knobs.locator('.edit-more').count()) === 1, await boxLog(page));
  check('the_canvas_cannot_scroll', (await page.locator('.react-flow').evaluate((el) => getComputedStyle(el).overflow)) === 'clip');
  await more('knobs');
  await knobs.locator('label:has-text("turbo") input').click();
  await until('the toggle written', () => field(/turbo: (\S+)/), (value) => value === 'false');
  check('text_stays_block', (linesOf(yaml(), 'knobs') ?? '').includes('summary: |'), linesOf(yaml(), 'knobs'));

  // Its shape is the box's own, with no field for it: with no picture, its corner pulls a new one, at rest or open, which writes what it makes; open, the
  // shape set has an × back to Auto. Width is only layout.
  await away();
  check('its_shape_by_its_corner', (await knobs.locator('.ratio-rest').innerText()) === '3:2', await knobs.locator('.ratio-rest').innerText());
  const widthBefore = (await rows()).knobs.w;
  const pull = async (dx, dy) => {
    // Firefox's driver drops a release past the window's edge, and fitted, knobs can be the box at it: the canvas pans first, so the pull ends inside.
    const { width, height } = page.viewportSize();
    const at = await knobs.locator('.corner').boundingBox();
    const [overX, overY] = [Math.max(0, at.x + 6 + dx - (width - 40)), Math.max(0, at.y + 6 + dy - (height - 40))];
    if (overX || overY) {
      await page.locator('.react-flow__pane').dispatchEvent('wheel', { deltaX: overX, deltaY: overY });
      await settled(page);
      await knobs.locator('.nm').hover();
    }
    const corner = await knobs.locator('.corner').boundingBox();
    await page.mouse.move(corner.x + 6, corner.y + 6);
    await page.mouse.down();
    await page.mouse.move(corner.x + 6 + dx, corner.y + 6 + dy, { steps: 6 });
    await page.mouse.up();
  };
  await knobs.locator('.nm').hover();
  await pull(60, 0);
  await until('the width written', rows, (now) => now.knobs.w && now.knobs.w !== widthBefore);
  // Pulled wider, it takes the nearest shape its tool makes (2a50453b1), and its width with it.
  const wider = await until('its shape written', () => field(/aspect_ratio: (\S+)/), (value) => value && value !== '"3:2"').catch(() => field(/aspect_ratio: (\S+)/));
  check('corner_takes_its_shape', wider && wider !== '"3:2"', wider);
  seen.width_before_reshape = widthBefore;
  await open('knobs');
  await pull(-40, 120);
  const taller = await until('reshaped open', () => field(/aspect_ratio: (\S+)/), (value) => value !== wider).catch(() => field(/aspect_ratio: (\S+)/));
  check('reshapes_while_open', taller && taller !== wider && (await page.locator('.card.here[data-asset="knobs"]').count()) === 1, { wider, taller });
  check('its_shape_says_so_open', (await knobs.locator('.ratio-rest').innerText()).trim() === taller?.replaceAll('"', ''), await knobs.locator('.ratio-rest').innerText());
  await knobs.locator('.ratio-rest .ratio-clear').click();
  await until('back to Auto', () => Promise.resolve(linesOf(yaml(), 'knobs') ?? ''), (lines) => !lines.includes('aspect_ratio'));
  check('its_x_is_auto', true);

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

  // Another's edit merges field by field (cl-904): of another asset, or another field, it applies; of the words being typed, the page's unsent
  // words stand and are saved, with nothing asked.
  await open('Sheet');
  await sheetText.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' mine');
  importYaml(folder, yaml().replace('      Written\n', '      Rewritten\n'));
  // Read again: the canvas is drawn in on Sheet, so what else is on it may not be drawn.
  await until('its etag read again', async () => (await of('kept_words'))?.fields?.description ?? '', (text) => text.includes('Rewritten'));
  await until('another\'s edit is no conflict', () => Promise.resolve(linesOf(yaml(), 'Sheet')), (lines) => lines?.includes('mine'));
  // Its write waits until theirs is in, as a slower network's would.
  let theirsIn;
  const going = new Promise((done) => (theirsIn = done));
  const late = (call) => (call.request().method() === 'PATCH' ? going.then(() => call.fallback().catch(() => {})) : call.fallback());
  await page.route('**/v1/projects/*/assets/Sheet', late);
  await sheetText.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' again');
  importYaml(folder, yaml().replace(' mine', ' theirs'));
  check('theirs_in_the_file', linesOf(yaml(), 'Sheet').includes(' theirs'), linesOf(yaml(), 'Sheet'));
  await page.waitForTimeout(1000);
  await shot('unsent-words-stand');
  check('unsent_words_stand', (await sheetText.innerText()).includes('mine again'), await sheetText.innerText());
  theirsIn();
  await page.unroute('**/v1/projects/*/assets/Sheet', late);
  await until('mine saved over theirs', () => Promise.resolve(linesOf(yaml(), 'Sheet')), (lines) => lines?.includes(' mine again'));
  check('nothing_asked', (await box('Sheet').locator('button:has-text("Keep mine")').count()) === 0);

  // A caret never rests in a name, where what's typed goes nowhere: a click on one puts it just after, and what's typed there is saved there.
  await open('Sheet');
  await sheetText.locator('.chip').first().click();
  const caret = () => page.evaluate(() => {
    const box = document.activeElement;
    const at = getSelection();
    if (!box?.matches('.mention-text') || !at?.rangeCount || !at.isCollapsed) return null;
    const { startContainer: node, startOffset: offset } = at.getRangeAt(0);
    const before = node === box ? box.childNodes[offset - 1] : node.nodeType === Node.TEXT_NODE && offset === 0 ? node.previousSibling : null;
    return before?.dataset?.mention ?? `${node.nodeName}@${offset}`;
  });
  // Each step waits on what it needs, and one that never comes is a failed check with what was there, so the cell goes on.
  const typedAfter = {};
  try {
    typedAfter.caret = await until('the caret just after the name', caret, (after) => after === 'source-a');
    await page.keyboard.type(' typed');
    typedAfter.box = await until('typed after the name, in the box', () => sheetText.innerText(), (text) => /source-a>?\s+typed/i.test(text));
    await page.mouse.click(1500, 1000);
    typedAfter.file = await until('typed after the name, in the file', async () => ({ lines: linesOf(yaml(), 'Sheet') }), (now) => now.lines?.includes('<source-a> typed'));
  } catch (error) {
    typedAfter.error = String(error.message ?? error).slice(0, 400);
  }
  check('typed_after_a_name', Boolean(typedAfter.file?.lines?.includes('<source-a> typed')), typedAfter);
  await away();

  // Right click: download, star, rename, delete, and what it's made with.
  await away();
  await box('source-b').locator('.nm').click({ button: 'right' });
  await page.waitForSelector('.asset-menu');
  check('file_menu', (await page.locator('.asset-menu > button').allTextContents()).join() === 'Download,Star,Rename,Delete', await page.locator('.asset-menu > button').allTextContents());
  await page.keyboard.press('Escape');
  await open('Sheet');
  await box('Sheet').getByRole('button', { name: 'Rename node', exact: true }).click({ button: 'right' });
  await page.waitForSelector('.asset-menu');
  const items = await page.locator('.asset-menu > button').allTextContents();
  check('box_menu', ['Change the tool', 'Rename', 'Delete'].every((item) => items.includes(item)), items);
  await page.keyboard.press('Escape');

  // A fresh box, from the list: one prompt, or a prompt and one beat that's in the file from the start. All else is behind More, which stays as it's left, says what's set there, and saves what `cozy status` reads.
  const statusOf = (name) => projectJson(folder).assets.find((held) => held.name === name);
  if (local) {
    const fixture = join(__dirname, '..', 'project', 'tool.py');
    importYaml(folder, `${yaml().trimEnd()}\n  fixture:\n    tool: ${fixture}/told\n`);
    await box('fixture').waitFor();
    // Read with this machine's Runtime, as a run reads it: PyPI's can't, and this may be the first run of it here.
    execFileSync(COZY, ['--json', 'run', `${fixture}/still`, 'prompt=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
    const wire = await until('its functions listed', async () => (await api(`/v1/tools?project=${id}`)).body.tools.filter((held) => held.local && held.id.startsWith(fixture)), (held) => held.length > 0, FIRST_RUN_MS);
    const implementationOf = (name) => wire.find((held) => held.function === name);
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
      const point = await openCanvas(page);
      const fresh = await dragOut(await implementationRow(page, 'image'), point.x, point.y);
      await chooseModel(page, fresh.name, implementationOf(name).id, 'image');
      await until('the selected implementation lands', () => of(fresh.name), (held) => held.tool === implementationOf(name).id);
      await box(fresh.name).getByRole('button', { name: 'Rename node', exact: true }).click();
      const naming = box(fresh.name).locator('.focus-heading input[aria-label="Name"]');
      await naming.fill(`${name}_1`);
      await naming.press('Enter');
      await box(`${name}_1`).waitFor();
      await open(`${name}_1`);
    };

    await place('still');
    const still = box('still_1');
    await until('one prompt', () => still.locator('.mention-text:not([data-field=description])').count(), (n) => n === 1);
    check('fresh_box_is_its_prompt', (await still.locator('.in-frame .mention-text[data-field=instructions]').count()) === 1 && (await still.locator('.here-all input, .here-all select, .beat, .edit-more').evaluateAll((elements) => elements.filter((element) => !element.closest('[data-tool-settings]')).length)) === 0, await still.innerText());
    check('fresh_box_marks_nothing', (await still.locator('.more-marks').count()) === 0 && (await still.locator('.more-toggle').innerText()).trim() === 'More', await still.innerText());
    await shot('fresh-prompt');
    await more('still_1');
    await still.locator('input[aria-label=Count]').waitFor();
    await still.locator('[role=radiogroup][aria-label=Style] [role=radio]:text-is("noise")').click();
    await until('the choice in the status', async () => statusOf('still_1').fields?.style, (style) => style === 'noise');
    await still.locator('.more-toggle').click();
    await still.locator('.edit-more').waitFor({ state: 'detached' });
    // Set, it's behind More with no word of it beside: opened, it's there.
    check('a_set_value_has_no_line', (await still.locator('.more-marks').count()) === 0 && (await still.locator('.more-toggle').innerText()).trim() === 'More', await still.locator('.more-toggle').innerText());
    await shot('fresh-marked');
    await open('still_1');
    check('shut_stays_shut', (await still.locator('.edit-more').count()) === 0);
    await more('still_1');
    check('a_set_value_behind_more', (await still.locator('[role=radiogroup][aria-label=Style] [role=radio][aria-checked=true]').innerText()) === 'noise', await still.locator('.edit-more').innerText());
    await open('still_1');
    check('open_stays_open', (await still.locator('.edit-more').count()) === 1);
    await reload();
    await open('still_1');
    check('open_outlasts_a_reload', (await still.locator('.edit-more').count()) === 1);
    await still.locator('[role=radiogroup][aria-label=Style] [role=radio]:text-is("flat")').click();
    await until('its default in the status', async () => statusOf('still_1').fields?.style, (style) => style === 'flat');
    await still.locator('.more-toggle').click();
    await still.locator('.edit-more').waitFor({ state: 'detached' });
    check('its_default_marks_nothing', (await still.locator('.more-marks').count()) === 0, await still.innerText());

    await place('told');
    const told = box('told_1');
    await until('a prompt and a beat', () => told.locator('.beat').count(), (n) => n === 1);
    check('fresh_shot_is_its_prompt_and_a_beat', (await told.locator('.in-frame .mention-text[data-field=instructions]').count()) === 1 && (await told.locator('.mention-text:not([data-field=description])').count()) === 2 && (await told.locator('input[type=range]').count()) === 1 && (await told.locator('.edit-more').count()) === 0, await told.innerText());
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
      // Render is with its words: on a take, its + goes to them.
      await (await renderOf(page, name)).click();
      await until(`${name}'s ${count}`, () => versions(name), (n) => n === count, FIRST_RUN_MS);
    };
    await render('Sheet', 1);
    await open('Sheet');
    await until('its picture, unreloaded', () => box('Sheet').locator('.media img').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n > 0);
    check('rendered_with_no_reload', await page.evaluate(() => window.notReloaded === true));
    await render('Sheet', 2);
    await open('Sheet');
    await until('its two retained previews', () => box('Sheet').locator('.image-history.here-side .take[data-take]').count(), (count) => count === 2);

    // Preview is local; double-click approval is the separate persistent action.
    const beforePreview = (await of('Sheet')).version;
    await previewVersion(page, 'Sheet', 1);
    check('thumbnail_preview_does_not_approve', (await of('Sheet')).version === beforePreview);
    await approveVersion(page, 'Sheet', 1);
    await until('first version selected in the project', () => of('Sheet'), (held) => held.version === 1);
    check('selected_version_is_persistent', await box('Sheet').locator('.image-history .take.on[data-take]').getAttribute('data-take') === '1');
    check('images_keep_the_original_right_previews', await box('Sheet').locator('.image-history.here-side .take[data-take]').count() === 2);
    check('image_view_has_no_description', await box('Sheet').locator('.desc-field').count() === 0);
    await openBox(page, 'Sheet');
    const viewButton = box('Sheet').getByRole('button', { name: 'View image', exact: true });
    if (await viewButton.count()) await viewButton.click();
    check('image_has_an_explicit_edit_action', await box('Sheet').getByRole('button', { name: 'Edit image', exact: true }).count() === 1);
    const selectedBefore = (await of('Sheet')).version;
    await renderOf(page, 'Sheet');
    check('editing_keeps_current_selection', (await of('Sheet')).version === selectedBefore && await box('Sheet').locator('.in-frame').count() === 1);
    await previewVersion(page, 'Sheet', 1);

    // Stored version deletion and Undo remain available independently of the removed strip.
    const asked = await versions('Sheet');
    const removed = await api(`/v1/projects/${id}/assets/Sheet/versions/2`, { method: 'DELETE' });
    check('version_delete_is_accepted', removed.status === 200);
    await until('version removed', () => versions('Sheet'), (n) => n === asked - 1);
    const history = (await api(`/v1/projects/${id}/history`)).body;
    const deletion = history.entries.find((entry) => !entry.undone && entry.names?.some((name) => name.toLowerCase() === 'sheet'));
    await api(`/v1/projects/${id}/history/undo`, { method: 'POST', body: JSON.stringify({ entry: deletion.id }) });
    await until('version restored', () => versions('Sheet'), (n) => n === asked);
    check('version_undo_retains_both_states', (await of('Sheet')).prefixes.length === asked);

    // Selecting a state keeps every version on this asset and creates no extracted version-node.
    await api(`/v1/projects/${id}/assets/Sheet`, { method: 'PATCH', body: JSON.stringify({ fields: { description: 'The sheet of two.' } }) });
    await until('described', () => of('Sheet'), (held) => held.fields?.description?.includes('sheet of two'));
    const filesBefore = readdirSync(join(folder, 'files')).length;
    await open('Sheet');
    // The drag carries what the page holds, and that came by the stream, a moment behind the answer above.
    check('image_description_stays_hidden', await box('Sheet').locator('.desc-field').count() === 0);
    await previewVersion(page, 'Sheet', 2);
    const selected = await of('Sheet');
    check('preview_keeps_approval_and_versions_on_one_asset', selected.version === 1 && selected.versions === asked && !(await of('Sheet_2')) && await box('Sheet').locator('.image-history .take.on[data-take]').getAttribute('data-take') === '2', selected);
    check('selection_creates_no_new_bytes', readdirSync(join(folder, 'files')).length === filesBefore);
    await shot('versions');

    // Something else to go on while one fails.
    await away();
    await dropBox('Other', 1100, 600);
    await render('Other', 1);

    // What a package shows beside its first text: both on the box as it opens, empty reading N/A, and what's typed is the file's, as the API reads it. An empty one is written nowhere.
    const shaped = join(__dirname, '..', 'project', 'shaped.py');
    // Read with this machine's Runtime, as tool.py is above.
    execFileSync(COZY, ['--json', 'run', `${shaped}/picture`, 'prompt=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: FIRST_RUN_MS });
    importYaml(folder, `${yaml().trimEnd()}\n  sounded:\n    tool: ${shaped}/shot\n`);
    // Render kept Other open, and drawn in on a box the canvas draws only what's near it: a new one is seen at rest, all of it in view.
    await away();
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
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
    await until('the soundscape as the API reads it', async () => statusOf('sounded').fields?.soundscape, (said) => said === 'Rain on steel.');
    check('an_empty_one_isnt_written', !('music' in statusOf('sounded').fields), statusOf('sounded').fields);
    await page.keyboard.type('Low strings.');
    await sounded.locator('.mention-text[data-field=music]').blur();
    await until('the music as the API reads it', async () => statusOf('sounded').fields?.music, (said) => said === 'Low strings.');
    await shot('sounded');

    // What a box mentions goes to the list of what's mentioned. A function's optional frames are the file's to set: filled from mentions, the first
    // picture took the canvas over the shot's aspect_ratio, and the second forced how it ends.
    importYaml(folder, `${yaml().trimEnd()}\n  mentioning:\n    tool: ${shaped}/shot\n    summary: <source-a> meets <source-b>.\n    aspect_ratio: "16:9"\n`);
    // Drawn in on a box, the canvas draws only what's near it: a new one is seen at rest, all of it in view.
    await away();
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('mentioning').waitFor();
    await projectAction(folder, 'render', 'mentioning', '--on', 'local');
    // A project's status counts an asset's versions; one asset's status lists them.
    const madeBy = projectJson(folder, 'GET', '/assets/mentioning').versions.at(-1).runs.at(-1);
    const sent = JSON.parse(execFileSync(COZY, ['--json', '--full', 'run', 'watch', String(madeBy), '--once'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] })).request.input;
    check('a_mention_is_no_keyframe', !('first_frame' in sent) && !('last_frame' in sent), sent);
    check('a_mention_is_a_reference', sent.summary?.references?.length === 2 && sent.aspect_ratio === '16:9', sent);

    // Text: a name for some words. It's on the list before any function, its box is only its words, with nothing to render, and a mention of it is a chip like any other.
    const textRow = page.locator('aside.tools [data-palette="nodes"] li.tool[data-node="text"]');
    check('text_is_on_the_list', (await textRow.count()) === 1, await page.locator('aside.tools').innerText());
    await away();
    const beforeTextClick = (await state()).assets.map((asset) => asset.name).sort();
    await textRow.click();
    await page.waitForTimeout(200);
    check('clicking_text_does_not_create', JSON.stringify((await state()).assets.map((asset) => asset.name).sort()) === JSON.stringify(beforeTextClick));
    await carry(page, textRow, await openCanvas(page));
    await box('text').waitFor();
    await open('text');
    const wording = box('text');
    await wording.locator('.mention-text[data-field=text]').waitFor();
    await wording.locator('.mention-text[data-field=text]').click();
    await page.keyboard.type('Painted by hand.');
    await wording.locator('.mention-text[data-field=text]').blur();
    await until('its words as the API reads them', async () => statusOf('text').fields?.text, (said) => said === 'Painted by hand.');
    check('text_has_no_render', (await wording.locator('button:text-is("Render"), .takes, button:text-is("Approve")').count()) === 0, await wording.innerText());
    check('text_has_a_kind', statusOf('text').kind === 'text', statusOf('text'));

    // Caption: what the package says lands in the field as an edit, and nothing's written until it's kept. Local code a project names can answer.
    const asker = join(__dirname, '..', 'project', 'tool.py');
    importYaml(folder, `${yaml().trimEnd()}\n  asker:\n    tool: ${asker}/told\n`);
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
    // Image view keeps its single selected picture; editing is explicit and has no description box.
    await openBox(page, 'Other');
    const viewOther = box('Other').getByRole('button', { name: 'View image', exact: true });
    if (await viewOther.count()) await viewOther.click();
    check('image_view_has_edit_without_description', await box('Other').getByRole('button', { name: 'Edit image', exact: true }).count() === 1 && await box('Other').locator('.desc-field').count() === 0);
    await renderOf(page, 'Other');
    await box('Other').locator('.in-frame .mention-text[data-field=instructions]').waitFor();
    check('image_edit_has_render_without_description', await box('Other').locator('button[aria-label=Render]').count() === 1 && await box('Other').locator('.desc-field').count() === 0);
    await api(`/v1/projects/${id}/assets/asker`, { method: 'DELETE' });
    await until('asker gone', () => box('asker').count(), (n) => n === 0);

    // X on what it was made from. What's approved shows nothing wrong, no dialog stops it, and Undo waits.
    const [heldFile, heldTakes] = [(await of('Sheet')).sha256, await versions('Sheet')];
    const filesThen = readdirSync(join(folder, 'files')).length;
    const deleting = page.waitForResponse((answer) => answer.request().method() === 'DELETE' && answer.url().endsWith('/assets/source-a'));
    await deleteBox('source-a');
    const edit = (await deleting).request().headers()['cozy-edit'];
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
    await renderOf(page, 'Sheet');
    await box('Sheet').locator('.chip.missing:has-text("source-a")').waitFor();
    check('missing_chip_on_edit', (await box('Sheet').locator('.chip.missing').innerText()) === '<Source-a>', await box('Sheet').locator('.chip.missing').innerText());
    await shot('missing');

    // Rendering it again fails, naming what's gone, and the rest goes on.
    await box('Sheet').locator('button[aria-label=Render]').click();
    const failed = await until('it says why', () => of('Sheet'), (held) => held.failure);
    check('fails_naming_it', failed.failure.code === 'input_missing' && failed.failure.message.includes('<source-a> is no longer in the project'), failed.failure);
    // The failure is a toast with its next steps, the box open and its picture standing, and no line on the box says it.
    await until('a toast says why', () => page.locator('.note .line').allTextContents(), (lines) => lines.some((line) => line.includes("Sheet didn't render") && line.includes('no longer in the project')));
    const toastSteps = page.locator('.note:has-text("Sheet didn\'t render")').first();
    check('its_next_steps_as_text', (await toastSteps.locator('.remedy, .copyable').count()) > 0, await toastSteps.innerText());
    check('render_keeps_it_open_saying_nothing', (await page.locator('.card.here[data-asset="Sheet"]').count()) === 1 && (await box('Sheet').locator('.status').count()) === 0, await box('Sheet').innerText());
    // The same failure again is said again, naming its box once.
    await page.locator('.note [aria-label=Dismiss]').evaluateAll((all) => all.forEach((one) => one.click()));
    // Render looked at the take it asked for, which never came: + is its words again, as they were.
    await (await renderOf(page, 'Sheet')).click();
    const again = await until('it says so again', () => page.locator('.note .line').allTextContents(), (lines) => lines.some((line) => line.includes("Sheet didn't render")));
    check('failed_again_said_again', again.every((line) => !line.includes('Sheet: ')), again);
    check('failed_runs_nothing', (await versions('Sheet')) === heldTakes && (await of('Sheet')).sha256 === heldFile, { takes: await versions('Sheet'), heldTakes });
    await render('Other', 2);
    check('the_rest_goes_on', true);
    await open('Sheet');
    const viewSheet = box('Sheet').getByRole('button', { name: 'View image', exact: true });
    if (await viewSheet.count()) await viewSheet.click();
    await away();
    await page.waitForTimeout(200);
    check('back_on_its_file_nothing_wrong', (await box('Sheet').locator('.dot.s-failed').count()) === 0);

    // Undo: back with its row, and nothing is missing. Its note has gone by now (10 s, Paul 11-05), so this is what its Undo sends; the button's own below.
    const back = await api(`/v1/projects/${id}/history/undo`, { method: 'POST', body: JSON.stringify({ edit }) });
    check('put_back', Boolean(edit) && back.status === 200 && back.body?.entries?.length === 1, { edit, back });
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
    // The box is back as it's asked for, ahead of the file and its versions: then all its versions, in the project and on the page.
    const restored = await until('its versions back', () => versions('Other'), (n) => n === takes).catch(() => versions('Other'));
    await open('Other');
    const shown = await box('Other').locator('.image-history.here-side .take[data-take]').count();
    check('undo_brings_its_versions', restored === takes && shown === takes, { takes, restored, shown });
    await away();

    // Rendering an edit appends another version and selects it without discarding earlier states.
    const beforeAppend = await versions('Other');
    await render('Other', beforeAppend + 1);
    const appended = await of('Other');
    check('render_appends_and_selects_new_state', appended.version === appended.prefixes.at(-1).number && appended.prefixes.length === beforeAppend + 1, appended);

    // Pick up from here: scrubbed to a frame, its words as they were, and the function told to keep up to there, for that render alone.
    // The dropped clip is 11 frames: frame 6 is one it has.
    // One file dropped asks its name first: Enter takes the one offered.
    await away();
    await drop(page, [{ name: 'Look.mp4', type: 'video/mp4', bytes: MP4 }], 400, 900);
    await page.waitForSelector('.nm-field');
    await page.keyboard.press('Enter');
    await box('look').waitFor();
    await until('look in the file', () => Promise.resolve(linesOf(yaml(), 'look')), Boolean);
    importYaml(folder, `${yaml().trimEnd()}\n  Clip:\n    summary: |\n      On <look>.\n    tool: ${shots}/shoot\n`);
    // By the project: it can land past the edge of the view, and what's off it isn't drawn.
    await until('Clip in the project', () => of('Clip'), Boolean);
    await render('Clip', 1);
    await until('its length measured', () => of('Clip'), (held) => held.prefixes?.[0]?.main?.duration_ms > 0);
    await open('Clip');
    // Rendered from its words, it's still on them: its take, pressed, is what's kept from.
    await box('Clip').locator('.take[aria-label="take 1"]').click();
    const clipFile = linesOf(yaml(), 'Clip');
    await more('Clip');
    await box('Clip').locator('.from-here .keeps').fill('6');
    await box('Clip').locator('button:text-is("Pick up from here")').click();
    await until('a new take', () => versions('Clip'), (n) => n === 2, FIRST_RUN_MS);
    const given = (await api(`/v1/projects/${id}/assets/Clip`)).body.versions.at(-1).files.given.path;
    // Listed as its run ends, it lands in files/ a moment later (Firefox, 42110239536).
    await until('what it was given, in files/', () => Promise.resolve(existsSync(join(folder, given))), Boolean);
    const told = JSON.parse(readFileSync(join(folder, given), 'utf8'));
    check('kept_up_to_there', told.pickup?.out_frame === 6, told);
    check('words_as_they_were', linesOf(yaml(), 'Clip') === clipFile && !yaml().includes('pickup'), linesOf(yaml(), 'Clip'));
    await open('Clip');
    await box('Clip').locator('.take[aria-label="take 2"]').waitFor();
    check('takes_are_called_takes', true);
    await shot('takes');

    // The terminal sees the same project.
    const status = projectJson(folder);
    check('api_sees_it', ['Sheet', 'Other', 'source-a', 'source-b', 'Clip'].every((name) => status.assets.some((held) => held.name === name)), status.assets.map((held) => held.name));
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

  // Its words and `|` outlast all of this, as list_keeps_words reads them; its comments went at the import (cl-803).
  check('words_and_text_kept', /\n  kept_words:\n    description: \|\n      Rewritten\n      by hand\.\n/.test(yaml()), yaml());
  const hashes = [...yaml().matchAll(/approved: "?([0-9a-f]{12})/g)].map((m) => m[1]);
  const shown = await page.evaluate(() => document.body.innerText + [...document.querySelectorAll('[title],[aria-label]')].map((node) => node.title + node.ariaLabel).join(' '));
  check('hash_not_shown', hashes.every((hash) => !shown.includes(hash)), hashes.filter((hash) => shown.includes(hash)));
  check('sidebar_still_only_nodes_and_tools', JSON.stringify(await page.locator('.tools h4').allTextContents()) === JSON.stringify(['Nodes', 'Tools']) && await page.locator('.tools .browse, .tools .box-shelf-head, .tools .tool-off, .tools input').count() === 0);
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  await new Promise((done) => setTimeout(done, 300));
  // Anything a project's route refused is a fault, however the page took it.
  // Nor a read of what the stream said changed, gone by then: that's how the page hears it's gone.
  const unasked = refused.filter((refusal) => refusal.url.includes('/v1/projects/') && !(refusal.code === 'asset_unknown' && refusal.method === 'GET'));
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
