// Versions by name and a tool on a box (Paul, 11-05), in a real browser on a real Calcifer. `<abr.2>` and `<self.1>` read a version, fixed, and make no cycle; a rename keeps
// them and every comment, and happens at once during a real render, whose take lands under the new name with what reads it reading on; a tool goes on a text box, on a
// picture, and swaps; its X changes only that; a picture joins a box with no tool; a take dragged into text is `<abr.2>`; a version something reads can be deleted, with
// Undo; `reclaim` keeps what a number names.
//   node versions.js <sign-in link from `cozy open --no-open`, or the address of a page that needs none>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: not on this computer's machine, which the rename during a
// render needs, as it renders the free `waits.py`. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { STEP_MS, boxOf, drop, lastLook, launch, lookAt, noStall, png, until } = require('./page');

const fixture = join(__dirname, '..', 'project', 'waits.py');
const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
let forget = async () => {};
let last = () => null;
async function died(why) {
  failures.push(`${during}: ${why}`);
  seen.file = last();
  await lastLook();
  await forget();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

(async () => {
  const link = process.argv[2];
  const base = new URL(link).origin;
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const refused = [];
  seen.refused = refused;
  page.on('response', (answer) => {
    const url = new URL(answer.url());
    if (answer.status() < 400 || !url.pathname.startsWith('/v1/') || answer.request().method() === 'HEAD') return;
    const row = { status: answer.status(), method: answer.request().method(), url: url.pathname, during };
    refused.push(row);
    answer.json().then(({ error }) => Object.assign(row, { code: error?.code, message: error?.message })).catch(() => {});
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await page.goto(link);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = (path, body) => api(path, { method: 'POST', body: JSON.stringify(body) });

  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-versions-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const at = `/v1/projects/${id}`;
  const state = async () => (await api(at)).body;
  const asset = async (name) => (await state()).assets.find((row) => row.name === name);
  const folder = (await state()).path;
  seen.project = { id, folder };
  forget = () => api(at, { method: 'DELETE' }).catch(() => {});
  const yamlAt = join(folder, 'project.yaml');
  const yaml = () => readFileSync(yamlAt, 'utf8');
  last = yaml;
  const lock = () => readFileSync(join(folder, 'project.lock'), 'utf8');
  const box = (name) => boxOf(page, name);
  const toast = (words) => until(`the toast ${words}`, () => page.locator('.notes, .note-list, [role=status]').allInnerTexts().then((all) => all.join(' | ')), (all) => all.includes(words)).then(() => true, (why) => String(why));
  const undoIn = (words) => page.locator(`.note:has-text("${words}") button:has-text("Undo")`).first();

  const pictures = mkdtempSync(join(tmpdir(), 'web-versions-'));
  const picture = (shade) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  const imported = (name, shade) => post(`${at}/imports`, { source: picture(shade), name, exact: true, at: { x: 100 + shade * 4, y: 100 } });
  const joined = (name, shade) => post(`${at}/assets/${name}/versions`, { source: picture(shade) });

  // Mentions, in a file a hand wrote, its comments kept whatever comes.
  const comments = ['# Mine, and kept.', '  # A note on words.', '    # Its own note.', '      Two lines,', '      kept as they are.'];
  await imported('Abr', 10);
  await joined('Abr', 20);
  await joined('Abr', 30);
  await imported('other', 40);
  const head = yaml();
  writeFileSync(yamlAt, `# Mine, and kept.\n${head.trimEnd()}
  # A note on words.
  words:
    text: |
      Two lines,
      kept as they are.
  fixed:
    tool: ${fixture}/wait
    # Its own note.
    instructions: <abr.2> beside <ABR.1>, and <Abr>
  edits:
    tool: ${fixture}/wait
    instructions: <self.1> again
  bare:
    tool: ${fixture}/wait
    instructions: <self> again
  one:
    tool: ${fixture}/wait
    instructions: <two.1>
  two:
    tool: ${fixture}/wait
    instructions: <one.1>
  foo.2:
    text: A name from before.
  says:
    tool: ${fixture}/wait
    instructions: <foo.2>
`);
  await box('fixed').waitFor();
  const fixed = await until('its reads', () => asset('fixed'), (row) => row?.reads?.length === 3);
  check('versions_are_reads', fixed.reads.map((read) => `${read.name}.${read.version ?? ''}`).sort().join() === 'Abr.,Abr.1,Abr.2', fixed.reads);
  check('no_cycle_of_versions', !(await state()).warnings?.some((line) => /made from it/.test(line.message)), (await state()).warnings);
  check('bare_self_warns_with_the_fix', (await state()).warnings?.some((line) => line.name === 'bare' && line.message.includes('<self.1>')), (await state()).warnings);
  check('a_name_like_a_version_stays_a_name', (await asset('says')).fields.instructions.includes('<foo.2>') && !(await asset('says')).missing?.includes('foo.2'));
  const was = (await asset('fixed')).inputs;
  await post(`${at}/approve`, { name: 'Abr', version: '3' });
  const moved = await until('what reads it moves', () => asset('fixed'), (row) => row.inputs !== was);
  const part = (key) => key.slice(8).match(/.{12}/g);
  check('only_the_moving_read_moved', part(was).filter((held) => part(moved.inputs).includes(held)).length === 2, [was, moved.inputs]);

  // Names.
  for (const [to, code] of [['abr.3', 'project_name'], ['self', 'project_name']]) {
    const refused = await post(`${at}/rename`, { from: 'other', to });
    check(`rename_to_${to}_refused`, refused.status === 400 && refused.body.error.code === code && refused.body.error.message.includes('use'), refused);
  }
  const made = await post(`${at}/assets`, { tool: '', name: 'b.7', fields: { text: 'x' } });
  check('a_new_name_ending_so_is_changed', made.body.name === 'b_7', made);
  check('a_typed_name_ending_so_is_refused', (await post(`${at}/assets`, { tool: '', name: 'b.8', fields: { text: 'x' }, exact: true })).status === 400);

  // A rename rewrites what says it, and only that.
  await box('other').waitFor();
  await post(`${at}/rename`, { from: 'Abr', to: 'Ada' });
  await until('the mentions', () => Promise.resolve(yaml()), (text) => text.includes('<Ada.2> beside <Ada.1>, and <Ada>'));
  check('rename_keeps_comments', comments.every((line) => yaml().includes(line)), yaml());
  check('rename_keeps_versions', (await asset('Ada')).versions === 3 && (await asset('Ada')).approved === 3, await asset('Ada'));

  // Deleting a version something reads: it stays read, a render fails clearly, and Undo brings it back.
  await box('Ada').waitFor();
  await box('Ada').locator('.nm').click({ button: 'right' }).catch(() => {});
  await page.keyboard.press('Escape');
  const before = (await asset('fixed')).inputs;
  const gone = await api(`${at}/assets/Ada/versions/2`, { method: 'DELETE' });
  check('delete_names_what_reads_it', JSON.stringify(gone.body.read_by) === '["fixed"]', gone.body);
  check('a_deleted_version_puts_nothing_out_of_date', (await asset('fixed')).inputs === before);
  await until('it says so', () => asset('fixed'), (row) => row.missing?.includes('Ada.2'));
  const reclaimed = await post(`${at}/reclaim`, { yes: true });
  check('reclaim_keeps_a_named_version', reclaimed.status === 200 && lock().includes('discarded') && (await api(`${at}/assets/Ada`)).body.discarded?.includes(2), [reclaimed, (await api(`${at}/assets/Ada`)).body.discarded]);
  const back = await api(`${at}/assets/Ada/versions/2`, { method: 'PUT', body: '{}' });
  check('undo_brings_it_back', back.status === 200 && !(await until('it reads again', () => asset('fixed'), (row) => !row.missing?.includes('Ada.2'))).missing?.length, back);

  // On the page: numbers, and a take into text.
  const shut = async () => {
    await page.keyboard.press('Escape');
    await page.goto(`${base}/#/projects/${id}`);
    await page.waitForTimeout(300);
  };
  const open = async (name) => {
    await page.goto(`${base}/#/projects/${id}/assets/${name}`);
    await page.locator(`.card[data-asset="${name}"]`).first().waitFor();
  };
  await open('Ada');
  const numbers = await until('its numbers', () => page.locator('.card[data-asset="Ada"] .take-no').allInnerTexts(), (held) => held.length === 3, 5000).catch((why) => String(why));
  check('thumbnails_say_their_number', [...numbers].sort().join() === '1,2,3', numbers);
  await shot('numbers');
  await shut();

  // A take dragged into a text says which, and out onto open canvas it's still a box of its own.
  const inView = async (name) => {
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box(name).locator('.card').waitFor();
  };
  await inView('fixed');
  await open('words');
  const text = page.locator('.card[data-asset="words"] .mention-text').first();
  await text.waitFor();
  const file = { sha256: (await asset('Ada')).prefixes.find((held) => held.number === 2).main.sha256, field: 'image', name: 'Ada_2', from: 'Ada', number: 2 };
  const carried = await page.evaluateHandle((held) => {
    const data = new DataTransfer();
    data.setData('application/x-cozy-file', JSON.stringify(held));
    return data;
  }, file);
  await text.dispatchEvent('dragover', { dataTransfer: carried });
  await text.dispatchEvent('drop', { dataTransfer: carried });
  const said = await until('the mention in its text', () => text.locator('[data-mention="Ada.2"]').count(), (n) => n === 1, 2000).catch((why) => String(why));
  check('a_take_into_text_is_its_version', said === 1, await text.innerText());
  await shut();
  await until('it in the file', () => Promise.resolve(yaml()), (held) => held.split('  words:')[1].split('  fixed:')[0].includes('<Ada.2>'));

  // Deleted on the page: the toast says what reads it, and Undo brings it back. A render of what needs it is refused, in words.
  await open('Ada');
  await page.locator('.card[data-asset="Ada"] .take-at:has(.take[aria-label="version 2"])').hover();
  await page.locator('.card[data-asset="Ada"] .take-off[aria-label="Discard version 2"]').click();
  check('delete_toast_names_readers', (await toast('fixed reads it')) === true);
  await until('what reads it marked', () => asset('fixed'), (row) => row.missing?.includes('Ada.2'));
  await undoIn('fixed reads it').click();
  await until('back', () => asset('Ada'), (row) => row.prefixes.some((held) => held.number === 2));
  await shut();

  // A tool on and off. Any image tool the list has: what it does here makes nothing.
  const listed = page.locator('.tools li.tool[data-kind="image"]');
  await listed.first().waitFor({ timeout: 20_000 }).catch(() => {});
  if (await listed.count()) {
    const row = listed.first();
    const tool = { id: await row.getAttribute('data-tool') };
    seen.tool = tool.id;
    const carry = async (name) => {
      await page.keyboard.press('f');
      await page.waitForTimeout(400);
      await row.scrollIntoViewIfNeeded();
      const from = await row.boundingBox();
      await page.mouse.move(from.x + 20, from.y + 8);
      await page.mouse.down();
      await page.mouse.move(from.x + 40, from.y + 20, { steps: 4 });
      const to = await box(name).locator('.card').boundingBox();
      await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
      await page.mouse.up();
    };
    const lines = (name) => yaml().match(new RegExp(`^  ${name}:(.*(?:\n(?:    .*|\\s*))*)`, 'm'))?.[1] ?? '';

    // On a text box its words are the prompt, at once.
    await box('words').waitFor();
    await carry('words');
    await box('words').locator('.add-on').first().waitFor({ timeout: 1000 });
    await until('the file', () => Promise.resolve(lines('words')), (text) => text.includes(`tool: ${tool.id}`) && text.includes('instructions: |'));
    check('words_are_the_prompt', lines('words').includes('Two lines,') && !lines('words').includes('text:'), lines('words'));
    await box('words').locator(`button[aria-label^="Take"]`).first().click();
    await until('the file', () => Promise.resolve(lines('words')), (text) => !text.includes('tool:') && text.includes('text: |'));
    check('x_on_a_box_of_no_pictures_leaves_text', (await asset('words')).kind === 'text' && comments.every((line) => yaml().includes(line)), lines('words'));
    await shot('text-again');

    // On a picture: its first version, approved, and the prompt starts with it.
    await box('other').waitFor();
    await carry('other');
    await until('the file', () => Promise.resolve(lines('other')), (text) => text.includes('<self.1>'));
    const other = await asset('other');
    check('a_picture_is_version_1', other.versions === 1 && other.approved === 1 && other.tool === tool.id, other);
    await shot('tool-on-picture');
    await box('other').locator(`button[aria-label^="Take"]`).first().click();
    await until('the tool gone', () => Promise.resolve(lines('other')), (text) => !text.includes('tool:'));
    const off = await asset('other');
    check('x_on_pictures_changes_only_that', off.versions === 1 && off.approved === 1 && off.fields.instructions === '<self.1>' && off.standing === 'current' && !off.tool, off);
    check('x_says_with_undo', (await toast('Its pictures and words stay')) === true);
    await undoIn('Its pictures and words stay').click();
    await until('the tool back', () => Promise.resolve(lines('other')), (text) => text.includes(`tool: ${tool.id}`));
    check('undo_puts_it_back', (await asset('other')).versions === 1);
    await box('other').locator(`button[aria-label^="Take"]`).first().click();
    await until('the tool gone', () => Promise.resolve(lines('other')), (text) => !text.includes('tool:'));
    await box('other').locator('.card').waitFor();

    // The stack: under the box at its left, the ratio at its right, and what's under it keeps clear.
    const rect = await box('other').locator('.card').boundingBox();
    seen.card = rect;

    // A box with no tool: a picture joins it, and it has its takes and Approve and no Render.
    const { x, y, width, height } = await box('other').locator('.card').boundingBox();
    await drop(page, [{ name: 'more.png', type: 'image/png', bytes: png(55) }], x + width / 2, y + height / 2);
    const grown = await until('the picture joined', () => asset('other'), (row) => row.versions === 2);
    check('a_dropped_picture_joins', grown.prefixes.at(-1).number === 2 && grown.approved === 1, grown);
    check('joining_makes_no_box', !(await state()).assets.some((row) => /^more/.test(row.name)));
    await box('other').locator('.card').click();
    await page.locator('.here-under .take').first().waitFor();
    check('no_render_with_no_tool', (await page.locator('.here-under .run').count()) === 0 && (await page.locator('.here-under .take').count()) === 2 && (await page.locator('.here-under .act:has-text("Unapprove"), .here-under .act:has-text("Approve")').count()) >= 1);
    await shot('no-tool');
    await page.keyboard.press('Escape');

    // A swap across families: what it can't take goes, in words, with Undo. Two tools of different families, where the list has them.
    const onPanel = await page.locator('.tools li.tool').evaluateAll((rows) => rows.map((held) => held.dataset.tool));
    const families = (await api(`/v1/tools?project=${id}`)).body.tools.filter((one) => one.model?.family && one.kind === 'image' && onPanel.includes(one.id));
    const first = families[0];
    const otherFamily = families.find((one) => one.model.family !== first?.model.family);
    seen.swapping = [first?.id, otherFamily?.id];
    if (first && otherFamily) {
      const dragIn = async (one, name) => {
        const line = page.locator(`.tools li.tool[data-tool="${one.id}"]`).first();
        await line.scrollIntoViewIfNeeded();
        const from = await line.boundingBox();
        await page.mouse.move(from.x + 20, from.y + 8);
        await page.mouse.down();
        await page.mouse.move(from.x + 40, from.y + 20, { steps: 4 });
        const to = await box(name).locator('.card').boundingBox();
        await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
        await page.mouse.up();
      };
      writeFileSync(yamlAt, `${yaml().trimEnd()}
  swapped:
    tool: ${first.id}
    instructions: A wave.
    aspect_ratio: "16:9"
    lora:
      - {ref: hf://someone/first-lora, strength: 0.7}
      - {ref: hf://someone/second-lora, strength: 0.5}
`);
      await box('swapped').waitFor();
      await box('swapped').locator('.add-on').nth(2).waitFor();
      await page.keyboard.press('f');
      await page.waitForTimeout(400);
      const card = await box('swapped').locator('.card').boundingBox();
      const list = await box('swapped').locator('.add-ons').boundingBox();
      check('stack_is_under_at_its_left', list.y >= card.y + card.height - 1 && Math.abs(list.x - card.x) < 3 && (await box('swapped').locator('.add-on').count()) === 3, { card, list });
      await shot('stack');
      await dragIn(otherFamily, 'swapped');
      check('a_swap_names_what_it_drops', (await toast('first-lora')) === true);
      await until('the file', () => Promise.resolve(lines('swapped')), (held) => held.includes(`tool: ${otherFamily.id}`) && !held.includes('lora'));
      check('a_swap_keeps_its_words', lines('swapped').includes('A wave.'), lines('swapped'));
      await undoIn('first-lora').click();
      await until('the file', () => Promise.resolve(lines('swapped')), (held) => held.includes(`tool: ${first.id}`) && held.includes('first-lora'));
      check('undo_puts_the_swap_back', lines('swapped').includes('second-lora'), lines('swapped'));
    } else seen.skipped_swap = 'no two families on the panel';
  } else seen.skipped_tools = 'nothing on the list makes a picture';

  // A rename during a real render: at once, nothing lost, and what reads it reads on.
  if ((await api('/v1/status')).body.local_usable && !process.env.ALONE) {
    writeFileSync(yamlAt, `${yaml().trimEnd()}
  sheet:
    tool: ${fixture}/wait
    instructions: A first picture.
    seconds: 1
  after:
    tool: ${fixture}/wait
    instructions: <sheet> and on
`);
    await box('sheet').waitFor();
    const render = (names, key) => api(`${at}/runs`, { method: 'POST', body: JSON.stringify({ names, on: ['local'] }), headers: { 'Idempotency-Key': key } });
    await render(['sheet'], `first-${Date.now()}`);
    const first = await until('its first version', () => asset('sheet'), (row) => row?.versions === 1 && !row.making?.length, 5 * STEP_MS);
    const approved = first.prefixes[0].number;
    // The next takes long enough to rename under it, and what reads it waits.
    writeFileSync(yamlAt, yaml().replace('    seconds: 1\n', '    seconds: 30\n'));
    await until('the longer one read', () => asset('sheet'), (row) => row?.fields?.seconds === 30);
    await render(['sheet', 'after'], `second-${Date.now()}`);
    await until('it going', () => asset('sheet'), (row) => row?.making?.length > 0, 2 * STEP_MS);
    await page.evaluate(() => {
      window.shown = [];
      const look = () => {
        const card = [...document.querySelectorAll('.react-flow__node .card')].find((held) => ['sheet', 'drawn'].includes(held.dataset.asset));
        if (card) window.shown.push({ name: card.dataset.asset, img: card.querySelectorAll('.media img, .media video').length, at: performance.now() });
        window.looking = requestAnimationFrame(look);
      };
      look();
    });
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('sheet').locator('.card').click({ button: 'right' });
    await page.locator('.asset-menu button:has-text("Rename")').click();
    await page.locator('input[aria-label="Name"]').fill('drawn');
    await page.evaluate(() => { window.entered = performance.now(); });
    await page.keyboard.press('Enter');
    const after = await until('the file and the lock', async () => [await asset('drawn'), (await state()).orphans ?? []], ([row]) => row?.versions === 1, 2000);
    check('rename_during_a_render_keeps_its_versions', after[0].versions === 1 && after[0].making?.length > 0 && !after[1].length, after);
    check('what_reads_it_says_the_new_name', yaml().includes('<drawn> and on'), yaml());
    await page.waitForTimeout(500);
    // From Enter on: before it, the fit has only just drawn the box, and its picture may still be on its way.
    const frames = await page.evaluate(() => { cancelAnimationFrame(window.looking); return window.shown.filter((frame) => frame.at >= window.entered); });
    check('no_empty_frame_after_enter', frames.length > 10 && frames.every((frame) => frame.img > 0), frames.filter((frame) => !frame.img).slice(0, 5));
    seen.frames = frames.length;
    await shot('renamed-rendering');
    const landed = await until('it landing', () => asset('drawn'), (row) => row?.versions === 2 && !row.making?.length, 5 * STEP_MS);
    check('the_take_lands_under_the_new_name', landed.prefixes.at(-1).number === 2 && lock().includes('drawn') && !/\n {2}sheet:/.test(lock()), landed);
    const reader = await until('what reads it', () => asset('after'), (row) => row?.versions === 1 && !row.making?.length, 5 * STEP_MS);
    check('what_reads_it_read_on', reader.standing === 'current' && reader.reads?.[0]?.name === 'drawn', reader);
    check('nothing_orphaned', !(await state()).orphans?.length, (await state()).orphans);

    // What a render can't read is refused, in words, and nothing runs.
    await render(['bare'], `bare-${Date.now()}`);
    const bare = await until('it refused', () => asset('bare'), (row) => row?.failure, 2 * STEP_MS);
    check('bare_self_is_refused_with_the_fix', bare.failure.code === 'input_missing' && bare.failure.message.includes('<self.1>') && !bare.versions, bare.failure);
    await render(['edits'], `edits-${Date.now()}`);
    const none = await until('it refused', () => asset('edits'), (row) => row?.failure, 2 * STEP_MS);
    check('a_version_never_made_is_refused', none.failure.code === 'input_missing' && !none.versions, none.failure);
    await api(`${at}/assets/Ada/versions/2`, { method: 'DELETE' });
    await render(['fixed'], `fixed-${Date.now()}`);
    const refused = await until('it refused', () => asset('fixed'), (row) => row?.failure, 2 * STEP_MS);
    check('a_deleted_version_fails_clearly', refused.failure.code === 'input_missing' && refused.failure.message.includes('was deleted'), refused.failure);
    check('that_failure_is_a_toast', (await toast('fixed didn\'t render')) === true);
    await api(`${at}/assets/Ada/versions/2`, { method: 'PUT', body: '{}' });
    check('no_approval_or_number_moved', approved === 1, first);

    // A version reaches its function: `<self.1>` and `<Ada.2>`, each that file and nothing else.
    await imported('seeded', 70);
    await box('seeded').waitFor();
    await api(`${at}/assets/seeded`, { method: 'PATCH', body: JSON.stringify({ tool: `${fixture}/wait` }) });
    await until('its prompt', () => asset('seeded'), (row) => row?.fields?.instructions === '<self.1>');
    writeFileSync(yamlAt, `${yaml().trimEnd()}\n  second:\n    tool: ${fixture}/wait\n    instructions: <Ada.2> as it was\n`);
    // Placed where there's room, which may be off the view: a box off it isn't drawn.
    await until('second placed', async () => (await api(`${at}/canvas`)).body?.cards?.second, Boolean);
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('second').waitFor();
    await render(['seeded', 'second'], `versions-${Date.now()}`);
    const ran = async (name) => {
      const row = await until(`${name}'s version`, () => asset(name), (held) => held?.prefixes?.some((take) => take.run) || held?.failure, 5 * STEP_MS);
      seen[`${name}_row`] = { failure: row.failure, versions: row.versions };
      return (await api(`/v1/runs/${row.prefixes.find((take) => take.run)?.run}?detail=full`)).body?.request?.input;
    };
    const version = (name, number) => asset(name).then((row) => row.prefixes.find((take) => take.number === number).main.sha256);
    const own = await ran('seeded');
    seen.self_input = own;
    check('self_reaches_its_function', own?.pictures?.[0]?.image?.includes(await version('seeded', 1)) && own.prompt === '<self 1>', own);
    const theirs = await ran('second');
    seen.version_input = theirs;
    check('a_version_reaches_its_function', theirs?.pictures?.[0]?.image?.includes(await version('Ada', 2)) && theirs.prompt === '<Ada 2> as it was', theirs);
  } else seen.skipped_render = 'no free tool runs here';

  await shut();
  check('no_node_on_screen', !/\bnode\b/i.test(await page.evaluate(() => document.body.innerText)));
  check('no_page_errors', errors.length === 0, errors);
  await forget();
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
