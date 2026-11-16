// create (cl-973 core cell 2): Tools › Nodes' Image, Video, Audio and Text, and a Hub tool from Tools › Image, each dragged out onto open canvas,
// is a box of its kind, blank, where it was let go; the tool's says "Package · Tool" under its picture. On the blank Image box a video tool
// changes nothing and says why at the bottom left, in one line; an image tool goes on. A box's menu never offers to change its tool (Paul,
// 11-12: a tool changes only by a drop from the palette). Each package the Hub was seeded with is listed under what it makes. A reload finds all
// five where they were. What makes pictures is one group, Image, editing one or not (cl-1068): a tool that reads no picture, dropped on a
// picture with no tool, says it can't edit images and changes nothing; one that reads it goes on, its prompt starting from it. The X on the
// tool line takes the tool off, Undo puts it back.
const { readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, observeStill, canvasCard, canvasNames, canvasPoint, canvasVisible, focus, png } = require('./fixtures');
const { carry, drawn, onCanvas, rows, rowsOf } = require('./geometry');
const { fit, leave } = require('./hands');

const NODES = ['image', 'video', 'audio', 'text'];
// Open canvas a box's room apart (a video's is 360 wide), right of the tools, under the bar, over the notes: the four nodes, then the Hub tool.
const POINTS = [{ x: 480, y: 170 }, { x: 840, y: 170 }, { x: 1280, y: 170 }, { x: 480, y: 590 }, { x: 860, y: 590 }];
/** Where Placer puts a box let go at a point of the canvas: its middle under it, its head 16 above (place.ts addTool, addNode). */
const placedAt = (flow) => ({ x: Math.round(flow.x - 120), y: Math.round(flow.y - 16) });
/** A tool as the page names it (shown.ts toolName). */
const named = (tool) => (tool.package_title && tool.package_title !== tool.label ? `${tool.package_title} · ${tool.label}` : tool.label);
/** Whether a tool reads a picture: a slot of one, or a prompt that takes one (palette.ts readsPictures). */
const reads = (fields) => fields.some((field) => (field.type === 'slot' && field.accept === 'image') ||
  (field.type === 'prompt' && (!field.takes || field.takes.includes('image'))) || (field.type === 'group' && reads(field.members ?? [])));

