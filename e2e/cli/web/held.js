// A save past project.db's write lock, held by another program (a SQLite browser's open transaction) longer than Calcifer waits. What the
// person did stays done, the top bar says what's in the way, nothing is written while it's held, and it's saved when it's let go, with no
// reload and nothing pressed again.
//   node held.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $HELD_FOLDER, $HELD_ID: a project of two Edits, `stays` and `goes`.
// Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { spawn } = require('child_process');
const { join } = require('path');
const { PYTHON, calcifer, signIn, STEP_MS, boxOf, lastLook, launch, lookAt, noStall, pointAt, rowsOf, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

(async () => {
  const base = calcifer();
  const [folder, id] = [process.env.HELD_FOLDER, process.env.HELD_ID];
  seen.project = { id, folder };
  // Its boxes, read past Calcifer: a reader isn't kept out by another's write lock.
  const boxes = () => rowsOf(folder, 'SELECT name FROM boxes ORDER BY ord').map(([name]) => name);
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1400, height: 800 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/#/projects/${encodeURIComponent(id)}`);
  await boxOf(page, 'goes').waitFor();

  // Its write lock taken, for longer than Calcifer waits on it (10 s).
  const hold = 'import sqlite3, sys, time\nc = sqlite3.connect(sys.argv[1], isolation_level=None)\nc.execute("BEGIN IMMEDIATE")\nprint("held", flush=True)\ntime.sleep(14)\nc.execute("ROLLBACK")';
  const holder = spawn(PYTHON, ['-c', hold, join(folder, 'project.db')]);
  const letGo = new Promise((done) => holder.once('exit', done));
  await new Promise((done, fail) => {
    holder.stdout.once('data', done);
    holder.once('error', fail);
    holder.once('exit', () => fail(new Error('nothing held it')));
  });

  const shut = page.locator('.react-flow__node .card[data-asset="goes"]');
  await pointAt(page, shut.locator('.nm'));
  await shut.locator('.head [aria-label=Delete]').click();
  await until('gone from the canvas', () => page.locator('.react-flow__node .card[data-asset="goes"]').count(), (n) => n === 0);
  check('shown_at_once', true);
  const note = page.locator('.top .pill:has-text("project.db is open in another program")');
  await note.waitFor();
  check('says_what_holds_it', true);
  check('not_written_while_held', boxes().includes('goes'), boxes());
  await letGo;
  await until('saved once it is let go', () => Promise.resolve(boxes()), (names) => !names.includes('goes'));
  check('saved_once_let_go', boxes().includes('stays'), boxes());
  await until('the note goes', () => note.count(), (n) => n === 0);
  check('the_note_goes', true);
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
