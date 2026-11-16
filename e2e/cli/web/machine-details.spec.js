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
// paper and dark.
const { execFileSync } = require('child_process');
const { randomUUID } = require('crypto');
const { rmSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { test, expect, fixtureTool } = require('./fixtures');

const SLOW = `${fixtureTool('tool.py')}/fetching`;
const MODEL = 'zhengpeng7/birefnet';
const PACKAGE = 'minimax-h3-tools';
const SIZES = [[1440, 900], [1920, 1080]];
const GB = 2 ** 30;
const softly = expect.configure({ soft: true });

/** The page's rule (Disk.tsx `nearlyFull`): under a tenth free, or too little past the machine's reserve for its biggest model. */
function nearlyFull(status) {
  const total = status.disk_total_bytes, free = status.disk_free_bytes;
  if (!total) return false;
  const reserve = Math.min(10 * GB, Math.max(GB, total / 50));
  const biggest = Math.max(0, ...(status.models ?? []).map((model) => model.total_bytes));
  return free < total / 10 || (biggest > 0 && free < reserve + biggest);
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
      await expect(warning).toContainText(/^Nearly full: \d+(\.\d)?(MB|GB) free of \d+(\.\d)?(GB|TB)/);
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
