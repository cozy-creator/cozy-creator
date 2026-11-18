// outsider-run (cl-1242), the release coordinator's ruling on an open encrypted project's run read from outside it: "What must not reach
// a reader outside the project is anything the user wrote: a box's name, a file name, a prompt." While an encrypted project's run goes
// on this computer's machine, in a stage saying its prompt's words (said.py), nothing its project wrote reaches a reader outside it: not
// its project's name or folder, its key (`KEY-N` is made from that name), its box or its stage's words; nor its own code's name on its
// run (the machine's installed environments still list it, a known leak cl-1253 closes, recorded here and not judged). Read four
// ways: `cozy --json machine show` and `cozy machine show` from outside the project, the machines list with no project, and what a page
// showing another project hears on its event stream. Each still lists the run, by its id, so this judges something; the encrypted
// project's own view still has every word; and the other project's page still reads "Another project's run".
// On the CPU e2e alone (web-outsider-run, with the project-encryption driver).
const { execFile } = require('child_process');
const { appendFileSync } = require('fs');
const { randomBytes } = require('crypto');
const { join, basename } = require('path');
const { test, expect, canvasIdle, fixtureTool } = require('./fixtures');

const MACHINE = 'local';
const TOOL = fixtureTool('said.py/said');
const BOX = 'quillon_heron_box';
const WORDS = 'Zephyrine marmalade orchard';
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** `cozy <words>` in `dir`, as a person's shell runs it there: {code, stdout, stderr}. */
const cozyIn = (dir, ...words) => new Promise((done) => {
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  execFile(process.env.COZY, words, { cwd: dir, maxBuffer: 1 << 26, timeout: 300_000 }, (error, stdout, stderr) => done({ code: error ? error.code ?? 1 : 0, stdout, stderr }));
});

