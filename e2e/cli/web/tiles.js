// A box on the canvas at rest is what Paul drew (cl-932, 11-12), in a real browser on a real Calcifer:
//     Node-Name - version (like v1) X (delete this node)
//     (Image / thumbnail / preview)
//     Tool (if any) - aspect ratio
//     Run progress - cancel button
// An idle picture with Qwen's Image, an idle video with H3's Shot, a dropped picture with no tool, and a box rendering on this computer: their
// rows and nothing else (no Edit, no Pick up from here, no chips), the render's lines under its picture with Cancel, at zoom 1 and 0.5, each
// a screenshot. Open, Edit and Pick up from here are still there. Cancel on the tile stops its render. A render's whole life, from Render to
// its take landing, leaves the tile's size as it was (cl-936).
//   node tiles.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// nothing renders. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, importYaml, lastLook, launch, lookAt, noStall, openBox, png, settled, until, upload } = require('./page');

const FIRST_RUN_MS = 300_000;
// A still made in slow steps, so its tile is seen rendering at both zooms; it's canceled from its tile.
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The tiles cell's picture tool: a weightless still of one random colour, made in slow steps."""

import os
import time

import msgspec
from cozy_runtime.author import Adjustments, App, Context, ImageFrame, Outputs, Seed, Telemetry
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageRequest, ImageResult, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    pace: float = 4.0


@app.tool(label="Tiles", named="tiles", accelerator=False)
def draw(ctx: Context, payload: Drawing, adj: Adjustments, out: Outputs, tel: Telemetry) -> ImageResult:
    width, height = ImageRequest(payload.instructions.text, None, None, payload.seed).size(adj, None, SIZES, megapixels=0.25)
    with tel.stage("denoise"):
        for step in range(30):
            time.sleep(payload.pace)
            ctx.progress(step, 30)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
