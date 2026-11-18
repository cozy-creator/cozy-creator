// shared-download (cl-1205), Paul 11-17, watching kirill fetch MiniMax H3 for three jobs that all need it: "This view is also misleading;
// it looks like we're downloading it 3 times, but we're actually just downloading it once. It should show the download occurring, and then
// it should also show the 3 jobs that are like 'waiting for download...' and they don't need to independently report the progress in
// theirs as well". Three boxes of one tool, each reading one model this machine doesn't hold, rendered at once. While its weights come:
// the machine lists one download of them, its bytes moving, naming all three runs; each run's progress names that download (`waiting_on`)
// and says it waits, with no bytes or share of its own; the machine's tab and its drawer list the download once and each run as waiting
// on it, a link to it; each box says "Waiting for <model> to download" with no count, and leads there. A fourth box, never rendered, is
// predicted on the same machine (the project prefers it, cl-1123): its fetch ahead is the same download, and its readiness counts that
// download's bytes. Each run's record still says what the machine had and needed (`measurements.weights`, cl-1102). Screenshots of the
// canvas, the tab's list and the drawer at 1440×900, paper and dark, are taken as soon as the download goes, before anything is judged,
// so a red run keeps them too.
// On the CPU e2e: weighted.py's still on this computer, reading BiRefNet's raw weights from the isolated Hub over a link slowed to 10 MB/s
// (ondemand.sh's slow_store), about 45 s. On a rental (release-gate/sessions/shared-download.sh): $SHARED_TOOL on $SHARED_MACHINE, over
// the pod's own link.
const { execFileSync } = require('child_process');
const { test, expect, themed, canvasIdle, fixtureTool } = require('./fixtures');

const MACHINE = process.env.SHARED_MACHINE || 'local';
const TOOL = process.env.SHARED_TOOL || fixtureTool('weighted.py/weighted');
const MODEL = process.env.SHARED_MODEL || 'zhengpeng7/birefnet';
const FIELDS = JSON.parse(process.env.SHARED_FIELDS || '{"seconds": 1}');
const SLOW = process.env.SHARED_MACHINE ? '' : '40mbit';
const BOXES = ['first', 'second', 'third'];
/** Never rendered: predicted alone. */
const AHEAD = 'ahead';
const softly = expect.configure({ soft: true });

