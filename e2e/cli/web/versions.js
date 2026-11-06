// Versions by name and a tool on a box (Paul, 11-05), in a real browser on a real Calcifer. `<abr.2>` and `<self.1>` read a version, fixed, and make no cycle; a rename keeps
// them and every comment, and happens at once during a real render, whose take lands under the new name with what reads it reading on; a tool goes on a text box, on a
// picture, and swaps, opening the box; its X takes its fields; a picture joins a box with no tool; a take dragged into text is `<abr.2>`; a version something reads can
// be deleted, with Undo; `reclaim` keeps what a number names. Takes beside the open picture, a double tap or Enter approving; a take picked is the `<self.N>` its words
// edit; a box of pictures merges into another; a box's shape is its tool's; open on a take, only its description, and its + writes the next one in the frame over
// what it mentions; Render stays Render while it makes, each take with its ×.
//   node versions.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: not on this computer's machine, which the rename during a
// render needs, as it renders the free `waits.py`. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { calcifer, signIn, MP4, STEP_MS, boxOf, drop, lastLook, launch, lookAt, noStall, openCanvas, png, renderOf, until } = require('./page');

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
  const base = calcifer();
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

  await signIn(page);
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
  const picture = (shade, width, height) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade, width, height));
    return path;
  };
  const imported = (name, shade) => post(`${at}/imports`, { source: picture(shade), name, exact: true, at: { x: 100 + shade * 4, y: 100 } });
  const joined = (name, shade, width, height) => post(`${at}/assets/${name}/versions`, { source: picture(shade, width, height) });

  // Mentions, in a file a hand wrote, its comments kept whatever comes.
  const comments = ['# Mine, and kept.', '  # A note on words.', '    # Its own note.', '      Two lines,', '      kept as they are.'];
  await imported('Abr', 10);
  // Its second square, so a slide along its row crosses a take of another shape.
  await joined('Abr', 20, 200, 200);
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
  // Square, as its tool makes with nothing it mentions in view, a box can land past the edge of the view, where it isn't drawn.
  await until('fixed placed', async () => (await api(`${at}/canvas`)).body?.cards?.fixed, Boolean);
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
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
    await page.locator(`.card.here[data-asset="${name}"]`).first().waitFor();
  };
  await open('Ada');
  // A take's number is in its name, on hover, and not drawn on its thumbnail.
  const numbers = await until('its numbers', () => page.locator('.card[data-asset="Ada"] .here-side .take[data-take]').evaluateAll((all) => all.map((one) => one.getAttribute('aria-label'))), (held) => held.length === 3, 5000).catch((why) => String(why));
  check('thumbnails_say_their_number', Array.isArray(numbers) && [...numbers].map((label) => label.match(/\d+$/)?.[0]).sort().join() === '1,2,3' && (await page.locator('.card[data-asset="Ada"] .take-no').count()) === 0, numbers);
  await shot('numbers');
  // Open, its takes are a column beside the picture, the newest on top.
  const big = await page.locator('.card.here[data-asset="Ada"] .media').first().boundingBox();
  const column = await page.locator('.card.here[data-asset="Ada"] .here-side .takes').boundingBox();
  const take = (number) => page.locator(`.card[data-asset="Ada"] .take[data-take="${number}"]`).boundingBox();
  const [newest, middle, oldest] = [await take(3), await take(2), await take(1)];
  check('takes_beside_the_picture', column.x >= big.x + big.width - 1 && Math.abs(newest.x - oldest.x) < 2 && newest.y < middle.y && middle.y < oldest.y, { big, column, newest, oldest });
  // The column scrolls by dragging it, a mouse's or a finger's, with no bar and a little carry after, its ends fading where there's more; a
  // press that doesn't move picks a take. (Sliding the pointer from take to take is gone: it fought the drag.)
  await imported('many', 50);
  for (const shade of [51, 52, 53, 54, 55, 56, 57, 58]) await joined('many', shade);
  await open('many');
  const side = page.locator('.card.here[data-asset="many"] .here-side');
  await side.locator('.take[data-take="1"]').waitFor();
  const sideBox = await side.boundingBox();
  const scrolled = () => side.evaluate((el) => el.scrollTop);
  const across = sideBox.x + sideBox.width / 2;
  await page.mouse.move(across, sideBox.y + sideBox.height - 12);
  await page.mouse.down();
  for (let step = 1; step <= 8; step++) await page.mouse.move(across, sideBox.y + sideBox.height - 12 - step * 18);
  await page.mouse.up();
  const byMouse = await scrolled();
  await page.waitForTimeout(500);
  const coasted = await scrolled();
  const looks = await side.evaluate((el) => ({ bar: el.offsetWidth - el.clientWidth, mask: getComputedStyle(el).maskImage || getComputedStyle(el).webkitMaskImage, top: el.style.getPropertyValue('--fade-top'), bottom: el.style.getPropertyValue('--fade-bottom') }));
  check('the_column_drags_with_a_mouse', byMouse > 40 && coasted >= byMouse && looks.bar === 0 && looks.top === '24px' && /gradient/.test(looks.mask), { byMouse, coasted, looks });
  // Under the take shown, how words name it: its one name, approved or not.
  const namedFirst = await page.locator('.card.here[data-asset="many"] .take-name .chip').allInnerTexts();
  const pickedBefore = await side.locator('.take.on').getAttribute('data-take');
  const another = side.locator(`.take[data-take]:not([data-take="${pickedBefore}"])`).first();
  const anotherNumber = await another.getAttribute('data-take');
  await another.click();
  check('a_still_press_picks', (await side.locator('.take.on').getAttribute('data-take')) === anotherNumber, { pickedBefore, anotherNumber });
  const takeName = page.locator('.card.here[data-asset="many"] .take-name .chip');
  const namedThen = await takeName.allInnerTexts();
  await takeName.last().click();
  check('the_take_shown_is_named_to_mention', namedFirst.join() === `<Many.${pickedBefore}>` && namedThen.join() === `<Many.${anotherNumber}>` && (await toast(`<many.${anotherNumber}>`)) === true, { namedFirst, namedThen });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 1 });
  const beforeFinger = await scrolled();
  const touchFrom = { x: across, y: sideBox.y + sideBox.height - 12 };
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [touchFrom] });
  for (let step = 1; step <= 6; step++) await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x: touchFrom.x, y: touchFrom.y - step * 18 }] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await page.waitForTimeout(500);
  const byFinger = await scrolled();
  await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: false });
  await cdp.detach();
  check('the_column_drags_with_a_finger', byFinger > beforeFinger + 40 && (await page.locator('.card.here[data-asset="many"]').count()) === 1, { beforeFinger, byFinger });

  // A take carried out of its box onto the canvas moves there: a box of its own holds it, it leaves its box, and what named it names that box.
  // One note's Undo puts all of it back. Carried out again, the new box dropped back on the old one merges in as its newest take.
  await imported('moving', 60);
  await joined('moving', 61);
  await joined('moving', 62);
  writeFileSync(yamlAt, `${yaml().trimEnd()}\n  moving_words:\n    text: <moving.2> on top.\n`);
  await box('moving_words').waitFor();
  const second = (await asset('moving')).prefixes.find((held) => held.number === 2).main.sha256;
  const carryOut = async () => {
    await open('moving');
    const movingHere = page.locator('.card.here[data-asset="moving"]');
    await movingHere.locator('.here-side .take[data-take="2"]').click();
    await movingHere.locator('.take-out').dragTo(page.locator('.stage'), { targetPosition: await openCanvas(page) });
    return until('moved out', async () => (await state()).assets.find((one) => one.sha256 === second && one.name !== 'moving'), Boolean).catch(() => null);
  };
  const movedOut = await carryOut();
  const leftBehind = await asset('moving');
  const renamedWords = await until('what named it', () => asset('moving_words'), (row) => row?.fields?.text?.includes('<moving_2>')).catch(() => null);
  check('a_take_carried_out_moves', movedOut?.name === 'moving_2' && !leftBehind.prefixes.some((held) => held.number === 2) && Boolean(renamedWords) && (await toast('Moved version 2 of moving out as moving_2')) === true, { movedOut: movedOut?.name, left: leftBehind.prefixes.map((held) => held.number), words: renamedWords?.fields?.text });
  await undoIn('Moved version 2 of moving').click();
  const putBack = await until('all back', async () => [await asset('moving'), await asset('moving_2'), await asset('moving_words')], ([row, gone, words]) => row.prefixes.some((held) => held.number === 2) && !gone && words.fields.text.includes('<moving.2>')).catch((why) => String(why));
  check('its_undo_puts_it_all_back', Array.isArray(putBack), putBack);
  const outAgain = (await carryOut())?.name ?? 'moving_2';
  await shut();
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  const outBox = await box(outAgain).locator('.nm').boundingBox();
  const homeBox = await box('moving').locator('.media').first().boundingBox();
  await page.mouse.move(outBox.x + 4, outBox.y + 4);
  await page.mouse.down();
  await page.mouse.move(outBox.x + 12, outBox.y + 4);
  await page.mouse.move(homeBox.x + homeBox.width / 2, homeBox.y + homeBox.height / 2, { steps: 8 });
  await page.waitForTimeout(100);
  await page.mouse.up();
  const mergedBack = await until('merged back', async () => [await asset('moving'), await asset(outAgain)], ([row, gone]) => !gone && row.prefixes.at(-1).main?.sha256 === second).catch((why) => String(why));
  check('dropped_back_it_merges_in', Array.isArray(mergedBack), mergedBack);
  await open('Ada');

  // A double tap approves a take, and again unapproves it; on the big picture too, and Enter, and none opens it large. A click opens a box at once.
  const ada = page.locator('.card.here[data-asset="Ada"]');
  const approval = async () => {
    const row = await asset('Ada');
    return row.pinned ? row.approved : null;
  };
  await ada.locator('.take[data-take="1"]').dblclick();
  check('a_double_tap_approves', (await until('take 1 approved', approval, (held) => held === 1).catch((why) => String(why))) === 1, await asset('Ada'));
  await ada.locator('.take[data-take="1"]').dblclick();
  check('again_unapproves', (await until('unapproved', approval, (held) => held === null).catch((why) => String(why))) === null, await asset('Ada'));
  // Unapproved, it's still the take looked at: one more double tap, or Enter, is about it.
  const still = await until('still on take 1', () => ada.locator('.take.on').getAttribute('data-take'), (on) => on === '1', 3000).catch(async () => ada.locator('.take.on').getAttribute('data-take'));
  check('unapproved_still_looked_at', still === '1', still);
  await ada.locator('.take[data-take="2"]').click();
  await ada.locator('.media').first().dblclick();
  check('a_double_tap_on_the_picture_approves', (await until('take 2 approved', approval, (held) => held === 2).catch((why) => String(why))) === 2, await asset('Ada'));
  check('no_large_view', (await page.locator('.viewer').count()) === 0);
  await ada.locator('.media').first().dblclick();
  await until('unapproved again', approval, (held) => held === null);
  await page.keyboard.press('Enter');
  check('enter_approves', (await until('enter', approval, (held) => held === 2).catch((why) => String(why))) === 2, await asset('Ada'));
  await page.keyboard.press('Enter');
  check('enter_unapproves', (await until('enter again', approval, (held) => held === null).catch((why) => String(why))) === null, await asset('Ada'));
  check('no_approve_button', (await ada.locator('button:text-is("Approve"), button:text-is("Unapprove")').count()) === 0, await ada.innerText());
  await post(`${at}/approve`, { name: 'Ada', version: '3' });
  await until('3 approved again', approval, (held) => held === 3);
  await shut();
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  const media = await box('Ada').locator('.media').first().boundingBox();
  // From the click to the box opened, as the page has it: the wait for a second tap was 200 ms.
  await page.evaluate(() => {
    window.openedIn = new Promise((done) => document.addEventListener('click', () => {
      const at = performance.now();
      new MutationObserver((_, watch) => {
        if (!document.querySelector('.card.here[data-asset="Ada"]')) return;
        watch.disconnect();
        done(performance.now() - at);
      }).observe(document.body, { subtree: true, attributes: true, childList: true });
    }, { capture: true, once: true }));
  });
  await page.mouse.click(media.x + media.width / 2, media.y + media.height / 2);
  const opened = await page.evaluate(() => window.openedIn);
  check('a_click_opens_at_once', opened < 200, opened);
  seen.click_opens_ms = Math.round(opened);
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
      // F fits the box worked on (447b9127b): at rest, it fits them all.
      if (await page.locator('.card.here').count()) {
        await page.keyboard.press('Escape');
        await page.locator('.card.here').waitFor({ state: 'detached' });
      }
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

    // On open canvas a picture tool's box opens as it lands, on its words, the caret in them, its shape Auto, and no name asked.
    const namesThen = (await state()).assets.map((held) => held.name);
    await row.dragTo(page.locator('.stage'), { targetPosition: { x: 1300, y: 860 } });
    const dropped = await until('a box of it', async () => (await state()).assets.find((held) => !namesThen.includes(held.name)), Boolean).catch(() => null);
    const droppedOpen = page.locator(`.card.here[data-asset="${dropped?.name}"]`);
    const caretOnIt = await until('the caret in its words', () => droppedOpen.locator('.in-frame .mention-text').evaluate((el) => el.contains(document.activeElement)).catch(() => false), Boolean).catch(() => false);
    const droppedShape = await droppedOpen.locator('.ratio-rest').innerText().then((text) => text.trim(), () => null);
    check('a_dropped_tool_opens_on_its_words', Boolean(dropped) && caretOnIt === true && droppedShape === 'Auto' && (await page.locator('.nm-field').count()) === 0, { dropped: dropped?.name, caretOnIt, droppedShape });
    if (dropped) {
      await api(`${at}/assets/${encodeURIComponent(dropped.name)}`, { method: 'DELETE' });
      await until('that box gone', () => box(dropped.name).count(), (n) => n === 0);
    }
    await shut();

    // On a text box its words are the prompt, at once, and the box opens.
    await box('words').waitFor();
    await carry('words');
    await box('words').locator('.add-on').first().waitFor({ timeout: 1000 });
    check('a_dropped_tool_opens_its_box', await page.locator('.card.here[data-asset="words"]').waitFor({ timeout: 2000 }).then(() => true, (why) => String(why)) === true);
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
    // An image tool on a picture opens it on its next take's words: the caret in them, the approved take the first they mention, ghosted behind them.
    const onIt = page.locator('.card.here[data-asset="other"]');
    const caret = await until('writing on it', () => onIt.locator('.in-frame .mention-text').evaluate((el) => el.contains(document.activeElement)).catch(() => false), Boolean).catch(() => false);
    const firstChip = await onIt.locator('.in-frame .chip').first().innerText().catch(() => null);
    const behind = await until('its picture behind', () => onIt.locator('.in-frame .ghosts img').count(), (n) => n === 1).catch(() => 0);
    check('a_tool_on_a_picture_writes_its_next_take', caret === true && firstChip === `<Self.${other.approved}>` && behind === 1, { caret, firstChip, behind });
    await onIt.locator('.in-frame .mention-text').evaluate((el) => el.blur()).catch(() => {});
    await shot('tool-on-picture');
    // Drawn out till words are larger than at 1:1, its tool chip stops short of its ratio.
    const middle = await box('other').locator('.card').boundingBox();
    await page.mouse.move(middle.x + middle.width / 2, middle.y + middle.height / 2);
    const scale = () => page.evaluate(() => Number(getComputedStyle(document.querySelector('.stage')).getPropertyValue('--text')));
    await page.keyboard.down('Control');
    for (let step = 0; step < 6 && (await scale()) < 1.5; step++) {
      await page.mouse.wheel(0, 200);
      await page.waitForTimeout(150);
    }
    await page.keyboard.up('Control');
    const chip = await box('other').locator('.add-on').first().boundingBox();
    const ratio = (await box('other').locator('.ratio-rest').count()) ? await box('other').locator('.ratio-rest').boundingBox() : null;
    if (ratio) check('chip_clear_of_its_ratio', chip.x + chip.width <= ratio.x, { chip, ratio, text: await scale() });
    else seen.no_ratio = tool.id;
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('other').locator(`button[aria-label^="Take"]`).first().click();
    await until('the tool gone', () => Promise.resolve(lines('other')), (text) => !text.includes('tool:'));
    // Its X takes the tool and its fields with it: its pictures, number and approval stay.
    const off = await asset('other');
    check('x_on_pictures_takes_its_fields', off.versions === 1 && off.approved === 1 && !off.fields?.instructions && !lines('other').includes('instructions') && off.standing === 'current' && !off.tool, { off, lines: lines('other') });
    check('x_says_with_undo', (await toast('Its pictures stay')) === true);
    await undoIn('Its pictures stay').click();
    await until('the tool back', () => Promise.resolve(lines('other')), (text) => text.includes(`tool: ${tool.id}`) && text.includes('<self.1>'));
    check('undo_puts_it_back', (await asset('other')).versions === 1 && (await asset('other')).fields?.instructions === '<self.1>', await asset('other'));
    // Back, it looks at its take: its tool is its next take's, behind its +.
    await open('other');
    await renderOf(page, 'other');
    await box('other').locator(`button[aria-label^="Take"]`).first().click();
    await until('the tool gone', () => Promise.resolve(lines('other')), (text) => !text.includes('tool:'));
    await box('other').locator('.card').waitFor();

    // A take picked, by a press or the arrows, is the one its words edit: the first `<self.M>` in them is `<self.N>`.
    await imported('picked', 80);
    await joined('picked', 85);
    await api(`${at}/assets/picked`, { method: 'PATCH', body: JSON.stringify({ tool: tool.id }) });
    await api(`${at}/assets/picked`, { method: 'PATCH', body: JSON.stringify({ fields: { instructions: 'Warmer <self.1>, as <self.1> was.' } }) });
    await until('its words', () => asset('picked'), (row) => row?.tool === tool.id && row.fields?.instructions === 'Warmer <self.1>, as <self.1> was.');
    // As the page has it: its tool on, and both its takes.
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('picked').locator('.add-on').first().waitFor();
    await open('picked');
    const pickedBox = page.locator('.card.here[data-asset="picked"]');
    await pickedBox.locator('.take[data-take="2"]').click();
    const edited = await until('take 2 in its words', () => asset('picked'), (row) => row.fields?.instructions === 'Warmer <self.2>, as <self.1> was.').catch((why) => String(why));
    check('picking_a_take_edits_its_self', edited?.fields?.instructions === 'Warmer <self.2>, as <self.1> was.', edited);
    await page.keyboard.press('ArrowRight');
    const stepped = await until('take 1 by the arrow', () => asset('picked'), (row) => row.fields?.instructions === 'Warmer <self.1>, as <self.1> was.').catch((why) => String(why));
    check('the_arrows_edit_it_too', stepped?.fields?.instructions === 'Warmer <self.1>, as <self.1> was.' && (await pickedBox.locator('.take.on').getAttribute('data-take')) === '1', stepped);
    await shut();

    // The stack: under the box at its left, the ratio at its right, and what's under it keeps clear.
    const rect = await box('other').locator('.card').boundingBox();
    seen.card = rect;

    // A box with no tool: a picture joins it, and it has its takes beside it, a double tap to approve, and no Render.
    const { x, y, width, height } = await box('other').locator('.card').boundingBox();
    await drop(page, [{ name: 'more.png', type: 'image/png', bytes: png(55) }], x + width / 2, y + height / 2);
    const grown = await until('the picture joined', () => asset('other'), (row) => row.versions === 2);
    check('a_dropped_picture_joins', grown.prefixes.at(-1).number === 2 && grown.approved === 1, grown);
    check('joining_makes_no_box', !(await state()).assets.some((row) => /^more/.test(row.name)));
    await box('other').locator('.card').click();
    await page.locator('.card.here .here-side .take').first().waitFor();
    check('no_render_with_no_tool', (await page.locator('.card.here .run').count()) === 0 && (await page.locator('.card.here .here-side .take').count()) === 2 && (await page.locator('.card.here button:text-is("Approve"), .card.here button:text-is("Unapprove")').count()) === 0);
    await page.locator('.card.here .take[data-take="2"]').dblclick();
    check('a_double_tap_approves_with_no_tool', (await until('2 approved', () => asset('other'), (row) => row.approved === 2).catch((why) => String(why)))?.approved === 2);
    await page.locator('.card.here .take[data-take="1"]').dblclick();
    await until('1 approved again', () => asset('other'), (row) => row.approved === 1);
    await shot('no-tool');
    await page.keyboard.press('Escape');

    // A box of pictures dropped on another merges in: what it shows is the newest take there, approved as that box was, and it's gone. One Undo puts both back.
    await imported('merging', 95);
    await box('merging').waitFor();
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    const into = await asset('other');
    const sha = (await asset('merging')).sha256;
    const dragBox = async (name, onto) => {
      const from = await box(name).locator('.nm').boundingBox();
      const to = await box(onto).locator('.media').first().boundingBox();
      await page.mouse.move(from.x + 4, from.y + 4);
      await page.mouse.down();
      await page.mouse.move(from.x + 12, from.y + 4);
      await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 8 });
      await page.waitForTimeout(100);
      await page.mouse.up();
    };
    await dragBox('merging', 'other');
    // Approved as the box was: the newest, once the take it's become is there to approve.
    const merged = await until('merged in', async () => [await asset('other'), await asset('merging')], ([row, gone]) => row.versions === into.versions + 1 && !gone && row.approved === row.prefixes.at(-1).number).catch((why) => String(why));
    const newest = Array.isArray(merged) ? merged[0].prefixes.at(-1) : null;
    check('a_box_merges_in', newest?.main?.sha256 === sha && merged[0].approved === newest.number && merged[0].pinned, merged);
    check('merge_says_with_undo', (await toast('merging merged into other')) === true);
    await undoIn('merging merged into other').click();
    const restored = await until('both back', async () => [await asset('other'), await asset('merging')], ([row, back]) => row.versions === into.versions && row.approved === into.approved && back?.sha256 === sha).catch((why) => String(why));
    check('one_undo_puts_both_back', Array.isArray(restored), restored);
    await box('merging').waitFor();
    // A box of words dropped on a box with a tool is a reference: its name goes in, and both stay.
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    const referred = await asset('picked');
    await dragBox('words', 'picked');
    const named = await until('a reference', () => asset('picked'), (row) => row.fields?.instructions?.includes('<words>')).catch((why) => String(why));
    check('words_on_a_tool_box_are_a_reference', named?.versions === referred.versions && Boolean(await asset('words')), named);

    // A box's shape is its tool's, and no field says it. With none set it's the first picture its words mention, as they change; its corner on a box
    // with no picture sets one, at rest or open, and open, the one set has an × back to Auto. A box showing a picture only scales.
    writeFileSync(yamlAt, `${yaml().trimEnd()}\n  framed:\n    tool: ${tool.id}\n    instructions: <Ada.2> beside <Ada>.\n`);
    // Placed where there's room, which may be off the view: a box off it isn't drawn.
    await until('framed placed', async () => (await api(`${at}/canvas`)).body?.cards?.framed, Boolean);
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('framed').waitFor();
    const shapeOf = async (name) => {
      const { width, height } = await box(name).locator('.media').first().boundingBox();
      return width / height;
    };
    const shapedAs = (name, ratio) => until(`${name} at ${ratio}`, () => shapeOf(name), (now) => Math.abs(now / ratio - 1) < 0.03).catch(async () => shapeOf(name));
    const rest = (name) => box(name).locator('.ratio-rest').innerText().then((text) => text.trim(), () => null);
    const square = await shapedAs('framed', 1);
    check('shaped_as_the_first_it_mentions', Math.abs(square - 1) < 0.03 && (await rest('framed')) === 'Auto', { square, rest: await rest('framed') });
    await api(`${at}/assets/framed`, { method: 'PATCH', body: JSON.stringify({ fields: { instructions: '<Ada> beside <Ada.2>.' } }) });
    const wide = await shapedAs('framed', 1.6);
    check('reshaped_as_its_words_change', Math.abs(wide - 1.6) < 0.05, wide);
    const shapeLine = () => Promise.resolve((lines('framed').match(/aspect_ratio: "?([\d.:]+)"?/) ?? [])[1] ?? null);
    const pullCorner = async (name, dx, dy) => {
      await box(name).locator('.media').first().hover();
      const corner = await box(name).locator('.corner').boundingBox();
      await page.mouse.move(corner.x + corner.width / 2, corner.y + corner.height / 2);
      await page.mouse.down();
      await page.mouse.move(corner.x + corner.width / 2 + dx, corner.y + corner.height / 2 + dy, { steps: 8 });
      await page.mouse.up();
    };
    await pullCorner('framed', 220, -20);
    const pulled = await until('a shape set', shapeLine, Boolean).catch(() => null);
    const pulledShape = pulled && pulled.split(':').map(Number).reduce((w, h) => w / h);
    check('its_corner_sets_its_shape', Boolean(pulled) && Math.abs((await shapedAs('framed', pulledShape)) / pulledShape - 1) < 0.03 && (await rest('framed')) === pulled, { pulled, rest: await rest('framed'), now: await shapeOf('framed') });
    await open('framed');
    const here = page.locator('.card.here[data-asset="framed"]');
    check('no_shape_field', (await here.locator('[data-field=aspect_ratio], select[aria-label="Aspect ratio"], [aria-label="Shape"]').count()) === 0, await here.innerText());
    await pullCorner('framed', -60, 160);
    const reshaped = await until('a shape set open', shapeLine, (now) => now && now !== pulled).catch(() => null);
    check('reshaped_while_open', Boolean(reshaped) && (await here.count()) === 1 && (await here.locator('.ratio-rest').innerText()).trim() === reshaped, { pulled, reshaped });
    await here.locator('.ratio-rest .ratio-clear').click();
    await until('back to Auto', shapeLine, (now) => now === null);
    check('its_x_goes_back_to_auto', (await here.locator('.ratio-rest').innerText()).trim() === 'Auto' && Math.abs((await shapedAs('framed', 1.6)) - 1.6) < 0.05, { rest: await here.locator('.ratio-rest').innerText(), now: await shapeOf('framed') });
    await shut();
    // A box whose own name has a dot is that box, as Calcifer reads it, not a take of the box before the dot: mentioned with nothing made yet,
    // behind the words it's its blank frame, never another box's picture.
    writeFileSync(yamlAt, `${yaml().trimEnd()}\n  ada.later:\n    tool: ${tool.id}\n    instructions: Not yet.\n  awaiting:\n    tool: ${tool.id}\n    instructions: <Ada.later> at dawn.\n`);
    await box('awaiting').waitFor();
    await open('awaiting');
    const awaitingHere = page.locator('.card.here[data-asset="awaiting"]');
    const blankGhost = await until('its blank frame behind', () => awaitingHere.locator('.in-frame .ghost-blank').count(), (n) => n === 1).catch(() => 0);
    check('a_dotted_name_is_its_own_box', blankGhost === 1 && (await awaitingHere.locator('.in-frame .ghosts img').count()) === 0 && (await awaitingHere.locator('.in-frame .ghost-blank').getAttribute('data-mention')) === 'Ada.later', { blankGhost, imgs: await awaitingHere.locator('.in-frame .ghosts img').count() });
    await shut();
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    const picturedWas = { shape: await shapeOf('picked'), width: (await box('picked').locator('.media').first().boundingBox()).width };
    await pullCorner('picked', 120, -40);
    const scaled = await until('it scaled', async () => (await box('picked').locator('.media').first().boundingBox()).width, (width) => width > picturedWas.width + 20).catch((why) => String(why));
    check('a_picture_only_scales', typeof scaled === 'number' && Math.abs((await shapeOf('picked')) / picturedWas.shape - 1) < 0.03 && !lines('picked').includes('aspect_ratio'), { picturedWas, scaled, now: await shapeOf('picked'), lines: lines('picked') });
    // Open on a take, only its description is under it. Its + writes the next take in the frame, over the pictures its words mention, faded, with its tool
    // and Render under it; a take goes back to looking.
    await open('picked');
    const writing = page.locator('.card.here[data-asset="picked"]');
    await writing.locator('.here-under .desc-field').waitFor();
    const onItsTake = await writing.locator('.here-under').innerText();
    const toolShown = await writing.locator('.tool-head, .more-toggle, .add-on, .ratio-rest, .status, .warned, button[aria-label=Render]').count();
    check('a_take_has_only_its_description', !/Render|Instructions|More/.test(onItsTake) && onItsTake.includes('Description') && toolShown === 0, { onItsTake, toolShown });
    await writing.locator('.here-side .take.add[aria-label="Its next take"]').click();
    await writing.locator('.in-frame .mention-text[data-field=instructions]').waitFor();
    const ghosted = await until('what it mentions, behind', () => writing.locator('.in-frame .ghosts img').count(), (n) => n > 0).catch(() => 0);
    const underWords = await writing.locator('.here-under').innerText();
    check('its_plus_writes_the_next_take', ghosted > 0 && /Render/.test(underWords) && (await writing.locator('.here-side .take.add.on').count()) === 1, { ghosted, underWords });
    await writing.locator('.here-side .take[data-take="1"]').click();
    check('a_take_leaves_its_words', (await writing.locator('.in-frame').count()) === 0 && (await writing.locator('.here-side .take.on').getAttribute('data-take')) === '1', await writing.innerText());
    await shut();
    // Its words take only the kinds its tool sends (`references`): a video box is refused when dragged onto it, saying why, isn't offered after `<`,
    // and written by hand it's a dimmed chip, which Calcifer doesn't send.
    const sends = (await api(`/v1/tools?project=${id}`)).body.tools.find((one) => one.id === tool.id)?.references ?? [];
    seen.references = sends;
    if (sends.length && !sends.includes('video')) {
      const reelAt = join(pictures, 'reel.mp4');
      writeFileSync(reelAt, MP4);
      await post(`${at}/imports`, { source: reelAt, name: 'reel', exact: true, at: { x: 100, y: 900 } });
      await box('reel').waitFor();
      await page.keyboard.press('f');
      await page.waitForTimeout(400);
      const wordsBefore = (await asset('picked')).fields?.instructions;
      const reelFrom = await box('reel').locator('.nm').boundingBox();
      const onto = await box('picked').locator('.media').first().boundingBox();
      await page.mouse.move(reelFrom.x + 4, reelFrom.y + 4);
      await page.mouse.down();
      await page.mouse.move(reelFrom.x + 12, reelFrom.y + 4);
      await page.mouse.move(onto.x + onto.width / 2, onto.y + onto.height / 2, { steps: 8 });
      await page.waitForTimeout(100);
      const why = await box('picked').locator('.refused-line').innerText().catch(() => null);
      await page.mouse.up();
      await page.waitForTimeout(500);
      check('a_video_is_refused_on_a_picture_tool', Boolean(why?.includes('reel is a video')) && (await asset('picked')).fields?.instructions === wordsBefore, { why, words: (await asset('picked')).fields?.instructions });
      await open('picked');
      await writing.locator('.here-side .take.add[aria-label="Its next take"]').click();
      const prompt = writing.locator('.in-frame .mention-text[data-field=instructions]');
      await prompt.click();
      await page.keyboard.press('End');
      await page.keyboard.type(' <re');
      const offeredNames = await until('the names offered', () => page.locator('.name-menu li').allInnerTexts(), () => true, 1000).catch(() => []);
      for (let i = 0; i < 4; i++) await page.keyboard.press('Backspace');
      // Left with nothing typed, so a write from elsewhere is the box's words and no conflict with them; open again on its next take.
      await shut();
      await api(`${at}/assets/picked`, { method: 'PATCH', body: JSON.stringify({ fields: { instructions: `${wordsBefore} <reel>` } }) });
      await until('the reel in its words', () => asset('picked'), (row) => row.fields?.instructions?.includes('<reel>'));
      await open('picked');
      await renderOf(page, 'picked');
      const dimmed = await until('a dimmed chip', () => prompt.locator('.chip.refused[data-mention="reel"]').count(), (n) => n === 1).catch(() => 0);
      check('a_video_isnt_offered_and_is_dimmed', !offeredNames.includes('reel') && dimmed === 1, { offeredNames, dimmed });
      await shut();
    }

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
    // What reads it says it waits for it, and nothing of a machine's work for another box.
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    const waitsSaid = await until('after says what it waits for', () => box('after').locator('.prog b').innerText().catch(() => ''), (text) => /^Waits for <sheet>/i.test(text)).catch(() => box('after').locator('.prog b').innerText().catch(() => ''));
    check('a_box_says_what_it_waits_for', /^Waits for <sheet>/i.test(waitsSaid) && !/Installing/.test(waitsSaid), waitsSaid);
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

    // Render stays Render while a box makes, and the box stays open: each press is one more take, each with its own × that cancels just it.
    writeFileSync(yamlAt, `${yaml().trimEnd()}\n  slow:\n    tool: ${fixture}/wait\n    instructions: Slowly.\n    seconds: 120\n`);
    await box('slow').waitFor();
    await open('slow');
    const slow = page.locator('.card.here[data-asset="slow"]');
    // Writing a new box has no description: that's for a take looked at.
    check('writing_has_no_description', (await slow.locator('.in-frame').count()) === 1 && (await slow.locator('.here-under .desc-field, .here-under hr.here-rule').count()) === 0, await slow.locator('.here-under').innerText());
    await (await renderOf(page, 'slow')).click();
    const firstMaking = await until('one making', () => asset('slow'), (row) => row?.making?.length === 1, 2 * STEP_MS);
    // Render from its words looks at the take being made: its tile is the one picked, and + goes back to the words, still there.
    const pickedFirst = await until('its tile picked', () => slow.locator('.here-side .take.making.on').getAttribute('title').catch(() => null), (title) => title === `Run ${firstMaking.making[0]}`).catch(() => null);
    check('render_picks_the_take_being_made', (await slow.count()) === 1 && pickedFirst === `Run ${firstMaking.making[0]}` && (await slow.locator('.here-side .take.add.on').count()) === 0, await slow.innerText());
    const renderButton = await renderOf(page, 'slow');
    check('plus_goes_back_to_its_words', (await slow.locator('.in-frame').innerText()).includes('Slowly.') && (await renderButton.innerText()).trim() === 'Render', await slow.locator('.in-frame').innerText());
    await renderButton.click();
    const both = await until('two making', () => asset('slow'), (row) => row?.making?.length === 2, 2 * STEP_MS).catch((why) => String(why));
    const pickedNewest = await until('the newest picked', () => slow.locator('.here-side .take.making.on').getAttribute('title').catch(() => null), (title) => title === `Run ${Math.max(...(both?.making ?? [0]))}`).catch(() => null);
    check('render_twice_picks_the_newest', pickedNewest === `Run ${Math.max(...(both?.making ?? [0]))}`, { pickedNewest, making: both?.making });
    const making = both?.making ?? [];
    const crosses = await until('a × each', () => slow.locator('.take-at:has(.take.making) .take-off').evaluateAll((all) => all.map((one) => one.getAttribute('aria-label'))), (labels) => labels.length === 2).catch((why) => String(why));
    check('each_press_one_more', making.length === 2 && Array.isArray(crosses) && making.every((run) => crosses.includes(`Cancel run ${run}`)), { making, crosses });
    await shot('making-two');
    const [older, newer] = [...making].sort((a, b) => a - b);
    // A tile of a take still being made is picked like any: its outline, and its progress on the frame.
    await slow.locator(`.here-side .take.making[title="Run ${older}"]`).click();
    const pickedOlder = await until('the older picked', () => slow.locator('.here-side .take.making.on').getAttribute('title').catch(() => null), (title) => title === `Run ${older}`).catch(() => null);
    check('a_take_being_made_is_picked_by_its_tile', pickedOlder === `Run ${older}` && (await slow.locator('.here-side .take.on').count()) === 1 && (await slow.locator('.media .media-making').count()) === 1, { pickedOlder, on: await slow.locator('.here-side .take.on').count() });
    const runState = async (number) => (await api(`/v1/runs/${number}`)).body?.status;
    await slow.locator(`.take-off[aria-label="Cancel run ${newer}"]`).click();
    const canceled = await until('the newer canceled', () => runState(newer), (status) => status === 'canceled', 2 * STEP_MS).catch((why) => String(why));
    check('its_x_cancels_that_take', canceled === 'canceled' && ['queued', 'in_progress'].includes(await runState(older)) && (await asset('slow')).making?.includes(older), { canceled, older: await runState(older), making: (await asset('slow')).making });
    await slow.locator(`.take-off[aria-label="Cancel run ${older}"]`).click();
    await until('the older canceled', () => runState(older), (status) => status === 'canceled', 2 * STEP_MS);
    check('canceled_makes_nothing', !(await until('nothing making', () => asset('slow'), (row) => !row?.making?.length, 2 * STEP_MS)).versions);
    await shut();

    // Render pressed as a take lands, while its file is still on its way here, is one more take: it was taken in and dropped (cl-774, 11-05).
    writeFileSync(yamlAt, `${yaml().trimEnd()}\n  quick:\n    tool: ${fixture}/wait\n    instructions: Quickly.\n`);
    await box('quick').waitFor();
    await open('quick');
    await (await renderOf(page, 'quick')).click();
    await until('its first listed', () => asset('quick'), (row) => row?.versions === 1, 2 * STEP_MS);
    // The take being made lands picked: its tile shows it, still the one looked at.
    const landedOn = await until('it landed picked', () => page.locator('.card.here[data-asset="quick"] .here-side .take.on').getAttribute('data-take').catch(() => null), (number) => number === '1').catch(() => null);
    check('a_take_lands_picked', landedOn === '1', landedOn);
    await (await renderOf(page, 'quick')).click();
    const again = await until('one more', () => asset('quick'), (row) => row?.versions === 2, 2 * STEP_MS).catch((why) => String(why));
    check('render_as_a_take_lands_is_one_more', again?.versions === 2, again);
    await shut();

    // A mention row of plain `File`s takes the kinds they're named for: draw.py's `image` and `voice` take pictures and sounds, and a picture
    // mentioned in it isn't marked refused.
    const draw = join(__dirname, '..', 'project', 'draw.py');
    writeFileSync(yamlAt, `${yaml().trimEnd()}\n  drawn_from:\n    tool: ${draw}/draw\n    instructions: <Ada> in the rain.\n`);
    // Placed where there's room, which may be off the view: a box off it isn't drawn.
    await until('drawn_from placed', async () => (await api(`${at}/canvas`)).body?.cards?.drawn_from, Boolean);
    await page.keyboard.press('f');
    await page.waitForTimeout(400);
    await box('drawn_from').waitFor();
    const drawTakes = await until('draw listed', async () => (await api(`/v1/tools?project=${id}`)).body.tools.find((one) => one.id.endsWith('draw.py/draw'))?.references, Boolean).catch(() => null);
    await open('drawn_from');
    const drawnChips = page.locator('.card.here[data-asset="drawn_from"] .in-frame .chip');
    await drawnChips.first().waitFor().catch(() => {});
    check('file_slots_take_their_named_kind', JSON.stringify(drawTakes) === '["audio","image"]' && (await drawnChips.count()) === 1 && (await page.locator('.card.here[data-asset="drawn_from"] .in-frame .chip.refused').count()) === 0, { drawTakes, chips: await drawnChips.evaluateAll((all) => all.map((one) => one.className)) });
    await shut();

    // A box approved at 1:1, its next take asked at 16:9 (Paul, 11-06): while it's made the box is 16:9 with no other take's picture in it;
    // landed, it's that take's shape with no bars, the run having asked 16:9; and each take shown is its own shape.
    writeFileSync(yamlAt, `${yaml().trimEnd()}\n  shapes:\n    tool: ${fixture}/wait\n    instructions: A whale.\n    aspect_ratio: "1:1"\n`);
    await box('shapes').waitFor();
    await render(['shapes'], `shapes-${Date.now()}`);
    await until('its square', () => asset('shapes'), (row) => row?.versions === 1 && !row.making?.length, 5 * STEP_MS);
    await post(`${at}/approve`, { name: 'shapes', version: '1' });
    await api(`${at}/assets/shapes`, { method: 'PATCH', body: JSON.stringify({ fields: { aspect_ratio: '16:9', seconds: 4 } }) });
    await until('16:9 asked', () => asset('shapes'), (row) => row?.fields?.aspect_ratio === '16:9' && row.pinned);
    await open('shapes');
    const shapesHere = page.locator('.card.here[data-asset="shapes"]');
    const frameShape = async () => {
      const { width, height } = await shapesHere.locator('.media').first().boundingBox();
      return width / height;
    };
    const square = await frameShape();
    await (await renderOf(page, 'shapes')).click();
    await until('being made', () => asset('shapes'), (row) => row?.making?.length === 1, 2 * STEP_MS);
    const whileMade = await frameShape();
    const noStale = (await shapesHere.locator('.media img').count()) === 0;
    const madeAs = await until('its 16:9 take', () => asset('shapes'), (row) => row?.versions === 2 && !row.making?.length, 5 * STEP_MS);
    const wide = madeAs.prefixes.find((held) => held.number === 2).main;
    // Its picture as made: the run asked 16:9.
    const pictureShape = await until('its picture', () => shapesHere.locator('.media img').last().evaluate((img) => img.naturalWidth / img.naturalHeight).catch(() => 0), (ratio) => ratio > 0).catch(() => 0);
    const landedShape = await until('the box at its take', frameShape, (now) => Math.abs(now / pictureShape - 1) < 0.03).catch(() => frameShape());
    check('a_take_is_made_and_shown_at_its_shape', Math.abs(square - 1) < 0.03 && Math.abs(whileMade / (16 / 9) - 1) < 0.03 && noStale && Math.abs(pictureShape / (16 / 9) - 1) < 0.05 && Math.abs(landedShape / pictureShape - 1) < 0.03 && (await shapesHere.locator('.here-side .take.on').getAttribute('data-take')) === '2', { square, whileMade, noStale, pictureShape, landedShape });
    // Clicked, a take is in its frame at once, never another's picture after: its tile's thumbnail scaled, then itself.
    const clickTo = (number, sha) => page.evaluate(async ({ number, sha }) => {
      const card = () => document.querySelector('.card.here[data-asset="shapes"]');
      const at = performance.now();
      card().querySelector(`.here-side .take[data-take="${number}"]`).click();
      let first;
      let stale = 0;
      return new Promise((done) => {
        const look = () => {
          const shown = [...card().querySelectorAll('.media img')].filter((img) => img.complete && img.naturalWidth);
          if (shown.some((img) => !img.src.includes(sha))) stale++;
          if (first === undefined && shown.some((img) => img.src.includes(sha))) first = performance.now() - at;
          if (first !== undefined || performance.now() - at > 10_000) done({ first, stale });
          else requestAnimationFrame(look);
        };
        requestAnimationFrame(look);
      });
    }, { number, sha: sha.replace('sha256:', '') });
    const toSquare = await clickTo(1, madeAs.prefixes[0].main.sha256);
    await page.waitForTimeout(300);
    const squareAgain = await frameShape();
    const toWide = await clickTo(2, wide.sha256);
    await page.waitForTimeout(300);
    seen.click_to_pixels = { toSquare, toWide };
    check('a_clicked_take_shows_at_once_at_its_shape', toSquare.first < 400 && toWide.first < 400 && toSquare.stale <= 1 && toWide.stale <= 1 && Math.abs(squareAgain - 1) < 0.03 && Math.abs((await frameShape()) / pictureShape - 1) < 0.03, { toSquare, toWide, squareAgain });
    await shut();

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
