// A save past project.db's write lock, held by another program (a SQLite browser's open transaction) longer than Calcifer waits. What the
// person did stays done, the top bar says what's in the way, nothing is written while it's held, and it's saved when it's let go, with no
// reload and nothing pressed again.
//   node held.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $HELD_FOLDER, $HELD_ID: a project of two Edits, `stays` and `goes`.
// Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { spawn } = require('child_process');
const { join } = require('path');
const { createInterface } = require('readline');
const { PYTHON, calcifer, signIn, STEP_MS, boxOf, lastLook, launch, lookAt, noStall, pointAt, rowsOf, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

/** An acquired SQLite writer, released by EOF only after the refusal and UI are observed. */
async function holdDatabase(file, events) {
  const script = 'import sqlite3, sys\nc = sqlite3.connect(sys.argv[1], isolation_level=None)\nc.execute("BEGIN IMMEDIATE")\nprint("acquired", flush=True)\nsys.stdin.buffer.read(1)\nc.execute("ROLLBACK")\nprint("released", flush=True)';
  const child = spawn(PYTHON, ['-c', script, file]);
  let stderr = '', released = false;
  child.stdin.on('error', (error) => events.push({ kind: 'holder_input_error', message: error.message, at_ms: Date.now() }));
  child.stderr.on('data', (bytes) => { stderr = (stderr + bytes).slice(-8192); });
  const closed = new Promise((done) => {
    child.once('error', (error) => done({ error: error.message }));
    child.once('close', (code, signal) => done({ code, signal }));
  });
  const lines = createInterface({ input: child.stdout });
  const acquired = new Promise((done, fail) => {
    lines.on('line', (line) => {
      events.push({ kind: 'lock', event: line, at_ms: Date.now(), pid: child.pid });
      if (line === 'acquired') done();
    });
    closed.then((end) => fail(new Error(`SQLite holder ended before acquisition: ${JSON.stringify(end)} ${stderr}`)));
  });
  const release = async () => {
    if (!released) {
      released = true;
      events.push({ kind: 'release_requested', at_ms: Date.now() });
      if (child.exitCode === null && child.signalCode === null) child.stdin.end();
    }
    const end = await closed;
    lines.close();
    if (end.error || end.code !== 0) throw new Error(`SQLite holder: ${JSON.stringify(end)} ${stderr}`);
  };
  try { await acquired; }
  catch (error) { await release().catch(() => {}); throw error; }
  return { release, held: () => child.exitCode === null && child.signalCode === null };
}

/** Only this deletion's request/replies, with no credentials or unrelated response bodies. */
function deletionEvidence(page, pathname, events) {
  const replies = [], pending = new Set();
  const matches = (request) => request.method() === 'DELETE' && new URL(request.url()).pathname === pathname;
  const requested = (request) => {
    if (matches(request)) events.push({ kind: 'request', method: request.method(), path: pathname, at_ms: Date.now() });
  };
  const answered = (response) => {
    if (!matches(response.request())) return;
    const read = (async () => {
      const reply = { kind: 'reply', status: response.status(), path: pathname, at_ms: Date.now() };
      try { reply.body = await response.json(); }
      catch (error) { reply.unread = error.message; }
      events.push(reply);
      replies.push(reply);
    })();
    pending.add(read);
    void read.then(() => pending.delete(read), () => pending.delete(read));
  };
  page.on('request', requested);
  page.on('response', answered);
  return { replies, stop: async () => {
    page.off('request', requested);
    page.off('response', answered);
    await Promise.allSettled([...pending]);
  } };
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
  const events = [];
  seen.lock = { events };
  let holder, evidence;
  try {
    page.on('pageerror', (error) => errors.push(error.message));
    await signIn(page);
    await page.waitForSelector('.top');
    await page.goto(`${base}/#/projects/${encodeURIComponent(id)}`);
    await boxOf(page, 'goes').waitFor();

    holder = await holdDatabase(join(folder, 'project.db'), events);
    evidence = deletionEvidence(page, `/v1/projects/${encodeURIComponent(id)}/assets/goes`, events);
    const shut = page.locator('.react-flow__node .card[data-asset="goes"]');
    await pointAt(page, shut.locator('.nm'));
    await shut.locator('.head [aria-label=Delete]').click();
    await until('gone from the canvas', () => page.locator('.react-flow__node .card[data-asset="goes"]').count(), (n) => n === 0);
    check('shown_at_once', true);
    const refusal = await until('this delete answers file_held while the lock is held', () => Promise.resolve(evidence.replies.find((reply) => reply.status === 503 && reply.body?.error?.code === 'file_held')), Boolean);
    check('refused_while_held', holder.held(), refusal);
    const note = page.locator('.top .pill:has-text("project.db is open in another program")');
    await note.waitFor();
    check('says_what_holds_it', true);
    const owed = await note.innerText();
    check('owed_while_held', /\b[1-9]\d* changes? waiting\b/.test(owed), owed);
    events.push({ kind: 'held_ui', text: owed, at_ms: Date.now() });
    check('not_written_while_held', boxes().includes('goes'), boxes());
    await holder.release();
    await until('saved once it is let go', () => Promise.resolve(boxes()), (names) => !names.includes('goes'));
    check('saved_once_let_go', boxes().includes('stays'), boxes());
    await until('the note goes', () => note.count(), (n) => n === 0);
    check('the_note_goes', true);
    check('no_page_errors', errors.length === 0, errors);
  } catch (error) {
    events.push({ kind: 'test_error', message: error.message, at_ms: Date.now() });
    throw error;
  } finally {
    try { if (holder) await holder.release(); }
    finally {
      await browser.close();
      if (evidence) await evidence.stop();
    }
  }
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
