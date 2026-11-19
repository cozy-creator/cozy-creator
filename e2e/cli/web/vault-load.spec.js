// vault-load (xs-215 calcifer 4, "one waitable project gate"; Paul, 11-17: no refusal that protects nothing). An encrypted project's
// lock, deadline lock, decrypt, encrypt, rename and move to the trash, each under a page's load: a render going, its pictures'
// thumbnails asked six at a time as a browser asks, and its runs read while the change waits. Nothing is refused for being busy
// (`project_busy`), a read asked once a closing change has the project is refused as locked, and each change ends within its bound;
// the times are kept to compare (`vault-load.json`). Then:
// - a page's read of a run going, queued behind a lock that waits past `JOURNAL_BUSY` (5 s) for a stalled upload, is answered once the
//   lock has had its turn (the run, or `project_locked` once the lock closes the view), never `project_busy`;
// - a render's output, landing as its project locks, is in the project with its version after the next unlock;
// - a program holding a file open in the view, which nothing ends: past `LOCK_FORCE` (60 s) the deadline lock forces the close, the
//   encrypted filesystem's provider is gone, and Calcifer says so once, at error; what Calcifer itself has open in the view then is kept
//   in the artifact (`calcifer-files.json`);
// - a page lock waiting behind a read while a cancel's edit, begun just after, asks for the run journal holding the project's writer:
//   the lock completes (on 9c2571cd87 the two waited on each other for good, review 7004122095).
const { execFile, execFileSync, spawn } = require('child_process');
const { mkdirSync, readFileSync, readdirSync, readlinkSync, writeFileSync, existsSync } = require('fs');
const { createHash, randomBytes, randomUUID } = require('crypto');
const http = require('http');
const { basename, dirname, join } = require('path');
const { homedir } = require('os');
const { test, expect, fixtureTool, png } = require('./fixtures');

const SLOW = `${fixtureTool('tool.py')}/slow`;
const PICTURES = 8;
const AT_ONCE = 6;
/** A closing change's bound (`LOCK_DRAIN`), and what a lock past it may take besides: its run's cancel settling. */
const DRAIN_MS = 10_000;
/** The deadline lock's bound (`LOCK_FORCE`). */
const FORCE_MS = 60_000;

test.use({ projectOptions: { encrypted: true } });

/** `{status, code}` of an answer, its body read. */
async function answered(response) {
  const text = await response.text().catch(() => '');
  let code;
  try { code = JSON.parse(text)?.error?.code; } catch { /* not JSON */ }
  return { status: response.status, code };
}

/** A page's load on the project: its thumbnails six at a time, at widths the drawers haven't made yet, and its runs read each 100 ms,
 *  every answer kept with when it was asked. `stop()` ends it and says what came back. */
function load(api, project, shas) {
  const began = Date.now();
  const answers = [];
  let going = true;
  const ask = async (what, path) => {
    const asked = Date.now() - began;
    const said = await api.raw('GET', path).then(answered, () => ({ status: 0 }));
    answers.push({ what, asked_ms: asked, ...said });
  };
  const thumbs = Array.from({ length: AT_ONCE }, async (_, lane) => {
    for (let i = lane; going; i += AT_ONCE) await ask('thumb', project.at(`/thumbs/${encodeURIComponent(shas[i % shas.length])}?w=${96 + (i % 24) * 8}`));
  });
  const reads = (async () => {
    while (going) {
      await ask('runs', project.at('/runs'));
      await new Promise((done) => setTimeout(done, 100));
    }
  })();
  return {
    began,
    answers,
    async stop() {
      going = false;
      await Promise.all([...thumbs, reads]);
      return answers;
    },
  };
}

/** What a load's answers came to, from `from_ms` on: counts by kind, status and code, and those refused for being busy. */
function tally(answers, from_ms = 0) {
  const counts = {};
  for (const one of answers.filter((one) => one.asked_ms >= from_ms)) {
    const key = `${one.what} ${one.status}${one.code ? ` ${one.code}` : ''}`;
    counts[key] = (counts[key] ?? 0) + 1;
  }
  return { counts, busy: answers.filter((one) => one.asked_ms >= from_ms && one.code === 'project_busy').length };
}

