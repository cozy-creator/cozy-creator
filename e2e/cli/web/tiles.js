// A box on the canvas at rest is what Paul drew (cl-932, 11-12), in a real browser on a real Calcifer:
//     Node-Name - version (like v1) X (delete this node)
//     (Image / thumbnail / preview)
//     Tool (if any) - aspect ratio
//     Run progress - cancel button
// An idle picture with Qwen's Image, an idle video with H3's Shot, a dropped picture with no tool, and a box rendering on this computer: their
// rows and nothing else (no Edit, no Pick up from here, no chips), the render's lines under its picture with Cancel, at zoom 1 and 0.5, each
// a screenshot. Labels at the page's body size and controls at least 28, on the tiles and in Focus, at 1440×900 and 1920×1080, light and
// dark, each a screenshot (cl-963). Two boxes stacked as close as side by side, nothing hanging on the lower, at 100% and 75%. Open, Edit
// is still there; an uploaded video offers no segment to redo. Cancel on the tile stops its render. A render's whole life, from Render to
// its take landing, leaves the tile's size as it was (cl-936).
//   node tiles.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so
// nothing renders. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectJson, calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, addBoxes, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, settled, until, upload } = require('./page');

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
    # As H3's last part decodes, the longest words a box says beside its count (cl-953).
    with tel.scope("Segment 2 of 2"), tel.stage("decode_video"):
        for frame in range(30):
            time.sleep(payload.pace)
            ctx.progress(frame, 30)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
