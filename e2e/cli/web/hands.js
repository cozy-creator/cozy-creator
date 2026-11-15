// What a person's hands do in the words and media cells (cl-973): files let go on the canvas, a box carried onto another and back, the
// canvas fitted, Focus left, a tool taken from the palette, a pointer at rest, Calcifer's ffmpeg. A carry itself is geometry.js's.
const { execFileSync } = require('child_process');
const { readdirSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { expect, box, canvasIdle, middle } = require('./fixtures');
const { carry } = require('./geometry');

/** Files from the computer let go at `at` (a point of the screen) on `target`, as a browser hands a drop over: `[{name, type, bytes}]`. */
async function dropFiles(page, files, at, target = '.stage') {
  const transfer = await page.evaluateHandle((list) => {
    const data = new DataTransfer();
    for (const file of list) data.items.add(new File([Uint8Array.from(atob(file.b64), (c) => c.charCodeAt(0))], file.name, { type: file.type }));
    return data;
  }, files.map((file) => ({ name: file.name, type: file.type, b64: Buffer.from(file.bytes).toString('base64') })));
  for (const type of ['dragenter', 'dragover', 'drop']) await page.dispatchEvent(target, type, { dataTransfer: transfer, clientX: at.x, clientY: at.y });
  await transfer.dispose();
}

/** Where each box of `names` stands, by name, once none is gliding or animating: else 'moving'. */
const standing = (page, names) => page.evaluate((names) => {
  const nodes = names.map((name) => document.querySelector(`.react-flow__node:has(.card[data-asset="${CSS.escape(name)}"])`));
  const moving = (el) => el.classList.contains('glide') || el.getAnimations({ subtree: true }).some((one) => one.playState === 'running' && one.effect?.getComputedTiming().iterations !== Infinity);
  return nodes.some((el) => !el || moving(el)) ? 'moving' : Object.fromEntries(names.map((name, i) => [name, nodes[i].style.transform]));
}, names);

/** Boxes let go on what takes them glide back where they were: `move` carries them, and they're back, still, before this ends. */
async function backAfter(page, names, move) {
  await expect.poll(() => standing(page, names), { message: `${names.join(', ')} still before they're carried` }).not.toBe('moving');
  const was = await standing(page, names);
  await move();
  await expect.poll(() => standing(page, names), { message: `${names.join(', ')} back where they were` }).toEqual(was);
}

/** Box `name` carried by its name and let go at `to`, on what takes it, and back where it was. */
const carryBox = async (page, name, to) => backAfter(page, [name], async () => carry(page, await middle(box(page, name).locator('.nm')), to));

/** The zoom menu's Zoom to fit: every box in view, the canvas still. */
async function fit(page) {
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
}

/** Focus left by Back to canvas: the canvas back and still. */
async function leave(page) {
  const region = page.getByRole('region', { name: 'Focused asset' });
  await region.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await expect(region).toHaveCount(0);
  await canvasIdle(page);
}

/** Tools › `operation` opened, and its row for tool `id`, to carry out. */
async function toolRow(page, operation, id) {
  const tools = page.getByRole('complementary', { name: 'Tools' });
  const category = tools.getByRole('button', { name: operation, exact: true });
  if ((await category.getAttribute('aria-expanded')) !== 'true') await category.click();
  await expect(category).toHaveAttribute('aria-expanded', 'true');
  const row = tools.locator(`li[data-implementation][data-tool=${JSON.stringify(id)}]`);
  await expect(row).toBeVisible();
  return row;
}

/** The pointer at rest on `locator`, as a hand rests it: moved onto it, then a little more. */
async function rest(page, locator) {
  const { x, y } = await middle(locator);
  await page.mouse.move(x - 12, y, { steps: 2 });
  await page.mouse.move(x, y, { steps: 4 });
}

/** How spread a picture's light is, drawn small in the page: a black frame is 0. */
const spread = (element) => element.evaluate((picture) => {
  const canvas = document.createElement('canvas');
  [canvas.width, canvas.height] = [64, 36];
  const drawn = canvas.getContext('2d');
  drawn.drawImage(picture, 0, 0, 64, 36);
  const pixels = drawn.getImageData(0, 0, 64, 36).data;
  const lumas = [];
  for (let i = 0; i < pixels.length; i += 4) lumas.push(0.299 * pixels[i] + 0.587 * pixels[i + 1] + 0.114 * pixels[i + 2]);
  const mean = lumas.reduce((sum, luma) => sum + luma, 0) / lumas.length;
  return Math.round(Math.sqrt(lumas.reduce((sum, luma) => sum + (luma - mean) ** 2, 0) / lumas.length));
});

/** `ffmpeg` or `ffprobe`: Calcifer's own, which it fetched for its thumbnails, else the one on PATH. */
function ffmpeg(tool) {
  const root = join(homedir(), '.cozy', 'tools', 'ffmpeg');
  const find = (dir) => {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return null;
    }
    for (const entry of entries) {
      const at = join(dir, entry.name);
      if (entry.isFile() && entry.name.replace(/\.exe$/, '') === tool) return at;
      if (entry.isDirectory()) {
        const found = find(at);
        if (found) return found;
      }
    }
    return null;
  };
  const own = find(root) ?? tool;
  return (...args) => execFileSync(own, args, { encoding: tool === 'ffprobe' ? 'utf8' : 'buffer', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 1 << 26 });
}

module.exports = { backAfter, carryBox, dropFiles, ffmpeg, fit, leave, rest, spread, toolRow };
