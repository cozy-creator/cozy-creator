// spacing (cl-1181), Paul 11-17: "I think also on the webui we can allow boxes to get closer to each other prior to pushing each other
// away, somewhat. It looks like a lot of vertical space above or below a box is being reserved, perhaps for tools, perhaps more than is
// needed." Four pairs, each lower box dragged up under its upper one until it pushes it: two pictures; words over a picture; a box with a
// tool (H3's Shot, a take) over one with a tool (Qwen Image 2's, a picture); a box rendering, its lines and Cancel, over a picture. The
// lower stays where it's let go, the upper goes straight up, and a render then moves nothing. Looked at 45% (where they come closest: the
// lines under a box still drawn, the name at its largest), 50%, 100% and 200%, at 1440×900 and 1920×1080: nothing of one lies on anything
// of another; at 45% what hangs under the upper and the head of the lower are 4 to 16 canvas px apart (words: 4 to 12), and at 100% 40 or
// less (before: 64 and 16 at 45%, 80 at 100%); a tool's box keeps the room its render draws, as the one rendering does. Screenshots and numbers are kept.
const { randomUUID } = require('crypto');
const { mkdirSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasCard, canvasIdle, canvasNames, fixtureTool, openPoint, MP4, png } = require('./fixtures');
const { camera, drag, rows, sameCamera } = require('./geometry');

const SIZES = [{ width: 1440, height: 900 }, { width: 1920, height: 1080 }];
/** The farthest the lines under a box are drawn, its name at its largest there (shown.ts FAR, NAME_MOST). */
const CLOSEST = 0.45;
const ZOOMS = [CLOSEST, 0.5, 1, 2];
// Each pair a column, its lower box far under its upper one until it's dragged up.
const PAIRS = {
  render: { x: 0, upper: 'making', lower: 'under_making' },
  pictures: { x: 520, upper: 'still_up', lower: 'still_down' },
  words: { x: 1040, upper: 'words_up', lower: 'under_words' },
  tools: { x: 1560, upper: 'shot', lower: 'busy' },
};
const NAMES = Object.values(PAIRS).flatMap((pair) => [pair.upper, pair.lower]);

/** What's drawn of each box, in canvas units: its picture or words, every label and control, its state dot. */
function drawnOf(names) {
  const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection, view = inspection.camera(), pane = root.getBoundingClientRect();
  const world = (r) => ({ x: (r.x - pane.x - view.x) / view.zoom, y: (r.y - pane.y - view.y) / view.zoom, w: r.width / view.zoom, h: r.height / view.zoom });
  return {
    zoom: view.zoom,
    boxes: Object.fromEntries(names.flatMap((name) => {
      const painted = inspection.card(name)?.rendered;
      if (!painted) return [];
      const parts = [{ part: 'body', ...world(painted.body) },
        ...Object.entries(painted.labels).filter(([, label]) => label.text && label.width >= 1 && label.height >= 1).map(([key, label]) => ({ part: key, ...world(label) })),
        ...painted.controls.map((control) => ({ part: control.part, ...world(control) })),
        ...(painted.stateDot ? [{ part: 'state dot', ...world(painted.stateDot) }] : [])];
      return [[name, { parts, progress: Boolean(painted.progress), picture: painted.media.key ? painted.media.ready || painted.media.failed : null }]];
    })),
  };
}

const meet = (a, b) => Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x) > 0.25 && Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y) > 0.25;
const round = (value) => Math.round(value * 10) / 10;

