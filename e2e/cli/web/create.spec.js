// create (cl-973 core cell 2): the Tools sidebar lists no blank boxes (Paul, 11-16: "if you want an image node, just drag an image on there, or
// drag an image tool on there"), and an operation of one tool is that one row, called what the operation is, with nothing to open (Paul,
// 11-16: Video Timeline, Text); a video edit is a row of Video (Paul, 11-16: no Edit Video). A picture, a video and a sound dropped from the
// computer, Tools › Text and a Hub tool from Tools › Image, each let go on open canvas, is a box of its kind where it was let go; the tool's
// says "Package · Tool" under its picture. On the dropped picture a video tool that reads pictures makes a video box from it, which Undo takes
// away (xs-204); an image tool on the video changes nothing and says why at the bottom left, in one line; on the picture it goes on. A box's
// menu never offers to change its tool (Paul, 11-12: a tool changes only by a drop from the palette). Each package the Hub was seeded with is
// listed under what it makes. A reload finds all five where they were. Any image tool goes on any picture (Paul,
// 11-16): SDXL, which reads none, goes on a picture with no tool and offers no reference, and the picture its words start from is struck
// through in red, its reason on hover and in one line under them; Qwen Image 2, which reads pictures, goes on in its place and uses it. The X
// on the tool line takes the tool off, Undo puts it back. The sidebar and SDXL's struck picture are kept at 1440×900, on paper and dark; the
// sidebar at 1920×1080 too.
// cl-1084's Tools follow-ups (Paul, 11-16: "reference inputs are offered only by tools that can use them (a text-to-image-only tool like SDXL
// shows none)"; "One name per model, cased right"): `<` offers only what the box's tool can use, so SDXL's list has no pictures and Qwen Image
// 2's has; each tool is named once, as its package writes its model (MiniMax H3 Edit beside MiniMax H3, never "Minimax H3 · MiniMax H3
// Edit"), in the Video group and on a box's tool line, kept on paper and dark; a group lists what makes something new before what edits.
// cl-1094 (Paul, 11-16: "We also don't need the text 'tools' in the top left box. I don't think it is useful for much unfortunately, since
// everything is a tool on this panel"): the sidebar has no heading and no "Tools" text; its name for a screen reader stays.
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, observeStill, canvasCard, canvasIdle, canvasNames, canvasPoint, canvasVisible, focus, png, MP4, wav } = require('./fixtures');
const { carry, drawn, onCanvas, rows, rowsOf } = require('./geometry');
const { dropFiles, fit, leave } = require('./hands');

const KINDS = ['image', 'video', 'audio', 'text'];
const FILES = { image: { name: 'Coast.png', type: 'image/png', bytes: png(150) }, video: { name: 'Take.mp4', type: 'video/mp4', bytes: MP4 }, audio: { name: 'Sea.wav', type: 'audio/wav', bytes: wav() } };
// Open canvas a box's room apart, right of the tools, under the bar, over the notes: the four kinds, then the Hub tool.
const POINTS = [{ x: 480, y: 170 }, { x: 840, y: 170 }, { x: 1280, y: 170 }, { x: 480, y: 590 }, { x: 860, y: 590 }];
/** Where Placer puts a box let go at a point of the canvas: its middle under it, its head 16 above (place.ts addTool, lay). */
const placedAt = (flow) => ({ x: Math.round(flow.x - 120), y: Math.round(flow.y - 16) });
/** A tool as the page names it (shown.ts toolName): its label alone where that says its package's name already. */
const named = (tool) => (!tool.package_title || tool.label.toLowerCase().includes(tool.package_title.toLowerCase()) ? tool.label : `${tool.package_title} · ${tool.label}`);
/** Each seeded package's name as it writes it, in its description (cl-1084: "their brands as they write them"). */
const WRITTEN = { 'qwen-image-2': 'Qwen-Image-2.1', 'minimax-h3': 'MiniMax H3', sdxl: 'SDXL', birefnet: 'BiRefNet', 'moss-image': 'MOSS-Image' };
/** Whether a tool reads a picture: a slot of one, or a prompt that takes one. */
const reads = (fields) => fields.some((field) => (field.type === 'slot' && field.accept === 'image') ||
  (field.type === 'prompt' && (!field.takes || field.takes.includes('image'))) || (field.type === 'group' && reads(field.members ?? [])));

