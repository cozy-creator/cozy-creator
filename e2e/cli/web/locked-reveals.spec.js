// locked-reveals (cl-1130). Paul, 11-15: "once it's encrypted (locked) there is no traces of it on the computer elsewhere, in any way";
// the coordinator, 11-17: "An encrypted project that is locked should reveal nothing about its contents until it is unlocked ... For a run
// that's in flight while the project is locked, the machine view may say 'a locked project's run' and nothing more."
// An encrypted project's run goes on local; local stops answering (its TensorD held with SIGSTOP, as a rental gone quiet), and the project
// is locked: it locks, its run is settled canceled and the stopped machine's clean-up is owed (cl-1064, as 0.26.1 shipped it: "the lock
// goes through, the project's processes on the unresponsive machine are stopped ..., and the cleanup finishes when the machine answers
// again"; it supersedes this cell's first premise, that the lock stayed locking while its run couldn't end, cl-1259). Then, while local
// still doesn't answer and again once its clean-up is paid, every route the page and the CLI read says of
// it no more than that it's there and locked: the machines' list and one machine (no record, id or name of its run: "a locked project's
// run" with its state), the events stream (no machine or run update naming it), the projects index and an address (no key), its key, its
// state, runs, assets, history, canvas, view, nodes, thumbnails and files (refused), a run by its id or name, activity and status; once it
// has locked, the machine's logs too. Local's tab says "A locked project's run", and the project's page says a run is still finishing, not
// that an app holds its files, at 1440×900 and 1920×1080, paper and dark, kept to look at. The coordinator, 11-17: "a run's waiting or held
// text never names another project's run. It says 'another project's run'": a plain project's run waits on local's CPU behind the encrypted
// one's, and neither as it waits nor once that project has locked does its record or its log name the other's run, key or id.
const { execFile, execFileSync } = require('child_process');
const { appendFileSync, mkdirSync, readFileSync } = require('fs');
const { randomBytes } = require('crypto');
const { homedir } = require('os');
const { join } = require('path');
const { test, expect, fixtureTool } = require('./fixtures');

const LOOKS = [
  { width: 1440, height: 900, theme: 'paper' },
  { width: 1440, height: 900, theme: 'dark' },
  { width: 1920, height: 1080, theme: 'paper' },
  { width: 1920, height: 1080, theme: 'dark' },
];
/** A weightless local still that takes `seconds`, warmed by setup.js. */
const SLOW = fixtureTool('tool.py/slow');
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** This computer's machine: `cozy machine serve` (its lock record's pid) and its TensorD (stuck.sh's stuck_service). */
function localMachine() {
  const holder = readFileSync(join(homedir(), '.cozy', 'machine', 'var', 'lib', 'cozy', 'machine', 'agent.lock.holder'), 'utf8');
  const parent = Number(/"pid":\s*(\d+)/.exec(holder)?.[1]);
  if (!parent) throw new Error(`no pid in this computer's machine's lock record: ${holder.slice(0, 200)}`);
  let children = '';
  try { children = execFileSync('ps', ['-o', 'pid=,stat=,comm=', '--ppid', String(parent)]).toString(); } catch { /* none */ }
  const tensord = children.split('\n').map((line) => line.trim().split(/\s+/))
    .filter(([pid, stat, comm]) => pid && comm === 'tensord' && !stat.startsWith('Z')).map(([pid]) => Number(pid));
  return [parent, ...tensord];
}

/** `/v1/events` as the CLI opens it, every event kept as it comes: `{events, close}`. */
function listen(api, query = '') {
  const events = [];
  const stop = new AbortController();
  const done = (async () => {
    const answer = await fetch(`${api.origin}/v1/events${query}`, { headers: { Authorization: `Bearer ${api.token}` }, signal: stop.signal });
    const reader = answer.body.getReader();
    const decoder = new TextDecoder();
    let held = '';
    for (;;) {
      const { value, done: ended } = await reader.read();
      if (ended) return;
      held += decoder.decode(value, { stream: true });
      for (let at; (at = held.indexOf('\n\n')) >= 0; held = held.slice(at + 2)) {
        const data = held.slice(0, at).split('\n').filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trimStart()).join('\n');
        if (data) events.push(JSON.parse(data));
      }
    }
  })().catch(() => {});
  return { events, close: () => { stop.abort(); return done; } };
}