test('create', { tag: '@engines' }, async ({ page, project, api }) => {
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`);
  const palette = page.getByRole('complementary', { name: 'Tools' });
  await expect(palette.getByRole('button', { name: 'Hide the tools' }), 'the tools open').toBeVisible();
  const category = (operation) => palette.locator(`[data-operation="${operation}"]`);
  const implementations = (operation) => palette.locator(`[data-implementation][data-tool-operation="${operation}"]`);
  /** `operation`'s list open, and only it. */
  const openOperation = async (operation) => {
    if (await category(operation).getAttribute('aria-expanded') !== 'true') await category(operation).click();
    await expect(category(operation)).toHaveAttribute('aria-expanded', 'true');
  };
  const made = {};

  /** Let go at POINTS[i], `row` makes one box more: its name, and where the drop was on the canvas. */
  const dropAt = async (row, i, what) => {
    const point = POINTS[i];
    const was = (await project.assets()).map((asset) => asset.name);
    expect(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.classList.contains('canvas-pane') && !document.querySelector('.canvas-root')?.canvasInspection?.hitTest({ x, y }), point), `${what}: let go on open canvas`).toBe(true);
    const flow = await onCanvas(page, point);
    await carry(page, row, point);
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

  const names = {};
  await test.step("drag Tools › Nodes' Image, Video, Audio and Text to four open points", async () => {
    for (const [i, kind] of NODES.entries()) {
      names[kind] = await dropAt(palette.locator(`[data-palette="nodes"] li[data-node="${kind}"]`), i, `Nodes › ${kind}`);
    }
    for (const kind of NODES) {
      await whereLetGo(names[kind]);
      const asset = await project.asset(names[kind]);
      if (kind === 'text') expect.soft(asset, 'Text: words').toMatchObject({ kind: 'text' });
      else expect.soft(asset, `${kind}: its kind, blank`).toMatchObject({ kind, versions: 0, fields: expect.objectContaining({ node_kind: kind }) });
      expect.soft(asset.tool || null, `${kind}: no tool`).toBeNull();
      expect.soft(asset.fields?.text ?? '', `${kind}: no words`).toBe('');
    }
  });

  let hub;
  const seeded = JSON.parse(readFileSync(join(process.env.OUT, 'web', 'fixtures.json'), 'utf8')).hub;
  /** The groups each tool is listed under, by id. */
  const under = {};
  await test.step('open Tools › Image and drag a Hub tool to a fifth point', async () => {
    // Each package the Hub was seeded with, listed under what it makes: every operation's list opened in turn, as a person opens them.
    const listed = {};
    for (const operation of await palette.locator('[data-palette="tools"] [data-operation]').evaluateAll((all) => all.filter((one) => !one.disabled).map((one) => one.dataset.operation))) {
      await openOperation(operation);
      const kind = await category(operation).getAttribute('data-kind');
      for (const id of await implementations(operation).evaluateAll((all) => all.map((one) => one.dataset.tool))) {
        (listed[id] ??= []).push(kind);
        (under[id] ??= []).push(operation);
      }
    }
    for (const [name, ids] of Object.entries(seeded)) {
      const media = tools.filter((tool) => ids.includes(tool.id) && ['image', 'video'].includes(tool.makes));
      expect.soft(media.length, `${name}: a tool that makes a picture or a video`).toBeGreaterThan(0);
      for (const tool of media) expect.soft(listed[tool.id] ?? [], `${tool.id}, listed under what it makes`).toContain(tool.makes);
    }

    // Qwen Image 2's, a package of the seed with nothing of its own on the page.
    await openOperation('image');
    const row = implementations('image').and(palette.locator('[data-tool^="qwen-image-2/"]')).first();
    const id = await row.getAttribute('data-tool');
    hub = tools.find((tool) => tool.id === id);
    expect(hub, `${id}: a tool the Hub lists`).toBeTruthy();
    names.tool = await dropAt(row, 4, `Tools › Image › ${id}`);
    await whereLetGo(names.tool);
    expect.soft(await project.asset(names.tool), 'the tool box: what it makes, of that tool, blank').toMatchObject({ kind: hub.makes, tool: hub.id, versions: 0 });
    await expect.configure({ soft: true }).poll(async () => (await canvasCard(page, names.tool))?.rendered?.labels.tool?.fullText, { message: 'it says "Package · Tool" under its picture' }).toBe(named(hub));
  });

  await test.step('one Image group: no Edit Image, and each tool that makes a picture under Image, editing one or not', async () => {
    // Paul, 11-16: "collapse image + image-edit in the tools to one section" (cl-1068).
    const groups = await palette.locator('[data-palette="tools"] [data-operation][data-kind="image"] b').allTextContents();
    expect.soft(groups, 'the groups of tools that make pictures').toEqual(['Image', 'Upscale Image']);
    const astray = tools.filter((tool) => tool.makes === 'image' && under[tool.id] && !(under[tool.id].length === 1 && ['image', 'upscale_image'].includes(under[tool.id][0])));
    expect.soft(astray.map((tool) => `${tool.id}: ${under[tool.id].join(', ')}`), 'each tool that makes a picture is listed once, under Image or Upscale Image').toEqual([]);
    // BiRefNet's reads a picture it must be given: an edit, under Image.
    const edits = tools.filter((tool) => seeded.birefnet.includes(tool.id) && tool.makes === 'image');
    expect.soft(edits.length, 'BiRefNet: a tool that edits a picture').toBeGreaterThan(0);
    for (const tool of edits) expect.soft(under[tool.id] ?? [], `${tool.id}, which edits a picture, under Image`).toEqual(['image']);
  });

  await test.step('drop a video tool, then an image tool, onto the blank Image box', async () => {
    const target = names.image;
    const onIt = () => canvasPoint(page, target);
    await openOperation('video');
    const video = implementations('video').and(palette.locator('[data-tool^="minimax-h3/"]')).first();
    const videoId = await video.getAttribute('data-tool');
    const videoWire = tools.find((tool) => tool.id === videoId);
    const before = await project.asset(target);
    const count = (await project.assets()).length;
    await carry(page, video, await onIt());
    const why = page.locator('.notes .note .line').filter({ hasText: `${named(videoWire)} makes video: drop it on a video box` });
    await expect(why, 'a video tool on a picture says why').toHaveCount(1);
    const [line, view] = [await why.evaluate((el) => {
      const r = el.getBoundingClientRect();
      const style = getComputedStyle(el);
      return { x: r.left, bottom: r.bottom, height: r.height, line: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.3 };
    }), page.viewportSize()];
    expect.soft(line.x < view.width / 2 && line.bottom > view.height * 0.6, `said at the bottom left: ${JSON.stringify(line)}`).toBe(true);
    expect.soft(Math.round(line.height / line.line), 'in one line').toBe(1);
    const after = await project.asset(target);
    expect.soft({ tool: after.tool ?? null, kind: after.kind, fields: after.fields, versions: after.versions }, 'the video tool changes nothing').toEqual({ tool: before.tool ?? null, kind: before.kind, fields: before.fields, versions: before.versions });
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);

    await openOperation('image');
    const image = implementations('image').and(palette.locator('[data-tool^="qwen-image-2/"]')).first();
    const imageId = await image.getAttribute('data-tool');
    await carry(page, image, await onIt());
    await expect.poll(async () => (await project.asset(target)).tool, { message: 'the image tool goes on the Image box' }).toBe(imageId);
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
  let reader;
  await test.step("on a picture with no tool, a tool that reads no picture says it can't edit images; one that reads it goes on", async () => {
    const held = Object.values(await rows(project)).filter((row) => Number.isFinite(row.x) && Number.isFinite(row.y));
    const at = { x: Math.round(Math.max(...held.map((row) => row.x + (row.w ?? 360))) + 120), y: Math.round(Math.min(...held.map((row) => row.y))) };
    await project.import(png(90), { name: PHOTO, at });
    await expect.poll(() => canvasNames(page), { message: 'the picture on the page' }).toContain(PHOTO);
    await fit(page);
    const onIt = () => canvasPoint(page, PHOTO);
    const before = await project.asset(PHOTO);
    expect(before.tool || null, 'the picture has no tool').toBeNull();
    const count = (await project.assets()).length;

    // SDXL's Image reads its words only: no picture slot, no prompt that takes one.
    await openOperation('image');
    const words = implementations('image').and(palette.locator('[data-tool^="sdxl/"]')).first();
    await words.scrollIntoViewIfNeeded();
    const wordsId = await words.getAttribute('data-tool');
    const wordsTool = tools.find((tool) => tool.id === wordsId);
    expect(wordsTool && !reads(wordsTool.fields), `${wordsTool?.id}: a tool that makes a picture and reads none`).toBe(true);
    await carry(page, words, await onIt());
    await expect.soft(page.locator('.notes .note .line').filter({ hasText: `${named(wordsTool)} can't edit images` }), "it says it can't edit images").toHaveCount(1);
    const after = await project.asset(PHOTO);
    expect.soft({ tool: after.tool || null, fields: after.fields, versions: after.versions }, 'and changes nothing').toEqual({ tool: null, fields: before.fields, versions: before.versions });
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);

    // Qwen Image 2's reads pictures in its prompt: it goes on, its prompt starting from the picture.
    const reading = implementations('image').and(palette.locator('[data-tool^="qwen-image-2/"]')).first();
    await reading.scrollIntoViewIfNeeded();
    const readerId = await reading.getAttribute('data-tool');
    reader = tools.find((tool) => tool.id === readerId);
    expect(reader && reads(reader.fields), `${reader?.id}: a tool that reads a picture`).toBe(true);
    await carry(page, reading, await onIt());
    await expect.poll(async () => (await project.asset(PHOTO)).tool, { message: 'a tool that reads a picture goes on it' }).toBe(reader.id);
    await expect.configure({ soft: true }).poll(async () => JSON.stringify((await project.asset(PHOTO)).fields?.instructions ?? null), { message: 'its prompt starts from the picture' }).toMatch(/<self\.\d+>/);
    expect.soft((await project.assets()).length, 'and makes no box').toBe(count);
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
});
