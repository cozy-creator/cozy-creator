// render (cl-973 core cell 11), on local: "A still" from the palette, typed in and rendered by Render, lists its take the frame after the
// press and keeps that one id until it lands in place, unreloaded, its pending tile and its Cancel naming its run KEY-N; another box rendered by Ctrl+Enter says Queued, then Rendering, with a
// faded preview of its latents until its take replaces it, and its finish note's View opens it. Render all: a box waiting on another says
// so and shows none of the other's progress; the other's whole job's meter rises as it renders; Cancel stops one waiting and one
// rendering, and their takes go.
// A failed render is said once, as a toast, and its take is gone: nothing in the column to pick or delete, the frame back on the take it
// showed, through a reload (cl-1070). The take watched in Focus is the one drawn
// when it lands, through a reload; its run's warnings are the box's, never a note. Two boxes rendered at once: the one whose tool is warm
// is ready before the other's install ends, waiting only for the CPU another run holds, which its log names (cl-1086). A picture's
// take on its way is, in its takes column, the shape it will be: the one set, else Auto's from the picture it edits (cl-1072).
const { writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasIdle, focus, middle, openPoint, fixtureTool, png, canvasCard, hoverBox, screenshotPixels } = require('./fixtures');
const { toolRow } = require('./hands');

const TOOL = fixtureTool('tool.py');
const STILL = `${TOOL}/still`;
const LATENTS = `${fixtureTool('latents.py')}/latents`;
const BROKEN = 'the crash tool fails, as the render cell asks';
const WARNED = 'a handled fallback, as the render cell asks';
// A run whose result warns, as a handled fallback does: no fixture tool's does. Its dependencies are tool.py's.
const WARNS = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The render cell's tools: a still of one colour whose result warns, and one that fails."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()


class Warned(msgspec.Struct):
    image: ImageFile
    prompt: str
    warnings: list[str]


@app.tool(label="Warns", named="warned", accelerator=False)
def warned(payload: Drawing) -> Warned:
    """A still of one colour, and a warning."""
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return Warned(image, payload.instructions.text, ["${WARNED}"])


@app.tool(label="Crashes", named="crashes", accelerator=False)
def crashes(payload: Drawing) -> Warned:
    """Fails."""
    raise RuntimeError("${BROKEN}")
`;
// A tool of an environment of its own (tool.py's and tomli-w): its first render installs it.
const INSTALLS = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19", "tomli-w>=1"]
# ///
"""The render cell's tool that needs an install: a still of one colour."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


@app.tool(label="Installs", named="installed", accelerator=False)
def installed(payload: Drawing) -> Picture:
    """A still of one colour, once its environment is installed."""
    return Picture(ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png"), payload.instructions.text)
`;

const softly = expect.configure({ soft: true });
const hex = (sha) => String(sha ?? '').replace(/^sha256:/, '');
const shaOf = (asset, number) => hex(asset?.prefixes?.find((version) => version.number === number)?.main?.sha256);
/** A render on its way, on its box at rest or over it in Focus: its two lines and Cancel. */
const face = (page, name) => page.getByRole('status', { name: `${name}'s render`, exact: true });
const focused = (page) => page.getByRole('region', { name: 'Focused asset' });
/** The take a picture's frame draws, once its pixels are in. */
const drawn = (scope) => scope.locator('.media-pixels .buffered-image').evaluateAll(([host]) => {
  const image = host?.querySelector('img');
  return image?.complete && image.naturalWidth > 0 ? (host.dataset.displayedSha ?? '').replace(/^sha256:/, '') : null;
});
const settled = async (project, name) => {
  const asset = await project.asset(name);
  return { takes: asset?.takes?.length ?? 0, making: asset?.making?.length ?? 0, versions: asset?.versions ?? 0 };
};

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

/** Carried out of a list as a hand carries it: pressed, moved across the list a little (which tells a carry from a scroll), let go at `to`. */
async function carry(page, from, to) {
  const start = await middle(from);
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(start.x + 12, start.y, { steps: 3 });
  await page.mouse.move(to.x, to.y, { steps: 8 });
  await page.mouse.up();
}

/** Every note the page puts up, as it comes, in each page this context loads. */
function keepNotes() {
  const said = (window.__notes = []);
  new MutationObserver(() => {
    for (const note of document.querySelectorAll('[data-note]')) if (!said.includes(note.innerText)) said.push(note.innerText);
  }).observe(document, { subtree: true, childList: true, characterData: true });
}