test('vault-load', { tag: '@solo' }, async ({ project, api, cli, removeAfter }, testInfo) => {
  test.skip(!(process.platform === 'linux' && process.env.GITHUB_ACTIONS === 'true'), 'it holds a mount and renders on local, on a fresh GitHub runner');
  test.setTimeout(1_200_000);
  const work = join(process.env.OUT, 'web', 'vault-load');
  mkdirSync(work, { recursive: true });
  const secret = join(work, `passphrase-${randomBytes(4).toString('hex')}`);
  writeFileSync(secret, `${project.passphrase}\n`, { mode: 0o600 });
  let folder = project.path;
  const seen = {};
  const keep = () => writeFileSync(join(work, 'vault-load.json'), JSON.stringify(seen, null, 2));
  const vault = async () => (await api.get(project.at('/vault')));
  const cozy = (...words) => cli('--json', 'vault', '--directory', folder, ...words);
  const unlock = async (idle) => {
    const said = await cozy('unlock', '--passphrase-file', secret, '--idle', String(idle));
    expect(said.code, `unlock: ${said.stdout.slice(-300)} ${said.stderr.slice(-300)}`).toBe(0);
    return JSON.parse(said.stdout).vault.view;
  };
  /** Until the vault reads `state`, or `ms`: how long it took, null if it never did. */
  const until = async (state, ms) => {
    const began = Date.now();
    while (Date.now() - began < ms) {
      if ((await vault()).state === state) return Date.now() - began;
      await new Promise((done) => setTimeout(done, 200));
    }
    return null;
  };
  /** A slow still asked of local, going: the render's promise once its run runs there. */
  const rendering = async (name, seconds) => {
    await project.add({ name, tool: SLOW, kind: 'image', fields: { instructions: `${name}, slowly.`, seconds } });
    const render = cli('--json', 'render', name, '--on', 'local');
    await expect.poll(async () => (await api.get(project.at('/runs'))).runs.some((run) => run.machine_state === 'running'),
      { message: `${name}'s run running on local`, timeout: 300_000, intervals: [500] }).toBe(true);
    return render;
  };
  /** A lock asked as a person asks it, timed: its answer's state or error, and when it was asked against `load`'s start. */
  const locking = async (from) => {
    const asked = Date.now();
    const said = await cozy('lock');
    let answer = {};
    try { answer = JSON.parse(said.stdout); } catch { /* none */ }
    return { asked_ms: asked - from, answer_ms: Date.now() - asked, code: said.code, state: answer.vault?.state ?? null, error: answer.error?.code ?? null };
  };

  const shas = [];
  await test.step(`${PICTURES} pictures imported and their thumbnails drawn`, async () => {
    for (let i = 0; i < PICTURES; i++) {
      const bytes = png(i * 30, 480, 320);
      await project.import(bytes, { name: `pic_${i}` });
      shas.push(`sha256:${createHash('sha256').update(bytes).digest('hex')}`);
    }
    await expect.poll(async () => (await Promise.all(shas.map((sha) => api.raw('GET', project.at(`/thumbs/${encodeURIComponent(sha)}`)).then((answer) => answer.status)))).every((status) => status === 200),
      { message: 'every thumbnail drawn', timeout: 120_000, intervals: [500] }).toBe(true);
  });

  await test.step('a lock while a render goes and a page reads: nothing refused for being busy, late reads refused as locked', async () => {
    const render = await rendering('slow_lock', 120);
    const page = load(api, project, shas);
    await new Promise((done) => setTimeout(done, 1_500));
    const lock = await locking(page.began);
    const locked_ms = await until('locked', 90_000);
    const answers = await page.stop();
    await render;
    const late = answers.filter((one) => one.asked_ms >= lock.asked_ms + lock.answer_ms);
    seen.lock = { ...lock, locked_ms, all: tally(answers), late: tally(answers, lock.asked_ms + lock.answer_ms) };
    keep();
    expect.soft(lock.error, `the lock's answer: ${JSON.stringify(lock)}`).toBeNull();
    expect.soft(locked_ms, 'locked once its run settled').not.toBeNull();
    expect.soft(seen.lock.all.busy, `answers refused for being busy: ${JSON.stringify(seen.lock.all.counts)}`).toBe(0);
    expect.soft(late.length > 0 && late.every((one) => one.code === 'project_locked'), `reads asked once it answered: ${JSON.stringify(seen.lock.late.counts)}`).toBe(true);
  });

  await test.step('a read queued behind a lock that waits past JOURNAL_BUSY for a stalled upload is answered once the lock has had its turn', async () => {
    await unlock(0);
    // A run going, read as the page reads it.
    const render = await rendering('slow_queued', 120);
    const run = (await api.get(project.at('/runs'))).runs.find((one) => one.machine_state === 'running');
    expect(run, 'a run of the project going').toBeTruthy();
    const body = randomBytes(1 << 20);
    const digest = `sha256:${createHash('sha256').update(body).digest('hex')}`;
    const at = new URL(api.origin);
    // One body, its first part sent and then still for 7 s: its read of the project held all that while.
    const upload = new Promise((done) => {
      const sending = http.request({ host: at.hostname, port: at.port, method: 'PUT', path: `/v1/project-files/${digest}?project=${encodeURIComponent(project.id)}`,
        headers: { Authorization: `Bearer ${api.token}`, 'Content-Type': 'application/octet-stream', 'Content-Length': body.length } },
        (answer) => { answer.resume(); answer.on('end', () => done({ status: answer.statusCode })); });
      sending.on('error', (error) => done({ ended: error.code ?? error.message }));
      sending.write(body.subarray(0, 64 << 10));
      setTimeout(() => sending.end(body.subarray(64 << 10)), 7_000);
    });
    await new Promise((done) => setTimeout(done, 1_000));
    const began = Date.now();
    const lock = locking(began);
    // The lock queued behind the upload's read by now: this read waits behind the lock.
    await new Promise((done) => setTimeout(done, 1_500));
    const asked = Date.now();
    const read = await api.raw('GET', project.at(`/runs/${encodeURIComponent(run.id)}`)).then(answered, () => ({ status: 0 }));
    seen.queued = { read: { ...read, after_ms: Date.now() - asked }, lock: await lock, upload: await upload, locked_ms: await until('locked', 90_000) };
    await render;
    keep();
    // Its turn came after the lock's: the run as it was, or refused as locked once the lock closed the view. Never busy, never at once.
    expect.soft(read.status === 200 || read.code === 'project_locked', `the run read behind the lock: ${JSON.stringify(seen.queued)}`).toBe(true);
    expect.soft(seen.queued.read.after_ms, `it waited its turn: ${JSON.stringify(seen.queued)}`).toBeGreaterThan(3_000);
    expect.soft(seen.queued.lock.error, `the lock behind the upload: ${JSON.stringify(seen.queued.lock)}`).toBeNull();
    expect.soft(seen.queued.locked_ms, 'locked').not.toBeNull();
  });

  await test.step('a render\'s output landing as its project locks is in the project, with its version, after the next unlock', async () => {
    const tries = [];
    for (let i = 0; i < 3; i++) {
      await unlock(0);
      const name = `last_${i}`;
      const before = new Set((await api.get(project.at('/runs'))).runs.map((one) => one.id));
      await project.add({ name, tool: SLOW, kind: 'image', fields: { instructions: `${name}, its last write.`, seconds: 2 } });
      const render = cli('--json', 'render', name, '--on', 'local');
      // Locked the moment its run completes: its output lands and its version is written as the lock closes the project.
      let run;
      const began = Date.now();
      while (!run && Date.now() - began < 300_000) {
        run = (await api.get(project.at('/runs'))).runs.find((one) => !before.has(one.id) && one.status === 'completed');
        if (!run) await new Promise((done) => setTimeout(done, 20));
      }
      expect(run, `${name}'s run completed`).toBeTruthy();
      const lock = await locking(began);
      await render;
      await until('locked', 90_000);
      await unlock(0);
      let versions = 0;
      await expect.poll(async () => (versions = (await project.asset(name))?.versions ?? 0), { message: `${name}'s version after the unlock`, timeout: 60_000, intervals: [500] }).toBe(1);
      tries.push({ run: run.id, lock, versions });
      await cozy('lock');
      await until('locked', 90_000);
    }
    seen.last = tries;
    keep();
  });

  await test.step('the deadline lock under a page\'s load', async () => {
    await unlock(8);
    const render = await rendering('slow_deadline', 12);
    const page = load(api, project, shas);
    await render;
    // Its reads hold the idle clock: they end with the render, and the idle limit counts from there.
    const answers = await page.stop();
    const locked_ms = await until('locked', 90_000);
    seen.deadline = { idle_s: 8, locked_after_render_ms: locked_ms, all: tally(answers) };
    keep();
    expect.soft(locked_ms, 'locked at its idle deadline').not.toBeNull();
    expect.soft(locked_ms ?? Infinity, 'locked within its idle limit and a lock\'s drain').toBeLessThanOrEqual(8_000 + DRAIN_MS + 5_000);
    expect.soft(seen.deadline.all.busy, `answers refused for being busy: ${JSON.stringify(seen.deadline.all.counts)}`).toBe(0);
  });

  await test.step('a decrypt, then an encrypt, under a page\'s load', async () => {
    await unlock(0);
    let page = load(api, project, shas);
    await new Promise((done) => setTimeout(done, 1_500));
    const decrypting = Date.now();
    const decrypt = await cozy('decrypt');
    const decrypt_ms = Date.now() - decrypting;
    let answers = await page.stop();
    seen.decrypt = { code: decrypt.code, ms: decrypt_ms, error: decrypt.code ? decrypt.stdout.slice(-300) : null, all: tally(answers) };
    expect.soft(decrypt.code, `decrypt: ${decrypt.stdout.slice(-300)}`).toBe(0);
    expect.soft((await vault()).state, 'plain once decrypted').toBe('plain');
    expect.soft(seen.decrypt.all.busy, `answers refused for being busy: ${JSON.stringify(seen.decrypt.all.counts)}`).toBe(0);
    page = load(api, project, shas);
    await new Promise((done) => setTimeout(done, 1_500));
    const encrypting = Date.now();
    const encrypt = await cozy('encrypt', '--passphrase-file', secret, '--recovery-out', join(work, `recovery-${randomBytes(4).toString('hex')}`), '--idle', '0');
    const encrypt_ms = Date.now() - encrypting;
    answers = await page.stop();
    seen.encrypt = { code: encrypt.code, ms: encrypt_ms, error: encrypt.code ? encrypt.stdout.slice(-300) : null, all: tally(answers) };
    keep();
    expect.soft(encrypt.code, `encrypt: ${encrypt.stdout.slice(-300)}`).toBe(0);
    expect.soft((await vault()).state, 'unlocked once encrypted').toBe('unlocked');
    expect.soft(seen.encrypt.all.busy, `answers refused for being busy: ${JSON.stringify(seen.encrypt.all.counts)}`).toBe(0);
  });

  await test.step('a rename under a page\'s load: it locks first, then moves, and moves back', async () => {
    if ((await vault()).state !== 'unlocked') await unlock(0);
    const page = load(api, project, shas);
    await new Promise((done) => setTimeout(done, 1_500));
    const was = basename(folder);
    const renaming = Date.now();
    const moved = await api.raw('PUT', project.at('/name'), { body: { name: `${was}-moved` } }).then(answered);
    const rename_ms = Date.now() - renaming;
    const answers = await page.stop();
    seen.rename = { ...moved, ms: rename_ms, all: tally(answers) };
    keep();
    expect.soft(moved.status, `the rename: ${JSON.stringify(seen.rename)}`).toBe(200);
    expect.soft(seen.rename.all.busy, `answers refused for being busy: ${JSON.stringify(seen.rename.all.counts)}`).toBe(0);
    if (moved.status === 200) {
      folder = join(dirname(folder), `${was}-moved`);
      // Locked, it's named by its folder: the index keeps no id of a locked project (cl-984).
      const back = await api.raw('PUT', `/v1/projects/${encodeURIComponent(folder)}/name`, { body: { name: was } }).then(answered);
      seen.rename.back = back;
      expect.soft(back.status, `the rename back: ${JSON.stringify(back)}`).toBe(200);
      if (back.status === 200) folder = join(dirname(folder), was);
    }
  });

  await test.step('a program holding a file in the view: past LOCK_FORCE the deadline lock forces the close, said once at error', async () => {
    const view = await unlock(5);
    const held = join(view, 'held-open.txt');
    writeFileSync(held, 'held open by another program\n');
    const unlocked = Date.now();
    // Another program with a file open in the view, which ignores SIGTERM and nothing ends: as an editor left open would.
    const holder = spawn('python3', ['-c', 'import signal, sys, time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\nf = open(sys.argv[1], "rb")\nprint("held", flush=True)\ntime.sleep(900)', held], { stdio: ['ignore', 'pipe', 'ignore'] });
    try {
      await new Promise((done) => holder.stdout.once('data', done));
      const providers = () => readdirSync('/proc').filter((pid) => /^\d+$/.test(pid) && pid !== String(holder.pid)).filter((pid) => {
        try { return readFileSync(`/proc/${pid}/cmdline`, 'utf8').includes(view); } catch { return false; }
      });
      const before = providers();
      // What Calcifer itself has open in the view, by path within it: kept in the artifact only, never said by Calcifer.
      const calciferPid = JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).pid;
      const calciferFiles = () => readdirSync(`/proc/${calciferPid}/fd`).map((fd) => { try { return readlinkSync(`/proc/${calciferPid}/fd/${fd}`); } catch { return ''; } })
        .filter((to) => to === view || to.startsWith(`${view}/`)).map((to) => to.slice(view.length) || '/');
      const files = {};
      await until('locking', 30_000);
      files.locking = calciferFiles();
      await new Promise((done) => setTimeout(done, 50_000));
      files.near_force = (await vault()).state === 'locked' ? null : calciferFiles();
      writeFileSync(join(work, 'calcifer-files.json'), JSON.stringify(files, null, 2));
      const locked_ms = await until('locked', FORCE_MS + 60_000);
      const mounted = readFileSync('/proc/self/mountinfo', 'utf8').split('\n').some((line) => line.includes(view));
      // Calcifer's own log, where it says what it did to a vault (`~/.cozy/calcifer/diagnostics.log`).
      const logged = readFileSync(join(homedir(), '.cozy', 'calcifer', 'diagnostics.log'), 'utf8');
      const forced = logged.split('\n').filter((line) => line.includes('had to force it closed') && line.includes(basename(folder)));
      seen.forced = { idle_s: 5, locked_after_unlock_ms: locked_ms === null ? null : Date.now() - unlocked, providers_before: before.length, providers_after: providers().length, mounted, error_lines: forced.length, line: forced[0]?.slice(0, 400) ?? null, calcifer_files: files };
      keep();
      expect.soft(locked_ms, 'locked past LOCK_FORCE').not.toBeNull();
      expect.soft(before.length, 'its provider was running').toBeGreaterThan(0);
      expect.soft(providers().length, 'its provider is gone').toBe(0);
      expect.soft(mounted, 'its view is off the mount table').toBe(false);
      expect.soft(forced.length, `the forced close said once at error: ${JSON.stringify(forced)}`).toBe(1);
      expect.soft(/\berror\b/i.test(forced[0] ?? ''), `said at error: ${forced[0]}`).toBe(true);
      // Cozy holds nothing inside the view: its mount's handle on the directory beneath it ("/", project-mount's ViewGuard) is no file.
      expect.soft([...(files.locking ?? []), ...(files.near_force ?? [])].filter((path) => path !== '/'), 'what Cozy holds inside the view').toEqual([]);
      expect.soft(/calcifer \(/.test(forced[0] ?? ''), `the forced close names only what else holds it: ${forced[0]}`).toBe(false);
    } finally {
      holder.kill('SIGKILL');
    }
  });

  await test.step('a move to the trash while a render goes and a page reads', async () => {
    await unlock(0);
    const render = await rendering('slow_trash', 120);
    const page = load(api, project, shas);
    await new Promise((done) => setTimeout(done, 1_500));
    const trashing = Date.now();
    const trashed = await api.raw('DELETE', project.at()).then(answered);
    const trash_ms = Date.now() - trashing;
    const answers = await page.stop();
    await render;
    seen.trash = { ...trashed, ms: trash_ms, gone: !existsSync(folder), all: tally(answers) };
    keep();
    expect.soft(trashed.status, `the move to the trash: ${JSON.stringify(seen.trash)}`).toBe(200);
    expect.soft(seen.trash.gone, 'its folder is gone from where it was').toBe(true);
    expect.soft(seen.trash.all.busy, `answers refused for being busy: ${JSON.stringify(seen.trash.all.counts)}`).toBe(0);
  });

  await test.step('a cancel whose edit begins while a page lock waits on a read: the lock completes', async () => {
    // A project of its own, named as the suite names what it makes, removed after the cell.
    const folder2 = `${project.path}-edit-${randomBytes(4).toString('hex')}`;
    const made = await api.post('/v1/projects', { path: folder2, passphrase: project.passphrase });
    removeAfter(folder2);
    const at = (rest = '') => `/v1/projects/${encodeURIComponent(made.id)}${rest}`;
    const view = (await api.get(at('/vault'))).view;
    const origin = new URL(api.origin);
    const open = (method, path, headers) => http.request({ host: origin.hostname, port: origin.port, method, path, headers: { Authorization: `Bearer ${api.token}`, ...headers } });
    const answer = (request) => new Promise((done) => {
      request.on('response', (response) => { let text = ''; response.on('data', (chunk) => { text += chunk; }); response.on('end', () => { let code; try { code = JSON.parse(text)?.error?.code; } catch { /* none */ } done({ status: response.statusCode, code }); }); });
      request.on('error', (error) => done({ ended: error.code ?? error.message }));
    });
    // A read held: an upload's first part, then still for 8 s.
    const body = randomBytes(1 << 20);
    const upload = open('PUT', `/v1/project-files/sha256:${createHash('sha256').update(body).digest('hex')}?project=${encodeURIComponent(made.id)}`, { 'Content-Type': 'application/octet-stream', 'Content-Length': body.length });
    const uploaded = answer(upload);
    upload.write(body.subarray(0, 64 << 10));
    setTimeout(() => upload.end(body.subarray(64 << 10)), 8_000);
    await new Promise((done) => setTimeout(done, 500));
    // A keyed cancel, its request let in before the lock: its body, sent after the lock queued, starts its edit, which holds the
    // project's writer and asks for the run journal.
    const ask = JSON.stringify({ names: [] });
    const cancel = open('POST', at('/cancel'), { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(ask), 'Idempotency-Key': `vault-load-${randomBytes(6).toString('hex')}` });
    const canceled = answer(cancel);
    cancel.flushHeaders();
    await new Promise((done) => setTimeout(done, 500));
    // The page's lock: an operation, as the page sends it (`/vault/operations`), whose first turn admits its receipt.
    const began = Date.now();
    const lock = open('PUT', at(`/vault/operations/p3_${randomUUID()}`), { 'Content-Type': 'application/json' });
    const locked = answer(lock);
    lock.end(JSON.stringify({ action: 'lock' }));
    await new Promise((done) => setTimeout(done, 1_000));
    cancel.end(ask);
    let state = '';
    while (Date.now() - began < 45_000) {
      state = (await api.get(at('/vault')).catch(() => ({})))?.state ?? '';
      if (state === 'locked') break;
      await new Promise((done) => setTimeout(done, 250));
    }
    seen.edit = { state, locked_ms: state === 'locked' ? Date.now() - began : null };
    if (state !== 'locked') {
      // Never locked: locked as a person would, bounded; else its view let go, so nothing under OUT is a mount the job can't collect.
      for (const request of [upload, cancel, lock]) request.destroy();
      const cozy = new Promise((done) => execFile(process.env.COZY, ['--json', 'vault', '--directory', folder2, 'lock'], { cwd: dirname(folder2), timeout: 60_000 }, (error) => done(!error)));
      seen.edit.cleanup = (await cozy) ? 'locked' : 'not locked';
      if ((await api.get(at('/vault')).catch(() => ({})))?.state !== 'locked') {
        for (const unmount of ['fusermount3', 'fusermount']) {
          try { execFileSync(unmount, ['-u', '-z', view]); seen.edit.cleanup += `; ${unmount} -u -z`; break; } catch (error) { seen.edit.cleanup += `; ${unmount}: ${String(error.message).slice(0, 120)}`; }
        }
      }
    }
    seen.edit.cancel = await Promise.race([canceled, new Promise((done) => setTimeout(() => done({ unanswered: true }), 5_000))]);
    seen.edit.lock = await Promise.race([locked, new Promise((done) => setTimeout(() => done({ unanswered: true }), 5_000))]);
    seen.edit.upload = await Promise.race([uploaded, new Promise((done) => setTimeout(() => done({ unanswered: true }), 5_000))]);
    keep();
    expect.soft(state, `the page's lock behind a read, beside a cancel's edit: ${JSON.stringify(seen.edit)}`).toBe('locked');
  });

  console.log(`vault-load: ${JSON.stringify(seen)}`);
  await testInfo.attach('vault-load', { body: JSON.stringify(seen, null, 2), contentType: 'application/json' });
});
