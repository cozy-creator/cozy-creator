// Every view of the page as a person sees it (cl-958, Paul 11-12: "you should test your own UI and look at your own UI more often"), kept as
// pictures to be looked at: at 1440×900 and 1920×1080, on paper and dark, on a project of weightless local tools. The home page; the canvas
// fitted, at 100% and far out, with boxes at rest, queued, rendering, with a take not looked at and failed, each also cropped; the machine's
// flyout and drawer; the drawers; the menus; the toasts; Focus on a picture, an H3 Shot, an Edit, an empty box and one rendering, each on its
// take and writing a new one; a long takes column before and after a mouse drag up it; Models & LoRAs; the logo. It judges nothing: a view it couldn't reach is listed under `missed`.
//   node look.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its pictures; $ALONE: on a shared computer, so
// nothing renders. Prints what it took as JSON; exits 1 only if it took nothing.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, BARS, DARK_OPEN, MP4, STEP_MS, boxOf, calcifer, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, openCanvas, png, projectFromPage, settled, signIn, until, upload } = require('./page');

const FIRST_RUN_MS = 300_000;
const SIZES = [[1440, 900], [1920, 1080]];
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The look cell's picture tool: a weightless still of one colour, made in steps; asked to, it fails."""

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
    pace: float = 0.2
    steps: int = 6
    fail: bool = False