`;

// A Shot as H3's is written, weightless: a summary that mentions pictures, made in steps into a 16:9 clip, as a blank video box is drawn.
const REEL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45", "msgspec>=0.19", "numpy>=2"]
# ///
"""The tiles cell's video tool: a summary, as MiniMax H3's Shot takes one, made in steps into a short grey clip."""

import time

import msgspec
import numpy as np
from cozy_runtime.author import App, Context, Outputs, Telemetry
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.media import VideoFile

app = App()


class Shot(msgspec.Struct, kw_only=True):
    summary: Prompt[Image] = Prompt()
    pace: float = 0.4


class Reel(msgspec.Struct):
    video: VideoFile


@app.tool(label="Shot", named="reel", accelerator=False)
def reel(ctx: Context, payload: Shot, out: Outputs, tel: Telemetry) -> Reel:
    with tel.stage("denoise"):
        for step in range(20):
            time.sleep(payload.pace)
            ctx.progress(step, 20)
    frames = np.full((24, 72, 128, 3), 40, dtype=np.uint8)
    for n in range(24):
        frames[n][:, n * 5 : n * 5 + 4] = 230
    return Reel(out.save_video(frames, fps=24))
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
const SHAPE = /^(\d+(\.\d+)?:\d+|Auto( · .+)?)$/;

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1700, height: 1300 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  // A tile's own shot, of what of it is on the screen.
  const shot = (name, clip) => {
    if (!process.env.SHOT) return Promise.resolve();
    const seen = clip && { x: Math.max(0, clip.x), y: Math.max(0, clip.y), width: Math.min(1700, clip.x + clip.width) - Math.max(0, clip.x), height: Math.min(1300, clip.y + clip.height) - Math.max(0, clip.y) };
    if (seen && (seen.width < 8 || seen.height < 8)) return Promise.resolve();
    return page.screenshot({ path: `${process.env.SHOT}-${name}.png`, ...(seen ? { clip: seen } : {}) });
  };

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json', ...(init.method === 'POST' ? { 'Idempotency-Key': `tiles-${Date.now()}-${Math.random()}` } : {}) } });
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
  const shotTool = join(project, `reel_${Date.now()}.py`);
  writeFileSync(shotTool, REEL);

  // A box this computer renders; a picture with Qwen's Image and a video with H3's Shot, each with a take; a picture with no tool.
  addBoxes(project, [
    { name: 'slow', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'A still of one colour.' } },
    { name: 'again', tool: `${tool}/draw`, kind: 'image', fields: { instructions: 'Another still, as warm as <plain>.', pace: 0.3 } },
    { name: 'reel', tool: `${shotTool}/reel`, kind: 'video', fields: { summary: 'The sea at dusk, the light of <plain>.' } },
  ]);
  const tools = [...((await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? []), ...((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [])];
  const image = tools.find((one) => one.package === 'qwen-image-2' && one.makes === 'image');
  const video = tools.find((one) => one.id === 'minimax-h3/generate') ?? tools.find((one) => one.package === 'minimax-h3' && one.makes === 'video');
  if (!image || !video) throw new Error(`the Hub lists no Qwen Image or H3 Shot: ${JSON.stringify(tools.map((one) => one.id))}`);
  await post(`${at}/assets`, { tool: image.id, kind: 'image', name: 'still', exact: true, fields: { instructions: 'a lighthouse at dusk' }, at: { x: 0, y: 0 } });
  await post(`${at}/assets/still/versions`, { source: await upload(held?.token, join(folder, 'take.png'), id) });
  await post(`${at}/assets`, { tool: video.id, kind: 'video', name: 'shot', exact: true, fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 5 }] }, at: { x: 380, y: 0 } });
  await post(`${at}/assets/shot/versions`, { source: await upload(held?.token, join(folder, 'take.mp4'), id) });
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'plain.png'), id), name: 'plain', exact: true, at: { x: 820, y: 0 } });
  /** Each of these drawn: fitted until they're all in view, as a canvas draws only what's in view, wherever they were placed. */
  const seeAll = (names) => until(`${names.join(', ')} on the canvas`, async () => {
    await fitCanvas(page);
    return Promise.all(names.map((name) => boxOf(page, name).isVisible()));
  }, (shown) => shown.every(Boolean));
  await seeAll(['still', 'shot', 'plain', 'slow', 'again', 'reel']);
  // The two this computer renders, in a row under the others.
  projectJson(project, 'PATCH', '/canvas', { cards: { slow: { x: 0, y: 560 }, again: { x: 380, y: 560 }, reel: { x: 760, y: 560 } } });
  await until('the rows placed', () => boxOf(page, 'again').evaluate((node) => node.getBoundingClientRect().top > document.querySelector('.react-flow__node:has(.card[data-asset="still"])').getBoundingClientRect().bottom), Boolean).catch(() => {});
  await page.mouse.move(2, 1298);
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
      // Its numbers whole at the box's width, whatever of its words gave way (cl-953).
      numbers: face ? ['.face-count', '.face-time'].map((selector) => {
        const held = face.querySelector(selector);
        if (!held) return null;
        const [r, line] = [held.getBoundingClientRect(), held.closest('p').getBoundingClientRect()];
        return { text: held.textContent.trim(), whole: r.width > 0 && r.right <= line.right + 0.5 && held.scrollWidth <= held.clientWidth + 1 };
      }) : null,
      // Under its picture: its tool and shape first, then its render.
      ordered: (!line || r(line).top >= m.bottom - 1) && (!face || r(face).top >= (line ? r(line).bottom : m.bottom) - 1),
      chips: node.querySelectorAll('.add-on, .add-ons').length,
      // How far what hangs under its picture reaches, from the picture's bottom.
      hangs: Math.max(m.bottom, ...[line, face].filter(Boolean).map((el) => r(el).bottom)) - m.bottom,
      // What its tool is called, as many times as the tile says it; and no Models & LoRAs on it.
      text: node.innerText,
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
    await until('slow rendering, its lines under it', () => tile('slow'), (now) => now.lines?.length === 2 && /Frame \d+ \/ 30/.test(now.lines[1]), FIRST_RUN_MS);
  } else seen.rendering = 'no local machine: no rendering tile';

  // How far a render's lines reach under a picture at 100%, seen on the box rendering here.
  let rendering = null;
  for (const want of [1, 0.5]) {
    await zoomTo(want);
    const tiles = {};
    for (const name of ['still', 'shot', 'plain', ...(local ? ['slow'] : [])]) {
      tiles[name] = await tile(name);
      await shot(`tile-${name}-zoom-${want}`, tiles[name].rect);
    }
    await shot(`tiles-zoom-${want}`);
    seen[`zoom_${want}`] = tiles;
    if (want === 1 && tiles.slow?.lines?.length === 2) rendering = tiles.slow.hangs;
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
    check(`${z}_nothing_else`, Object.values(tiles).every((one) => one.chips === 0 && one.stray === 0 && one.ordered && !/Models\s*&\s*LoRAs/i.test(one.text)), tiles);
    // Paul, 11-12: the tool once, beneath the picture, "MiniMax H3 · Shot".
    const times = (text, word) => text.split(word).length - 1;
    check(`${z}_tool_once_beneath`, times(still.text, named(image)) === 1 && times(film.text, named(video)) === 1 && times(still.text, image.package_title ?? named(image)) === 1 && times(film.text, video.package_title ?? named(video)) === 1, { still: still.text, shot: film.text });
    check(`${z}_idle_say_no_render`, [still, film, plain].every((one) => one.lines === null && !one.wave), tiles);
    if (slow) check(`${z}_count_and_time_whole`, /^Frame \d+ \/ 30$/.test(slow.numbers?.[0]?.text ?? '') && slow.numbers[0].whole && (!slow.numbers[1] || slow.numbers[1].whole), slow);
    if (slow) check(`${z}_rendering_dot_left_of_its_name`, /^state-dot\[(Rendering|Queued)\]$/.test(slow.head[0] ?? '') && slow.head[1] === 'nm:slow', slow.head);
    if (slow) check(`${z}_rendering_lines_under_with_cancel`, slow.lines?.length === 2 && slow.cancel === 'Cancel' && slow.wave && slow.ordered && Boolean(slow.tool) && (slow.shape === null || SHAPE.test(slow.shape)), slow);
  }
  check('no_edit_or_pick_up_at_rest', (await page.locator('.react-flow__node button:text-is("Edit"), .react-flow__node button:text-is("Pick up from here")').count()) === 0);

  // Cancel on the tile stops its render, and its lines go.
  if (local) {
    await zoomTo(1);
    await boxOf(page, 'slow').locator('.tile-under .face-cancel').evaluate((button) => button.click());
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
        // An image being made shows, in its frame, the pictures its run reads, full height, and the start of its words (cl-940).
        const making = card.querySelector('.media .making');
        const media = card.querySelector('.media').getBoundingClientRect();
        const pictures = [...(making?.querySelectorAll('.making-pictures img') ?? [])].filter((img) => img.complete && img.naturalWidth > 0);
        const now = {
          size: [c.width, c.height, b.width, b.height].map((n) => Math.round((n / zoom()) * 10) / 10).join('x'),
          rendering: Boolean(card.querySelector('.tile-under .run-face')),
          pictures: pictures.length,
          fullHeight: pictures.every((img) => Math.abs(img.getBoundingClientRect().height - media.height) < 1.5),
          words: making?.querySelector('.making-words')?.textContent ?? '',
          chip: making?.querySelector('.making-words .chip')?.textContent ?? '',
        };
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
    // While it's made: <plain>, full height, and its words with <plain> marked; once made, its take in their place (cl-940).
    check('making_shows_references_and_words', sizes.some((now) => now.rendering && now.pictures === 1 && now.fullHeight && /Another still, as warm as/.test(now.words) && /^<?plain>?$/i.test(now.chip)), sizes);
    const after = await boxOf(page, 'again').evaluate((node) => ({ making: node.querySelectorAll('.media .making').length, label: node.querySelector('.node-version-label')?.textContent ?? '' }));
    check('result_replaces_them', after.making === 0 && after.label === 'v2', after);
  }

  // A video being made shows its summary and the pictures it reads the same way, and its take replaces them; it never resizes (cl-947).
  if (local) {
    const reelOf = async () => (await api(at)).body.assets.find((one) => one.name === 'reel');
    const size = () => boxOf(page, 'reel').evaluate((node) => {
      const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
      const [card, box] = [node.querySelector('.card').getBoundingClientRect(), node.getBoundingClientRect()];
      return [card.width, card.height, box.width, box.height].map((n) => Math.round((n / zoom) * 10) / 10).join('x');
    });
    const before = await size();
    await boxOf(page, 'reel').evaluate((node) => {
      const seen = (window.reelSeen = []);
      const zoom = () => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
      const look = () => {
        const card = node.querySelector('.card');
        if (!card) return;
        const [c, b] = [card.getBoundingClientRect(), node.getBoundingClientRect()];
        const making = card.querySelector('.media .making');
        const media = card.querySelector('.media').getBoundingClientRect();
        const pictures = [...(making?.querySelectorAll('.making-pictures img') ?? [])].filter((img) => img.complete && img.naturalWidth > 0);
        const now = {
          size: [c.width, c.height, b.width, b.height].map((n) => Math.round((n / zoom()) * 10) / 10).join('x'),
          rendering: Boolean(card.querySelector('.tile-under .run-face')),
          pictures: pictures.length,
          fullHeight: pictures.every((img) => Math.abs(img.getBoundingClientRect().height - media.height) < 1.5),
          words: making?.querySelector('.making-words')?.textContent ?? '',
          chip: making?.querySelector('.making-words .chip')?.textContent ?? '',
        };
        if (JSON.stringify(seen.at(-1)) !== JSON.stringify(now)) seen.push(now);
      };
      new MutationObserver(look).observe(node, { subtree: true, childList: true, attributes: true, characterData: true });
      new ResizeObserver(look).observe(node);
      look();
    });
    await post(`${at}/runs`, { names: ['reel'], on: ['local'] });
    await until('reel has its take', reelOf, (row) => row?.versions === 1 && !row.making?.length, FIRST_RUN_MS);
    await settled(page);
    await page.waitForTimeout(1000);
    const samples = await page.evaluate(() => window.reelSeen);
    seen.reel_render = { before, samples };
    const after = await boxOf(page, 'reel').evaluate((node) => ({ making: node.querySelectorAll('.media .making').length, label: node.querySelector('.node-version-label')?.textContent ?? '', video: Boolean(node.querySelector('.media .media-pixels img, .media .media-pixels video')) }));
    check('video_making_shows_summary_and_references', samples.some((now) => now.rendering && now.pictures === 1 && now.fullHeight && /The sea at dusk, the light of/.test(now.words) && /^<?plain>?$/i.test(now.chip)), samples);
    check('video_take_replaces_them', after.making === 0 && after.label === 'v1' && after.video, after);
    check('video_render_leaves_its_size', samples.every((now) => now.size === before) && (await size()) === before, { before, sizes: [...new Set(samples.map((now) => now.size))] });
  }

  // Open, what it can do is still there.
  await openBox(page, 'still');
  check('open_still_edits', (await page.locator('.card.here[data-asset="still"] button[aria-label="Edit image"]').count()) === 1);
  await closeBox(page);
  await openBox(page, 'shot');
  // Its take was uploaded, so no run says its segments and none is offered to redo (cl-951; web-scrub's rendered take is).
  check('open_video_offers_no_redo_without_its_seams', (await page.locator('.card.here[data-asset="shot"] .redo-segment').count()) === 0);
  await shot('open-shot');
  await closeBox(page);

  // Stacked as close as side by side (Paul, 11-12: "allow for tiles to get a little closer on the top and bottoms"): a picture dragged up
  // under a tool's box lands clear's 16 under what it keeps, and what hangs under the upper one, a render's lines too (cl-936), never lies on
  // the lower one. At 100% and 75%, each a screenshot.
  await post(`${at}/assets`, { tool: image.id, kind: 'image', name: 'upper', exact: true, fields: { instructions: 'a lighthouse at dawn' }, at: { x: 0, y: 2400 } });
  await post(`${at}/assets/upper/versions`, { source: await upload(held?.token, join(folder, 'take.png'), id) });
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'plain.png'), id), name: 'lower', exact: true, at: { x: 520, y: 2400 } });
  projectJson(project, 'PATCH', '/canvas', { cards: { upper: { x: 0, y: 2400 }, lower: { x: 520, y: 2400 } } });
  await seeAll(['upper', 'lower']);
  await boxOf(page, 'upper').locator('.media').first().click();
  if (!(await page.locator('.zoom-menu').count())) await page.click('.zoom-at');
  await page.getByRole('menuitem', { name: /^Zoom to selection/ }).click();
  await settled(page);
  await zoomTo(1);
  const stack = () => page.evaluate(() => {
    const parts = (name) => {
      const card = document.querySelector(`.react-flow__node .card[data-asset="${name}"]`);
      const r = (el) => el.getBoundingClientRect();
      const media = r(card.querySelector(':scope > .media'));
      const under = [...card.querySelectorAll(':scope > .tile-under > *')].map(r).filter((b) => b.height > 0);
      const head = [...card.querySelectorAll(':scope > .head > *')].map(r).filter((b) => b.height > 0);
      return { left: media.left, right: media.right, top: media.top, bottom: media.bottom, under: Math.max(media.bottom, ...under.map((b) => b.bottom)), head: Math.min(media.top, ...head.map((b) => b.top)) };
    };
    return { upper: parts('upper'), lower: parts('lower'), zoom: new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a };
  });
  const apart = await stack();
  const grab = { x: (apart.lower.left + apart.lower.right) / 2, y: (apart.lower.top + apart.lower.bottom) / 2 };
  const to = { x: grab.x + apart.upper.left - apart.lower.left, y: grab.y + apart.upper.bottom + 8 - apart.lower.top };
  await page.mouse.move(grab.x, grab.y);
  await page.mouse.down();
  await page.mouse.move(to.x, to.y, { steps: 16 });
  await page.mouse.up();
  await settled(page);
  await page.waitForTimeout(500);
  const near = await stack();
  const [above, below] = near.upper.top <= near.lower.top ? [near.upper, near.lower] : [near.lower, near.upper];
  const at100 = { room: (below.head - above.bottom) / near.zoom, clear: (below.head - above.under) / near.zoom, above: near.upper.top <= near.lower.top ? 'upper' : 'lower', rendering };
  seen.stacked = { near, ...at100 };
  check('stacked_one_under_the_other', above.left < below.right && below.left < above.right && below.top > above.bottom, near);
  check('stacked_nothing_under_it_lies_on_the_box_below', at100.clear >= 0, at100);
  check('stacked_closer_than_before', at100.room < 180, at100);
  if (rendering !== null && at100.above === 'upper') check('stacked_a_render_under_it_fits', at100.room - rendering >= 0, at100);
  await shot('stacked-zoom-1');
  await zoomTo(0.75);
  const at75 = await stack();
  seen.stacked_75 = at75;
  await shot('stacked-zoom-0.75');

  // As a person sees it (cl-963, Paul 11-12: "things were just far too small or hard to see or read"), at 1440×900 and 1920×1080, light and
  // dark: the tiles at 100%, and a picture with two takes in Focus. Every label at least the page's body size; every control at least 28.
  await post(`${at}/assets/still/versions`, { source: await upload(held?.token, join(folder, 'plain.png'), id) });
  const sizes = () => page.evaluate(() => {
    const px = (el) => (el ? parseFloat(getComputedStyle(el).fontSize) : null);
    const box = (el) => { if (!el) return null; const r = el.getBoundingClientRect(); return { w: Math.round(r.width), h: Math.round(r.height) }; };
    const labels = [...document.querySelectorAll('.react-flow .card .tile-line .ratio-rest, .react-flow .card .tile-line .node-tool, .react-flow .card .head .node-version-label')];
    const view = document.querySelector('.focus-viewport');
    return {
      body: px(document.body),
      labels: labels.map((el) => ({ text: el.textContent, px: px(el) })),
      trash: box(document.querySelector('.react-flow .card[data-asset="still"] .head button[aria-label="Delete"]')),
      close: box(view?.querySelector('button[aria-label="Close focus"]')),
      discard: box(view?.querySelector('.take-off[aria-label^="Discard"]')),
      untool: box(view?.querySelector('.image-tool button[aria-label$=" tool"]')),
      // Its tool's words start past a selected box's ring, never under it.
      ring: (() => {
        const card = document.querySelector('.react-flow__node.selected .card[data-asset="still"]');
        const line = card?.querySelector(':scope > .tile-under .tile-line');
        return line ? line.getBoundingClientRect().top - card.querySelector(':scope > .media').getBoundingClientRect().bottom : null;
      })(),
      number: px(view?.querySelector('.variant-number')),
    };
  });
  for (const [width, height] of [[1440, 900], [1920, 1080]]) for (const dark of [false, true]) {
    const look = `${width}x${height}-${dark ? 'dark' : 'light'}`;
    await page.setViewportSize({ width, height });
    await page.evaluate((dark) => localStorage.setItem('cozy.theme', dark ? 'dark' : 'paper'), dark);
    await page.reload();
    await seeAll(['still']);
    await boxOf(page, 'still').locator('.media').first().click();
    if (!(await page.locator('.zoom-menu').count())) await page.click('.zoom-at');
    await page.getByRole('menuitem', { name: /^Zoom to selection/ }).click();
    await settled(page);
    await zoomTo(1);
    const tiled = await sizes();
    await shot(`look-${look}-tiles`);
    await openBox(page, 'still');
    await page.locator('.focus-viewport .take-off[aria-label^="Discard"]').first().waitFor();
    await page.waitForTimeout(400);
    const focused = await sizes();
    await shot(`look-${look}-focus`);
    await closeBox(page);
    seen[`look_${look}`] = { tiled, focused };
    check(`look_${look}_tile_labels_at_body_size`, tiled.labels.length >= 3 && tiled.labels.every((one) => one.px >= tiled.body - 0.01), tiled);
    check(`look_${look}_tile_trash_28`, tiled.trash?.w >= 28 && tiled.trash?.h >= 28, tiled.trash);
    check(`look_${look}_tool_words_clear_of_the_ring`, tiled.ring >= 9, tiled.ring);
    check(`look_${look}_focus_close_and_discard_28`, focused.close?.w >= 28 && focused.close?.h >= 28 && focused.discard?.w >= 28 && focused.discard?.h >= 28 && focused.untool?.w >= 28 && focused.untool?.h >= 28, focused);
    check(`look_${look}_focus_take_number_at_body_size`, focused.number >= focused.body - 0.01, focused);
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