`;

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why instanceof Error ? why.stack ?? why.message : why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

/** A tool as the page names it (shown.ts toolName). */
const named = (tool) => (tool.package_title && tool.package_title !== tool.label ? `${tool.package_title} · ${tool.label}` : tool.label);
const SHAPE = /^(\d+(\.\d+)?:\d+|Auto)$/;

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  const shot = (name, clip) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png`, ...(clip ? { clip } : {}) }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json', ...(init.method === 'POST' ? { 'Idempotency-Key': `tiles-${Date.now()}-${Math.random()}` } : {}) } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-tiles-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id, path: project } = await projectFromPage(page);
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: project };
  const local = (await api('/v1/status')).body?.local_usable && !process.env.ALONE;
  const folder = mkdtempSync(join(tmpdir(), 'web-tiles-'));
  writeFileSync(join(folder, 'take.png'), png(80));
  writeFileSync(join(folder, 'plain.png'), png(150));
  writeFileSync(join(folder, 'take.mp4'), MP4);
  const tool = join(project, `tiles_${Date.now()}.py`);
  writeFileSync(tool, TOOL);

  // A box this computer renders; a picture with Qwen's Image and a video with H3's Shot, each with a take; a picture with no tool.
  importYaml(project, `version: 4\nassets:\n  slow:\n    tool: ${tool}/draw\n    instructions: A still of one colour.\n  again:\n    tool: ${tool}/draw\n    instructions: Another still.\n    pace: 0.3\n`);
  const tools = [...((await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? []), ...((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [])];
  const image = tools.find((one) => one.package === 'qwen-image-2' && one.makes === 'image');
  const video = tools.find((one) => one.id === 'minimax-h3/generate') ?? tools.find((one) => one.package === 'minimax-h3' && one.makes === 'video');
  if (!image || !video) throw new Error(`the Hub lists no Qwen Image or H3 Shot: ${JSON.stringify(tools.map((one) => one.id))}`);
  await post(`${at}/assets`, { tool: image.id, kind: 'image', name: 'still', exact: true, fields: { instructions: 'a lighthouse at dusk' }, at: { x: 0, y: 0 } });
  await post(`${at}/assets/still/versions`, { source: await upload(held?.token, join(folder, 'take.png')) });
  await post(`${at}/assets`, { tool: video.id, kind: 'video', name: 'shot', exact: true, fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 5 }] }, at: { x: 380, y: 0 } });
  await post(`${at}/assets/shot/versions`, { source: await upload(held?.token, join(folder, 'take.mp4')) });
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'plain.png')), name: 'plain', exact: true, at: { x: 820, y: 0 } });
  for (const name of ['still', 'shot', 'plain', 'slow']) await boxOf(page, name).waitFor();
  await page.mouse.move(2, 998);
  await page.keyboard.press('f');
  await settled(page);

  /** A tile as drawn: its head's parts in order, what's in its picture that can be pressed, and what hangs under it, in order. */
  const tile = (name) => boxOf(page, name).evaluate((node) => {
    const card = node.querySelector('.card');
    const visible = (el) => el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden' && getComputedStyle(el).display !== 'none';
    const head = [...card.querySelectorAll(':scope > .head > *')].filter(visible).filter((el) => !el.classList.contains('sp'));
    const media = card.querySelector(':scope > .media');
    const m = media.getBoundingClientRect();
    const under = card.querySelector(':scope > .tile-under');
    const line = under?.querySelector('.tile-line');
    const face = under?.querySelector('.run-face');
    const r = (el) => el && el.getBoundingClientRect();
    return {
      head: head.map((el) => el.className.split(' ')[0] + (el.getAttribute('aria-label') ? `[${el.getAttribute('aria-label')}]` : '') + (el.classList.contains('nm') || el.classList.contains('node-version-label') ? `:${el.textContent}` : '')),
      pressable: [...media.querySelectorAll('button, [role=button]')].filter(visible).map((el) => el.getAttribute('aria-label') || el.textContent),
      tool: line?.querySelector('.node-tool')?.textContent ?? null,
      shape: line?.querySelector('.ratio-rest')?.textContent ?? null,
      lines: face ? [...face.querySelectorAll('p')].map((p) => p.innerText.replace(/\s+/g, ' ').trim()) : null,
      cancel: face?.querySelector('.face-cancel')?.textContent ?? null,
      // Under its picture: its tool and shape first, then its render.
      ordered: (!line || r(line).top >= m.bottom - 1) && (!face || r(face).top >= (line ? r(line).bottom : m.bottom) - 1),
      chips: node.querySelectorAll('.add-on, .add-ons').length,
      stray: card.querySelectorAll(':scope > .ratio-rest, .head .node-tool').length,
      wave: Boolean(card.querySelector(':scope > .render-wave')),
      rect: (() => { const all = [card, under].filter(Boolean).map(r); const top = Math.min(...all.map((b) => b.top)) - 40, bottom = Math.max(...all.map((b) => b.bottom)) + 8, left = Math.min(...all.map((b) => b.left)) - 8, right = Math.max(...all.map((b) => b.right)) + 8; return { x: Math.max(0, left), y: Math.max(0, top), width: right - Math.max(0, left), height: bottom - Math.max(0, top) }; })(),
    };
  });
  const zoom = () => page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
  const zoomTo = async (want) => {
    for (let i = 0; i < 20 && Math.abs((await zoom()) - want) > 0.005; i++) {
      await page.click((await zoom()) > want ? 'button[aria-label="Zoom out"]' : 'button[aria-label="Zoom in"]');
      await settled(page);
    }
    await until(`the camera at ${want}`, zoom, (now) => Math.abs(now - want) < 0.005);
    await settled(page);
  };

  // Rendering here: its tile says how under its picture, with Cancel.
  if (local) {
    await post(`${at}/runs`, { names: ['slow'], on: ['local'] });
    await until('slow rendering, its lines under it', () => tile('slow'), (now) => now.lines?.length === 2 && /Step \d+ \/ 30/.test(now.lines[1]), FIRST_RUN_MS);
  } else seen.rendering = 'no local machine: no rendering tile';

  for (const want of [1, 0.5]) {
    await zoomTo(want);
    const tiles = {};
    for (const name of ['still', 'shot', 'plain', ...(local ? ['slow'] : [])]) {
      tiles[name] = await tile(name);
      await shot(`tile-${name}-zoom-${want}`, tiles[name].rect);
    }
    await shot(`tiles-zoom-${want}`);
    seen[`zoom_${want}`] = tiles;
    const z = `at_${want}`;
    const { still, shot: film, plain, slow } = tiles;
    // Its head: its name, its version and its ×, nothing else.
    check(`${z}_still_head`, JSON.stringify(still.head) === JSON.stringify(['nm:still', 'node-version-label:v1', 'iconb[Delete]']), still.head);
    check(`${z}_video_head`, JSON.stringify(film.head) === JSON.stringify(['nm:shot', 'node-version-label:v1', 'iconb[Delete]']), film.head);
    check(`${z}_plain_head`, JSON.stringify(plain.head) === JSON.stringify(['nm:plain', 'node-version-label:v1', 'iconb[Delete]']), plain.head);
    // Its picture: nothing to press on it at rest.
    check(`${z}_no_buttons_on_pictures`, [still, film, plain, slow].filter(Boolean).every((one) => one.pressable.length === 0), Object.fromEntries(Object.entries(tiles).map(([name, one]) => [name, one.pressable])));
    // Under it: its tool and shape; no chips, and nothing else.
    check(`${z}_still_tool_and_shape`, still.tool === named(image) && SHAPE.test(still.shape ?? ''), still);
    check(`${z}_video_tool_and_shape`, film.tool === named(video) && SHAPE.test(film.shape ?? ''), film);
    check(`${z}_plain_shape_only`, plain.tool === null && SHAPE.test(plain.shape ?? ''), plain);
    check(`${z}_nothing_else`, Object.values(tiles).every((one) => one.chips === 0 && one.stray === 0 && one.ordered), tiles);
    check(`${z}_idle_say_no_render`, [still, film, plain].every((one) => one.lines === null && !one.wave), tiles);
    if (slow) check(`${z}_rendering_lines_under_with_cancel`, slow.lines?.length === 2 && slow.cancel === 'Cancel' && slow.wave && slow.ordered && Boolean(slow.tool) && SHAPE.test(slow.shape ?? ''), slow);
  }
  check('no_edit_or_pick_up_at_rest', (await page.locator('.react-flow__node button:text-is("Edit"), .react-flow__node button:text-is("Pick up from here")').count()) === 0);

  // Cancel on the tile stops its render, and its lines go.
  if (local) {
    await zoomTo(1);
    await boxOf(page, 'slow').locator('.tile-under .face-cancel').click();
    const stopped = await until('slow stopped', async () => ({ asset: (await api(at)).body.assets.find((one) => one.name === 'slow'), lines: (await tile('slow')).lines }), (now) => !now.asset?.making?.length && now.lines === null, 60_000).catch((why) => ({ why: String(why) }));
    check('cancel_on_the_tile', !stopped.why && !stopped.asset?.versions, stopped);
  }

  // A render's whole life leaves a tile's size as it was (cl-936): a box with a take, rendered again, sampled from the ask to the landing.
  if (local) {
    const versions = async () => (await api(at)).body.assets.find((one) => one.name === 'again')?.versions ?? 0;
    await post(`${at}/runs`, { names: ['again'], on: ['local'] });
    await until('again has a take', versions, (n) => n === 1, FIRST_RUN_MS);
    await settled(page);
    const size = () => boxOf(page, 'again').evaluate((node) => {
      const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
      const [card, box] = [node.querySelector('.card').getBoundingClientRect(), node.getBoundingClientRect()];
      return [card.width, card.height, box.width, box.height].map((n) => Math.round((n / zoom) * 10) / 10).join('x');
    });
    const before = await size();
    await boxOf(page, 'again').evaluate((node) => {
      const sizes = (window.againSizes = []);
      const zoom = () => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
      const look = () => {
        const card = node.querySelector('.card');
        if (!card) return;
        const [c, b] = [card.getBoundingClientRect(), node.getBoundingClientRect()];
        const now = { size: [c.width, c.height, b.width, b.height].map((n) => Math.round((n / zoom()) * 10) / 10).join('x'), rendering: Boolean(card.querySelector('.tile-under .run-face')) };
        if (JSON.stringify(sizes.at(-1)) !== JSON.stringify(now)) sizes.push(now);
      };
      new MutationObserver(look).observe(node, { subtree: true, childList: true, attributes: true, characterData: true });
      new ResizeObserver(look).observe(node);
      look();
    });
    await post(`${at}/runs`, { names: ['again'], on: ['local'] });
    await until('again\'s second take', versions, (n) => n === 2, FIRST_RUN_MS);
    await settled(page);
    await page.waitForTimeout(1000);
    const sizes = await page.evaluate(() => window.againSizes);
    seen.render_sizes = { before, sizes };
    check('render_leaves_its_size', sizes.some((now) => now.rendering) && sizes.every((now) => now.size === before) && (await size()) === before, { before, sizes });
  }

  // Open, what it can do is still there.
  await openBox(page, 'still');
  check('open_still_edits', (await page.locator('.card.here[data-asset="still"] button[aria-label="Edit image"]').count()) === 1);
  await closeBox(page);
  await openBox(page, 'shot');
  check('open_video_picks_up', (await page.locator('.card.here[data-asset="shot"] button:text-is("Pick up from here")').count()) >= 1);
  await shot('open-shot');
  await closeBox(page);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
