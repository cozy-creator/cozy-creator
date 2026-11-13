// takes (cl-973 core cell 12): a click on a take previews it and approves nothing; a double-click approves it, marked current, and Undo puts
// back what was approved before (the first file a box is given is approved), Redo the double-click. Picking take 2 of a box whose words say <self.1> makes them <self.2> (Paul 11-06), and a mention of an
// approved box reads its approved take, not its newest. A take's discard is a trash can, and Undo brings it back with its file. A new take
// leaves the approval alone and is "Not looked at yet" until it's seen; a reload keeps all of it, and the takes column is never empty. An
// empty box offers no New; on an H3 box with a take, New makes a draft.
const { test, expect, canvasIdle, focus, fixtureTool, png, DARK_OPEN, MP4 } = require('./fixtures');

const TOOL = fixtureTool('tool.py');
const SHOOT = `${fixtureTool('shots.py')}/shoot`;
const H3 = 'minimax-h3/generate';
const PICKED = '<a> walks past <self.2>.';

const softly = expect.configure({ soft: true });
const hex = (sha) => String(sha ?? '').replace(/^sha256:/, '');
const shaOf = (asset, number) => hex(asset?.prefixes?.find((version) => version.number === number)?.main?.sha256);
const approval = (asset) => ({ pinned: Boolean(asset?.pinned), approved: asset?.approved ?? null });
const focused = (page) => page.getByRole('region', { name: 'Focused asset' });
/** A take's tile in Focus's column, by what its box calls its takes (`version` for a picture, `take` for a video). */
const tile = (region, noun, number) => region.getByRole('button', { name: `${noun} ${number}`, exact: true });

async function fit(page) {
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
}

async function back(page) {
  await focused(page).getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await expect(focused(page)).toHaveCount(0);
  await canvasIdle(page);
}

/** How many takes Focus's column lists for the box open, each time that changes, in each page this context loads. */
function recordColumns() {
  const log = (window.__columns = []);
  new MutationObserver(() => {
    const open = document.querySelector('[data-focused-asset]');
    if (!open) return;
    const side = open.querySelector('.here-side');
    const now = { box: open.dataset.focusedAsset, tiles: side ? side.querySelectorAll('.take[data-take], .take.making').length : 0 };
    if (JSON.stringify(now) !== JSON.stringify(log.at(-1))) log.push(now);
  }).observe(document, { subtree: true, childList: true, attributes: true });
}