/** A repository as the page says it (activity.ts `modelWords`): `zhengpeng7/birefnet` is `Birefnet`. */
const modelWords = (subject) => subject.replace(/^hf:\/\//, '').split('/')[1].split(/[-_]/).filter(Boolean).map((word) => word[0].toUpperCase() + word.slice(1)).join(' ');
/** An activity id as the page anchors its row (activity.ts `anchorOf`). */
const anchorOf = (id) => `download-${id.replace(/[^\w-]+/g, '-')}`;

/** The isolated Hub's store link slowed to `rate` (tc on the loopback, from its MinIO's port), as ondemand.sh's slow_store; undone after. */
function slowStore(rate) {
  if (!rate) return () => {};
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
  test.setTimeout(1_200_000);
  const words = `Waiting for ${modelWords(MODEL)} to download`;
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
  const runs = async () => (await api.get(project.at('/runs?limit=100'))).runs.filter((run) => run.kind === 'call');
  const machine = async () => (await api.get('/v1/machines')).machines.find((one) => one.name === MACHINE);
  const downloads = async () => ((await machine())?.status?.activity ?? []).filter((op) => op.kind === 'download' && op.subject === MODEL);
  const tab = () => page.locator(`nav.mtabs .mtab[data-machine="${MACHINE}"]`);
  const list = () => page.getByRole('dialog', { name: /is doing$/ });

  // Its renders and its predictions go to MACHINE (cl-1123: a box's tool gets its preferred machine ready before Render).
  await api.call('PUT', project.at('/machines'), { machines: [MACHINE] });
  for (const [at, name] of [...BOXES, AHEAD].entries()) {
    await project.add({ name, tool: TOOL, kind: 'image', fields: { instructions: `A lighthouse on a rocky coast at dusk, ${name} take, photo.`, ...FIELDS }, at: { x: at * 380, y: 0 } });
  }
  if (TOOL.startsWith('/')) {
    await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the local tool listed', timeout: 120_000 })
      .toContain(TOOL);
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(project.url());
  await expect(page.locator('.stage')).toBeVisible();
  await canvasIdle(page);

  const restore = slowStore(SLOW);
  try {
    const rendering = cli('--json', 'render', ...BOXES, '--on', MACHINE);

    await test.step('as the weights come: the canvas, the machine tab and the drawer, paper and dark', async () => {
      await expect.poll(async () => {
        const going = (await downloads()).some((op) => op.bytes_total > 0 && op.bytes_done < op.bytes_total);
        return going && (await runs()).length === BOXES.length;
      }, { message: `${MODEL} downloading on ${MACHINE} for three runs`, timeout: 600_000, intervals: [250] }).toBe(true);
      await canvasIdle(page);
      await shoot('canvas');
      await tab().click();
      await expect(list()).toBeVisible();
      await shoot('machine-tab', list());
      await page.keyboard.press('Escape');
      await page.goto(project.url(`machines/${encodeURIComponent(MACHINE)}`));
      await expect(page.locator('ul.queue')).toBeVisible();
      await shoot('drawer', page.locator('ul.queue'));
      await page.goto(project.url());
      await expect(page.locator('.stage')).toBeVisible();
      await canvasIdle(page);
    });

    let item, waiting;
    await test.step('the machine lists one download of the weights, naming the three runs and the predicted box; each run names it and says no bytes of its own', async () => {
      await expect.poll(async () => {
        const listed = await downloads();
        waiting = (await runs()).filter((run) => run.progress?.waiting_on?.length);
        item = listed.length === 1 ? listed[0] : undefined;
        return Boolean(item && item.bytes_total > 0 && item.bytes_done < item.bytes_total && waiting.length === BOXES.length);
      }, { message: `one ${MODEL} download on ${MACHINE}, three runs waiting on it`, timeout: 120_000, intervals: [250] }).toBe(true);
      test.info().annotations.push({ type: 'download', description: JSON.stringify(item) });
      for (const run of waiting) {
        test.info().annotations.push({ type: `run ${run.name}`, description: JSON.stringify(run.progress) });
        softly(run.progress.waiting_on, `${run.name} names the download it waits on`).toEqual([item.id]);
        softly([run.progress.bytes_done, run.progress.bytes_total], `${run.name} says no bytes of its own`).toEqual([0, 0]);
        softly(run.progress.stage, `${run.name} says it waits`).toMatch(/waiting for \S+ to download$/);
        softly(run.progress.done_fraction ?? null, `${run.name} says no share of its own`).toBeNull();
      }
      softly(item.release, 'the download says its release').toBeTruthy();
      softly(item.names ?? [], 'the download names the three runs').toEqual(expect.arrayContaining(waiting.map((run) => run.name)));
      // The predicted box: its fetch ahead is this download, named by its box, and its readiness counts this download's bytes.
      const ahead = ((await cli.json('status').catch(() => null))?.board?.ahead ?? []).find((row) => (row.names ?? []).includes(AHEAD));
      test.info().annotations.push({ type: 'predicted', description: JSON.stringify(ahead ?? null) });
      softly(item.names ?? [], 'the download names the predicted box too').toContain(AHEAD);
      if (ahead?.total) softly(ahead.total, "the predicted box's readiness counts the one download's bytes").toBeLessThanOrEqual(item.bytes_total);
    });

    await test.step('each box says it waits on the download, with no count, and its words lead there', async () => {
      await softly.poll(async () => Object.entries(await faces(page)).map(([, face]) => face && { ...face, words: face.words?.replace(/\s+/g, ' ') }), { message: 'each box waits on the download', timeout: 30_000 })
        .toEqual(BOXES.map(() => ({ words, count: null, leads: true })));
    });

    await test.step("the machine's tab lists the download once, with its bytes, and each run waiting on it, a link to it", async () => {
      await tab().click();
      await expect(list()).toBeVisible();
      await expect(list().locator('[data-doing="download"]'), 'one download row').toHaveCount(1);
      await expect(list().locator('[data-doing="download"] .said')).toContainText(/ of .* · for /);
      const waits = list().locator('[data-doing="run"] a.said.waits');
      await expect(waits, 'each run waits on it').toHaveCount(BOXES.length);
      for (const said of await waits.allTextContents()) softly(said).toBe(words);
      await waits.first().click();
      await expect(list().locator('[data-doing="download"].lit'), 'its link lights the download').toHaveCount(1);
      await page.keyboard.press('Escape');
    });

    await test.step("the machine's drawer the same, at the download's row a box's words lead to", async () => {
      await page.goto(project.url(`machines/${encodeURIComponent(MACHINE)}#${anchorOf(item.id)}`));
      const queue = page.locator('ul.queue');
      await expect(queue.locator('[data-queue="download"]'), 'one download row').toHaveCount(1);
      await expect(queue.locator('[data-queue="run"] a.said.waits'), 'each run waits on it').toHaveCount(BOXES.length);
    });

    const rendered = await rendering;
    softly(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
    await test.step('once the weights are here, each run goes on to its own work, and its record says what the machine had and needed', async () => {
      await expect.poll(async () => (await runs()).filter((run) => run.status === 'completed').length, { message: 'all three rendered', timeout: 600_000 }).toBe(BOXES.length);
      for (const run of await runs()) {
        const full = await cli.json('--full', 'run', 'watch', run.name, '--once');
        const weights = full.measurements?.weights;
        test.info().annotations.push({ type: `weights ${run.name}`, description: JSON.stringify(weights ?? null) });
        softly(weights && ['has', 'needs', 'extras'].every((key) => typeof weights[key] === 'number'), `${run.name}'s record says has, needs and extras`).toBe(true);
      }
    });
  } finally {
    restore();
  }
});