/** Notes that come over `ms`, in the page: the declared window a note that would come is waited out in. */
const notesOver = (page, ms) => page.evaluate((ms) => new Promise((done) => {
  const seen = [];
  const look = new MutationObserver(() => {
    for (const note of document.querySelectorAll('[data-note]')) if (!seen.includes(note.innerText)) seen.push(note.innerText);
  });
  look.observe(document.body, { subtree: true, childList: true, characterData: true });
  setTimeout(() => { look.disconnect(); done(seen); }, ms);
}), ms);

/** What `name` says of its render, each time it changes: its face's words, its dot, its meter and its faded preview, at rest or in Focus. */
const recordBox = (page, name) => page.evaluate((name) => {
  const log = ((window.__said ??= {})[name] = []);
  const look = () => {
    const open = document.querySelector(`[data-focused-asset=${JSON.stringify(name)}]`);
    const card = open?.querySelector('.card') ?? document.querySelector(`.canvas-node .card[data-asset=${JSON.stringify(name)}]`);
    const shown = document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered;
    const face = [...document.querySelectorAll('[role="status"]')].find(one => one.getAttribute('aria-label') === `${name}'s render`);
    const meter = card?.querySelector('[role="progressbar"][aria-label="The whole job"]');
    const preview = card?.querySelector('.latent-preview img');
    return {
      face: face?.querySelector('.face-state b')?.textContent ?? (shown?.progress ? [shown.labels.progress?.fullText, shown.labels.count?.fullText].filter(Boolean).join(' ') : null),
      time: face?.querySelector('.face-time')?.textContent ?? shown?.labels.time?.fullText ?? null,
      dot: (open?.querySelector('.focus-title .state-dot') ?? card?.querySelector('.head .state-dot'))?.getAttribute('aria-label') ?? ({ queued: 'Queued', running: 'Rendering', failed: 'Failed', unseen: 'New, not looked at' }[shown?.state] ?? null),
      meter: meter ? Number(meter.getAttribute('aria-valuenow')) : shown?.meter && shown.progress?.fraction !== undefined ? Math.round(shown.progress.fraction * 100) : null,
      preview: preview?.complete && preview.naturalWidth > 0 ? Number(getComputedStyle(preview).opacity) : shown?.latent?.opacity ?? null,
    };
  };
  const sample = () => {
    const now = look();
    if (JSON.stringify(now) !== JSON.stringify(log.at(-1))) log.push(now);
  };
  // Native Focus can commit Queued and then Rendering between frames; preserve that ordering while also sampling Pixi frames.
  new MutationObserver(sample).observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
  const frame = () => { sample(); requestAnimationFrame(frame); };
  frame();
}, name);
const renderState = (page, name) => page.evaluate(name => {
  const shown = document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered;
  if (shown) return shown.progress ? [shown.labels.progress?.fullText, shown.labels.count?.fullText].filter(Boolean).join(' ') : null;
  return [...document.querySelectorAll('[role="status"]')].find(one => one.getAttribute('aria-label') === `${name}'s render`)?.querySelector('.face-state b')?.textContent ?? null;
}, name);
const said = (page, name) => page.evaluate((name) => window.__said[name], name);

/** Focus's takes column for `name` on every change, by take id, with what each pending tile of a run and its Cancel say; and, heard before
 *  the page's own handler, the column as Render was pressed and in the next frame. */
const recordTakes = (page, name) => page.evaluate((name) => {
  const look = () => {
    const side = document.querySelector(`[data-focused-asset="${CSS.escape(name)}"] .here-side`);
    const ids = (selector) => [...(side?.querySelectorAll(selector) ?? [])].map((tile) => tile.getAttribute('data-take-id')).filter(Boolean);
    const runs = [...(side?.querySelectorAll('.take.making[data-run]') ?? [])].map((tile) => ({
      run: tile.getAttribute('data-run'), tile: tile.getAttribute('title'), cancel: tile.closest('.take-at')?.querySelector('.take-off')?.getAttribute('aria-label') ?? null,
    }));
    return { open: Boolean(side), making: ids('.take.making[data-take-id]'), landed: ids('.take[data-take][data-take-id]'), runs };
  };
  const takes = (window.__takes = { log: [look()], pressed: null });
  new MutationObserver(() => {
    const now = look();
    if (JSON.stringify(now) !== JSON.stringify(takes.log.at(-1))) takes.log.push(now);
  }).observe(document.body, { subtree: true, childList: true, attributes: true });
  addEventListener('click', ({ target }) => {
    if (!target.closest?.('button[aria-label="Render"]')) return;
    const before = look();
    requestAnimationFrame(() => { takes.pressed = { before, after: look() }; });
  }, true);
}, name);