test('takes', async ({ page, project, api }) => {
  const numbers = async (name) => ((await project.asset(name))?.prefixes ?? []).map((version) => version.number);
  // `a`: three takes, its render slow enough to look away from. `b`: a video of two whose words mention `a` and its own take 1.
  await project.add({ name: 'a', tool: `${TOOL}/slow`, kind: 'image', fields: { instructions: 'A still, slowly.', seconds: 4 }, at: { x: 0, y: 0 } });
  for (const shade of [40, 120, 200]) await api.post(project.at('/assets/a/versions'), { source: await project.upload(png(shade)) });
  await project.add({ name: 'b', tool: SHOOT, kind: 'video', at: { x: 400, y: 0 } });
  for (const clip of [MP4, DARK_OPEN]) await api.post(project.at('/assets/b/versions'), { source: await project.upload(clip) });
  await api.patch(project.at('/assets/b'), { fields: { summary: '<a> walks past <self.1>.' } });
  await project.add({ name: 'empty', tool: `${TOOL}/still`, kind: 'image', fields: { instructions: 'Nothing made yet.' }, at: { x: 800, y: 0 } });
  const shots = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}&q=minimax`)).tools.map((tool) => tool.id);
  expect(shots, "the Hub lists H3's Shot").toContain(H3);
  await project.add({ name: 'h3', tool: H3, kind: 'video', fields: { summary: 'A lighthouse at dusk.', beats: [{ description: 'Waves.', duration: 5 }] }, at: { x: 1200, y: 0 } });
  await api.post(project.at('/assets/h3/versions'), { source: await project.upload(MP4) });
  await expect.poll(async () => ({ a: await numbers('a'), b: await numbers('b'), h3: await numbers('h3') })).toEqual({ a: [1, 2, 3], b: [1, 2], h3: [1] });
  // The page's tools list is read once: with the local code these boxes name in it, a pick knows which words are b's prompt.
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the local tools listed', timeout: 120_000 })
    .toEqual(expect.arrayContaining([`${TOOL}/slow`, SHOOT]));
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await fit(page);

  await test.step('click take 2, double-click take 3, press Ctrl+Z', async () => {
    const region = await focus(page, 'a');
    // The first file joined is approved (format::can_initialize_approval): take 1, not 3.
    const was = approval(await project.asset('a'));
    expect(was.approved, 'take 3 not approved yet').not.toBe(3);
    await tile(region, 'version', 2).click();
    await expect.soft(tile(region, 'version', 2), 'the click previews it').toHaveAttribute('aria-pressed', 'true');
    await expect.soft(region.getByLabel('Take', { exact: true })).toHaveText('v2');
    expect.soft(approval(await project.asset('a')), 'and approves nothing').toEqual(was);
    await tile(region, 'version', 3).dblclick();
    await expect.poll(async () => approval(await project.asset('a')), { message: 'the double-click approves take 3' }).toEqual({ pinned: true, approved: 3 });
    expect.soft((await project.asset('a')).version, 'its state is take 3').toBe(3);
    await expect.soft(tile(region, 'version', 3), 'marked current').toHaveAttribute('aria-current', 'true');
    await page.keyboard.press('ControlOrMeta+z');
    // Back to what was approved before, not to take 2: the click approved nothing.
    await expect.poll(async () => approval(await project.asset('a')), { message: 'Undo' }).toEqual(was);
    await expect.soft(tile(region, 'version', 3)).not.toHaveAttribute('aria-current', 'true');
    await page.keyboard.press('ControlOrMeta+Shift+z');
    await expect.poll(async () => approval(await project.asset('a')), { message: 'Redo' }).toEqual({ pinned: true, approved: 3 });
    await expect.soft(tile(region, 'version', 3)).toHaveAttribute('aria-current', 'true');
    await back(page);
  });

  await test.step('on a second box, mentioning <a> and <self.1>, pick take 2', async () => {
    const region = await focus(page, 'b');
    await tile(region, 'take', 2).click();
    await expect.soft(tile(region, 'take', 2)).toHaveAttribute('aria-pressed', 'true');
    // Paul 11-06: the take picked is the one its words edit.
    await softly.poll(async () => (await project.asset('b'))?.fields?.summary, { message: 'its first <self.M> is the take picked' }).toBe(PICKED);
    await back(page);
  });

  await test.step('discard take 1 with its trash, then Ctrl+Z', async () => {
    const region = await focus(page, 'a');
    const one = shaOf(await project.asset('a'), 1);
    await tile(region, 'version', 1).hover();
    const discard = region.getByRole('button', { name: 'Discard version 1', exact: true });
    await expect.soft(discard.locator('svg'), 'a trash can').toHaveAttribute('data-icon', 'trash');
    await discard.click();
    await expect.poll(() => numbers('a'), { message: 'take 1 discarded' }).toEqual([2, 3]);
    await expect.soft(tile(region, 'version', 1)).toHaveCount(0);
    await page.keyboard.press('ControlOrMeta+z');
    await expect.poll(() => numbers('a'), { message: 'Undo brings it back' }).toEqual([1, 2, 3]);
    const back1 = await project.asset('a');
    expect.soft(shaOf(back1, 1), 'with its file').toBe(one);
    await softly.poll(() => tile(region, 'version', 1).locator('img').evaluateAll(([image]) => Boolean(image?.complete && image.naturalWidth > 0)), { message: 'its picture drawn' }).toBe(true);
    expect.soft(approval(back1), 'the approval stands').toEqual({ pinned: true, approved: 3 });
    await back(page);
  });

  await test.step('render a new take and reload', async () => {
    await page.context().addInitScript(recordColumns);
    await page.evaluate(recordColumns);
    const columns = [];
    let region = await focus(page, 'a');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect(region.locator('.here-side .take.making'), 'its take on its way').toHaveCount(1);
    // Looked away from, at what's approved: its take lands not looked at.
    await tile(region, 'version', 3).click();
    await expect.poll(async () => (await project.asset('a'))?.versions, { message: "a's new take", timeout: 120_000 }).toBe(4);
    const a = await project.asset('a');
    expect.soft(approval(a), 'a new take leaves the approval alone').toEqual({ pinned: true, approved: 3 });
    expect.soft(a.unseen ?? [], 'not looked at yet').toContain(4);
    await expect.soft(tile(region, 'version', 4).getByRole('img', { name: 'Not looked at yet', exact: true })).toBeVisible();
    await expect.soft(tile(region, 'version', 3)).toHaveAttribute('aria-current', 'true');
    await tile(region, 'version', 4).click();
    await softly.poll(async () => (await project.asset('a'))?.unseen ?? [], { message: 'seen' }).not.toContain(4);
    await expect.soft(tile(region, 'version', 4).getByRole('img', { name: 'Not looked at yet', exact: true }), 'until it is seen').toHaveCount(0);
    await back(page);

    // `b` mentions `a`: its run reads a's approved take, not its newest, and its own take 2 as picked.
    region = await focus(page, 'b');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('b'))?.versions, { message: "b's new take", timeout: 120_000 }).toBe(3);
    const b = await project.asset('b');
    const run = b.prefixes.find((version) => version.number === 3).run;
    const input = JSON.stringify((await api.get(`${project.at(`/runs/${run}`)}?detail=full`)).request?.input ?? null);
    expect.soft(input.includes(shaOf(a, 3)), `<a> reads its approved take 3: ${input.slice(0, 600)}`).toBe(true);
    expect.soft(input.includes(shaOf(a, 4)), 'not its newest').toBe(false);
    expect.soft(input.includes(shaOf(b, 2)), '<self.2> reads the take picked').toBe(true);
    await back(page);

    region = await focus(page, 'a');
    columns.push(...(await page.evaluate(() => window.__columns)));
    await page.reload();
    region = focused(page);
    await expect(region).toHaveAttribute('data-focused-asset', 'a');
    await canvasIdle(page);
    for (const number of [1, 2, 3, 4]) await expect.soft(tile(region, 'version', number), `take ${number} after a reload`).toBeVisible();
    await expect.soft(tile(region, 'version', 3), 'still current').toHaveAttribute('aria-current', 'true');
    const kept = await project.asset('a');
    expect.soft({ numbers: kept.prefixes.map((version) => version.number), first: shaOf(kept, 1), ...approval(kept) }, 'the reload keeps everything')
      .toEqual({ numbers: [1, 2, 3, 4], first: shaOf(a, 1), pinned: true, approved: 3 });
    expect.soft((await project.asset('b'))?.fields?.summary).toBe(PICKED);
    columns.push(...(await page.evaluate(() => window.__columns)));
    expect.soft(columns.filter((look) => ['a', 'b'].includes(look.box) && look.tiles === 0), 'the takes column is never empty').toEqual([]);
    await back(page);
  });

  await test.step('open an empty box, then an H3 box with a take', async () => {
    let region = await focus(page, 'empty');
    await expect.soft(region.getByRole('button', { name: /^(Its next take|New video|Its words)$/ }), 'an empty box offers no New').toHaveCount(0);
    await expect.soft(region.locator('.focus-heading')).not.toContainText('New');
    await back(page);
    region = await focus(page, 'h3');
    await expect.soft(region.getByLabel('Take', { exact: true })).toHaveText('v1');
    const next = region.getByRole('button', { name: 'New video', exact: true });
    await expect(next).toBeVisible();
    await next.click();
    await expect.soft(region.getByLabel('Take', { exact: true }), 'New makes a draft').toHaveText('New take');
    await expect.soft(region.locator('.in-frame [role="textbox"][data-field="summary"]'), 'written in the frame').toBeVisible();
    await expect.soft(region.locator('.focus-viewer .media.writing')).toHaveCount(1);
    await back(page);
  });
});