test('create', { tag: '@engines' }, async ({ page, project, api }) => {
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`);
  const palette = page.getByRole('complementary', { name: 'Tools' });
  await expect(palette.getByRole('button', { name: 'Hide the tools' }), 'the tools open').toBeVisible();
  const category = (operation) => palette.locator(`button[data-operation="${operation}"]`);
  const implementations = (operation) => palette.locator(`[data-implementation][data-tool-operation="${operation}"]`);
  /** `operation`'s list open, and only it. */
  const openOperation = async (operation) => {
    if (await category(operation).getAttribute('aria-expanded') !== 'true') await category(operation).click();
    await expect(category(operation)).toHaveAttribute('aria-expanded', 'true');
  };
  const shots = join(process.env.OUT, 'web', 'create');
  mkdirSync(shots, { recursive: true });
  /** The page at each of `sizes` on paper and dark, each a reload as the page's own switch keeps it, `again` putting back what's looked at,
   *  and `part` of it cropped: kept for a person to look at (Paul, 11-12: look at your own UI). Then as it was. */
  const look = async (what, again, part, sizes = [[1440, 900]]) => {
    const [was, size] = [await page.evaluate(() => localStorage.getItem('cozy.theme')), page.viewportSize()];
    const theme = (chosen) => page.evaluate((one) => (one === null ? localStorage.removeItem('cozy.theme') : localStorage.setItem('cozy.theme', one)), chosen);
    for (const one of ['paper', 'dark']) for (const [width, height] of sizes) {
      await theme(one);
      await page.setViewportSize({ width, height });
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await again();
      await canvasIdle(page);
      await page.screenshot({ path: join(shots, `${what}-${width}x${height}-${one}.png`) });
      await part().screenshot({ path: join(shots, `${what}-part-${width}x${height}-${one}.png`) });
    }
    await theme(was);
    await page.setViewportSize(size);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await again();
  };
  const made = {};

  /** Let go at POINTS[i] by `act`, one box more: its name, and where the drop was on the canvas. */
  const dropAt = async (act, i, what) => {
    const point = POINTS[i];
    const was = (await project.assets()).map((asset) => asset.name);
    expect(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.classList.contains('canvas-pane') && !document.querySelector('.canvas-root')?.canvasInspection?.hitTest({ x, y }), point), `${what}: let go on open canvas`).toBe(true);
    const flow = await onCanvas(page, point);
    await act(point);
    let name;
    await expect.poll(async () => {
      name = (await project.assets()).map((asset) => asset.name).find((one) => !was.includes(one));
      return name ?? null;
    }, { message: `${what} makes a box` }).not.toBeNull();
    await expect.poll(() => canvasVisible(page, name), { message: `${what} on the canvas` }).toBe(true);
    made[name] = { what, point, flow };
    return name;
  };
  /** A box where it was let go: on screen over the point, and its row the point's place. */
  const whereLetGo = async (name) => {
    const { what, point, flow } = made[name];
    // The box as drawn: its node, its name over its card, and the card.
    const card = await canvasCard(page, name), r = card.bounds;
    const rect = { left: r.x, top: r.y, right: r.x + r.width, bottom: r.y + r.height };
    expect.soft(rect.left <= point.x && point.x <= rect.right && rect.top <= point.y && point.y <= rect.bottom, `${what}: drawn over where it was let go, ${JSON.stringify(point)} in ${JSON.stringify(rect)}`).toBe(true);
    const row = (await rowsOf(project, [name]))[name];
    const want = placedAt(flow);
    expect.soft(Math.abs(row.x - want.x) <= 1 && Math.abs(row.y - want.y) <= 1, `${what}: its row ${JSON.stringify(row)} is the drop's place on the canvas, ${JSON.stringify(want)}`).toBe(true);
  };

  await test.step('the Tools sidebar: no list of blank boxes; an operation of one tool is that row; no Edit Video', async () => {
    const listedAll = async () => {
      await expect(palette.locator('.tool-groups.stale')).toHaveCount(0);
      await expect(category('image')).toBeEnabled();
    };
    await listedAll();
    // cl-1094: "everything is a tool on this panel", so it isn't titled Tools.
    await expect.soft(palette.getByRole('heading'), 'no heading in the sidebar (cl-1094)').toHaveCount(0);
    await expect.soft(palette.getByText(/^\s*tools\s*$/i), 'no "Tools" text in the sidebar (cl-1094)').toHaveCount(0);
    await look('tools', listedAll, () => palette, [[1440, 900], [1920, 1080]]);
    // Paul, 11-16: "why do I have a video timeline tool, section, for just one node, that is a video timeline? doesn't make much sense"
    await expect.soft(palette.locator('[data-palette="nodes"]'), 'no Nodes (Paul, 11-16)').toHaveCount(0);
    const timeline = palette.locator('[data-palette="tools"] li[data-entry="timeline"]');
    await expect.soft(timeline, 'Video Timeline, one tool: one row to drag').toHaveAttribute('data-tool', 'cozy/edit');
    await expect.soft(timeline, 'called what the operation is').toHaveText('Video Timeline');
    await expect.soft(category('timeline'), 'with nothing to open').toHaveCount(0);
    const text = palette.locator('[data-palette="tools"] li[data-entry="text"]');
    await expect.soft(text, 'Text, the way words come in, one row').toHaveAttribute('data-tool', 'text');
    await expect.soft(text).toHaveText('Text');
    await expect.soft(palette.locator('[data-operation="edit_video"], [data-entry="edit_video"]'), 'no Edit Video (Paul, 11-16)').toHaveCount(0);
  });

  const names = {};
  await test.step('a picture, a video and a sound dropped from the computer, and Tools › Text, let go at four open points', async () => {
    for (const [i, kind] of KINDS.entries()) {
      if (kind === 'text') {
        names.text = await dropAt((point) => carry(page, palette.locator('[data-palette="tools"] li[data-implementation][data-tool="text"]'), point), i, 'Tools › Text');
        continue;
      }
      names[kind] = await dropAt(async (point) => {
        await dropFiles(page, [FILES[kind]], point);
        // One file asks its name, its stem chosen: Enter takes it, unless it's taken already.
        const field = page.getByRole('textbox', { name: 'Name', exact: true });
        if (await field.waitFor({ state: 'visible', timeout: 5000 }).then(() => true, () => false)) await field.press('Enter', { timeout: 2000 }).catch(() => {});
      }, i, `${FILES[kind].name} dropped`);
    }
    for (const kind of KINDS) {
      await whereLetGo(names[kind]);
      const asset = await project.asset(names[kind]);
      expect.soft(asset, `${kind}: a box of its kind`).toMatchObject({ kind });
      expect.soft(asset.tool || null, `${kind}: no tool`).toBeNull();
      if (kind === 'text') expect.soft(asset.fields?.text ?? '', 'Text: no words').toBe('');
      else await expect.configure({ soft: true }).poll(async () => (await project.asset(names[kind])).versions, { message: `${kind}: its file, version 1` }).toBe(1);
    }
  });

  let hub;
  const seeded = JSON.parse(readFileSync(join(process.env.OUT, 'web', 'fixtures.json'), 'utf8')).hub;
  /** The groups each tool is listed under, by id. */
  const under = {};
  await test.step('every group opened in turn, and a Hub tool from Tools › Image to a fifth point', async () => {
    // Each package the Hub was seeded with, listed under what it makes: every group opened in turn, as a person opens them, and each row of one.
    const listed = {};
    const seen = async () => {
      const shown = await palette.locator('[data-palette="tools"] [data-implementation]').evaluateAll((all) => all.map((one) => ({ id: one.dataset.tool, operation: one.dataset.toolOperation, kind: one.dataset.kind })));
      for (const { id, operation, kind } of shown) {
        (listed[id] ??= new Set()).add(kind);
        (under[id] ??= new Set()).add(operation);
      }
    };
    await seen();
    for (const operation of await palette.locator('[data-palette="tools"] button[data-operation]').evaluateAll((all) => all.filter((one) => !one.disabled).map((one) => one.dataset.operation))) {
      await openOperation(operation);
      expect.soft(await implementations(operation).count(), `${operation}: a group opens to several tools`).toBeGreaterThan(1);
      await seen();
    }
    for (const [name, ids] of Object.entries(seeded)) {
      const media = tools.filter((tool) => ids.includes(tool.id) && ['image', 'video'].includes(tool.makes));
      expect.soft(media.length, `${name}: a tool that makes a picture or a video`).toBeGreaterThan(0);
      for (const tool of media) expect.soft([...(listed[tool.id] ?? [])], `${tool.id}, listed under what it makes`).toContain(tool.makes);
    }

    // Qwen Image 2's, a package of the seed with nothing of its own on the page.
    await openOperation('image');
    const row = implementations('image').and(palette.locator('[data-tool^="qwen-image-2/"]')).first();
    const id = await row.getAttribute('data-tool');
    hub = tools.find((tool) => tool.id === id);
    expect(hub, `${id}: a tool the Hub lists`).toBeTruthy();
    names.tool = await dropAt((point) => carry(page, row, point), 4, `Tools › Image › ${id}`);
    await whereLetGo(names.tool);
    expect.soft(await project.asset(names.tool), 'the tool box: what it makes, of that tool, blank').toMatchObject({ kind: hub.makes, tool: hub.id, versions: 0 });
    await expect.configure({ soft: true }).poll(async () => (await canvasCard(page, names.tool))?.rendered?.labels.tool?.fullText, { message: 'it says "Package · Tool" under its picture' }).toBe(named(hub));
  });

  await test.step('one Image group and one Video group: each tool that makes a picture under Image, a video edit under Video', async () => {
    const entries = await palette.locator('[data-palette="tools"] [data-entry]').evaluateAll((all) => all.map((one) => ({ kind: one.dataset.kind, label: one.querySelector(':scope > b, :scope > button > b')?.textContent })));
    const of = (kind) => entries.filter((one) => one.kind === kind).map((one) => one.label);
    // Paul, 11-16: "collapse image + image-edit in the tools to one section" (cl-1068).
    expect.soft(of('image'), 'what makes pictures').toEqual(['Image', 'Upscale Image']);
    // Paul, 11-16: "eliminate the Edit Video section, and instead place it inside of 'Video' section".
    expect.soft(of('video'), 'what makes video').toEqual(['Video', 'Upscale Video', 'Video Timeline']);
    const listedOnce = (kind, groups) => tools.filter((tool) => tool.makes === kind && under[tool.id] && !(under[tool.id].size === 1 && groups.includes([...under[tool.id]][0])));
    expect.soft(listedOnce('image', ['image', 'upscale_image']).map((tool) => `${tool.id}: ${[...under[tool.id]].join(', ')}`), 'each tool that makes a picture is listed once, under Image or Upscale Image').toEqual([]);
    expect.soft(listedOnce('video', ['video', 'upscale_video', 'timeline']).map((tool) => `${tool.id}: ${[...under[tool.id]].join(', ')}`), 'each tool that makes video is listed once, under Video, Upscale Video or Video Timeline').toEqual([]);
    // BiRefNet's reads a picture it must be given: an edit, under Image.
    const edits = tools.filter((tool) => seeded.birefnet.includes(tool.id) && tool.makes === 'image');
    expect.soft(edits.length, 'BiRefNet: a tool that edits a picture').toBeGreaterThan(0);
    for (const tool of edits) expect.soft([...(under[tool.id] ?? [])], `${tool.id}, which edits a picture, under Image`).toEqual(['image']);
    // MiniMax H3's edit, which changes a video it's given (its plate): a row of Video, called what it does.
    const plate = tools.find((tool) => tool.id === 'minimax-h3/edit');
    expect.soft(plate, 'the seed has MiniMax H3 Edit').toBeTruthy();
    if (plate) {
      expect.soft([...(under[plate.id] ?? [])], `${plate.id}, under Video`).toEqual(['video']);
      await openOperation('video');
      const row = implementations('video').and(palette.locator('[data-tool="minimax-h3/edit"]'));
      // Its label names its model already (packages#940: "MiniMax H3 Edit"): said once, there and on hover (cl-1084).
      await expect.soft(row, 'called what it does, its model said once').toHaveText('MiniMax H3 Edit');
      await expect.soft(row, 'and so on hover').toHaveAttribute('title', /^MiniMax H3 Edit(\n|$)/);
      const shot = implementations('video').and(palette.locator('[data-tool="minimax-h3/generate"]'));
      await expect.soft(shot, 'beside MiniMax H3, cased as its package writes it').toHaveText('MiniMax H3');
      const video = await implementations('video').allTextContents();
      expect.soft(video.indexOf('MiniMax H3') < video.indexOf('MiniMax H3 Edit'), `what makes video before what edits it: ${video.join(', ')}`).toBe(true);
      await look('tools-video', async () => { await expect(palette.locator('.tool-groups.stale')).toHaveCount(0); await openOperation('video'); }, () => palette);
    }
    // One name per model, as its package writes it (cl-1084): each seeded package's title, and its row in Image; there what makes a picture
    // comes first, by name, then what edits one (BiRefNet's).
    for (const [name, ids] of Object.entries(seeded)) {
      for (const tool of tools.filter((tool) => ids.includes(tool.id))) expect.soft(tool.package_title, `${tool.id}: its package's name as it writes it`).toBe(WRITTEN[name]);
    }
    await openOperation('image');
    const image = await implementations('image').evaluateAll((all) => all.map((one) => ({ id: one.dataset.tool, label: one.textContent })));
    const at = (name) => image.findIndex((one) => one.id.startsWith(`${name}/`));
    for (const name of ['moss-image', 'qwen-image-2', 'sdxl', 'birefnet']) expect.soft(image[at(name)]?.label, `${name}'s row in Image`).toBe(WRITTEN[name]);
    const order = ['moss-image', 'qwen-image-2', 'sdxl', 'birefnet'].map(at);
    expect.soft(order.every((i, n) => i >= 0 && (n === 0 || i > order[n - 1])), `MOSS-Image, Qwen-Image-2.1, SDXL, then BiRefNet: ${image.map((one) => one.label).join(', ')}`).toBe(true);
  });

  await test.step('a video tool on the dropped picture makes a video box from it, which Undo takes away; an image tool on the video changes nothing and says why; on the picture it goes on', async () => {
    const target = names.image;
    const onIt = () => canvasPoint(page, target);
    await openOperation('video');
    // MiniMax H3's Shot, whose words read pictures: a tool on a box of another kind it reads makes a new box from it (xs-204).
    const video = implementations('video').and(palette.locator('[data-tool="minimax-h3/generate"]'));
    const videoId = await video.getAttribute('data-tool');
    const before = await project.asset(target);
    const was = (await project.assets()).map((asset) => asset.name);
    await carry(page, video, await onIt());
    let made;
    await expect.poll(async () => (made = (await project.assets()).find((asset) => !was.includes(asset.name)))?.name ?? null, { message: 'a video tool on a picture makes a box' }).not.toBeNull();
    expect.soft(made, 'a video box of the video tool').toMatchObject({ kind: 'video', tool: videoId });
    expect.soft(JSON.stringify(made.fields), 'that reads the picture').toContain(`<${target}>`);
    const kept = await project.asset(target);
    expect.soft({ tool: kept.tool ?? null, kind: kept.kind, fields: kept.fields, versions: kept.versions }, 'the picture stays as it was').toEqual({ tool: before.tool ?? null, kind: before.kind, fields: before.fields, versions: before.versions });
    await page.locator('.top .history').getByRole('button', { name: 'Undo', exact: true }).click();
    await expect.poll(async () => (await project.assets()).map((asset) => asset.name).sort(), { message: 'Undo takes it away' }).toEqual([...was].sort());
    const count = was.length;

    // Qwen Image 2's words read pictures only: on the video it changes nothing, and says why at the bottom left, in one line.
    await openOperation('image');
    const image = implementations('image').and(palette.locator('[data-tool^="qwen-image-2/"]')).first();
    const imageId = await image.getAttribute('data-tool');
    const imageWire = tools.find((tool) => tool.id === imageId);
    const onVideo = await project.asset(names.video);
    await carry(page, image, await canvasPoint(page, names.video));
    const why = page.locator('.notes .note .line').filter({ hasText: `${named(imageWire)} can't use videos, so it can't go on ${names.video}` });
    await expect(why, 'an image tool on a video says why').toHaveCount(1);
    const [line, view] = [await why.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const style = getComputedStyle(el);
      return { x: r.left, bottom: r.bottom, height: r.height, line: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.3 };
    }), page.viewportSize()];
    expect.soft(line.x < view.width / 2 && line.bottom > view.height * 0.6, `said at the bottom left: ${JSON.stringify(line)}`).toBe(true);
    expect.soft(Math.round(line.height / line.line), 'in one line').toBe(1);
    const after = await project.asset(names.video);
    expect.soft({ tool: after.tool ?? null, kind: after.kind, fields: after.fields, versions: after.versions }, 'the image tool changes nothing').toEqual({ tool: onVideo.tool ?? null, kind: onVideo.kind, fields: onVideo.fields, versions: onVideo.versions });
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);

    await carry(page, image, await onIt());
    await expect.poll(async () => (await project.asset(target)).tool, { message: 'the image tool goes on the picture' }).toBe(imageId);
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);
  });

  await test.step('right-click the tool box, then reload', async () => {
    const { x, y } = await canvasPoint(page, names.tool);
    await page.mouse.click(x, y, { button: 'right' });
    const menu = page.getByRole('menu', { name: `${names.tool}'s menu` });
    await expect(menu).toBeVisible();
    const items = await menu.getByRole('menuitem').allTextContents();
    expect.soft(items, "the box's menu").toEqual(expect.arrayContaining(['Star', 'Rename', 'Delete']));
    expect.soft(items.filter((item) => /change the tool/i.test(item)), 'no "Change the tool"').toEqual([]);
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);

    const all = Object.values(names);
    // Looked at once nothing slides: a box the image tool's taller box pushed aside may still be on its way.
    const still = (when) => expect.poll(() => observeStill(page, 300), { message: `the canvas at rest ${when}` }).toEqual([]);
    await still('before the reload');
    const [held, shown] = [await rows(project), await drawn(page)];
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    for (const name of all) await expect.poll(() => canvasVisible(page, name), { message: `${name} after the reload` }).toBe(true);
    await still('after the reload');
    expect.soft(Object.fromEntries(all.map((name) => [name, held[name]])), 'the rows as they were').toEqual(Object.fromEntries(Object.entries(await rows(project)).filter(([name]) => all.includes(name))));
    const again = await drawn(page);
    for (const name of all) {
      expect.soft(Math.abs(again[name].x - shown[name].x) <= 1 && Math.abs(again[name].y - shown[name].y) <= 1, `${name} drawn where it was: ${JSON.stringify(shown[name])} then ${JSON.stringify(again[name])}`).toBe(true);
    }
  });

  // A picture with no tool, right of the rest (cl-1068).
  const PHOTO = 'photo';
  /** What `<` typed at the end of `region`'s words offers, each name with what its box is, and the `<` taken back. */
  const offered = async (region) => {
    const words = region.locator('.in-frame .mention-text').first();
    if (!(await words.evaluate((el) => el === document.activeElement))) await words.click();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.type(' <');
    const list = page.locator('.name-menu[role="listbox"]');
    await list.waitFor({ state: 'visible', timeout: 5000 }).catch(() => {});
    const shown = await list.getByRole('option').allTextContents();
    await page.keyboard.press('Backspace');
    await page.keyboard.press('Backspace');
    await expect(list, 'the list goes with the `<`').toHaveCount(0);
    await expect.poll(async () => (await project.asset(PHOTO)).fields?.instructions ?? '', { message: 'the words as they were' }).not.toMatch(/<\s*$/);
    const kinds = {};
    for (const name of shown) kinds[name] = (await project.asset(/^self\.\d+$/.test(name) ? PHOTO : name))?.kind ?? 'none';
    return kinds;
  };
  let reader;
  await test.step("any image tool goes on a picture: SDXL, which reads none, offers no reference and strikes the picture through in red, saying why; Qwen Image 2 uses it", async () => {
    // Paul, 11-16: "you can attach an image tool to any image. However, it won't be able to use references to images if it's not capable of
    // doing so, such as our SDXL tool"; "if you write <Self.3> or something it has a red line crossing it out".
    const held = Object.values(await rows(project)).filter((row) => Number.isFinite(row.x) && Number.isFinite(row.y));
    const at = { x: Math.round(Math.max(...held.map((row) => row.x + (row.w ?? 360))) + 120), y: Math.round(Math.min(...held.map((row) => row.y))) };
    await project.import(png(90), { name: PHOTO, at });
    await expect.poll(() => canvasNames(page), { message: 'the picture on the page' }).toContain(PHOTO);
    await fit(page);
    const onIt = () => canvasPoint(page, PHOTO);
    const before = await project.asset(PHOTO);
    expect(before.tool || null, 'the picture has no tool').toBeNull();
    const count = (await project.assets()).length;
    /** PHOTO in Focus, its words open in the frame over its picture. */
    const writing = async () => {
      const region = await focus(page, PHOTO);
      const words = region.locator('.in-frame .mention-text').first();
      const edit = region.getByRole('button', { name: 'Edit image', exact: true });
      await expect(words.or(edit).first()).toBeVisible();
      if (!(await words.isVisible())) await edit.click();
      await expect(words).toBeVisible();
      return region;
    };

    // SDXL's Image reads its words only: no picture slot, no prompt that takes one.
    await openOperation('image');
    const words = implementations('image').and(palette.locator('[data-tool^="sdxl/"]')).first();
    await words.scrollIntoViewIfNeeded();
    const wordsId = await words.getAttribute('data-tool');
    const wordsTool = tools.find((tool) => tool.id === wordsId);
    expect(wordsTool && !reads(wordsTool.fields), `${wordsTool?.id}: a tool that makes a picture and reads none`).toBe(true);
    await carry(page, words, await onIt());
    await expect.poll(async () => (await project.asset(PHOTO)).tool, { message: 'SDXL goes on the picture' }).toBe(wordsId);
    expect.soft((await project.asset(PHOTO)).versions, 'its picture stays').toBe(before.versions);
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);
    // Its words, as Edit opens them, start from the picture (`<self.N>`); a few more typed, then saved as Focus is left.
    let region = await writing();
    const typed = region.locator('.in-frame .mention-text').first();
    if (!(await typed.evaluate((el) => el === document.activeElement))) await typed.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' a lighthouse');
    await expect.poll(async () => JSON.stringify((await project.asset(PHOTO)).fields), { message: 'what was typed, saved' }).toContain('a lighthouse');
    await leave(page);
    let photo;
    await expect.poll(async () => JSON.stringify({ fields: (photo = await project.asset(PHOTO)).fields, unused: photo.unused ?? [] }),
      { message: "Cozy says SDXL doesn't support the image its words start from" }).toMatch(/"mention":"self\.\d+"/);
    const self = photo.unused.find((one) => /^self\.\d+$/.test(one.mention));
    // Paul, 11-16 (cl-1084): "does not support this reference".
    expect.soft(self.why, 'said in plain words').toBe("SDXL doesn't support reference images");
    region = await writing();
    const chip = region.locator(`.in-frame .chip[data-mention=${JSON.stringify(self.mention)}]`).first();
    await expect(chip, 'the image SDXL doesn\'t support, marked').toHaveClass(/\bunsupported\b/);
    const style = await chip.evaluate((el) => {
      const probe = document.body.appendChild(document.createElement('i'));
      probe.style.color = 'var(--bad)';
      const red = getComputedStyle(probe).color;
      probe.remove();
      const s = getComputedStyle(el);
      return { line: s.textDecorationLine, color: s.color, stroke: s.textDecorationColor, red };
    });
    expect.soft(style.line, 'struck through').toContain('line-through');
    expect.soft({ color: style.color, stroke: style.stroke }, 'in red').toEqual({ color: style.red, stroke: style.red });
    await expect.soft(chip, 'why, on hover').toHaveAttribute('title', self.why);
    await expect.soft(region.locator('.in-frame .unsupported-note'), 'and in one line under the words').toHaveText(self.why);
    await expect.soft(region.locator('.slot-control[data-accept="image"]'), 'SDXL offers no reference').toHaveCount(0);
    await expect.soft(region.locator('.in-frame .backdrop'), 'and the picture it can\'t use is no reference behind its words').toHaveCount(0);
    // Nor does `<` offer one: SDXL's words take a Text box's words, and no picture, video or sound (cl-1084).
    const sdxlOffers = await offered(region);
    expect.soft(Object.entries(sdxlOffers).filter(([, kind]) => kind !== 'text'), `SDXL's \`<\` offers no picture: ${JSON.stringify(sdxlOffers)}`).toEqual([]);
    expect.soft(Object.keys(sdxlOffers), 'and does offer the Text box').toContain(names.text);
    await look('sdxl-unsupported', writing, () => page.getByRole('region', { name: 'Focused asset' }).locator('.in-frame'));
    await leave(page);

    // Qwen Image 2's reads pictures in its prompt: it goes on in SDXL's place, and the picture its words start from is its reference.
    await openOperation('image');
    const reading = implementations('image').and(palette.locator('[data-tool^="qwen-image-2/"]')).first();
    await reading.scrollIntoViewIfNeeded();
    const readerId = await reading.getAttribute('data-tool');
    reader = tools.find((tool) => tool.id === readerId);
    expect(reader && reads(reader.fields), `${reader?.id}: a tool that reads a picture`).toBe(true);
    await carry(page, reading, await onIt());
    await expect.poll(async () => (await project.asset(PHOTO)).tool, { message: 'a tool that reads a picture goes on it' }).toBe(reader.id);
    await expect.configure({ soft: true }).poll(async () => JSON.stringify((await project.asset(PHOTO)).fields?.instructions ?? null), { message: 'its prompt starts from the picture' }).toMatch(/<self\.\d+>/);
    await expect.configure({ soft: true }).poll(async () => (await project.asset(PHOTO)).unused ?? [], { message: 'nothing it can\'t use' }).toEqual([]);
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);
    region = await writing();
    await expect.soft(region.locator('.in-frame .chip[data-mention^="self."]').first(), 'the picture, used').not.toHaveClass(/\bunsupported\b/);
    await expect.soft(region.locator('.in-frame .unsupported-note'), 'with nothing to say').toHaveCount(0);
    await expect.soft(region.locator('.in-frame .backdrop'), 'the picture its reference, behind its words').toHaveCount(1);
    const qwenOffers = await offered(region);
    expect.soft(Object.values(qwenOffers), `Qwen Image 2's \`<\` offers pictures: ${JSON.stringify(qwenOffers)}`).toContain('image');
    await leave(page);
  });

  await test.step("the tool line's X takes the tool off, and Undo puts it back", async () => {
    // Paul, 11-16: "there should be an X next to it, not a trash can. You are not deleting anything; you are closing it."
    const region = await focus(page, PHOTO);
    const off = region.getByRole('button', { name: 'Remove image tool', exact: true });
    await expect(off).toBeVisible();
    await expect.soft(off, 'it says what it does').toHaveAttribute('title', `Take ${named(reader)} off`);
    await expect.soft(off.locator('svg'), 'an X, not a trash can').toHaveAttribute('data-icon', 'close');
    const versions = (await project.asset(PHOTO)).versions;
    await off.click();
    await expect.poll(async () => (await project.asset(PHOTO)).tool || null, { message: 'the tool is off' }).toBeNull();
    const note = page.locator('.notes .note').filter({ hasText: `Took ${named(reader)} off ${PHOTO}. Its images stay.` });
    await expect(note, 'a note says so, with Undo').toHaveCount(1);
    expect.soft((await project.asset(PHOTO)).versions, 'its picture stays').toBe(versions);
    await note.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect.poll(async () => (await project.asset(PHOTO)).tool, { message: 'Undo puts it back' }).toBe(reader.id);
    await leave(page);
  });

  await test.step('MiniMax H3 Edit on the dropped video: its tool line says it once, as the package writes it, on paper and dark', async () => {
    // cl-1084: the tool line said "Minimax H3 · MiniMax H3 Edit".
    const target = names.video;
    await fit(page);
    await openOperation('video');
    const edit = implementations('video').and(palette.locator('[data-tool="minimax-h3/edit"]'));
    await edit.scrollIntoViewIfNeeded();
    await carry(page, edit, await canvasPoint(page, target));
    await expect.poll(async () => (await project.asset(target)).tool, { message: 'MiniMax H3 Edit goes on the video' }).toBe('minimax-h3/edit');
    await expect.configure({ soft: true }).poll(async () => (await canvasCard(page, target))?.rendered?.labels.tool?.fullText, { message: 'its tool line on the canvas' }).toBe('MiniMax H3 Edit');
    const line = () => page.getByRole('region', { name: 'Focused asset' }).locator('.tool-line').first();
    const focused = async () => {
      await focus(page, target);
      await expect(line()).toBeVisible();
    };
    await focused();
    await expect.soft(line().locator('.image-tool-name'), 'and in Focus').toHaveText('MiniMax H3 Edit');
    expect.soft(await page.locator('body').innerText(), 'no tool named twice, or cased other than as its package writes it').not.toMatch(/Minimax H3|MiniMax H3 · MiniMax H3/);
    await look('tool-line', focused, line);
    await leave(page);
  });
});
