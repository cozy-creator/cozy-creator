// machine-details (cl-1100), Paul 11-16: "when you open up the details of a machine, it should also show the following: 1. it's run queue,
// list of jobs / operations queued up / in progress 2. a list of packages installed (package section), and a list of models downloaded
// (model section) 3. its disc-space usage 4. a list of its GPUs and GPU utilization. This is what Runpod does. That way we can see how hard
// its GPUs are working. Making sure its GPUs are working, and making sure disc space isn't too full, is probably the top thing we need to
// get right"
// This computer's machine's drawer, opened once and never reloaded, so all of it is the machine stream's: its GPUs as it reports them (a
// runner's: none); its disk as a bar, free of total and the share used as it reports them, by models, packages, Runtime, run files and the
// rest; BiRefNet installed by the CLI under Models with its size, and a package under Packages with its version; three renders of a slow
// tool in the queue, the running one first and the other two queued, by their boxes' names; and with the disk filled past nearly full
// (under a tenth free), the warning, gone once it's freed. Solo: it fills the runner's disk. Screenshots of each, 1440×900 and 1920×1080,
// paper and dark. Its second test, GPU use, is PR B's (below).
const { execFileSync } = require('child_process');
const { randomUUID } = require('crypto');
const { rmSync, writeFileSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { test, expect, fixtureTool } = require('./fixtures');

const SLOW = `${fixtureTool('tool.py')}/fetching`;
const MODEL = 'zhengpeng7/birefnet';
const PACKAGE = 'minimax-h3-tools';
const SIZES = [[1440, 900], [1920, 1080]];
const GB = 2 ** 30;
const softly = expect.configure({ soft: true });

/** Calcifer's rule, which the page shows (Disk.tsx `nearlyFull`, cl-1175): its room for downloads under what its usual models need. */
function nearlyFull(status) {
  return Boolean(status.disk_total_bytes && status.disk_needs_bytes);
}

test('machine-details', { tag: '@solo' }, async ({ page, project, api, cli }) => {
  test.setTimeout(600_000);
  const local = async () => (await api.get('/v1/machines/local')).status;
  const drawer = page.locator('.drawer');
  const section = (title) => drawer.locator('section.sect').filter({ has: page.locator('h3 > span', { hasText: new RegExp(`^${title}$`) }) });
  const shoot = async (label, titles) => {
    for (const [width, height] of SIZES) {
      await page.setViewportSize({ width, height });
      for (const theme of ['paper', 'dark']) {
        await page.evaluate((dark) => {
          if (dark) document.documentElement.dataset.theme = 'dark';
          else delete document.documentElement.dataset.theme;
          return new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        }, theme === 'dark');
        await section(titles[0]).scrollIntoViewIfNeeded();
        await page.screenshot({ path: test.info().outputPath(`${label}-${width}x${height}-${theme}.png`) });
        for (const title of titles) await section(title).screenshot({ path: test.info().outputPath(`${label}-${title.toLowerCase()}-${width}x${height}-${theme}.png`) });
      }
    }
    await page.evaluate(() => { delete document.documentElement.dataset.theme; });
    await page.setViewportSize({ width: 1440, height: 900 });
  };

  // Its drawer, open before anything happens on it: from here every change reaches it by the machines' stream.
  await cli.json('machine', 'show', 'local');
  await page.goto(project.url('machines/local'));
  await expect(section('GPUs'), 'the drawer has a GPUs section').toBeVisible({ timeout: 60_000 });

  await test.step('GPUs: each it reports, or none', async () => {
    const gpus = (await local()).gpus ?? [];
    if (!gpus.length) await expect(section('GPUs')).toContainText('No GPUs. Runs here use the CPU.');
    for (const gpu of gpus) await expect(section('GPUs').locator(`[data-gpu="${gpu.index}"]`)).toContainText(gpu.name);
  });

  await test.step(`Models and Packages: ${MODEL} and ${PACKAGE} installed by the CLI appear, with a size and a version`, async () => {
    const queue = section('Queue');
    let downloading = false;
    const watch = setInterval(() => {
      void queue.locator('[data-queue="download"]').count().then((n) => { downloading ||= n > 0; }, () => {});
    }, 250);
    const done = (said, what) => { if (said.code !== 0) throw new Error(`cozy --json ${what}: exit ${said.code}: ${said.stderr.slice(-500)} ${said.stdout.slice(-500)}`); };
    try {
      done(await cli('--json', 'model', 'install', MODEL, '--on', 'local'), `model install ${MODEL}`);
    } finally {
      clearInterval(watch);
    }
    // Seen only if its bytes take longer than a glance: said, not judged.
    test.info().annotations.push({ type: 'download row', description: downloading ? `the queue showed ${MODEL}'s download` : `${MODEL} came too fast to see` });
    const model = section('Models').locator(`[data-model="${MODEL}"]`);
    await expect(model, `${MODEL} under Models`).toBeVisible({ timeout: 60_000 });
    await expect(model.locator('.size')).toHaveText(/^\d+(\.\d)?(B|KB|MB|GB|TB)$/);
    done(await cli('--json', 'package', 'install', PACKAGE, '--on', 'local'), `package install ${PACKAGE}`);
    const installed = async () => (await local()).environments?.find((env) => env.package.endsWith(PACKAGE)) ?? null;
    await expect.poll(installed, { message: `${PACKAGE} installed on local`, timeout: 60_000 }).not.toBeNull();
    const held = await installed();
    const row = section('Packages').locator(`[data-package="${held.package}"]`);
    await expect(row, `${PACKAGE} under Packages`).toBeVisible({ timeout: 60_000 });
    await expect(row.locator('.what')).toContainText(held.release);
  });

  await test.step('Disk: the bar, free of total and the share used as the machine says, by what fills it', async () => {
    const disk = section('Disk');
    await expect(disk.locator('.diskbar')).toBeVisible();
    await expect.poll(async () => {
      const status = await local();
      const said = (await disk.locator('.disk-sum').textContent()) ?? '';
      const used = Number(/(\d+)% used/.exec(said)?.[1] ?? NaN);
      return Math.abs(used - (100 * (status.disk_total_bytes - status.disk_free_bytes)) / status.disk_total_bytes) <= 1;
    }, { message: 'its share used, within a point of what it reports', timeout: 60_000 }).toBe(true);
    await expect(disk.locator('.disk-legend li')).toHaveText([/^Models \d/, /^Packages \d/, /^Runtime \d/, /^Run files \d/, /^Other files \d/], { timeout: 90_000 });
    await expect(disk.locator('.disk-legend [data-kind="models"] b')).not.toHaveText('0B', { timeout: 90_000 });
    await shoot('installed', ['GPUs', 'Disk', 'Packages', 'Models']);
  });

  const names = ['q1', 'q2', 'q3'];
  let fill = null;
  try {
    await test.step('Queue: three renders, the running one first, the other two queued, by their boxes', async () => {
      for (const [at, name] of names.entries()) {
        await project.add({ name, tool: SLOW, kind: 'image', fields: { seconds: 240 + at }, at: { x: at * 360, y: 0 } });
      }
      await api.call('POST', project.at('/runs'), { names, on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
      const runs = section('Queue').locator('[data-queue="run"]');
      await expect.poll(async () => (await runs.evaluateAll((rows) => rows.map((row) => row.dataset.state))).join(' '), { message: 'one running first, two waiting', timeout: 180_000 })
        .toMatch(/^running (queued|preparing) (queued|preparing)$/);
      const making = new Set();
      for (const name of names) for (const run of (await project.asset(name)).making ?? []) making.add(run);
      const shown = await runs.evaluateAll((rows) => rows.map((row) => Number(row.dataset.run)));
      softly(shown.every((run) => making.has(run)) && shown.length === 3, `the rows are q1-q3's runs: ${JSON.stringify({ shown, making: [...making] })}`).toBe(true);
      softly((await runs.locator('b').allTextContents()).sort(), 'each row named for its box').toEqual(names);
      await expect(runs.nth(1).locator('.said')).toHaveText(/^Queued|^Waiting|^Getting ready/);
      await shoot('queue', ['Queue']);
    });

    await test.step('Nearly full: the warning shows with the disk filled to under a tenth free, and goes once it is freed', async () => {
      const before = await local();
      const total = before.disk_total_bytes;
      // Past nearly full, still above where the machine starts clearing its caches (its reserve, 1/50 of the disk, 1-10GB).
      const leave = Math.max(Math.min(10 * GB, Math.max(GB, total / 50)) + GB, total * 0.05);
      const take = before.disk_free_bytes - leave;
      if (take > 0) {
        fill = join(homedir(), `cl-1100-fill-${randomUUID()}`);
        execFileSync('fallocate', ['-l', String(Math.floor(take)), fill]);
      }
      await expect.poll(async () => nearlyFull(await local()), { message: 'the machine reports its disk nearly full', timeout: 90_000 }).toBe(true);
      const warning = section('Disk').locator('.disk-full');
      await expect(warning).toBeVisible({ timeout: 30_000 });
      await expect(warning).toContainText(/^Nearly full: \d+(\.\d)?(MB|GB) of room for models, and its usual models need \d+(\.\d)?(GB|TB)/);
      await expect(warning).toContainText('free up space on this computer');
      await shoot('nearly-full', ['Disk']);
      if (fill) rmSync(fill, { force: true });
      fill = null;
      await expect.poll(async () => nearlyFull(await local()) === ((await warning.count()) > 0), { message: 'the warning as the rule says, once freed', timeout: 90_000 }).toBe(true);
      const after = await local();
      if (!nearlyFull(after)) await expect(warning).toHaveCount(0);
    });
  } finally {
    if (fill) rmSync(fill, { force: true });
    await api.post(project.at('/cancel'), { names: ['all'] }).catch(() => {});
  }
});

/** The first `count` `gpus` events of Calcifer's stream of `name`'s GPU use, each parsed, then the stream closed; fewer if none come for
 *  `wait` ms. */
async function gpuReadings(calcifer, name, count, wait = 5_000) {
  const stop = new AbortController();
  const answer = await fetch(`${calcifer.origin}/v1/machines/${encodeURIComponent(name)}/gpus`, {
    headers: { Authorization: `Bearer ${calcifer.token}`, Accept: 'text/event-stream' },
    signal: stop.signal,
  });
  if (!answer.ok) throw new Error(`GET /v1/machines/${name}/gpus: ${answer.status} ${(await answer.text()).slice(0, 300)}`);
  const reader = answer.body.pipeThrough(new TextDecoderStream()).getReader();
  const readings = [];
  let held = '';
  try {
    while (readings.length < count) {
      const next = await Promise.race([reader.read(), new Promise((done) => setTimeout(() => done(null), wait))]);
      if (!next || next.done) break;
      held += next.value;
      for (let end = held.indexOf('\n\n'); end >= 0; end = held.indexOf('\n\n')) {
        const lines = held.slice(0, end).split('\n');
        held = held.slice(end + 2);
        if (lines.includes('event: gpus') || lines.includes('event:gpus')) readings.push(JSON.parse(lines.filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trim()).join('\n')));
      }
    }
  } finally {
    stop.abort();
  }
  return readings;
}

// Paul's 4th: "a list of its GPUs and GPU utilization ... That way we can see how hard its GPUs are working". Calcifer's stream of a
// machine's GPU use answers as SSE, a reading a second while followed and none otherwise. A machine with no NVIDIA GPU (a runner's) sends
// one empty reading, and its drawer opens no stream. With NVIDIA GPUs (GPU_MACHINE names one, on a pod), the readings come each second,
// and the drawer shows each GPU's busy share and memory in use under it, as the stream says.
test('machine-details: GPU use', { tag: '@solo' }, async ({ page, project, api, calcifer, cli }) => {
  const name = process.env.GPU_MACHINE || 'local';
  if (name === 'local') await cli.json('machine', 'show', 'local');
  const status = (await api.get(`/v1/machines/${encodeURIComponent(name)}`)).status;
  const gpus = status?.gpus ?? [];
  const readings = await gpuReadings(calcifer, name, gpus.length ? 3 : 2);
  if (!gpus.length) {
    expect(readings, 'one empty reading from a machine with no GPU, then nothing').toEqual([{ at_unix_ms: expect.any(Number), gpus: [] }]);
  } else {
    expect(readings.length, 'a reading each second').toBe(3);
    expect(readings[2].at_unix_ms - readings[0].at_unix_ms, 'two seconds across three readings').toBeGreaterThan(1_500);
    for (const reading of readings) {
      for (const gpu of reading.gpus) {
        expect(gpu.busy_percent).toBeGreaterThanOrEqual(0);
        expect(gpu.busy_percent).toBeLessThanOrEqual(100);
        expect(gpu.memory_used_bytes).toBeLessThanOrEqual(gpus.find((one) => one.index === gpu.index).memory_bytes);
      }
    }
  }

  const asked = [];
  page.on('request', (request) => { if (new URL(request.url()).pathname.endsWith('/gpus')) asked.push(request.url()); });
  if (process.env.GPU_LOG) await page.addInitScript(heardGpus);
  await page.goto(project.url(`machines/${encodeURIComponent(name)}`));
  const section = page.locator('.drawer section.sect').filter({ has: page.locator('h3 > span', { hasText: /^GPUs$/ }) });
  await expect(section).toBeVisible({ timeout: 60_000 });
  if (!gpus.length) {
    await expect(section).toContainText('No GPUs');
    await page.waitForTimeout(3_000);
    expect(asked, 'no GPU stream for a machine with none').toEqual([]);
    return;
  }
  for (const gpu of gpus) {
    const busy = section.getByRole('meter', { name: `GPU ${gpu.index} busy` });
    await expect(busy, `GPU ${gpu.index}'s busy share shows`).toBeVisible({ timeout: 30_000 });
    await expect(section.locator(`[data-gpu="${gpu.index}"] .gpu-use b`).first()).toHaveText(/^\d{1,3}%$/);
    await expect(section.getByRole('meter', { name: `GPU ${gpu.index} memory in use` })).toBeVisible();
  }
  expect(asked.length, 'one stream while the drawer is open').toBeGreaterThan(0);
  for (const [width, height] of SIZES) {
    await page.setViewportSize({ width, height });
    for (const theme of ['paper', 'dark']) {
      await page.evaluate((dark) => { if (dark) document.documentElement.dataset.theme = 'dark'; else delete document.documentElement.dataset.theme; }, theme === 'dark');
      await section.screenshot({ path: test.info().outputPath(`gpu-use-${width}x${height}-${theme}.png`) });
    }
  }
  // GPU_LOG (release-gate/sessions/llm-tools.sh, during renders on the rental): for GPU_LOG_S seconds, twice a second, what the drawer
  // shows for each GPU beside the last reading the page heard; then every reading it heard. The session holds these to nvidia-smi.
  if (process.env.GPU_LOG && gpus.length) {
    await page.evaluate(() => { delete document.documentElement.dataset.theme; });
    const until = Date.now() + Number(process.env.GPU_LOG_S || 60) * 1000;
    const lines = [];
    while (Date.now() < until) {
      lines.push(await page.evaluate(() => ({
        at: Date.now(),
        last: window.__gpuLast ?? null,
        shown: [...document.querySelectorAll('.drawer li.gpu[data-gpu]')].map((gpu) => ({
          index: Number(gpu.dataset.gpu),
          busy: Number(gpu.querySelector('.meter.busy')?.getAttribute('aria-valuenow') ?? NaN),
          memory: gpu.querySelectorAll('.gpu-use b')[1]?.textContent ?? null,
        })),
      })));
      await page.waitForTimeout(500);
    }
    writeFileSync(process.env.GPU_LOG, lines.map((line) => JSON.stringify(line)).join('\n') + '\n');
    const heard = await page.evaluate(() => window.__gpuHeard ?? []);
    writeFileSync(`${process.env.GPU_LOG}.heard`, heard.map((one) => JSON.stringify(one)).join('\n') + '\n');
    expect(heard.length, 'the page heard a reading each second').toBeGreaterThan(Number(process.env.GPU_LOG_S || 60) / 2);
  }
});

/** In the page, before its scripts: each `gpus` reading its stream of a machine's GPU use brings, kept in `__gpuHeard` (the last in
 *  `__gpuLast`), the page's own copy of the stream untouched. */
function heardGpus() {
  const real = window.fetch;
  window.__gpuHeard = [];
  window.fetch = async function (input, init) {
    const response = await real.call(this, input, init);
    try {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      if (!url.pathname.endsWith('/gpus') || (init?.method ?? 'GET') !== 'GET' || !response.body) return response;
      const [ours, theirs] = response.body.tee();
      (async () => {
        const reader = ours.pipeThrough(new TextDecoderStream()).getReader();
        let held = '';
        for (;;) {
          const { value, done } = await reader.read();
          if (done) return;
          held += value;
          for (let end = held.indexOf('\n\n'); end >= 0; end = held.indexOf('\n\n')) {
            const lines = held.slice(0, end).split('\n');
            held = held.slice(end + 2);
            if (!lines.some((line) => /^event: ?gpus$/.test(line))) continue;
            const reading = JSON.parse(lines.filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trim()).join('\n'));
            window.__gpuLast = reading;
            window.__gpuHeard.push({ heard: Date.now(), ...reading });
          }
        }
      })().catch(() => {});
      return new Response(theirs, { status: response.status, statusText: response.statusText, headers: response.headers });
    } catch {
      return response;
    }
  };
}