test('spacing', async ({ page, project, api }) => {
  test.setTimeout(300_000);
  const shots = join(process.env.OUT, 'web', 'spacing');
  mkdirSync(shots, { recursive: true });
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`);
  const qwen = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  const shotTool = tools.find((tool) => tool.id === 'minimax-h3/generate') ?? tools.find((tool) => tool.package === 'minimax-h3' && tool.makes === 'video');
  expect(qwen && shotTool, 'the Hub lists Qwen Image 2 and H3').toBeTruthy();

  const { render, pictures, words, tools: tooled } = PAIRS;
  const low = (pair) => ({ x: pair.x, y: 700 });
  await project.add({ name: render.upper, tool: fixtureTool('tool.py/slow'), kind: 'image', fields: { instructions: 'slowly', seconds: 300 }, at: { x: render.x, y: 0 } });
  await project.import(png(40), { name: render.lower, at: low(render) });
  await project.import(png(90), { name: pictures.upper, at: { x: pictures.x, y: 0 } });
  await project.import(png(140), { name: pictures.lower, at: low(pictures) });
  await project.add({ name: words.upper, kind: 'text', fields: { text: 'a quiet harbour at dawn, boats tied along the wall' }, at: { x: words.x, y: 0 } });
  await project.import(png(190), { name: words.lower, at: low(words) });
  await project.add({ name: tooled.upper, tool: shotTool.id, kind: 'video', fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves on black rocks.', duration: 5 }] }, at: { x: tooled.x, y: 0 } });
  await api.post(project.at(`/assets/${tooled.upper}/versions`), { source: await project.upload(MP4) });
  await project.import(png(240), { name: tooled.lower, at: low(tooled) });
  await api.patch(project.at(`/assets/${tooled.lower}`), { tool: qwen.id });
  await expect.poll(() => canvasNames(page)).toEqual(expect.arrayContaining(NAMES));

  /** The page's own view saved, then `zoom` about `at` (a point on the canvas) from a reload, every picture there decoded. */
  const viewAt = async (zoom, at) => {
    const now = await camera(page);
    await expect.poll(async () => {
      const { viewport } = await api.get(project.at('/view'));
      return Boolean(viewport) && sameCamera({ x: viewport.x, y: viewport.y, zoom: viewport.zoom }, now);
    }, { message: "the page's view saved" }).toBe(true);
    const { width, height } = page.viewportSize();
    await api.patch(project.at('/view'), { viewport: { x: Math.round((width + 230) / 2 - at.x * zoom), y: Math.round(height / 2 - at.y * zoom), zoom } });
    await page.reload();
    await expect.poll(() => canvasNames(page)).toEqual(expect.arrayContaining(NAMES));
    await canvasIdle(page);
    expect((await camera(page)).zoom, `at ${zoom}`).toBeCloseTo(zoom, 4);
  };
  /** At rest with no pointer over a box, every picture in view decoded. */
  const passive = async () => {
    const { x, y } = await openPoint(page);
    await page.mouse.move(x, y);
    await canvasIdle(page);
    await expect.poll(async () => {
      const { boxes } = await page.evaluate(drawnOf, NAMES);
      return Object.entries(boxes).filter(([, box]) => box.picture === false).map(([name]) => name);
    }, { message: 'every picture in view decoded' }).toEqual([]);
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  };
  const zoomMenu = async (item) => {
    await page.getByRole('button', { name: /more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
    await canvasIdle(page);
  };
  /** A box's picture, as the API's row and the page's drawing have it: its top, and its bottom on the canvas. */
  const bodyOf = async (name) => {
    const { boxes } = await page.evaluate(drawnOf, [name]);
    const body = boxes[name]?.parts.find((part) => part.part === 'body');
    return body && { top: body.y, bottom: body.y + body.h };
  };

  await test.step('drag each lower box up under its upper one, until it pushes it', async () => {
    await page.setViewportSize(SIZES[0]);
    // The camera moved once, so the page has a view of its own saved.
    await zoomMenu('Zoom to fit');
    await zoomMenu('Zoom to 50%');
    await viewAt(0.5, { x: 960, y: 550 });
    await passive();
    for (const [what, pair] of Object.entries(PAIRS)) {
      const before = await rows(project);
      const upper = await bodyOf(pair.upper);
      const head = (await canvasCard(page, pair.lower)).title;
      const zoom = (await camera(page)).zoom;
      // Its top let go 4 under the upper box's picture, in the room that box keeps under it.
      const from = { x: head.x + 8, y: head.y + head.height / 2 };
      const to = { x: from.x, y: from.y + (upper.bottom + 4 - before[pair.lower].y) * zoom };
      expect(await page.evaluate(({ x, y }) => document.querySelector('.canvas-root').canvasInspection.hitTest({ x, y })?.name ?? null, to), `${what}: let go on no box`).toBe(null);
      await drag(page, from, to);
      await expect.poll(async () => (await rows(project))[pair.upper]?.y, { message: `${what}: ${pair.upper} is pushed` }).toBeLessThan(before[pair.upper].y);
      await canvasIdle(page);
      const after = await rows(project);
      expect.soft(Math.abs(after[pair.lower].y - (upper.bottom + 4)) <= 2 && after[pair.lower].x === before[pair.lower].x,
        `${what}: ${pair.lower} stays where it was let go, ${round(upper.bottom + 4)}: ${JSON.stringify(after[pair.lower])}`).toBe(true);
      expect.soft(after[pair.upper].x, `${what}: ${pair.upper} goes straight up`).toBe(before[pair.upper].x);
    }
  });

  const numbers = [];
  /** Each pair looked at `zoom` about where they meet: what's drawn of every box on no other's, and how far apart the pair's are. */
  const look = async (what, pair, tag) => {
    await passive();
    const { zoom, boxes } = await page.evaluate(drawnOf, NAMES);
    const names = Object.keys(boxes).sort();
    for (const [i, a] of names.entries()) for (const b of names.slice(i + 1)) {
      const on = boxes[a].parts.flatMap((pa) => boxes[b].parts.filter((pb) => meet(pa, pb)).map((pb) => `${a}'s ${pa.part} on ${b}'s ${pb.part}`));
      expect.soft(on, `${what} at ${zoom}, ${tag}: nothing of one box on another`).toEqual([]);
    }
    const [upper, lower] = [boxes[pair.upper], boxes[pair.lower]];
    expect(upper && lower, `${what} at ${zoom}, ${tag}: both drawn`).toBeTruthy();
    const hangs = upper.parts.reduce((most, part) => (part.y + part.h > most.y + most.h ? part : most));
    const head = lower.parts.reduce((least, part) => (part.y < least.y ? part : least));
    const body = upper.parts.find((part) => part.part === 'body');
    const said = { pair: what, zoom: Math.round(zoom * 100) / 100, size: tag, gap: round(head.y - (hangs.y + hangs.h)), under: `${hangs.part} to ${round(hangs.y + hangs.h - body.y - body.h)} under its picture`,
      over: `${head.part} from ${round(lower.parts.find((part) => part.part === 'body').y - head.y)} over its picture`,
      room: round((await rows(project))[pair.lower].y - (body.y + body.h)), rendering: upper.progress };
    numbers.push(said);
    await page.screenshot({ path: join(shots, `${what}-${String(Math.round(zoom * 100)).padStart(3, '0')}-${tag}.png`) });
    return said;
  };

  /** A pair at each zoom, at each size. */
  const pairAt = async (what) => {
    const pair = PAIRS[what];
    const seen = [];
    for (const size of SIZES) {
      await page.setViewportSize(size);
      const tag = `${size.width}x${size.height}`;
      const held = (await rows(project))[pair.upper];
      const at = { x: held.x + (held.w ?? 240) / 2, y: (await rows(project))[pair.lower].y };
      await viewAt(CLOSEST, at);
      seen.push(await look(what, pair, tag));
      for (const zoom of ZOOMS.slice(1)) {
        await zoomMenu(`Zoom to ${zoom * 100}%`);
        seen.push(await look(what, pair, tag));
      }
    }
    for (const said of seen) expect.soft(said.gap, `${what}: apart at ${said.zoom}, ${said.size}`).toBeGreaterThan(0);
    return seen;
  };
  const closest = (seen) => seen.filter((said) => said.zoom === CLOSEST);
  const full = (seen) => seen.filter((said) => said.zoom === 1);

  // Local renders one at a time on this job's machine: started only now, and ended as soon as it's looked at.
  const { local_usable: renders } = await api.get('/v1/status');
  const rendered = [];
  if (renders) {
    await test.step('a box rendering, its lines and Cancel under it: a render moves nothing', async () => {
      const before = await rows(project);
      try {
        await api.call('POST', project.at('/runs'), { names: [render.upper], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
        await expect.poll(async () => {
          const painted = (await canvasCard(page, render.upper))?.rendered;
          return Boolean(painted?.progress && painted.controls.some((control) => control.part === 'cancel'));
        }, { message: `${render.upper}'s render is drawn under it, with Cancel`, timeout: 60_000 }).toBe(true);
        await canvasIdle(page);
        const now = await rows(project);
        expect.soft(NAMES.filter((name) => now[name].x !== before[name].x || now[name].y !== before[name].y), 'a render moves no box').toEqual([]);
        rendered.push(...await pairAt('render'));
      } finally {
        await api.raw('POST', project.at('/cancel'), { body: { names: [render.upper] } });
      }
      expect.soft(rendered.every((said) => said.rendering), 'its render drawn at every look').toBe(true);
      for (const said of closest(rendered)) expect.soft(said.gap >= 4 && said.gap <= 16, `rendering: what hangs under it and the head under it 4 to 16 apart where they come closest: ${JSON.stringify(said)}`).toBe(true);
      for (const said of full(rendered)) expect.soft(said.gap, `rendering: at 100%, ${JSON.stringify(said)}`).toBeLessThanOrEqual(40);
    });
  }

  await test.step('two pictures', async () => {
    const seen = await pairAt('pictures');
    for (const said of closest(seen)) expect.soft(said.gap >= 4 && said.gap <= 16, `pictures: its shape's line and the head under it 4 to 16 apart where they come closest: ${JSON.stringify(said)}`).toBe(true);
    for (const said of full(seen)) expect.soft(said.gap, `pictures: at 100%, ${JSON.stringify(said)}`).toBeLessThanOrEqual(40);
  });

  await test.step('words, with nothing under them, over a picture', async () => {
    const seen = await pairAt('words');
    for (const said of closest(seen)) expect.soft(said.gap >= 4 && said.gap <= 12, `words: nothing kept under them but the head under them, 4 to 12 apart where they come closest: ${JSON.stringify(said)}`).toBe(true);
    for (const said of full(seen)) expect.soft(said.gap, `words: at 100%, ${JSON.stringify(said)}`).toBeLessThanOrEqual(40);
  });

  await test.step("a tool's box over a tool's box: it keeps the room its render draws, and no more", async () => {
    const seen = await pairAt('tools');
    if (rendered.length) {
      for (const said of seen) expect.soft(Math.abs(said.room - rendered[0].room) <= 1, `tools: the room under ${tooled.upper} is the rendering box's, ${rendered[0].room}: ${JSON.stringify(said)}`).toBe(true);
    }
  });

  writeFileSync(join(shots, 'numbers.json'), JSON.stringify(numbers, null, 2));
});