test('outsider-run', { tag: '@vault' }, async ({ page, project, api, removeAfter }) => {
  test.skip(!(process.platform === 'linux' && process.env.GITHUB_ACTIONS === 'true'), "it runs an encrypted project's run on this computer's machine, on a fresh GitHub runner");
  test.setTimeout(900_000);
  const outside = join(process.env.OUT, 'web');
  const folder = join(outside, 'projects', `web-outsider-quillon-marsh-7319-${randomBytes(3).toString('hex')}`);
  removeAfter(folder);
  const vault = await api.post('/v1/projects', { path: folder, passphrase: randomBytes(18).toString('base64url') });
  const vaultAt = (rest = '') => `/v1/projects/${encodeURIComponent(vault.id)}${rest}`;

  await api.post(vaultAt('/assets'), { name: BOX, tool: TOOL, exact: true, kind: 'image', fields: { instructions: WORDS, seconds: 240 }, at: { x: 0, y: 0 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(vault.id)}`)).tools.map((tool) => tool.id), { message: 'the local tool listed', timeout: 120_000 })
    .toContain(TOOL);
  // Another project's page, open before the run starts, hearing the stream its tabs share (`client.events`, `client.show`) as one
  // showing that project and the machines' view: a machine's events come as it changes, as when a run starts on it.
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(project.url());
  await expect(page.locator('.stage')).toBeVisible();
  await canvasIdle(page);
  await page.evaluate(async (shown) => {
    window.__heard = '';
    window.__hearing = new AbortController();
    const answer = await fetch('/v1/events', { headers: { Accept: 'text/event-stream' }, signal: window.__hearing.signal });
    const reader = answer.body.getReader();
    const decoder = new TextDecoder();
    let told = false;
    (async () => {
      for (;;) {
        const read = await reader.read().catch(() => ({ done: true }));
        if (read.done) return;
        window.__heard += decoder.decode(read.value, { stream: true });
        const stream = (window.__heard.match(/"type":"hello"[^}]*"stream":"([^"]+)"/) ?? [])[1];
        if (stream && !told) {
          told = true;
          await fetch(`/v1/events/${encodeURIComponent(stream)}`, { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ projects: [shown], machines: true, gpus: false }) });
        }
      }
    })();
  }, project.id);
  const rendering = cozyIn(folder, '--json', 'render', BOX, '--on', MACHINE);
  let run;
  try {
    // Its run, in its own project's view, in the stage that says its words.
    await expect.poll(async () => {
      run = (await api.get(vaultAt('/runs?limit=20'))).runs.find((each) => (each.progress?.stage ?? '').includes(WORDS));
      return Boolean(run);
    }, { message: 'the encrypted project\'s run, in its stage saying its words', timeout: 600_000, intervals: [500] }).toBe(true);
    const key = run.name.replace(/-\d+$/, '');
    const named = (await api.get(vaultAt(''))).name ?? '';
    test.info().annotations.push({ type: 'its own view', description: JSON.stringify({ id: run.id, name: run.name, key, project: run.project, named, target: run.target, stage: run.progress?.stage }) });
    expect(run.name, 'its own view names it by its key').toMatch(/^[A-Z0-9]+-\d+$/);
    // What the user wrote or chose, any of which outside the project is a leak.
    const markers = {
      "its project's folder": new RegExp(escape(basename(folder))),
      "its project's name": new RegExp(escape(run.project || named || basename(folder))),
      'its key': new RegExp(`\\b${escape(key)}-\\d+\\b`),
      'its box': new RegExp(escape(BOX)),
      "its stage's words": /Zephyrine|marmalade/,
    };
    const leaks = (text) => Object.entries(markers).filter(([, re]) => re.test(text)).map(([name]) => name);
    // Its own code's name is judged on its run's entry alone. A known leak remains, recorded and not judged: the machine's installed
    // environments list a local package by name to any reader (`status.environments[].package`), plain or encrypted, as its project's
    // own code; a local package is its project's (the release coordinator, 11-18), and cl-1253 makes it so. When it lands, judge
    // `ownCode` on the whole of each document here, as `markers` are.
    const ownCode = /local\/said\b/;
    /** Its run's entries in a machine's picture, a machines list or a stream's machine events, as text. */
    const entries = (doc) => {
      const machines = doc?.machines ?? (doc?.machine ? [doc.machine] : [doc]);
      return machines.flatMap((machine) => machine?.status?.runs ?? []).filter((held) => held.id === run.id || held.run?.id === run.id).map((held) => JSON.stringify(held));
    };
    /** Where in a JSON document each leak is: its paths, so a red says which part of the machine carries it. */
    const where = (value, at = '$', found = []) => {
      if (typeof value === 'string') {
        for (const [name, re] of [...Object.entries(markers), ["its own code's name", ownCode]]) if (re.test(value)) found.push(`${name} at ${at}`);
      } else if (value && typeof value === 'object') {
        for (const [key, inner] of Object.entries(value)) where(inner, Array.isArray(value) ? `${at}[${key}]` : `${at}.${key}`, found);
      }
      return found;
    };
    const looks = {};

    await test.step('`cozy --json machine show` from outside the project lists the run by its id, and nothing its project wrote', async () => {
      const shown = await cozyIn(outside, '--json', 'machine', 'show', MACHINE);
      looks.json = shown.stdout;
      expect(shown.code, `machine show: ${shown.stderr.slice(0, 300)}`).toBe(0);
      expect(shown.stdout, 'it lists the run').toContain(run.id);
      const listed = (JSON.parse(shown.stdout).status?.runs ?? []).find((held) => held.id === run.id || held.run?.id === run.id);
      test.info().annotations.push({ type: 'machine show --json, its run', description: JSON.stringify(listed ?? null).slice(0, 1500) });
      test.info().annotations.push({ type: 'machine show --json, where (a leak at $.status.environments is cl-1253, not judged here)', description: JSON.stringify(where(JSON.parse(shown.stdout))) });
      expect.soft(leaks(shown.stdout), 'nothing of its project\'s').toEqual([]);
      expect.soft(entries(JSON.parse(shown.stdout)).filter((entry) => ownCode.test(entry)), "its run's entry doesn't name its own code").toEqual([]);
      // Its record, field by field (`Run::outsider`): words a run makes (`text`) never ride on it, whatever the run, and none of what
      // its project wrote does. This run streams no words (no CPU fixture does without a model); the shape says none could.
      const record = listed?.run ?? {};
      const may = ['id', 'number', 'name', 'machine_number', 'kind', 'target', 'hub', 'machine', 'project_id', 'status', 'cancel_requested', 'machine_state',
        'output', 'created_at_ms', 'accepted_at_ms', 'started_at_ms', 'settled_at_ms', 'progress', 'queue_position', 'execution_ms'];
      expect.soft(Object.keys(record).filter((field) => !may.includes(field)), 'its record has only what an outsider may know: no text, project, log lines, reason or result').toEqual([]);
      expect.soft([record.name ?? '', record.target ?? '', record.hub ?? '', (record.output ?? []).length, record.progress?.stage ?? '', listed?.waiting ?? ''],
        'no name, tool, hub, outputs, stage or waiting words').toEqual(['', '', '', 0, '', '']);
    });

    await test.step('`cozy machine show` from outside the project: the same', async () => {
      const shown = await cozyIn(outside, 'machine', 'show', MACHINE);
      const lines = shown.stdout.split('\n').filter((line) => line.includes(run.id) || /^\s*run /.test(line) || leaks(line).length);
      test.info().annotations.push({ type: 'machine show, its lines', description: JSON.stringify(lines) });
      expect.soft(leaks(shown.stdout), 'nothing of its project\'s').toEqual([]);
      expect.soft(shown.stdout.split('\n').filter((line) => line.includes(run.id) && ownCode.test(line)), "its run's line doesn't name its own code").toEqual([]);
    });

    await test.step('the machines list read with no project: the same', async () => {
      const document = await api.get('/v1/machines');
      const listed = JSON.stringify(document);
      expect(listed, 'it lists the run').toContain(run.id);
      expect.soft(leaks(listed), 'nothing of its project\'s').toEqual([]);
      expect.soft(entries(document).filter((entry) => ownCode.test(entry)), "its run's entry doesn't name its own code").toEqual([]);
    });

    await test.step("a page showing another project: what its event stream hears, and its machine tab says \"Another project's run\"", async () => {
      let heard = '';
      await expect.poll(async () => {
        heard = await page.evaluate(() => window.__heard);
        return heard.includes('"machine.updated"') && heard.includes(run.id);
      }, { message: 'the page hears the machine with the run', timeout: 120_000, intervals: [1000] }).toBe(true);
      await page.evaluate(() => window.__hearing.abort());
      looks.stream = heard;
      expect.soft(leaks(heard), 'nothing of its project\'s').toEqual([]);
      const events = heard.split('\n').filter((line) => line.startsWith('data: {')).map((line) => { try { return JSON.parse(line.slice(6)); } catch { return null; } })
        .filter((event) => event?.type === 'machine.updated');
      expect.soft(events.flatMap(entries).filter((entry) => ownCode.test(entry)), "its run's entry doesn't name its own code").toEqual([]);
      await page.locator(`nav.mtabs .mtab[data-machine="${MACHINE}"]`).click();
      const list = page.getByRole('dialog', { name: /is doing$/ });
      await expect(list).toBeVisible();
      await expect(list.getByText("Another project's run"), "the page reads it as another project's").toBeVisible({ timeout: 30_000 });
      await page.screenshot({ path: test.info().outputPath('machine-tab-1440x900-paper.png') });
      await page.keyboard.press('Escape');
    });

    await test.step("its own project's view still has every word", async () => {
      const own = (await api.get(vaultAt('/runs?limit=20'))).runs.find((each) => each.id === run.id);
      expect(own?.name).toBe(run.name);
      expect(own?.progress?.stage ?? '').toContain(WORDS);
    });
    test.info().annotations.push({ type: 'leaks', description: JSON.stringify({ json: leaks(looks.json ?? ''), stream: leaks(looks.stream ?? '') }) });
  } finally {
    // Its run holds for minutes: canceled, in its project, once judged.
    if (run) await cozyIn(folder, '--json', 'run', 'cancel', String(run.number));
    await rendering;
    // Locked, its folder can go.
    await cozyIn(folder, '--json', 'vault', 'lock');
  }
});
