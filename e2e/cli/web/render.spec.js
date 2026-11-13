// render (cl-973 core cell 11), on local: "A still" from the palette, typed in and rendered by Render, lists its take the frame after the
// press and keeps that one id until it lands in place, unreloaded; another box rendered by Ctrl+Enter says Queued, then Rendering, with a
// faded preview of its latents until its take replaces it, and its finish note's View opens it. Render all: a box waiting on another says
// so and shows none of the other's progress; the other's whole job's meter rises as it renders; Cancel stops one waiting and one
// rendering, and their takes go.
// A failed take stays red, its error with Copy, through a reload, until its trash deletes it. The take watched in Focus is the one drawn
// when it lands, through a reload; its run's warnings are the box's, never a note.
const { writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, box, canvasIdle, focus, middle, openPoint, fixtureTool, png } = require('./fixtures');

const TOOL = fixtureTool('tool.py');
const STILL = `${TOOL}/still`;
const LATENTS = `${fixtureTool('latents.py')}/latents`;
const CRASH = `${fixtureTool('shots.py')}/crash`;
const BROKEN = 'the crash tool fails, as the render cell asks';
const WARNED = 'a handled fallback, as the render cell asks';
// A run whose result warns, as a handled fallback does: no fixture tool's does. Its dependencies are tool.py's.
const WARNS = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The render cell's tool: a still of one colour whose result warns."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame, Outputs
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
def warned(payload: Drawing, out: Outputs) -> Warned:
    """A still of one colour, and a warning."""
    image = out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return Warned(image, payload.instructions.text, ["${WARNED}"])
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
    const card = document.querySelector(`.react-flow__node .card[data-asset="${CSS.escape(name)}"]`);
    const open = document.querySelector(`[data-focused-asset="${CSS.escape(name)}"]`);
    const face = [...document.querySelectorAll('[role="status"]')].find((one) => one.getAttribute('aria-label') === `${name}'s render`);
    const meter = card?.querySelector('[role="progressbar"][aria-label="The whole job"]');
    const preview = card?.querySelector('.latent-preview img');
    return {
      face: face?.querySelector('.face-state b')?.textContent ?? null,
      dot: (open?.querySelector('.focus-title .state-dot') ?? card?.querySelector('.head .state-dot'))?.getAttribute('aria-label') ?? null,
      meter: meter ? Number(meter.getAttribute('aria-valuenow')) : null,
      preview: preview?.complete && preview.naturalWidth > 0 ? Number(getComputedStyle(preview).opacity) : null,
    };
  };
  new MutationObserver(() => {
    const now = look();
    if (JSON.stringify(now) !== JSON.stringify(log.at(-1))) log.push(now);
  }).observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
  log.push(look());
}, name);
const said = (page, name) => page.evaluate((name) => window.__said[name], name);

/** Focus's takes column for `name` on every change, by take id; and, heard before the page's own handler, the column as Render was pressed
 *  and in the next frame. */
