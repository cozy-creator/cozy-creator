// shared-download (cl-1205), Paul 11-17, watching kirill fetch MiniMax H3 for three jobs that all need it: "This view is also misleading;
// it looks like we're downloading it 3 times, but we're actually just downloading it once. It should show the download occurring, and then
// it should also show the 3 jobs that are like 'waiting for download...' and they don't need to independently report the progress in
// theirs as well". Three boxes of weighted.py's still, each reading BiRefNet's raw weights, rendered at once on this cold machine over a
// link slowed to 10 MB/s (ondemand.sh's slow_store), so the weights take about 45 s. While they come: the machine lists one download of them,
// its bytes moving, naming all three runs; each run's progress names that download (`waiting_on`) and says it waits, with no bytes of its
// own; the machine's tab and its drawer list the download once and each run as waiting on it, a link to it; each box says "Waiting for
// Birefnet to download" with no count. Screenshots of the canvas, the tab's list and the drawer at 1440×900, paper and dark, are kept with
// the job.
const { execFileSync } = require('child_process');
const { test, expect, themed, canvasIdle, fixtureTool } = require('./fixtures');

const WEIGHTED = fixtureTool('weighted.py/weighted');
const MODEL = 'zhengpeng7/birefnet';
const BOXES = ['first', 'second', 'third'];
const softly = expect.configure({ soft: true });

/** The isolated Hub's store link slowed to `rate` (tc on the loopback, from its MinIO's port), as ondemand.sh's slow_store; undone after. */
function slowStore(rate = '240mbit') {
  const minio = execFileSync('pgrep', ['-af', 'minio'], { encoding: 'utf8' });
  const port = minio.match(/--address[ =]127\.0\.0\.1:(\d+)/)?.[1];
  if (!port) throw new Error(`the isolated Hub's MinIO port wasn't found: ${minio.slice(0, 300)}`);
  const tc = (...args) => execFileSync('sudo', ['tc', ...args], { stdio: 'pipe' });
  tc('qdisc', 'add', 'dev', 'lo', 'root', 'handle', '1:', 'prio');
  tc('qdisc', 'add', 'dev', 'lo', 'parent', '1:3', 'handle', '30:', 'tbf', 'rate', rate, 'burst', '1mb', 'latency', '1s');
  tc('filter', 'add', 'dev', 'lo', 'parent', '1:', 'protocol', 'ip', 'prio', '1', 'u32', 'match', 'ip', 'sport', port, '0xffff', 'flowid', '1:3');
  return () => { try { tc('qdisc', 'del', 'dev', 'lo', 'root'); } catch { /* already gone */ } };
}

/** Each box's run line on the canvas: its words, its count and whether its words lead to its download (presentation `leads`). */
const faces = (page) => page.evaluate((names) => Object.fromEntries(names.map((name) => {
  const shown = document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered;
  return [name, shown?.progress ? { words: shown.labels.progress?.fullText ?? null, count: shown.labels.count?.fullText ?? null, leads: Boolean(shown.progress.leads) } : null];
})), BOXES);

