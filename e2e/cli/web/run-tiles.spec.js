// run-tiles (cl-1074), Paul 11-16: "there should be a way, when I'm focused on a node, and there are images to the right (preview images
// representing runs) to click on a per-run cancel button." "I want to be able to cancel runs like this selectively." "we should probably
// also display on the thumbnail to the right what machine the run is running on, that might be interesting, in addition to its
// percentage of completion and such." "When they say percentage, they should indicate like, first, download percentage, and then they
// should indicate like inference percentage, as phases, that way users can differentiate the two."
// In Focus on a box with three takes on their way, each tile in its column has a cancel of its own, seen with no hover, at least 28 px,
// on the tile and clear of its words. A take running says its machine, and its phase by name with that phase's own percentage, as its
// run's record has it: Downloading, then Rendering, never one number for both; one waiting says Queued. Its cancel stops its run alone:
// the others go on. Screenshots of each phase at 1440×900 and 1920×1080, paper and dark, are kept with the job.
const { test, expect, canvasIdle, focus, fixtureTool, png } = require('./fixtures');

// A still that spends its first `download` seconds as a weight download does, then `seconds` drawing: each in ten steps.
const FETCHING = `${fixtureTool('tool.py')}/fetching`;
const SIZES = [[1440, 900], [1920, 1080]];
const softly = expect.configure({ soft: true });

async function fit(page) {
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
}

/** Each take on its way in Focus's column, top first: its run, what its tile says, and its cancel: its box, its name, and whether it shows
 *  and is what a press at its middle meets, with no pointer over the column. */
const tilesOf = (page) => page.evaluate(() => {
  const box = (el) => { if (!el) return null; const r = el.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height }; };
  const column = document.querySelector('[data-focused-asset] .here-side');
  return [...(column?.querySelectorAll('.take-at') ?? [])].filter((at) => at.querySelector(':scope > .take.making')).map((at) => {
    const tile = at.querySelector(':scope > .take.making'), off = at.querySelector(':scope > .take-off'), style = off && getComputedStyle(off);
    const offBox = box(off), hit = offBox && document.elementFromPoint(offBox.x + offBox.width / 2, offBox.y + offBox.height / 2);
    return {
      run: tile.dataset.run ? Number(tile.dataset.run) : null,
      phase: tile.querySelector('.take-phase')?.textContent ?? null,
      said: tile.querySelector('.take-said')?.textContent ?? null,
      where: tile.querySelector('.take-where')?.textContent ?? null,
      text: tile.innerText.replace(/\s+/g, ' ').trim(),
      tile: box(tile),
      words: [...tile.querySelectorAll('.take-phase, .take-said, .take-where')].map(box),
      off: offBox && { ...offBox, name: off.getAttribute('aria-label'), shown: style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity) > 0.5, hit: Boolean(hit && off.contains(hit)) },
    };
  });
});