/** What Focus's viewer draws of `name` on every change: whether it waits on a take being made, and the take whose pixels show. */
const recordViewer = (page, name) => page.evaluate((name) => {
  const look = () => {
    const open = document.querySelector(`[data-focused-asset="${CSS.escape(name)}"]`);
    const media = open?.querySelector('.focus-viewer .media');
    const pixels = media?.querySelector('.media-pixels');
    const host = pixels?.querySelector('.buffered-image');
    const image = host?.querySelector('img');
    return {
      pending: Boolean(media?.classList.contains('pending-image')),
      sha: pixels && image && getComputedStyle(pixels).visibility !== 'hidden' ? (host.dataset.displayedSha ?? '').replace(/^sha256:/, '') || null : null,
    };
  };
  const log = (window.__viewer = [look()]);
  new MutationObserver(() => {
    const now = look();
    if (JSON.stringify(now) !== JSON.stringify(log.at(-1))) log.push(now);
  }).observe(document.body, { subtree: true, childList: true, attributes: true });
}, name);

test('render', async ({ page, project, api, cli }) => {
  const warns = join(project.path, 'warns.py');
  writeFileSync(warns, WARNS);
  // Its environment readied while the rest goes: no box names it yet, so the palette's list isn't held reading it.
  const primed = cli('run', `${warns}/warned`, 'instructions=primed', '--on', 'local');
  // `slow` and `after`, which reads it, are what Cancel stops; `slow` names tool.py, so the palette lists its tools.
  await project.add({ name: 'slow', tool: `${TOOL}/slow`, kind: 'image', fields: { instructions: 'Slow.', seconds: 30 }, at: { x: 0, y: 0 } });
  await project.add({ name: 'after', tool: STILL, kind: 'image', fields: { instructions: 'After <slow>.' }, at: { x: 360, y: 0 } });
  await project.add({ name: 'latent', tool: LATENTS, kind: 'image', fields: { instructions: 'A denoised still', steps: 16, seconds: 0.5 }, at: { x: 720, y: 0 } });
  const before = new Set(['slow', 'after', 'latent']);
  // The palette lists the local code the project's boxes name: read, it's in the page's first list.
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the local tools listed', timeout: 120_000 })
    .toEqual(expect.arrayContaining([STILL, LATENTS]));
  await page.context().addInitScript(keepNotes);
  const notes = [];
  const pull = async () => notes.push(...(await page.evaluate(() => window.__notes)));
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await page.evaluate(() => { window.__unreloaded = true; });
  let first;

  await test.step('"A still" from the palette, typed in and rendered by Render; another box by Ctrl+Enter', async () => {
    const still = await toolRow(page, 'Image', STILL);
    await expect(still).toHaveAttribute('data-tool-operation', 'image');
    await carry(page, still, await openPoint(page));
    const fresh = async () => (await project.assets()).filter((asset) => asset.tool === STILL && !before.has(asset.name)).map((asset) => asset.name);
    await expect.poll(fresh, { message: 'a box of A still, dropped' }).toHaveLength(1);
    [first] = await fresh();
    await fit(page);

    let region = await focus(page, first);
    await region.locator('.in-frame [role="textbox"][data-field="instructions"]').click();
    await page.keyboard.type('A red lighthouse');
    await recordTakes(page, first);
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(() => page.evaluate(() => window.__takes.pressed), { message: 'the frame after Render' }).not.toBeNull();
    const { before: was, after: next } = await page.evaluate(() => window.__takes.pressed);
    const take = next.making.find((id) => !was.making.includes(id));
    expect(take, 'its take listed in the frame after Render').toBeTruthy();
    await expect.poll(async () => (await project.asset(first))?.versions, { message: `${first}'s take`, timeout: 120_000 }).toBe(1);
    const made = await project.asset(first);
    expect.soft(made.fields?.instructions, 'what was typed is what rendered').toBe('A red lighthouse');
    expect.soft(made.prefixes[0].id, 'it lands as the version of its id').toBe(take);
    await expect.soft(region.getByRole('button', { name: 'version 1', exact: true })).toHaveAttribute('data-take-id', take);
    const log = await page.evaluate(() => window.__takes.log);
    const listed = log.findIndex((look) => look.making.includes(take) || look.landed.includes(take));
    expect.soft(log.slice(listed).filter((look) => look.open && !look.making.includes(take) && !look.landed.includes(take)), 'never off the column until it lands').toEqual([]);
    // Its pending tile and its Cancel name its run KEY-N, as the run drawer does, in every look (cl-987, #3341).
    const { key } = await api.get(project.at('/key'));
    const named = log.flatMap((look) => look.runs);
    expect.soft(Boolean(key) && named.length > 0 && named.every(({ run, tile, cancel }) => tile === `Run ${key}-${run}` && cancel === `Cancel run ${key}-${run}`),
      `its pending tile and its Cancel say Run ${key}-N: ${JSON.stringify(named.slice(0, 4))}`).toBe(true);
    await softly.poll(() => drawn(region.locator('.focus-viewer')), { message: 'its take drawn where it was made' }).toBe(shaOf(made, 1));
    expect.soft(await page.evaluate(() => window.__unreloaded), 'with no reload').toBe(true);
    await back(page);

    // The other: Ctrl+Enter in its words, then watched at rest.
    region = await focus(page, 'latent');
    await recordBox(page, 'latent');
    await region.locator('.in-frame [role="textbox"][data-field="instructions"]').click();
    await page.keyboard.press('End');
    await page.keyboard.type(' at dusk');
    const asked = page.waitForRequest((request) => request.method() === 'POST' && /^\/v1\/projects\/[^/]+\/runs$/.test(new URL(request.url()).pathname));
    await page.keyboard.press('ControlOrMeta+Enter');
    await asked;
    await back(page);
    await expect.poll(async () => (await canvasCard(page, 'latent'))?.rendered?.latent?.opacity ?? 0, { message: 'a passive latent preview is drawn', timeout: 120_000 }).toBeGreaterThan(0);
    const latentBody = (await canvasCard(page, 'latent')).rendered.body, previewPixels = screenshotPixels(await page.screenshot({ scale: 'css' }));
    const previewColours = new Set();
    for (let y = 7; y < 12; y++) for (let x = 1; x < 12; x++) previewColours.add(previewPixels(latentBody.x + latentBody.width * x / 12, latentBody.y + latentBody.height * y / 12).map(value => Math.round(value / 8)).join());
    expect.soft(previewColours.size, 'the passive latent preview paints nonuniform pixels below the prompt text').toBeGreaterThan(8);
    await expect.poll(async () => (await project.asset('latent'))?.versions, { message: "latent's take", timeout: 120_000 }).toBe(1);
    const latent = await project.asset('latent');
    await softly.poll(async () => { const shown = (await canvasCard(page, 'latent'))?.rendered; return shown?.media.ready ? shown.media.sha : null; }, { message: 'its take replaces the preview' }).toBe(shaOf(latent, 1));
    await expect.poll(async () => { const shown = (await canvasCard(page, 'latent'))?.rendered; return Boolean(shown && !shown.landing && !shown.glide && shown.transform.alpha === 1); }, { message: 'the final output presentation settles' }).toBe(true);
    const main = latent.prefixes.find(version => version.number === 1).main;
    const answer = await api.raw('GET', project.at(`/${main.path.split('/').map(encodeURIComponent).join('/')}`));
    expect(answer.ok, 'the real final PNG can be read').toBe(true);
    const expectedPixels = screenshotPixels(Buffer.from(await answer.arrayBuffer()));
    const finalBody = (await canvasCard(page, 'latent')).rendered.body, finalPixels = screenshotPixels(await page.screenshot({ scale: 'css' }));
    const mean = (read, x, y, width, height) => {
      const sum = [0, 0, 0]; let count = 0;
      for (let py = Math.ceil(y); py < y + height; py++) for (let px = Math.ceil(x); px < x + width; px++) { read(px, py).forEach((value, c) => { sum[c] += value; }); count++; }
      return sum.map(value => value / count);
    };
    for (const [u, v] of [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]]) {
      const actual = mean(finalPixels, finalBody.x + finalBody.width * (u - 0.1), finalBody.y + finalBody.height * (v - 0.1), finalBody.width * 0.2, finalBody.height * 0.2);
      const expected = mean(expectedPixels, expectedPixels.width * (u - 0.1), expectedPixels.height * (v - 0.1), expectedPixels.width * 0.2, expectedPixels.height * 0.2);
      expect.soft(Math.max(...actual.map((value, c) => Math.abs(value - expected[c]))), `final output replaces faded latent pixels at ${u},${v}`).toBeLessThanOrEqual(24);
    }
    const sayings = await said(page, 'latent');
    const dots = sayings.map((look) => look.dot);
    const queued = dots.indexOf('Queued');
    expect.soft(queued, 'its dot says Queued').toBeGreaterThanOrEqual(0);
    expect.soft(dots.indexOf('Rendering', queued), 'then Rendering').toBeGreaterThan(queued);
    const faces = sayings.map((look) => look.face).filter(Boolean);
    expect.soft(faces[0], 'its face says Queued first').toMatch(/^Queued/);
    expect.soft(faces.some((words) => /^Denoising/.test(words)), `then its stage: ${faces.join(' | ')}`).toBe(true);
    const faded = sayings.map((look) => look.preview).filter((opacity) => opacity !== null);
    expect.soft(faded.length, 'a preview of its latents shows as it renders').toBeGreaterThan(0);
    expect.soft(faded.every((opacity) => opacity > 0 && opacity < 1), `faded: ${faded.join(' ')}`).toBe(true);
    await softly.poll(async () => (await canvasCard(page, 'latent'))?.rendered?.latent ?? null, { message: 'its take replaces it' }).toBeNull();
    await softly.poll(() => renderState(page, 'latent')).toBeNull();

    // Landed at rest: its note names it and its take, and View opens it on that take.
    const note = page.locator('[data-note]').filter({ hasText: 'Version 1 of latent is ready' });
    await expect.soft(note).toBeVisible();
    await note.getByRole('button', { name: 'View', exact: true }).click();
    region = focused(page);
    await expect.soft(region).toHaveAttribute('data-focused-asset', 'latent');
    await canvasIdle(page);
    await expect.soft(region.getByLabel('Take', { exact: true })).toHaveText('v1');
    await back(page);
    expect.soft(notes.concat(await page.evaluate(() => window.__notes)).filter((text) => text.includes(`of ${first} is ready`)), 'a take landing where it is looked at says nothing').toEqual([]);
  });

  await test.step('Render all, on a slow box that mentions the first', async () => {
    await api.patch(project.at('/assets/slow'), { fields: { instructions: `After <${first}>, slowly.` } });
    await expect.poll(async () => (await project.asset('slow'))?.fields?.instructions).toBe(`After <${first}>, slowly.`);
    await recordBox(page, 'after');
    await recordBox(page, 'slow');
    await page.getByRole('button', { name: /^Render all \d+$/ }).click();
    await expect.poll(() => renderState(page, 'after'), { message: 'after waits for slow', timeout: 60_000 }).toMatch(/^Waiting for <slow>/);
  });

  await test.step('Cancel one box while it waits for its input and another while it renders', async () => {
    await hoverBox(page, 'after');
    await face(page, 'after').getByRole('button', { name: "Cancel after's render", exact: true }).click();
    await page.mouse.move(2, 500);
    await expect.poll(() => settled(project, 'after'), { message: 'after canceled, its take gone' }).toEqual({ takes: 0, making: 0, versions: 0 });
    await softly.poll(() => renderState(page, 'after')).toBeNull();
    await expect.poll(() => renderState(page, 'slow'), { message: 'slow renders', timeout: 90_000 }).toMatch(/^Drawing/);
    // The whole job's meter, at rest, rises as it goes and never goes back. Nothing like slow ran here, so its estimate is its own steps',
    // and those say no time left until its second (43405964751: 99 from then on, "6s / 6s" then "7s / 21s"): the meter isn't pinned by it.
    const rising = async () => {
      const meter = (await said(page, 'slow')).map((look) => look.meter).filter((value) => value !== null);
      return meter.length > 1 && meter.every((value, i) => i === 0 || value >= meter[i - 1]) && meter.at(-1) > meter[0] ? 'rising' : `not rising: ${meter.join(' ') || 'no meter'}`;
    };
    await softly.poll(rising, { message: "slow's whole job's meter", timeout: 15_000 }).toBe('rising');
    // Its first step timed as it ran (3 s), not from its own ctx.progress (~0 ms): once it's done ("Drawing 1 / 10": a stage no model's
    // steps), the time left is its 9 steps to go,
    // not none (43405964751: "4s / 4s" after it, "7s / 21s" only after the second).
    const span = (said) => said.split(' ').reduce((all, part) => all + Number(part.slice(0, -1)) * { s: 1, m: 60, h: 3600 }[part.at(-1)], 0);
    const firstStep = async () => {
      const look = (await said(page, 'slow')).filter((one) => /(?:^|\s)1 \/ 10$/.test(one.face ?? '') && one.time).at(-1);
      const [since, whole] = look ? look.time.replace(/^ - /, '').split(' / ').map(span) : [];
      return look ? whole - since : null;
    };
    await softly.poll(firstStep, { message: "slow's time left once its first step is done, s", timeout: 15_000 }).toBeGreaterThanOrEqual(10);
    const [run] = (await project.asset('slow'))?.making ?? [];
    expect(run, "slow's run").toBeTruthy();
    await hoverBox(page, 'slow');
    await face(page, 'slow').getByRole('button', { name: "Cancel slow's render", exact: true }).click();
    await page.mouse.move(2, 500);
    await expect.poll(() => settled(project, 'slow'), { message: 'slow canceled, its take gone', timeout: 30_000 }).toEqual({ takes: 0, making: 0, versions: 0 });
    await softly.poll(async () => (await api.get(project.at(`/runs/${run}`))).status, { message: `run ${run}` }).toBe('canceled');
    await softly.poll(() => renderState(page, 'slow')).toBeNull();
    const waited = (await said(page, 'after')).map((look) => look.face).filter(Boolean);
    expect.soft(waited.some((words) => /^Waiting for <slow>/.test(words)), `after says it waits for slow: ${waited.join(' | ')}`).toBe(true);
    expect.soft(waited.filter((words) => /Drawing|\b\d+ \/ 10\b/.test(words)), "after shows none of slow's progress").toEqual([]);
  });

  await test.step('Render the crash tool', async () => {
    // cl-1070 (Paul, 11-16): "do not keep failed runs around as if they were takes I can select"; "an error, when it happens, is a toast
    // notification, and the take disappears". It's said once, its take leaves the column with nothing to delete, and the frame that
    // watched it goes back to the take it showed.
    await project.add({ name: 'broken', tool: `${warns}/crashes`, kind: 'image', fields: { instructions: 'A still that fails.' }, at: { x: 0, y: 420 } });
    await api.post(project.at('/assets/broken/versions'), { source: await project.upload(png(60, 16, 16)) });
    await api.post(project.at('/unpin'), { name: 'broken' });
    await expect.poll(async () => { const held = await project.asset('broken'); return { versions: held?.versions, pinned: Boolean(held?.pinned) }; })
      .toEqual({ versions: 1, pinned: false });
    const v1 = shaOf(await project.asset('broken'), 1);
    await fit(page);
    let region = await focus(page, 'broken');
    await expect(region.getByLabel('Take', { exact: true })).toHaveText('v1');
    await expect.poll(() => drawn(region.locator('.focus-viewer')), { message: 'v1 drawn' }).toBe(v1);
    await recordTakes(page, 'broken');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('broken'))?.failure?.message ?? '', { message: 'it failed', timeout: 120_000 }).toContain(BROKEN);
    expect.soft((await page.evaluate(() => window.__takes.log)).some((look) => look.making.length > 0), 'its take listed while it rendered').toBe(true);
    const toast = page.locator('[data-note]').filter({ hasText: "broken didn't render" });
    await expect.soft(toast, 'a toast says it').toHaveCount(1);
    await expect.soft(toast).toContainText(BROKEN);
    await softly.poll(async () => (await project.asset('broken'))?.takes ?? [], { message: 'no failed take kept', timeout: 15_000 }).toEqual([]);
    const gone = async (when) => {
      await expect.soft(region.locator('.here-side .take.making'), `${when}: no take of it in the column, nothing to delete`).toHaveCount(0);
      await expect.soft(region.getByLabel('Take', { exact: true }), `${when}: the frame back on the take it showed`).toHaveText('v1');
      await softly.poll(() => drawn(region.locator('.focus-viewer')), { message: `${when}: v1 drawn again` }).toBe(v1);
    };
    await gone('failed');
    await notesOver(page, 2000);
    await expect.soft(toast, 'said once').toHaveCount(1);
    await pull();
    await page.reload();
    region = focused(page);
    await expect(region).toHaveAttribute('data-focused-asset', 'broken');
    await canvasIdle(page);
    await gone('after a reload');
    await expect.soft(toast, 'not said again after a reload').toHaveCount(0);
    await back(page);
  });

  await test.step('Watch one box land in Focus, then reload', async () => {
    const run = await primed;
    expect(run.code, `cozy run of warns.py: ${run.stderr.slice(-400)}`).toBe(0);
    await project.add({ name: 'warned', tool: `${warns}/warned`, kind: 'image', fields: { instructions: 'A still that warns.' }, at: { x: 360, y: 420 } });
    for (const shade of [30, 90]) await api.post(project.at('/assets/warned/versions'), { source: await project.upload(png(shade, 16, 16)) });
    // The first file joined is approved (format::can_initialize_approval): unapproved, the take it makes is its state, with its warnings.
    await api.post(project.at('/unpin'), { name: 'warned' });
    await expect.poll(async () => { const held = await project.asset('warned'); return { versions: held?.versions, pinned: Boolean(held?.pinned) }; })
      .toEqual({ versions: 2, pinned: false });
    await fit(page);
    const region = await focus(page, 'warned');
    await recordViewer(page, 'warned');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('warned'))?.versions, { message: "warned's third take", timeout: 120_000 }).toBe(3);
    const warned = await project.asset('warned');
    expect.soft({ version: warned.version, pinned: Boolean(warned.pinned) }, 'the take it made is its state').toEqual({ version: 3, pinned: false });
    const third = shaOf(warned, 3);
    await softly.poll(() => drawn(region.locator('.focus-viewer')), { message: 'the take watched is drawn as it lands' }).toBe(third);
    await expect.soft(region.getByLabel('Take', { exact: true })).toHaveText('v3');
    const viewer = await page.evaluate(() => window.__viewer);
    const watching = viewer.findIndex((look) => look.pending);
    expect.soft(watching, 'it watched the take being made').toBeGreaterThanOrEqual(0);
    expect.soft(viewer.slice(watching).filter((look) => look.sha && look.sha !== third), 'no other take drawn as it lands').toEqual([]);
    // Its run's warning is the box's: in its project's warnings and on its head, never a note.
    await softly.poll(async () => ((await project.state()).warnings ?? []).filter((line) => line.name === 'warned').map((line) => line.message).join('\n'), { message: "the run's warning, the box's" })
      .toContain(WARNED);
    const warnings = region.getByRole('button', { name: 'Warnings', exact: true });
    await expect.soft(warnings).toHaveAttribute('title', new RegExp(WARNED));
    // Waited out past when a note would come; with every note since the reload, one up before the window is seen too.
    const meanwhile = (await notesOver(page, 2000)).concat(await page.evaluate(() => window.__notes));
    expect.soft(meanwhile.filter((text) => text.includes('warned')), 'a take landing where it is looked at says nothing').toEqual([]);

    await pull();
    await page.reload();
    await expect(focused(page)).toHaveAttribute('data-focused-asset', 'warned');
    await canvasIdle(page);
    await softly.poll(() => drawn(focused(page).locator('.focus-viewer')), { message: 'the same take after a reload' }).toBe(third);
    await expect.soft(focused(page).getByLabel('Take', { exact: true })).toHaveText('v3');
    await expect.soft(focused(page).getByRole('button', { name: 'Warnings', exact: true })).toHaveAttribute('title', new RegExp(WARNED));
    await pull();
    expect.soft(notes.filter((text) => text.includes(WARNED)), 'no note says a warning').toEqual([]);
  });

  await test.step("Render two boxes at once: the one whose tool is warm is ready before the other's install ends, waiting only for the CPU, named", async () => {
    // One render of both, so they're ready in one turn of its walk: `eager` on tool.py, installed and read by this project in the first step;
    // `fresh` on installs.py, whose first render installs it (cl-973: a box that can start now never waits for another's install).
    // cl-1086 (Paul: "a run never waits on something it doesn't need"; "every wait is tied to what it waits on, observed"): a run of
    // tool.py's `slow` holds this machine's one CPU slot first, so eager waits for it whatever else the lane runs, and its start (a cold
    // executor, ~2 s) judges nothing. Its log says what it waits for as the wait begins, naming that run, on the machine's clock: it is
    // ready before fresh's install ends, and nothing it waits for is fresh's. Once the CPU is free, eager goes before fresh.
    const installs = join(project.path, 'installs.py');
    writeFileSync(installs, INSTALLS);
    await project.add({ name: 'eager', tool: STILL, kind: 'image', fields: { instructions: 'Ready now.' }, at: { x: 720, y: 420 } });
    await project.add({ name: 'fresh', tool: `${installs}/installed`, kind: 'image', fields: { instructions: 'Once installed.' }, at: { x: 1080, y: 420 } });
    const runs = async () => (await api.get(project.at('/runs?limit=1000'))).runs;
    const earlier = new Set((await runs()).map((run) => run.id));
    const made = async (kind, target) => (await runs()).find((run) => !earlier.has(run.id) && run.kind === kind && run.target === target);
    const holding = cli('run', `${TOOL}/slow`, 'instructions=Holds the CPU.', 'seconds=600', '--on', 'local');
    let hold;
    await expect.poll(async () => (hold = await made('call', 'local/tool/slow'))?.started_at_ms ?? 0, { message: 'a slow run holds the CPU', timeout: 120_000 })
      .toBeGreaterThan(0);
    const rendering = cli('--json', 'render', 'eager', 'fresh', '--on', 'local');
    await expect.poll(async () => (await made('warm', 'install local/installs'))?.status, { message: "fresh's install, while eager waits", timeout: 120_000 })
      .toBe('completed');
    await api.post(project.at(`/runs/${encodeURIComponent(hold.id)}/cancel`));
    const takes = async () => ({ eager: (await project.asset('eager'))?.versions ?? 0, fresh: (await project.asset('fresh'))?.versions ?? 0 });
    await expect.poll(takes, { message: 'both takes', timeout: 120_000 }).toEqual({ eager: 1, fresh: 1 });
    const rendered = await rendering;
    await holding;
    expect.soft(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
    const install = await made('warm', 'install local/installs');
    const eager = await made('call', 'local/tool/still');
    const fresh = await made('call', 'local/installs/installed');
    // What each run's machine said, as its log keeps it: its stages and waits, each when it began, on the machine's clock.
    const stages = async (run) => {
      if (!run) return [];
      const stream = await (await api.raw('GET', project.at(`/runs/${encodeURIComponent(run.id)}/events`))).text();
      return stream.split('\n').filter((line) => line.startsWith('data:')).map((line) => JSON.parse(line.slice(5)))
        .filter((event) => event.type === 'run.log' && event.level === 'stage').map((event) => ({ at: event.at_ms, text: event.text }));
    };
    const [waited, building] = [await stages(eager), await stages(install)];
    // Built, the install gives up the machine's install line and describes what it built (said, not judged: a warm uv cache builds in 0.7 s).
    const built = building.find((line) => line.text.startsWith('describing '))?.at;
    // Ready: queued for what it waits on, or else starting.
    const queued = waited.filter((line) => / in the queue\b/.test(line.text));
    const ready = queued[0]?.at ?? waited.find((line) => /^(importing |starting the executor)/.test(line.text))?.at ?? eager?.started_at_ms;
    const times = (run) => run && { number: run.number, status: run.status, created: run.created_at_ms, started: run.started_at_ms, ended: run.settled_at_ms };
    const told = JSON.stringify({ hold: hold.id, built, ready, eager: { ...times(eager), log: waited }, install: times(install), fresh: times(fresh) });
    expect.soft([install, eager, fresh].map((run) => run?.status), `fresh's install, eager's call, fresh's call: ${told}`).toEqual(['completed', 'completed', 'completed']);
    expect.soft(ready < install?.settled_at_ms, `eager is ready, waiting only for the CPU, before fresh's install ends: ${told}`).toBe(true);
    expect.soft(queued.map((line) => line.text).join('\n'), `eager's wait names the run holding the CPU: ${told}`).toContain(hold.id);
    const theirs = waited.filter((line) => /install/.test(line.text) || [install?.id, fresh?.id].some((id) => id && line.text.includes(id)));
    expect.soft(theirs, `eager waits for nothing of fresh's: ${told}`).toEqual([]);
    expect.soft(eager?.started_at_ms < fresh?.started_at_ms, `the CPU free, eager goes before fresh: ${told}`).toBe(true);
  });
});