test('shared-download', async ({ page, project, api, cli }) => {
  test.setTimeout(300_000);
  const shoot = async (label, region) => {
    for (const theme of ['paper', 'dark']) {
      await page.evaluate((dark) => {
        if (dark) document.documentElement.dataset.theme = 'dark';
        else delete document.documentElement.dataset.theme;
        return new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      }, theme === 'dark');
      await themed(page, theme);
      await page.screenshot({ path: test.info().outputPath(`${label}-1440x900-${theme}.png`) });
      if (region) await region.screenshot({ path: test.info().outputPath(`${label}-1440x900-${theme}-region.png`) });
    }
    await page.evaluate(() => { delete document.documentElement.dataset.theme; });
  };
  for (const [at, name] of BOXES.entries()) {
    await project.add({ name, tool: WEIGHTED, kind: 'image', fields: { instructions: `A still, ${name}.`, seconds: 1 }, at: { x: at * 380, y: 0 } });
  }
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the local tool listed', timeout: 120_000 })
    .toContain(WEIGHTED);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(project.url());
  await expect(page.locator('.stage')).toBeVisible();
  await canvasIdle(page);

  const restore = slowStore('80mbit');
  try {
    const rendering = cli('--json', 'render', ...BOXES, '--on', 'local');
    const runs = async () => (await api.get(project.at('/runs?limit=100'))).runs.filter((run) => run.kind === 'call' && run.target === 'local/weighted/weighted');
    const local = async () => (await api.get('/v1/machines')).machines.find((machine) => machine.name === 'local');

    let item, waiting;
    await test.step('the machine lists one download of the weights, naming all three runs; each run names it and says no bytes of its own', async () => {
      await expect.poll(async () => {
        const downloads = ((await local())?.status?.activity ?? []).filter((op) => op.kind === 'download' && op.subject === MODEL);
        const now = await runs();
        waiting = now.filter((run) => run.progress?.waiting_on?.length);
        item = downloads.length === 1 ? downloads[0] : undefined;
        return Boolean(item && item.bytes_total > 0 && item.bytes_done < item.bytes_total && waiting.length === BOXES.length && (item.names ?? []).length === BOXES.length);
      }, { message: `one ${MODEL} download, with three runs waiting on it`, timeout: 180_000, intervals: [250] }).toBe(true);
      for (const run of waiting) {
        softly(run.progress.waiting_on, `${run.name} names the download it waits on`).toEqual([item.id]);
        softly([run.progress.bytes_done, run.progress.bytes_total], `${run.name} says no bytes of its own`).toEqual([0, 0]);
        softly(run.progress.stage, `${run.name} says it waits`).toMatch(/waiting for birefnet to download$/);
        softly(run.progress.done_fraction ?? null, `${run.name} says no share of its own`).toBeNull();
      }
      softly(item.release, 'the download says its release').toBe('0.1.0');
      softly((item.names ?? []).slice().sort(), 'the download names the three runs').toEqual(waiting.map((run) => run.name).sort());
    });

    await test.step('each box says it waits on the download, with no count, and its words lead there', async () => {
      await softly.poll(async () => Object.values(await faces(page)).map((face) => face && { ...face, words: face.words?.replace(/\s+/g, ' ') }), { message: 'each box waits on the download', timeout: 30_000 })
        .toEqual(BOXES.map(() => ({ words: 'Waiting for Birefnet to download', count: null, leads: true })));
      await shoot('canvas');
    });

    await test.step("the machine's tab lists the download once, with its bytes, and each run waiting on it, a link to it", async () => {
      await page.locator('nav.mtabs .mtab[data-machine="local"]').click();
      const list = page.getByRole('dialog', { name: /is doing$/ });
      await expect(list).toBeVisible();
      await expect(list.locator('[data-doing="download"]'), 'one download row').toHaveCount(1);
      await expect(list.locator('[data-doing="download"] .said')).toContainText(/of .* · for runs /);
      const waits = list.locator('[data-doing="run"] a.said.waits');
      await expect(waits, 'each run waits on it').toHaveCount(BOXES.length);
      for (const said of await waits.allTextContents()) softly(said).toBe('Waiting for Birefnet to download');
      await waits.first().click();
      await expect(list.locator('[data-doing="download"].lit'), 'its link lights the download').toHaveCount(1);
      await shoot('machine-tab', list);
      await page.keyboard.press('Escape');
    });

    await test.step("the machine's drawer the same, and a box's words lead to the download's row there", async () => {
      await page.goto(project.url(`machines/local#download-${item.id.replace(/[^\w-]+/g, '-')}`));
      const queue = page.locator('ul.queue');
      await expect(queue.locator('[data-queue="download"]'), 'one download row').toHaveCount(1);
      await expect(queue.locator('[data-queue="run"] a.said.waits'), 'each run waits on it').toHaveCount(BOXES.length);
      await shoot('drawer', queue);
    });

    const rendered = await rendering;
    softly(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
    await test.step('once the weights are here, each run goes on to its own work', async () => {
      await expect.poll(async () => (await runs()).filter((run) => run.status === 'completed').length, { message: 'all three rendered', timeout: 180_000 }).toBe(BOXES.length);
    });
  } finally {
    restore();
  }
});