const overlaps = (a, b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
const within = (a, b) => a.x >= b.x - 0.5 && a.y >= b.y - 0.5 && a.x + a.width <= b.x + b.width + 0.5 && a.y + a.height <= b.y + b.height + 0.5;
const percent = (said) => (/^\d{1,3}%$/.test(said ?? '') ? Number(said.slice(0, -1)) : null);

test('run-tiles', async ({ page, project, api }) => {
  test.setTimeout(300_000);
  const record = (run) => api.get(project.at(`/runs/${run}`));
  /** The takes on their way, as Calcifer lists them: their ids and runs. One that ended with no version is listed apart (`failure`). */
  const takes = async () => ((await project.asset('a'))?.takes ?? []).filter((take) => !take.failure).map((take) => ({ id: take.id, run: take.run ?? null }));
  /** The run doing its own work now, and where it is: its stage, how far, its machine. */
  const working = async () => {
    for (const { run } of await takes()) {
      if (run == null) continue;
      const said = await record(run);
      if (said.status === 'in_progress' && said.progress?.total) return { run, machine: said.machine, stage: said.progress.stage, share: Math.round((100 * said.progress.completed) / said.progress.total) };
    }
    return null;
  };
  /** Every tile on its way: its cancel shows with no hover, at least 28 px, on its tile, clear of its words, and takes the press. */
  const cancels = (tiles, when) => {
    for (const tile of tiles) {
      const off = tile.off;
      softly(off?.shown && off.hit, `${when}: run ${tile.run}'s cancel shows and takes a press, no hover: ${JSON.stringify(off)}`).toBe(true);
      softly(off && off.width >= 28 && off.height >= 28, `${when}: run ${tile.run}'s cancel at least 28 px: ${JSON.stringify(off)}`).toBe(true);
      softly(off && within(off, tile.tile), `${when}: run ${tile.run}'s cancel on its tile: ${JSON.stringify({ off, tile: tile.tile })}`).toBe(true);
      softly(off && tile.words.some((words) => overlaps(off, words)), `${when}: run ${tile.run}'s cancel covers none of its words`).toBe(false);
    }
  };
  /** Screenshots of Focus and of its column at each size, paper and dark, with nothing under the pointer. */
  const shoot = async (region, label) => {
    for (const [width, height] of SIZES) {
      await page.setViewportSize({ width, height });
      await canvasIdle(page);
      const title = await region.locator('.focus-title').boundingBox();
      await page.mouse.move(title.x + 4, title.y + title.height / 2);
      for (const theme of ['paper', 'dark']) {
        await page.evaluate((dark) => {
          if (dark) document.documentElement.dataset.theme = 'dark';
          else delete document.documentElement.dataset.theme;
          return new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        }, theme === 'dark');
        await page.screenshot({ path: test.info().outputPath(`${label}-${width}x${height}-${theme}.png`) });
        await region.locator('.here-side').screenshot({ path: test.info().outputPath(`${label}-${width}x${height}-${theme}-column.png`) });
      }
    }
    await page.evaluate(() => { delete document.documentElement.dataset.theme; });
    await page.setViewportSize({ width: 1440, height: 900 });
    await canvasIdle(page);
  };

  // `a`: two takes made, and its tool fetches for 30 s, then draws for 30 s. A runner's machine runs one CPU run at a time: one goes, two wait.
  await project.add({ name: 'a', tool: FETCHING, kind: 'image', fields: { download: 30, seconds: 30 }, at: { x: 0, y: 0 } });
  for (const shade of [60, 180]) await api.post(project.at('/assets/a/versions'), { source: await project.upload(png(shade)) });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the local tool listed', timeout: 120_000 })
    .toContain(FETCHING);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await fit(page);
  const region = await focus(page, 'a');
  const column = region.locator('.here-side .take.making');
  const render = region.getByRole('button', { name: 'Render', exact: true });
  // Each a take of its own: its tool draws a new seed each render (tool.py's `fetching`).
  for (let n = 1; n <= 3; n++) {
    await render.click();
    await expect(column, `take ${n} on its way`).toHaveCount(n);
    await expect.poll(async () => (await takes()).length, { message: `take ${n} listed` }).toBe(n);
  }

  let first;
  await test.step('one take downloading, two queued: each says so, its machine and its download share; each has its cancel', async () => {
    await expect.poll(async () => {
      const now = await working();
      if (now && /^downloading/i.test(now.stage) && now.share >= 10 && now.share <= 60) first = now;
      return Boolean(first);
    }, { message: 'a run downloading, 10% or more', timeout: 150_000 }).toBe(true);
    await shoot(region, 'downloading');
    await softly.poll(async () => {
      const now = await working();
      const tile = (await tilesOf(page)).find((one) => one.run === first.run);
      return Boolean(now && tile && tile.phase === 'Downloading' && tile.where === first.machine && Math.abs(percent(tile.said) - now.share) <= 10);
    }, { message: `run ${first.run}'s tile says Downloading, its share and ${first.machine}` }).toBe(true);
    const tiles = await tilesOf(page);
    softly(tiles.filter((tile) => tile.run !== first.run).map((tile) => [tile.phase, tile.said]), `the others say Queued, no share: ${JSON.stringify(tiles.map((tile) => tile.text))}`)
      .toEqual([['Queued', null], ['Queued', null]]);
    cancels(tiles, 'downloading');
  });

  await test.step('the same take rendering: its tile says Rendering and the render share, not the download one', async () => {
    let drawing;
    await expect.poll(async () => {
      const now = await working();
      // Past its first step: a stage's count may stay the last stage's until its own first.
      if (now?.run === first.run && /^drawing/i.test(now.stage) && now.share >= 10 && now.share <= 60) drawing = now;
      return Boolean(drawing);
    }, { message: `run ${first.run} drawing, 10% or more`, timeout: 90_000 }).toBe(true);
    await shoot(region, 'rendering');
    await softly.poll(async () => {
      const now = await working();
      const tile = (await tilesOf(page)).find((one) => one.run === first.run);
      return Boolean(now?.run === first.run && tile && tile.phase === 'Rendering' && tile.where === first.machine && Math.abs(percent(tile.said) - now.share) <= 10);
    }, { message: `run ${first.run}'s tile says Rendering, its share and ${first.machine}` }).toBe(true);
    cancels(await tilesOf(page), 'rendering');
  });

  await test.step("cancel the running take from its tile: its run alone stops, and the next starts", async () => {
    const others = (await takes()).filter((take) => take.run !== first.run);
    await region.locator(`.take-at:has(> .take.making[data-run="${first.run}"]) > .take-off`).click();
    await expect.poll(async () => (await record(first.run)).status, { message: `run ${first.run} canceled` }).toBe('canceled');
    await expect(column, 'its tile gone, the others there').toHaveCount(2);
    const left = await takes();
    expect.soft(left.map((take) => take.id).sort(), 'the other two takes listed still').toEqual(others.map((take) => take.id).sort());
    for (const take of left) if (take.run != null) expect.soft((await record(take.run)).status, `run ${take.run} goes on`).not.toBe('canceled');
    let next;
    await expect.poll(async () => { next = await working(); return Boolean(next && next.run !== first.run); }, { message: 'the next take running', timeout: 90_000 }).toBe(true);
    await softly.poll(async () => {
      const tile = (await tilesOf(page)).find((one) => one.run === next.run);
      return Boolean(tile && ['Downloading', 'Rendering'].includes(tile.phase) && tile.where === next.machine && percent(tile.said) !== null);
    }, { message: `run ${next.run}'s tile says its phase, its share and ${next.machine}` }).toBe(true);
    cancels(await tilesOf(page), 'after a cancel');

    // The one waiting, then the one running: each from its own tile, the other going on meanwhile.
    const waiting = (await takes()).find((take) => take.run !== next.run);
    await region.locator(`.take-at:has(> .take.making[data-take-id="${waiting.id}"]) > .take-off`).click();
    await expect(column, 'the waiting one gone').toHaveCount(1);
    await expect.poll(async () => (await takes()).map((take) => take.id), { message: 'only the running take left' }).not.toContain(waiting.id);
    if (waiting.run != null) await expect.poll(async () => (await record(waiting.run)).status, { message: `run ${waiting.run} canceled` }).toBe('canceled');
    expect.soft((await record(next.run)).status, `run ${next.run} goes on`).toBe('in_progress');
    await region.locator(`.take-at:has(> .take.making[data-run="${next.run}"]) > .take-off`).click();
    await expect.poll(async () => (await record(next.run)).status, { message: `run ${next.run} canceled` }).toBe('canceled');
    await expect(column, 'none on its way').toHaveCount(0);
    await softly.poll(async () => {
      const a = await project.asset('a');
      return { versions: a.versions, making: a.making ?? [] };
    }, { message: 'nothing made, nothing running' }).toEqual({ versions: 2, making: [] });
  });
});