@app.tool(label="Still", named="still", accelerator=False)
def draw(ctx: Context, payload: Drawing, adj: Adjustments, out: Outputs, tel: Telemetry) -> ImageResult:
    width, height = ImageRequest(payload.instructions.text, None, None, payload.seed).size(adj, None, SIZES, megapixels=0.25)
    with tel.stage("denoise"):
        for step in range(payload.steps):
            time.sleep(payload.pace)
            ctx.progress(step, payload.steps)
    if payload.fail:
        raise RuntimeError("asked to fail, as the look cell wants: a long error line, so a person sees how one reads when it wraps")
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
`;

const seen = { looks: 0, shots: [], missed: {} };
let during = 'the start';
noStall(seen, died);
async function died(why) {
  seen.missed[during] = String(why?.stack ?? why).slice(0, 800);
  await lastLook();
  console.log(JSON.stringify({ failures: seen.shots.length ? [] : ['took nothing'], seen }, null, 2));
  process.exit(seen.shots.length ? 0 : 1);
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright, BARS);
  const pages = {};
  for (const theme of ['paper', 'dark']) {
    const context = await browser.newContext({ viewport: { width: SIZES[0][0], height: SIZES[0][1] } });
    await context.addInitScript((dark) => { try { localStorage.setItem('cozy.theme', dark ? 'dark' : 'paper'); } catch {} }, theme === 'dark');
    const page = await context.newPage();
    page.setDefaultTimeout(STEP_MS);
    page.on('pageerror', (error) => (seen.page_errors ??= []).push(`${theme}: ${error.message}`));
    await signIn(page);
    await page.waitForSelector('.top');
    pages[theme] = page;
  }
  lookAt(pages.paper);
  const both = async (what) => { for (const [theme, page] of Object.entries(pages)) await what(page, theme); };
  const out = (name) => `${process.env.SHOT}-${name}.png`;

  /** `name` on both pages at both sizes: `open` once a page, `prepare` after each resize. What can't be reached is said under `missed`. */
  async function shoot(name, { open, prepare, full = false } = {}) {
    during = name;
    for (const [theme, page] of Object.entries(pages)) {
      try {
        if (open) await open(page, theme);
      } catch (why) {
        seen.missed[`${name} ${theme}`] = String(why.message ?? why).slice(0, 400);
        continue;
      }
      for (const [width, height] of SIZES) {
        const label = `${name}-${width}x${height}-${theme}`;
        try {
          await page.setViewportSize({ width, height });
          await page.waitForTimeout(200);
          if (prepare) await prepare(page, theme);
          await settled(page);
          await page.waitForTimeout(150);
          if (process.env.SHOT) await page.screenshot({ path: out(`look-${label}`), fullPage: full });
          seen.shots.push(label);
          seen.looks++;
        } catch (why) {
          seen.missed[label] = String(why.message ?? why).slice(0, 400);
        }
      }
    }
  }
  /** A close look at `locator` on each page, at the first size, `pad` around it. */
  async function crop(name, locate, { pad = 12, open } = {}) {
    during = name;
    for (const [theme, page] of Object.entries(pages)) {
      try {
        await page.setViewportSize({ width: SIZES[0][0], height: SIZES[0][1] });
        if (open) await open(page, theme);
        await settled(page);
        const box = await locate(page).evaluate((el, pad) => {
          // A box's lines under it hang outside its node.
          const rects = [el, ...el.querySelectorAll('.tile-under')].map((one) => one.getBoundingClientRect());
          const r = { left: Math.min(...rects.map((one) => one.left)), top: Math.min(...rects.map((one) => one.top)), right: Math.max(...rects.map((one) => one.right)), bottom: Math.max(...rects.map((one) => one.bottom)) };
          return { x: Math.max(0, r.left - pad), y: Math.max(0, r.top - pad), right: Math.min(innerWidth, r.right + pad), bottom: Math.min(innerHeight, r.bottom + pad) };
        }, pad);
        const clip = { x: box.x, y: box.y, width: box.right - box.x, height: box.bottom - box.y };
        if (clip.width < 4 || clip.height < 4) throw new Error(`off screen: ${JSON.stringify(clip)}`);
        if (process.env.SHOT) await page.screenshot({ path: out(`look-crop-${name}-${theme}`), clip });
        seen.shots.push(`crop-${name}-${theme}`);
        seen.looks++;
      } catch (why) {
        seen.missed[`crop-${name}-${theme}`] = String(why.message ?? why).slice(0, 400);
      }
    }
  }
  const tryStep = async (name, what) => {
    during = name;
    try {
      return await what();
    } catch (why) {
      seen.missed[name] = String(why.stack ?? why).slice(0, 600);
      return undefined;
    }
  };
  const goto = async (page, path) => {
    await page.goto(base + path);
    await page.waitForSelector('.top');
    await page.waitForTimeout(500);
  };
  const leave = async (page) => {
    for (let i = 0; i < 4 && (await page.locator('.card.here, [role=menu], .activity, .main-menu[open], [role=dialog]').count()); i++) {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);
    }
    await closeBox(page).catch(() => {});
  };

  // The home page, before any project; the projects drawer.
  await shoot('home', { open: (page) => goto(page, '/') });
  await shoot('drawer-projects', { open: (page) => goto(page, '/projects') });

  const paper = pages.paper;
  const held = { token: holderToken() };
  const api = (path, init = {}) => paper.evaluate(async ({ path, init }) => {
    const answer = await fetch(path, { ...init, headers: { 'cozy-project-storage': '3', 'Content-Type': 'application/json', ...(init.method === 'POST' ? { 'Idempotency-Key': `look-${Date.now()}-${Math.random()}` } : {}) } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init });
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  during = 'a project';
  await goto(paper, '/projects');
  await paper.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-look-${Date.now()}`));
  await paper.getByLabel('Create without encryption', { exact: true }).check();
  await paper.click('form button[type=submit]');
  await paper.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const projectPath = new URL(paper.url()).pathname;
  const { id, path: project } = await projectFromPage(paper);
  seen.project = { id, folder: project };
  const at = `/v1/projects/${id}`;
  const assetOf = async (name) => (await api(at)).body.assets.find((one) => one.name === name);
  const local = (await api('/v1/status')).body?.local_usable && !process.env.ALONE;
  seen.local = Boolean(local);

  // What's on it: pictures and clips, a picture with Qwen's Image and a take, an H3 Shot with one, empty ones of each, an Edit of two clips, and
  // boxes this computer renders: one slow, one after it, one approved with a new take, one that fails.
  during = 'the boxes';
  const folder = mkdtempSync(join(tmpdir(), 'web-look-'));
  writeFileSync(join(folder, 'amy.png'), png(30, 640, 400));
  writeFileSync(join(folder, 'boat.png'), png(190, 640, 400));
  writeFileSync(join(folder, 'take.png'), png(110, 640, 400));
  writeFileSync(join(folder, 'clip_a.mp4'), MP4);
  writeFileSync(join(folder, 'clip_b.mp4'), DARK_OPEN);
  const tool = join(project, `look_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  const tools = [...((await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? []), ...((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [])];
  const imageTool = tools.find((one) => one.package === 'qwen-image-2' && one.makes === 'image' && one.fields.some((field) => field.type === 'prompt'));
  const shotTool = tools.find((one) => one.id === 'minimax-h3/generate') ?? tools.find((one) => one.package === 'minimax-h3' && one.makes === 'video');
  seen.tools = { image: imageTool?.id ?? null, shot: shotTool?.id ?? null };
  const imports = [['amy', 'amy.png', 1300, 0], ['boat', 'boat.png', 1700, 0], ['clip_a', 'clip_a.mp4', 1300, 420], ['clip_b', 'clip_b.mp4', 1700, 420]];
  for (const [name, file, x, y] of imports) await post(`${at}/imports`, { source: await upload(held?.token, join(folder, file), id), name, exact: true, at: { x, y } });
  const boxes = [];
  if (imageTool) {
    await post(`${at}/assets`, { tool: imageTool.id, kind: 'image', name: 'still', exact: true, fields: { instructions: '<amy> in a red coat on the pier at dusk, the lighthouse behind her.' }, at: { x: 0, y: 0 } });
    await post(`${at}/assets/still/versions`, { source: await upload(held?.token, join(folder, 'take.png'), id) });
    await post(`${at}/assets`, { tool: imageTool.id, kind: 'image', name: 'empty_image', exact: true, fields: { instructions: '' }, at: { x: 0, y: 1100 } });
    boxes.push('still', 'empty_image');
  }
  if (shotTool) {
    const beats = [{ description: 'Waves roll in under a low sun.', duration: 5 }, { description: '<boat> sails past the lighthouse.', duration: 5 }];
    await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'shot', exact: true, fields: { summary: '<amy> watches the harbour at dusk as the boats come in.', beats }, at: { x: 440, y: 0 } });
    await post(`${at}/assets/shot/versions`, { source: await upload(held?.token, join(folder, 'clip_b.mp4'), id) });
    await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'empty_shot', exact: true, fields: { summary: '', beats: [{ description: '', duration: 5 }] }, at: { x: 440, y: 1100 } });
    boxes.push('shot', 'empty_shot');
  }
  await post(`${at}/assets`, { tool: 'cozy/edit', kind: 'video', name: 'edit', exact: true, fields: { timeline: { fps: 24, tracks: [{ clips: [{ clip: '<clip_a>' }, { clip: '<clip_b>' }] }] } }, at: { x: 900, y: 1100 } });
  boxes.push('edit');
  const mine = [
    ['slow', { instructions: 'A still of <amy> by the sea, made slowly so the box is seen rendering: its words over the picture it reads.', pace: 4, steps: 90 }, 0, 540],
    ['waits', { instructions: 'Made after slow, so it waits its turn.', pace: 4, steps: 90 }, 440, 540],
    ['fresh', { instructions: 'A quick still, approved, then made again.' }, 880, 540],
    ['broken', { instructions: 'This one fails.', fail: true }, 1300, 840],
  ];
  for (const [name, fields, x, y] of mine) await post(`${at}/assets`, { tool: `${tool}/draw`, kind: 'image', name, exact: true, fields, at: { x, y } });
  boxes.push(...mine.map(([name]) => name), ...imports.map(([name]) => name));
  // A picture with sixteen takes, so its column in Focus is longer than the window.
  writeFileSync(join(folder, 'many_0.png'), png(40, 640, 400));
  await post(`${at}/imports`, { source: await upload(held?.token, join(folder, 'many_0.png'), id), name: 'many', exact: true, at: { x: 2100, y: 0 } });
  for (let take = 1; take < 16; take++) {
    writeFileSync(join(folder, `many_${take}.png`), png(40 + take * 12, 640, 400));
    await post(`${at}/assets/many/versions`, { source: await upload(held?.token, join(folder, `many_${take}.png`), id) });
  }
  // Off the view a box isn't drawn (culled): fitted, every one is.
  await until('every box in the project', async () => (await api(at)).body.assets.map((one) => one.name), (names) => boxes.every((name) => names.includes(name)));
  await both(async (page) => {
    if (new URL(page.url()).pathname !== projectPath) await goto(page, projectPath);
    await until(`every box drawn (${page === paper ? 'paper' : 'dark'})`, async () => {
      await fitCanvas(page);
      return page.evaluate((names) => names.filter((name) => !document.querySelector(`.react-flow__node .card[data-asset="${name}"]`)), boxes);
    }, (missing) => missing.length === 0, 30_000);
  });

  // Renders on this computer: fresh's take approved, then another (not looked at); broken failing; slow rendering and waits after it.
  const versions = async (name) => (await assetOf(name))?.versions ?? 0;
  const toastShot = async (name, text) => {
    during = name;
    for (const [theme, page] of Object.entries(pages)) {
      const note = page.locator('.notes .note', { hasText: text }).first();
      if (!(await note.waitFor({ timeout: 15_000 }).then(() => true, () => false))) {
        seen.missed[`${name}-${theme}`] = `no toast "${text}": ${JSON.stringify(await page.locator('.notes .note').allInnerTexts())}`;
        continue;
      }
      if (process.env.SHOT) await page.screenshot({ path: out(`look-${name}-${SIZES[0][0]}x${SIZES[0][1]}-${theme}`) });
      const box = await page.locator('.notes').boundingBox();
      if (box && process.env.SHOT) await page.screenshot({ path: out(`look-crop-${name}-${theme}`), clip: { x: Math.max(0, box.x - 12), y: Math.max(0, box.y - 12), width: box.width + 24, height: box.height + 24 } });
      seen.shots.push(`${name}-${theme}`);
      seen.looks++;
    }
  };
  if (local) {
    await tryStep('fresh v1', async () => {
      await post(`${at}/runs`, { names: ['fresh'], on: ['local'] });
      await until('fresh v1', () => versions('fresh'), (n) => n === 1, FIRST_RUN_MS);
      const first = (await assetOf('fresh')).version;
      await post(`${at}/approve`, { name: 'fresh', version: String(first) });
      await until('fresh approved', () => assetOf('fresh'), (row) => row.pinned && row.approved === first);
      await post(`${at}/runs`, { names: ['fresh'], on: ['local'] });
      await until('fresh\'s next take unseen', () => assetOf('fresh'), (row) => row.versions === 2 && row.unseen?.length > 0, FIRST_RUN_MS);
    });
    await toastShot('toast-finished', 'fresh finished');
    await tryStep('broken fails', async () => {
      await post(`${at}/runs`, { names: ['broken'], on: ['local'] });
      await until('broken failed', () => assetOf('broken'), (row) => row?.failure, FIRST_RUN_MS);
    });
    await toastShot('toast-failed', "broken didn't render");
    await tryStep('slow rendering', async () => {
      await post(`${at}/runs`, { names: ['slow'], on: ['local'] });
      await post(`${at}/runs`, { names: ['waits'], on: ['local'] });
      await until('slow saying how far it is', () => boxOf(paper, 'slow').evaluate((node) => node.querySelectorAll('.tile-under .run-face p').length), (n) => n >= 2, FIRST_RUN_MS);
      await paper.waitForTimeout(6000);
    });
    seen.states = Object.fromEntries(await Promise.all(['slow', 'waits', 'fresh', 'broken'].map(async (name) => [name, await boxOf(paper, name).locator('.state-dot').getAttribute('aria-label').catch(() => null)])));
  }

  // The canvas: fitted (as it opens), at 100%, far out.
  const zoomMenu = async (page, item) => {
    if (!(await page.locator('.zoom-menu').count())) await page.click('.zoom-at');
    await page.getByRole('menuitem', { name: item }).click();
    await settled(page);
  };
  const zoomOf = (page) => page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
  await shoot('canvas-fit', { open: (page) => leave(page), prepare: (page) => fitCanvas(page) });
  await shoot('canvas-100', { prepare: async (page) => { await fitCanvas(page); await zoomMenu(page, /^Zoom to 100%/); } });
  // Past the far band (below 20%): a box is its picture and its name.
  await shoot('canvas-farthest', {
    prepare: async (page) => {
      await fitCanvas(page);
      for (let i = 0; i < 16 && (await zoomOf(page)) > 0.16; i++) {
        await page.click('button[aria-label="Zoom out"]');
        await settled(page);
      }
    },
  });
  await shoot('canvas-far', {
    prepare: async (page) => {
      await fitCanvas(page);
      for (let i = 0; i < 12 && (await zoomOf(page)) > 0.26; i++) {
        await page.click('button[aria-label="Zoom out"]');
        await settled(page);
      }
    },
  });

  // Each box close, at 100%: selected, then at rest.
  const tileOf = (page, name) => boxOf(page, name);
  for (const name of boxes) {
    const centre = async (page) => {
      await leave(page);
      await fitCanvas(page);
      const media = page.locator(`.react-flow__node .card[data-asset="${name}"] .media`).first();
      const { x, y, width, height } = await media.boundingBox();
      await page.mouse.click(x + width / 2, y + height / 2);
      await page.keyboard.press('Shift+2');
      await settled(page);
      await page.keyboard.press('Shift+0');
      await settled(page);
    };
    await crop(`tile-${name}-selected`, (page) => tileOf(page, name), { pad: 36, open: centre });
    await crop(`tile-${name}`, (page) => tileOf(page, name), {
      pad: 36,
      open: async (page) => {
        const { x, y } = await openCanvas(page);
        await page.mouse.click(x, y);
        await page.mouse.move(x, y);
        await page.waitForTimeout(300);
      },
    });
  }
  await both((page) => leave(page).then(() => fitCanvas(page)));

  // Menus: the main one, the zoom one, a box's own.
  await shoot('menu-main', { open: async (page) => { await leave(page); await page.click('summary[aria-label="Menu"]'); await page.locator('.main-menu nav').waitFor(); } });
  await both(leave);
  await shoot('menu-zoom', { open: async (page) => { await leave(page); await page.click('.zoom-at'); await page.locator('.zoom-menu').waitFor(); } });
  await both(leave);
  await shoot('menu-box', {
    open: async (page) => {
      await leave(page);
      await fitCanvas(page);
      const media = page.locator('.react-flow__node .card[data-asset="still"] .media, .react-flow__node .card[data-asset="slow"] .media').first();
      const { x, y, width, height } = await media.boundingBox();
      await page.mouse.click(x + width / 2, y + height / 2, { button: 'right' });
      await page.locator('[role=menu]').first().waitFor();
    },
  });
  await both(leave);

  // The machine's flyout, while it renders, and its drawer.
  await shoot('machine-flyout', {
    open: async (page) => {
      await leave(page);
      await page.click('.mtab[data-machine="local"], .mtab');
      await page.locator('.activity').waitFor();
    },
  });
  await crop('machine-flyout', (page) => page.locator('.activity'), { pad: 16 });
  await both(leave);
  await shoot('drawer-machine', { open: (page) => goto(page, `${projectPath}/machines/local`) });
  await shoot('drawer-machines', { open: (page) => goto(page, `${projectPath}/machines`) });
  await shoot('drawer-runs', { open: (page) => goto(page, `${projectPath}/runs`) });
  const slowRun = await tryStep('the slow run', async () => (await assetOf('slow'))?.making?.[0]);
  if (slowRun) await shoot('drawer-run', { open: (page) => goto(page, `${projectPath}/runs/${slowRun}`) });
  const failedRun = await tryStep('the failed run', async () => ((await api(`/v1/projects/${id}/runs?limit=50`)).body?.runs ?? []).find((run) => run.project_id === id && run.status === 'failed')?.number);
  if (failedRun) await shoot('drawer-run-failed', { open: (page) => goto(page, `${projectPath}/runs/${failedRun}`) });
  await shoot('drawer-files', { open: (page) => goto(page, `${projectPath}/files`) });
  await shoot('drawer-account', { open: (page) => goto(page, `${projectPath}/account`) });
  await shoot('drawer-projects-open', { open: (page) => goto(page, `${projectPath}/projects`) });
  // The open project's own tab: Projects, ready for a new one's name (cl-958: the bar's one + is Add files).
  await shoot('drawer-projects-from-tab', {
    open: async (page) => {
      await goto(page, projectPath);
      await page.click('nav.ptabs a.ptab.on');
      await page.locator('.drawer input[aria-label="New project\'s folder"]').waitFor();
    },
  });
  await shoot('drawer-library', { open: (page) => goto(page, `${projectPath}/packages`) });
  await shoot('drawer-models', { open: (page) => goto(page, `${projectPath}/models`) });
  await both((page) => goto(page, projectPath));

  // Focus: a box rendering, watched as its take.
  // Rendered again just before, so it's being made while looked at.
  if (local) {
    await tryStep('slow again', async () => {
      await post(`${at}/cancel`, { names: ['waits'] }).catch(() => {});
      await post(`${at}/runs`, { names: ['slow'], on: ['local'] });
      await until('slow making again', () => assetOf('slow'), (row) => row?.making?.length > 0, FIRST_RUN_MS);
      await paper.waitForTimeout(8000);
    });
    await shoot('focus-rendering', { open: (page) => openBox(page, 'slow') });
  }
  await both(leave);
  // Done with what's being made.
  await tryStep('cancel', () => post(`${at}/cancel`, { names: ['slow', 'waits'] }));

  // Focus on a picture: its take, and writing its next.
  const newTake = async (page, name) => {
    const here = page.locator(`.card.here[data-asset="${name}"]`).first();
    const next = here.locator(':scope > .focus-actions .focus-new');
    if (await next.count()) await next.click();
    else {
      const edit = here.getByRole('button', { name: 'Edit image', exact: true });
      if (await edit.count()) await edit.click();
    }
    await page.waitForTimeout(500);
  };
  if (imageTool) {
    await shoot('focus-image', { open: (page) => openBox(page, 'still') });
    await shoot('focus-image-new', { open: (page) => newTake(page, 'still') });
    await shoot('picker-models-image', {
      open: async (page) => {
        await page.locator('.card.here button[aria-label="Models and LoRAs"]').first().click();
        await page.getByRole('dialog', { name: 'Models and LoRAs', exact: true }).waitFor();
      },
    });
    await both(leave);
    await shoot('focus-image-empty', { open: (page) => openBox(page, 'empty_image') });
    await both(leave);
  }
  // An H3 Shot: its take (the player, its scrubber, Pick up), its next (Summary and Beats in the frame, what they mention behind), Beat 2
  // open, Models & LoRAs; and an empty one.
  if (shotTool) {
    await shoot('focus-shot', { open: (page) => openBox(page, 'shot') });
    await crop('focus-shot-player', (page) => page.locator('.card.here .media').first(), { pad: 60 });
    await shoot('focus-shot-new', { open: (page) => newTake(page, 'shot') });
    await shoot('focus-shot-beat2', {
      open: async (page) => {
        await page.locator('.card.here .beat-toggle', { hasText: 'Beat 2' }).first().click();
        await page.waitForTimeout(400);
      },
    });
    await shoot('picker-models-shot', {
      open: async (page) => {
        await page.locator('.card.here button[aria-label="Models and LoRAs"]').first().click();
        await page.getByRole('dialog', { name: 'Models and LoRAs', exact: true }).waitFor();
      },
    });
    await both(leave);
    await shoot('focus-shot-empty', { open: (page) => openBox(page, 'empty_shot') });
    await both(leave);
  }
  // The Edit: its clips on a track.
  await shoot('focus-edit', { open: (page) => openBox(page, 'edit') });
  await both(leave);
  // Takes: one approved with a new one beside it; a failed box.
  if (local) {
    await shoot('focus-takes', { open: (page) => openBox(page, 'fresh') });
    await both(leave);
    await shoot('focus-failed', { open: (page) => openBox(page, 'broken') });
    await both(leave);
  }
  // A dropped picture, no tool.
  await shoot('focus-picture', { open: (page) => openBox(page, 'amy') });
  await both(leave);
  // A long takes column in Focus, at its top, then after a mouse drag up it from its bottom: what the drag did is what's shown (cl-885).
  const column = (page) => page.locator('.card.here[data-asset="many"] .image-history.here-side').last();
  const toTop = async (page) => {
    if (!(await column(page).count())) await openBox(page, 'many');
    await column(page).evaluate((element) => void (element.scrollTop = 0));
  };
  await shoot('focus-long-takes', { open: (page) => openBox(page, 'many'), prepare: toTop });
  await shoot('focus-long-takes-dragged', {
    prepare: async (page, theme) => {
      await toTop(page);
      await settled(page);
      const side = await column(page).boundingBox();
      const across = side.x + side.width / 2;
      await page.mouse.move(across, side.y + side.height - 12);
      await page.mouse.down();
      for (let step = 1; step <= 8; step++) await page.mouse.move(across, side.y + side.height - 12 - step * 18);
      await page.mouse.up();
      await page.waitForTimeout(500);
      const size = page.viewportSize();
      (seen.long_takes ??= {})[`${size.width}x${size.height}-${theme}`] = {
        in_focus: await page.locator('.focus-viewport').count() > 0,
        scrolled: await column(page).evaluate((element) => element.scrollTop).catch(() => null),
      };
    },
  });
  await both(leave);

  // The logo, and the top bar.
  await crop('logo', (page) => page.locator('.top .brand'), { pad: 8 });
  await crop('topbar', (page) => page.locator('.top'), { pad: 0 });

  await browser.close();
  console.log(JSON.stringify({ failures: seen.shots.length ? [] : ['took nothing'], seen }, null, 2));
  process.exit(seen.shots.length ? 0 : 1);
})().catch(died);