// cl-1072 (Paul, 11-16): "the aspect ratio of the preview should be equal to what we expect it to be like of the final generated image";
// "if it's non-deterministic (i.e., 'auto' is set and there is no default) then just go with whatever aspect ratio the other reference
// images are". A 16:9 set, and an edit with Auto of a 32×18 picture (`still` takes 1:1 or 16:9): its queued and rendering tile is 16:9.
test('a picture on its way has the shape it will be in its takes column', async ({ page, project, api }) => {
  await project.add({ name: 'wide', tool: STILL, kind: 'image', fields: { instructions: 'A wide still.', aspect_ratio: '16:9' }, at: { x: 0, y: 0 } });
  await project.add({ name: 'edited', tool: STILL, kind: 'image', fields: { instructions: 'Warmer.' }, at: { x: 360, y: 0 } });
  await api.post(project.at('/assets/edited/versions'), { source: await project.upload(png(120, 32, 18)) });
  await expect.poll(async () => (await project.asset('edited'))?.versions).toBe(1);
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the local tool listed', timeout: 120_000 })
    .toContain(STILL);
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await fit(page);
  for (const [name, edit] of [['wide', false], ['edited', true]]) {
    const region = await focus(page, name);
    if (edit) await region.getByRole('button', { name: 'Edit image', exact: true }).click();
    await page.evaluate((name) => {
      const looks = (window.__tiles = []);
      new MutationObserver(() => {
        for (const tile of document.querySelectorAll(`[data-focused-asset="${CSS.escape(name)}"] .here-side .take.making`)) {
          const look = { width: tile.offsetWidth, height: tile.offsetHeight };
          if (!looks.some((held) => held.width === look.width && held.height === look.height)) looks.push(look);
        }
      }).observe(document.body, { subtree: true, childList: true, attributes: true });
    }, name);
    const before = (await project.asset(name))?.versions ?? 0;
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset(name))?.versions, { message: `${name}'s take`, timeout: 120_000 }).toBe(before + 1);
    if (edit) expect.soft((await project.asset(name))?.fields?.instructions, 'an edit of its take 1').toContain('<self.1>');
    const looks = await page.evaluate(() => window.__tiles);
    expect.soft(looks.length > 0 && looks.every(({ width, height }) => Math.abs(width - height * 16 / 9) <= 1), `${name}'s tile on its way is 16:9: ${JSON.stringify(looks)}`).toBe(true);
    await back(page);
  }
});
