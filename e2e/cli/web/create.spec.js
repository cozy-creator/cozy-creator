// create (cl-973 core cell 2): Tools › Nodes' Image, Video, Audio and Text, and a Hub tool from Tools › Image, each dragged out onto open canvas,
// is a box of its kind, blank, where it was let go; the tool's says "Package · Tool" under its picture. On the blank Image box a video tool
// changes nothing and says why at the bottom left, in one line; an image tool goes on. A box's menu never offers to change its tool (Paul,
// 11-12: a tool changes only by a drop from the palette). Each package the Hub was seeded with is listed under what it makes. A reload finds all
// five where they were.
const { readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, box } = require('./fixtures');
const { carry, drawn, onCanvas, rows, rowsOf } = require('./geometry');

const NODES = ['image', 'video', 'audio', 'text'];
// Open canvas a box's room apart (a video's is 360 wide), right of the tools, under the bar, over the notes: the four nodes, then the Hub tool.
const POINTS = [{ x: 480, y: 170 }, { x: 840, y: 170 }, { x: 1280, y: 170 }, { x: 480, y: 590 }, { x: 860, y: 590 }];
/** Where Placer puts a box let go at a point of the canvas: its middle under it, its head 16 above (place.ts addTool, addNode). */
const placedAt = (flow) => ({ x: Math.round(flow.x - 120), y: Math.round(flow.y - 16) });
/** A tool as the page names it (shown.ts toolName). */
const named = (tool) => (tool.package_title && tool.package_title !== tool.label ? `${tool.package_title} · ${tool.label}` : tool.label);

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
    expect(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.classList.contains('react-flow__pane'), point), `${what}: let go on open canvas`).toBe(true);
    const flow = await onCanvas(page, point);
    await carry(page, row, point);
    let name;
    await expect.poll(async () => {
      name = (await project.assets()).map((asset) => asset.name).find((one) => !was.includes(one));
      return name ?? null;
    }, { message: `${what} makes a box` }).not.toBeNull();
    await expect(box(page, name), `${what} on the canvas`).toBeVisible();
    made[name] = { what, point, flow };
    return name;
  };
  /** A box where it was let go: on screen over the point, and its row the point's place. */
  const whereLetGo = async (name) => {
    const { what, point, flow } = made[name];
    const rect = await box(page, name).boundingBox();
    expect.soft(rect.x <= point.x && point.x <= rect.x + rect.width && rect.y <= point.y && point.y <= rect.y + rect.height, `${what}: drawn over where it was let go, ${JSON.stringify(point)} in ${JSON.stringify(rect)}`).toBe(true);
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
  await test.step('open Tools › Image and drag a Hub tool to a fifth point', async () => {
    // Each package the Hub was seeded with, listed under what it makes: every operation's list opened in turn, as a person opens them.
    const seeded = JSON.parse(readFileSync(join(process.env.OUT, 'web', 'fixtures.json'), 'utf8')).hub;
    const listed = {};
    for (const operation of await palette.locator('[data-palette="tools"] [data-operation]').evaluateAll((all) => all.filter((one) => !one.disabled).map((one) => one.dataset.operation))) {
      await openOperation(operation);
      const kind = await category(operation).getAttribute('data-kind');
      for (const id of await implementations(operation).evaluateAll((all) => all.map((one) => one.dataset.tool))) (listed[id] ??= []).push(kind);
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
    await expect.soft(box(page, names.tool).locator('.tile-under .node-tool'), 'it says "Package · Tool" under its picture').toHaveText(named(hub));
  });

  await test.step('drop a video tool, then an image tool, onto the blank Image box', async () => {
    const target = names.image;
    const onIt = async () => {
      const rect = await box(page, target).locator('.media').first().boundingBox();
      return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
    };
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
    const { x, y } = await box(page, names.tool).locator('.media').first().boundingBox().then((r) => ({ x: r.x + r.width / 2, y: r.y + r.height / 2 }));
    await page.mouse.click(x, y, { button: 'right' });
    const menu = page.getByRole('menu', { name: `${names.tool}'s menu` });
    await expect(menu).toBeVisible();
    const items = await menu.getByRole('menuitem').allTextContents();
    expect.soft(items, "the box's menu").toEqual(expect.arrayContaining(['Star', 'Rename', 'Delete']));
    expect.soft(items.filter((item) => /change the tool/i.test(item)), 'no "Change the tool"').toEqual([]);
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);

    const all = Object.values(names);
    const [held, shown] = [await rows(project), await drawn(page)];
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    for (const name of all) await expect(box(page, name), `${name} after the reload`).toBeVisible();
    expect.soft(Object.fromEntries(all.map((name) => [name, held[name]])), 'the rows as they were').toEqual(Object.fromEntries(Object.entries(await rows(project)).filter(([name]) => all.includes(name))));
    const again = await drawn(page);
    for (const name of all) {
      expect.soft(Math.abs(again[name].x - shown[name].x) <= 1 && Math.abs(again[name].y - shown[name].y) <= 1, `${name} drawn where it was: ${JSON.stringify(shown[name])} then ${JSON.stringify(again[name])}`).toBe(true);
    }
  });
});
