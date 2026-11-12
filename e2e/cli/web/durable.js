// cl-904 (Paul, 11-12): "I just edited a box, a few minutes later something happened and the whole page refreshed, and then the box was back in
// some other previous state (the text I deleted was back again)". The page holds what's typed and flushes it to Calcifer a moment after a pause, on
// leaving the box, as the tab hides and as the page goes; Calcifer's changes come back on the stream and merge field by field, what's typed and
// unsent winning, with no prompt. A save read back before its answer, typing going on: kept. Calcifer killed while a box is typed in: no reload,
// every character saved. Typed and the box left at once: there when it's opened again, and saved. Typed and the page reloaded at once: saved.
// Another writer's words while the page's are on their way: both stand, merged by character (Automerge text). Two pages typing in one box at
// once: both keep theirs.
//   node durable.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $RESTART: kills this Calcifer with -9 and brings it back. Prints what it saw as
// JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { exec } = require('child_process');
const { readFileSync } = require('fs');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, addBoxes, lastLook, launch, lookAt, noStall, openBox, rowsOf, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const holder = () => JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
const WORDS = 'one two three four five six seven eight nine ten eleven twelve'.split(' ');

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-durable-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: folder } = await projectFromPage(page);
  seen.project = { id, folder };
  addBoxes(folder, [{ name: 'words', fields: { text: 'What Calcifer held.' } }]);

  // What project.db holds of the box's words, read past Calcifer.
  const inDb = () => rowsOf(folder, "SELECT f.value FROM fields f JOIN boxes b ON f.box = b.key WHERE b.name = 'words' AND f.field = 'text'").map(([value]) => JSON.parse(value))[0];
  const saving = (wanted) => until(`"${wanted}" in project.db`, () => Promise.resolve(inDb()), (value) => value === wanted).catch(() => {});
  // An open box is in Focus, over the canvas (after a reload its canvas card says `here` too; Focus is the later).
  const text = () => page.locator('.card.here[data-asset="words"] .mention-text').last();
  const shown = async () => (await text().innerText()).trim();
  const pills = () => page.locator('.top .pill').allInnerTexts();
  const saved = () => until('all saved', pills, (lines) => !lines.some((line) => /offline|Saving|Not saved/.test(line)));
  const notes = () => page.locator('.notes .note .line').allInnerTexts();
  const typeOver = async (words) => {
    await openBox(page, 'words');
    await text().click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type(words);
  };
  /** The page's writes of the box wait: `land` lets each reach Calcifer, `answer` lets Calcifer's answer reach the page. */
  const holdWrites = async () => {
    let land, answer;
    const landing = new Promise((done) => (land = done));
    const answering = new Promise((done) => (answer = done));
    const asked = [];
    const path = '**/v1/projects/*/assets/words';
    const route = async (call) => {
      if (call.request().method() !== 'PATCH') return call.fallback();
      asked.push(call.request().postDataJSON());
      await landing;
      const response = await call.fetch();
      await answering;
      await call.fulfill({ response }).catch(() => {});
    };
    await page.route(path, route);
    return { asked, land, answer, done: () => page.unroute(path, route) };
  };
  await boxOf(page, 'words').waitFor();
  await page.evaluate(() => { window.unreloaded = true; });

  // A save lands, typing goes on, and the page reads its words back before the save's answer comes: what's typed stays, and goes next.
  let held = await holdWrites();
  const first = 'Saved, its answer late.';
  await typeOver(first);
  await until('its save asked', () => Promise.resolve(held.asked.length), (n) => n === 1);
  held.land();
  // Typed on with no pause, so it's still typed when the stream says the save landed and the page reads it back.
  const on = ' Typed on, and on, and on.';
  await page.keyboard.type(on, { delay: 90 });
  check('own_save_landed', inDb() === first, inDb());
  check('read_back_keeps_what_is_typed', (await shown()) === first + on, await shown());
  held.answer();
  await held.done();
  await saving(first + on);
  check('typed_on_saved', inDb() === first + on, inDb());
  await saved();

  // Calcifer killed and back while words go on being typed, each with a pause that sends it: no reload, and every character is saved.
  await typeOver('Typed through a restart:');
  await page.evaluate(() => {
    window.pillsSeen = [];
    new MutationObserver(() => {
      for (const pill of document.querySelectorAll('.top .pill')) if (!window.pillsSeen.includes(pill.textContent)) window.pillsSeen.push(pill.textContent);
    }).observe(document.body, { subtree: true, childList: true, characterData: true });
  });
  const pid = holder().pid;
  let restarted;
  for (const [i, word] of WORDS.entries()) {
    if (i === 3) restarted = new Promise((done) => exec(process.env.RESTART, { shell: 'bash' }, (error) => done(error)));
    await page.keyboard.type(` ${word}`, { delay: 25 });
    await page.waitForTimeout(350);
  }
  const all = `Typed through a restart: ${WORDS.join(' ')}`;
  check('calcifer_restarted', !(await restarted) && holder().pid !== pid, { before: pid, after: holder().pid });
  await saved();
  await saving(all);
  seen.restart = { held: inDb(), pills: await page.evaluate(() => window.pillsSeen) };
  check('every_character_saved', inDb() === all, seen.restart);
  check('restart_no_reload', await page.evaluate(() => window.unreloaded === true));
  check('restart_shows_what_was_typed', (await shown()) === all, await shown());

  // Typed, and the box left at once, before the pause that sends it, then opened again: the words are there, and saved (Paul 11-12: "When I type
  // text into a box and then come back to it, the text is gone").
  const left = 'Typed, and left at once.';
  await typeOver(left);
  await closeBox(page);
  await openBox(page, 'words');
  check('left_and_back_shows_what_was_typed', (await shown()) === left, await shown());
  await saving(left);
  check('left_and_back_saved', inDb() === left, inDb());
  await saved();

  // Typed, and the page reloaded before the pause that sends it: it goes as the page goes.
  const quick = 'Typed, and the page reloaded at once.';
  await typeOver(quick);
  await page.reload();
  await boxOf(page, 'words').waitFor();
  await saving(quick);
  check('reload_at_once_saved', inDb() === quick, inDb());

  // Another writer's words while the page's are on their way: the page's unsent words stand until its write is answered, and then both stand,
  // merged by character, with nothing asked (cl-904: Automerge text).
  held = await holdWrites();
  await typeOver('Mine.');
  await until('its save asked', () => Promise.resolve(held.asked.length), (n) => n >= 1);
  const theirs = 'Theirs.';
  const wrote = await fetch(`http://${holder().address}/v1/projects/${id}/assets/words`, {
    method: 'PATCH', headers: { Authorization: `Bearer ${holder().token}`, 'Content-Type': 'application/json' }, body: JSON.stringify({ fields: { text: `${quick} ${theirs}` } }),
  });
  check('theirs_written', wrote.ok && inDb() === `${quick} ${theirs}`, { status: wrote.status, held: inDb() });
  // The stream brings theirs back to the page.
  await page.waitForTimeout(1500);
  check('unsent_words_stand', (await shown()) === 'Mine.', await shown());
  held.land();
  held.answer();
  await held.done();
  await until('both in project.db', () => Promise.resolve(inDb()), (value) => value?.includes('Mine.') && value.includes(theirs)).catch(() => {});
  check('both_writers_words_merged', inDb()?.includes('Mine.') && inDb().includes(theirs), inDb());
  await until('the page shows what was merged', shown, (text) => text === inDb()).catch(() => {});
  check('merged_words_shown', (await shown()) === inDb(), { shown: await shown(), held: inDb() });
  check('nothing_asked', (await page.locator('.card.here button:has-text("Keep mine"), .card.here button:has-text("Take the file")').count()) === 0);

  // Two pages type in the box at once, each in its own place in the words: both stand, merged by character, on both pages.
  await typeOver('Middle.');
  await saving('Middle.');
  await saved();
  const other = await context.newPage();
  other.on('pageerror', (error) => errors.push(error.message));
  await other.goto(page.url());
  await openBox(other, 'words');
  const otherText = () => other.locator('.card.here[data-asset="words"] .mention-text').last();
  await until('the other page reads it', async () => (await otherText().innerText()).trim(), (text) => text === 'Middle.');
  held = await holdWrites();
  await text().click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.type(' A-end.');
  await until('its save asked', () => Promise.resolve(held.asked.length), (n) => n >= 1);
  await otherText().click();
  await other.keyboard.press('ControlOrMeta+Home');
  await other.keyboard.type('B-start. ');
  await saving('B-start. Middle.');
  held.land();
  held.answer();
  await held.done();
  const both = 'B-start. Middle. A-end.';
  await saving(both);
  check('two_pages_merged', inDb() === both, inDb());
  await until('the first page shows both', shown, (said) => said === both).catch(() => {});
  await until('the second page shows both', async () => (await otherText().innerText()).trim(), (said) => said === both).catch(() => {});
  seen.two_pages = { first: await shown(), second: (await otherText().innerText()).trim(), held: inDb() };
  check('two_pages_show_both', seen.two_pages.first === both && seen.two_pages.second === both, seen.two_pages);
  await other.close();

  check('nothing_refused', !(await notes()).some((line) => /Couldn't/.test(line)), await notes());
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