const recordTakes = (page, name) => page.evaluate((name) => {
  const look = () => {
    const side = document.querySelector(`[data-focused-asset="${CSS.escape(name)}"] .here-side`);
    const ids = (selector) => [...(side?.querySelectorAll(selector) ?? [])].map((tile) => tile.getAttribute('data-take-id')).filter(Boolean);
    return { open: Boolean(side), making: ids('.take.making[data-take-id]'), landed: ids('.take[data-take][data-take-id]') };
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
    const tools = page.getByRole('complementary', { name: 'Tools' });
    const image = tools.locator('button[data-operation="image"]');
    await image.click();
    await expect(image).toHaveAttribute('aria-expanded', 'true');
    const still = tools.locator(`li[data-implementation][data-tool-operation="image"][data-tool=${JSON.stringify(STILL)}]`);
    await expect(still).toBeVisible();
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
    await expect.poll(async () => (await project.asset('latent'))?.versions, { message: "latent's take", timeout: 120_000 }).toBe(1);
    const latent = await project.asset('latent');
    await softly.poll(() => drawn(box(page, 'latent')), { message: 'its take replaces the preview' }).toBe(shaOf(latent, 1));
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
    await expect.soft(box(page, 'latent').locator('.latent-preview'), 'its take replaces it').toHaveCount(0);
    await expect.soft(face(page, 'latent')).toHaveCount(0);

    // Landed at rest: its note names it and its take, and View opens it on that take.
    const note = page.locator('[data-note]').filter({ hasText: 'latent finished: version 1' });
    await expect.soft(note).toBeVisible();
    await note.getByRole('button', { name: 'View', exact: true }).click();
    region = focused(page);
    await expect.soft(region).toHaveAttribute('data-focused-asset', 'latent');
    await canvasIdle(page);
    await expect.soft(region.getByLabel('Take', { exact: true })).toHaveText('v1');
    await back(page);
    expect.soft(notes.concat(await page.evaluate(() => window.__notes)).filter((text) => text.includes(`${first} finished`)), 'a take landing where it is looked at says nothing').toEqual([]);
  });

  await test.step('Render all, on a slow box that mentions the first', async () => {
    await api.patch(project.at('/assets/slow'), { fields: { instructions: `After <${first}>, slowly.` } });
    await expect.poll(async () => (await project.asset('slow'))?.fields?.instructions).toBe(`After <${first}>, slowly.`);
    await recordBox(page, 'after');
    await recordBox(page, 'slow');
    await page.getByRole('button', { name: /^Render all \d+$/ }).click();
    await expect(face(page, 'after').locator('.face-state b'), 'after waits for slow').toHaveText(/^Waiting for <slow>/, { timeout: 60_000 });
  });

  await test.step('Cancel one box while it waits for its input and another while it renders', async () => {
    await face(page, 'after').getByRole('button', { name: "Cancel after's render", exact: true }).click();
    await expect.poll(() => settled(project, 'after'), { message: 'after canceled, its take gone' }).toEqual({ takes: 0, making: 0, versions: 0 });
    await expect.soft(face(page, 'after')).toHaveCount(0);
    await expect(face(page, 'slow').locator('.face-state b'), 'slow renders').toHaveText(/^Drawing/, { timeout: 90_000 });
    // The whole job's meter, at rest, rises as it goes. Nothing like slow ran here, so its estimate is its own steps': latents.py's is
    // setup's 0 s steps (43401337115: 99 on every frame of latent's 8 s), and the meter never goes back within a run.
    const rising = async () => {
      const meter = (await said(page, 'slow')).map((look) => look.meter).filter((value) => value !== null);
      return meter.length > 1 && meter.every((value, i) => i === 0 || value >= meter[i - 1]) && meter.at(-1) > meter[0] ? 'rising' : `not rising: ${meter.join(' ') || 'no meter'}`;
    };
    await softly.poll(rising, { message: "slow's whole job's meter", timeout: 15_000 }).toBe('rising');
    const [run] = (await project.asset('slow'))?.making ?? [];
    expect(run, "slow's run").toBeTruthy();
    await face(page, 'slow').getByRole('button', { name: "Cancel slow's render", exact: true }).click();
    await expect.poll(() => settled(project, 'slow'), { message: 'slow canceled, its take gone', timeout: 30_000 }).toEqual({ takes: 0, making: 0, versions: 0 });
    await softly.poll(async () => (await api.get(project.at(`/runs/${run}`))).status, { message: `run ${run}` }).toBe('canceled');
    await expect.soft(face(page, 'slow')).toHaveCount(0);
    const waited = (await said(page, 'after')).map((look) => look.face).filter(Boolean);
    expect.soft(waited.some((words) => /^Waiting for <slow>/.test(words)), `after says it waits for slow: ${waited.join(' | ')}`).toBe(true);
    expect.soft(waited.filter((words) => /Drawing|\b\d+ \/ 10\b/.test(words)), "after shows none of slow's progress").toEqual([]);
  });

  await test.step('Render the crash tool', async () => {
    await project.add({ name: 'broken', tool: CRASH, kind: 'video', fields: { crash: BROKEN }, at: { x: 0, y: 420 } });
    await expect.poll(() => project.asset('broken')).toBeTruthy();
    await fit(page);
    let region = await focus(page, 'broken');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('broken'))?.takes?.filter((take) => take.failure).length, { message: 'its take failed', timeout: 120_000 }).toBe(1);
    const failed = region.locator('.here-side .take.making[data-state="failed"]');
    const error = region.locator('.focus-viewer .run-face.failed');
    const red = async (when) => {
      await expect.soft(failed, `${when}: listed, saying so`).toHaveText('Failed');
      await expect.soft(failed.getByRole('img', { name: 'Failed', exact: true }), `${when}: its red dot`).toBeVisible();
      await expect.soft(error, `${when}: its error in the frame`).toContainText(BROKEN);
      await expect.soft(error.getByRole('button', { name: 'Copy', exact: true }), `${when}: Copy`).toBeVisible();
    };
    await red('failed');
    await pull();
    await page.reload();
    region = focused(page);
    await expect(region).toHaveAttribute('data-focused-asset', 'broken');
    await canvasIdle(page);
    await failed.click();
    await red('after a reload');
    await region.getByRole('button', { name: 'Delete this failed take', exact: true }).click();
    await expect.poll(async () => (await project.asset('broken'))?.takes?.length ?? 0, { message: 'its take deleted' }).toBe(0);
    await expect.soft(failed).toHaveCount(0);
    await pull();
    await page.reload();
    await expect(focused(page)).toHaveAttribute('data-focused-asset', 'broken');
    await canvasIdle(page);
    await expect.soft(failed, 'deleted, through a reload').toHaveCount(0);
    expect.soft((await project.asset('broken')).takes ?? [], 'deleted for good').toEqual([]);
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
    const meanwhile = await notesOver(page, 2000);
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
});