/** What `path` answers as the CLI asks it, whatever its status: `{status, text}`; status 0 when it doesn't answer in `ms`. */
async function read(api, method, path, body, ms = 30_000) {
  try {
    const answer = await fetch(api.origin + path, {
      method,
      headers: { Authorization: `Bearer ${api.token}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(ms),
    });
    return { status: answer.status, text: await answer.text() };
  } catch {
    return { status: 0, text: '' };
  }
}

/** What a stream at `path` says in its first `ms`, or until it ends: `{status, text}`. */
async function heard(api, path, ms = 5_000) {
  const stop = new AbortController();
  const timer = setTimeout(() => stop.abort(), ms);
  let status = 0;
  let text = '';
  try {
    const answer = await fetch(api.origin + path, { headers: { Authorization: `Bearer ${api.token}` }, signal: stop.signal });
    status = answer.status;
    const reader = answer.body.getReader();
    const decoder = new TextDecoder();
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      text += decoder.decode(value, { stream: true });
    }
  } catch { /* its time is up */ } finally {
    clearTimeout(timer);
  }
  return { status, text };
}

test.use({ projectOptions: { encrypted: true } });

/** `cozy <words>` in `dir`, as a person's shell runs it there: {code, stdout, stderr}. */
const cozyIn = (dir, ...words) => new Promise((done) => {
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  execFile(process.env.COZY, words, { cwd: dir, maxBuffer: 1 << 26, timeout: 300_000 }, (error, stdout, stderr) => done({ code: error ? error.code ?? 1 : 0, stdout, stderr }));
});

test('locked-reveals', { tag: '@solo' }, async ({ page, project, api, cli, guards, removeAfter }, testInfo) => {
  test.skip(!(process.platform === 'linux' && process.env.GITHUB_ACTIONS === 'true'), 'it holds this computer\'s machine still, on a fresh GitHub runner');
  test.setTimeout(900_000);
  const shots = join(process.env.OUT, 'web', 'locked-reveals');
  mkdirSync(shots, { recursive: true });
  // A page on a locked project's routes is refused; the browser says each refused load.
  guards.allow(/Failed to load resource/);
  const vault = async () => (await api.get(project.at('/vault'))).state;
  /** Everywhere's appearance: a locked project has no other. Put back as Paper after. */
  const looks = (theme) => api.call('PUT', '/v1/appearance', { theme });
  const all = listen(api);
  const runs = listen(api, '?scope=runs');
  let held = [];
  let going;
  let behind;
  try {
    // The stream shows the project and a machine's view, as the page's does.
    await expect.poll(() => all.events.find((event) => event.type === 'hello')?.stream, { message: 'the events stream says hello' }).toBeTruthy();
    const stream = all.events.find((event) => event.type === 'hello').stream;
    await api.call('PUT', `/v1/events/${stream}`, { projects: [project.id], machines: true });

    let run;
    await test.step('an encrypted project\'s run goes on local', async () => {
      going = cli('run', SLOW, 'seconds:=600', '--on', 'local');
      await expect.poll(async () => {
        run = (await api.get(project.at('/runs'))).runs.find((one) => one.machine_state === 'running');
        return Boolean(run);
      }, { message: 'its run running on local', timeout: 300_000, intervals: [1_000] }).toBe(true);
    });
    const key = run.name.replace(/-\d+$/, '');
    testInfo.annotations.push({ type: 'run', description: JSON.stringify({ id: run.id, name: run.name, target: run.target, key }) });
    expect(key && key !== run.name, `its run is named KEY-N: ${run.name}`).toBe(true);
    const markers = {
      'its run\'s id': new RegExp(escape(run.id)),
      'its run\'s name': new RegExp(`\\b${escape(run.name)}\\b`),
      'its project\'s id': new RegExp(escape(project.id)),
      'its key': new RegExp(`"key"\\s*:\\s*"${escape(key)}"`),
    };
    /** Which markers `text` holds, but those `allowed`. */
    const leaks = (text, allowed = []) => Object.entries(markers).filter(([name, re]) => !allowed.includes(name) && re.test(text)).map(([name]) => name);

    // A plain project whose run waits behind the encrypted one's on local's CPU (one slot on a 4-core runner).
    const folder = join(process.env.OUT, 'web', 'projects', `web-locked-reveals-other-${randomBytes(4).toString('hex')}`);
    removeAfter(folder);
    const other = await api.post('/v1/projects', { path: folder, unencrypted: true });
    const otherAt = (rest = '') => `/v1/projects/${encodeURIComponent(other.id)}${rest}`;
    let waiter;
    await test.step('another project\'s run waits behind it, and says so without naming it', async () => {
      behind = cozyIn(folder, 'run', SLOW, 'seconds:=1', '--on', 'local');
      await expect.poll(async () => {
        waiter = (await api.get(otherAt('/runs'))).runs[0];
        return waiter?.waiting ?? '';
      }, { message: 'the other project\'s run waits for local\'s CPU', timeout: 120_000, intervals: [500] }).toContain('machine\'s CPU');
      testInfo.annotations.push({ type: 'waits', description: waiter.waiting });
      expect.soft(leaks(JSON.stringify(waiter)), `the other project's run waits without naming this one's: ${waiter.waiting}`).toEqual([]);
      expect.soft(waiter.waiting, 'it waits for another project\'s run').toContain('another project\'s run');
    });
    /** What the other project keeps of its run, record and log: nothing of this one. */
    const otherSays = async (when) => {
      for (const path of [otherAt(`/runs/${waiter.number}?detail=full`), otherAt(`/runs/${waiter.number}/events`)]) {
        const answer = await heard(api, path);
        expect.soft(answer.status, `${when}: GET ${path} answered`).toBe(200);
        expect.soft(leaks(answer.text), `${when}: GET ${path} says nothing of the encrypted project`).toEqual([]);
      }
    };
    await otherSays('waiting');
    const ROUTE = [
      // [method, path, refused, markers it may hold, body]
      ['GET', '/v1/machines', false, []],
      ['GET', '/v1/projects?limit=1000', false, ['its project\'s id']],
      ['GET', project.at('/address'), false, ['its project\'s id']],
      ['GET', project.at('/vault'), false, ['its project\'s id']],
      ['GET', project.at('/key'), true, ['its project\'s id']],
      ['GET', project.at(), true, ['its project\'s id']],
      ['GET', project.at('/runs'), true, ['its project\'s id']],
      ['GET', project.at(`/runs/${run.number}`), true, ['its project\'s id']],
      ['GET', project.at('/history'), true, ['its project\'s id']],
      ['GET', project.at('/canvas'), true, ['its project\'s id']],
      ['GET', project.at('/view'), true, ['its project\'s id']],
      ['GET', project.at('/nodes'), true, ['its project\'s id']],
      ['GET', project.at('/benchmarks'), true, ['its project\'s id']],
      ['GET', project.at('/assets/anything'), true, ['its project\'s id']],
      ['GET', project.at(`/thumbs/${'0'.repeat(64)}`), true, ['its project\'s id']],
      ['GET', project.at('/files/anything'), true, ['its project\'s id']],
      ['POST', `/v1/project-files/held?project=${encodeURIComponent(project.id)}`, true, ['its project\'s id'], { digests: [] }],
      ['GET', `/v1/runs/${run.id}`, true, ['its run\'s id']],
      ['GET', `/v1/runs/${run.name}`, true, ['its run\'s name']],
      ['GET', '/v1/activity', false, []],
      ['GET', '/v1/status', false, []],
    ];
    /** Every route, read now: what each holds of the project that it mustn't, and a refused one answered. */
    const readAll = async (when, extra = []) => {
      for (const [method, path, refused, allowed, body] of [...ROUTE, ...extra]) {
        const answer = await read(api, method, path, body);
        expect.soft(leaks(answer.text, allowed), `${when}: ${method} ${path} (${answer.status}) says nothing of the locked project`).toEqual([]);
        if (refused) expect.soft(answer.status >= 400 && answer.status < 500, `${when}: ${method} ${path} refused: ${answer.status}`).toBe(true);
      }
    };
    /** Local's runs, as `/v1/machines` lists them. */
    const localRuns = async () => ((await api.get('/v1/machines')).machines.find((machine) => machine.name === 'local')?.status?.runs ?? []);

    await test.step('local stops answering, and the project locks: its run is settled canceled, the stopped machine\'s clean-up owed', async () => {
      held = localMachine();
      testInfo.annotations.push({ type: 'held', description: held.join(' ') });
      for (const pid of held) process.kill(pid, 'SIGSTOP');
      const said = await cli('--json', 'vault', '--directory', project.path, 'lock');
      expect(said.code, `cozy vault lock: ${said.stdout.slice(-400)} ${said.stderr.slice(-200)}`).toBe(0);
      const answered = JSON.parse(said.stdout.trim().split('\n').at(-1));
      expect(answered.vault?.state, 'the lock goes through though local doesn\'t answer (cl-1064)').toBe('locked');
      expect.soft(answered.ended, 'the lock says it ended the run').toContain(run.number);
      expect.soft(answered.owed, 'the lock says local\'s clean-up is owed for when it answers').toBeTruthy();
      expect(await vault(), 'it is locked').toBe('locked');
    });
    // What either stream says from here is said with the project locked.
    await new Promise((done) => setTimeout(done, 1_000));
    const [allFrom, runsFrom] = [all.events.length, runs.events.length];

    await test.step('locked, local still silent: every route says nothing of it, and local says only that a locked project\'s run is there', async () => {
      await readAll('locked, local silent');
      await otherSays('locked, local silent');
      const there = await localRuns();
      expect.soft(there.filter((one) => one.run?.project_id === project.id || (one.id ?? '').includes(run.id) || one.run?.id === run.id), 'local holds no record of its run').toEqual([]);
      const sealed = there.filter((one) => one.locked);
      expect.soft(sealed.map((one) => Object.keys(one).sort()), 'local says a locked project\'s run is there, its state and nothing more').toEqual([['locked', 'state']]);
      // A machine read now, as `cozy machine show` reads it, isn't answered: the stream hears local change.
      const one = await read(api, 'GET', '/v1/machines/local', undefined, 60_000);
      expect.soft(leaks(one.text), `locked, local silent: GET /v1/machines/local (${one.status}) says nothing of the locked project`).toEqual([]);
      await expect.poll(() => all.events.slice(allFrom).some((event) => event.type === 'machine.updated' && event.machine?.name === 'local'),
        { message: 'the stream tells local changed', timeout: 60_000 }).toBe(true);
    });

    await test.step('local\'s tab says "A locked project\'s run", at each size', async () => {
      for (const { width, height, theme } of LOOKS) {
        const tag = `${width}x${height}-${theme}`;
        await page.setViewportSize({ width, height });
        await looks(theme);
        await page.goto(project.url());
        const gate = page.locator('[data-vault-state="locked"]');
        await expect(gate, 'the project\'s page says it is locked').toBeVisible();
        await page.screenshot({ path: join(shots, `locked-page-${tag}.png`) });
        await page.getByRole('navigation', { name: 'Machines', exact: true }).locator('.mtab[data-machine="local"]').click();
        const working = page.getByRole('dialog', { name: /^What .+ is doing$/ });
        await expect(working, 'a click on local lists what it is doing').toBeVisible();
        // Its run first, as it's running; the other project's after it, waiting.
        await expect.soft(working.locator('li.doing[data-doing="other"] b').first(), `${tag}: it says a locked project's run is there`).toHaveText('A locked project\'s run');
        await expect.soft(working.getByText('This project\'s run'), `${tag}: and nothing of whose it is`).toHaveCount(0);
        await page.screenshot({ path: join(shots, `locked-machine-${tag}.png`) });
        await page.keyboard.press('Escape');
      }
    });

    await test.step('local answers again: the project stays locked, and its owed clean-up is paid', async () => {
      for (const pid of [...held].reverse()) process.kill(pid, 'SIGCONT');
      held = [];
      expect(await vault(), 'still locked').toBe('locked');
      await expect.poll(async () => (await localRuns()).filter((one) => one.locked || one.run?.project_id === project.id).length,
        { message: 'local holds nothing of it once its clean-up is paid', timeout: 120_000, intervals: [1_000] }).toBe(0);
    });

    await test.step('locked: every route and the machine\'s logs say nothing of it', async () => {
      await readAll('locked', ['tensord', 'tensorfs', 'calcifer'].map((log) => ['GET', `/v1/machines/local/logs/${log}?lines=0`, false, []]));
      expect.soft((await localRuns()).filter((one) => one.locked || one.run?.project_id === project.id), 'local holds nothing of it').toEqual([]);
      await expect.poll(async () => (await api.get(otherAt(`/runs/${waiter.number}`))).status, { message: 'the other project\'s run ended', timeout: 120_000, intervals: [1_000] })
        .toBe('completed');
      await otherSays('locked');
    });

    await test.step('the streams said nothing of it once it was locking', async () => {
      const told = all.events.slice(allFrom).filter((event) => event.type !== 'project.vault');
      expect.soft(told.filter((event) => leaks(JSON.stringify(event)).length).map((event) => `${event.type}: ${leaks(JSON.stringify(event)).join(', ')}`),
        'no event names it').toEqual([]);
      expect.soft(runs.events.slice(runsFrom).filter((event) => leaks(JSON.stringify(event)).length).map((event) => event.type), 'no run event names it').toEqual([]);
    });
  } finally {
    for (const pid of held) { try { process.kill(pid, 'SIGCONT'); } catch { /* gone */ } }
    await all.close();
    await runs.close();
    await looks('paper');
    // The encrypted project is locked before its folder goes: a mounted view keeps it.
    if ((await vault().catch(() => 'locked')) !== 'locked') await cli('--json', 'vault', '--directory', project.path, 'lock');
    await Promise.race([Promise.all([going, behind]), new Promise((done) => setTimeout(done, 30_000))]);
  }
  guards.over();
});
