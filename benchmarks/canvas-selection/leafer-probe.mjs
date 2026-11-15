import { chromium } from 'playwright';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const arg = (name, fallback) => process.argv.find(value => value.startsWith(`--${name}=`))?.slice(name.length + 3) ?? fallback;
const base = arg('base', 'http://127.0.0.1:5197');
const count = Number(arg('n', '1000'));
const lane = arg('gpu', 'default');
const out = resolve(arg('out', '/home/fidika/cozy_v2/outputs/canvas-library-selection/leafer-pixel-probe'));
const tag = arg('tag', `${Date.now()}-${lane}-n${count}`);
const chrome = process.env.BENCH_CHROME || '/home/fidika/cozy_v2/outputs/canvas-library-selection/browser/chrome-linux64/chrome';
const settings = lane === 'nvidia-vulkan' ? {
  env: { ...process.env, __NV_PRIME_RENDER_OFFLOAD: '1', __GLX_VENDOR_LIBRARY_NAME: 'nvidia', __VK_LAYER_NV_optimus: 'NVIDIA_only', VK_DRIVER_FILES: '/usr/share/vulkan/icd.d/nvidia_icd.json' },
  args: ['--use-angle=vulkan', '--enable-features=Vulkan', '--ignore-gpu-blocklist', '--enable-unsafe-webgpu', '--disable-software-rasterizer'],
} : { env: process.env, args: [] };
mkdirSync(out, { recursive: true });
const receipt = { tag, base, count, lane, chrome, headless: true, args: settings.args, started: new Date().toISOString(), errors: [], samples: [], pass: false };
let browser;
try {
  browser = await chromium.launch({ executablePath: chrome, headless: true, args: ['--no-sandbox', ...settings.args], env: settings.env, ignoreDefaultArgs: ['--enable-unsafe-swiftshader'] });
  const page = await browser.newPage({ viewport: { width: 1600, height: 900 }, deviceScaleFactor: 1 });
  page.on('pageerror', error => receipt.errors.push(String(error)));
  const cdp = await browser.newBrowserCDPSession();
  receipt.system = await cdp.send('SystemInfo.getInfo');
  receipt.processIds = (await cdp.send('SystemInfo.getProcessInfo')).processInfo.map(process => process.id);
  await page.goto(`${base}/probe.html`);
  receipt.canvasControls = await page.evaluate(async () => {
    const image = new Image();
    image.src = `${location.origin}/cards/0/256.webp`;
    await image.decode();
    const controls = [];
    for (const willReadFrequently of [false, true]) for (const side of [32, 1600]) {
      const canvas = document.createElement('canvas');
      canvas.width = side;
      canvas.height = side === 32 ? 32 : 900;
      const ctx = canvas.getContext('2d', { willReadFrequently });
      ctx.fillStyle = '#ff0000';
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      const solid = [...ctx.getImageData(4, 4, 1, 1).data];
      ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
      const photo = [...ctx.getImageData(canvas.width / 2, canvas.height / 2, 1, 1).data];
      controls.push({ side, willReadFrequently, solid, photo, solidPass: solid.join(',') === '255,0,0,255', photoOpaque: photo[3] === 255 });
    }
    return controls;
  });
  console.log(JSON.stringify({ tag, canvasControls: receipt.canvasControls }));
  await page.goto(`${base}/?mode=leafer&n=${count}`);
  await page.waitForFunction(() => window.bench?.ready || window.bench?.error);
  const initializationError = await page.evaluate(() => window.bench.error);
  if (initializationError) {
    receipt.initializationError = initializationError;
    receipt.rejectionScreenshot = resolve(out, `${tag}-rejected.png`);
    await page.screenshot({ path: receipt.rejectionScreenshot });
    throw new Error(initializationError);
  }
  for (const view of ['fit', 'detail']) {
    if (view === 'detail') await page.evaluate(() => window.bench.centerCard('0', 0.8));
    const ready = await page.evaluate(() => window.bench.waitReady(10000));
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const pixels = await page.evaluate(async () => {
      const sample = window.bench.sample();
      const card = window.bench.getCard('0');
      const { camera, tier } = sample;
      const canvas = document.querySelector('#surface canvas');
      if (!canvas) return { pass: false, reason: 'missing scene canvas', sample };
      const bounds = canvas.getBoundingClientRect();
      const actual = canvas.getContext('2d', { willReadFrequently: true });
      if (!actual) return { pass: false, reason: 'missing 2d context', sample };
      const image = new Image();
      image.src = `${location.origin}/cards/${card.asset}/${tier}.webp`;
      await image.decode();
      const expected = document.createElement('canvas');
      expected.width = canvas.width;
      expected.height = canvas.height;
      const ctx = expected.getContext('2d');
      const ratioX = canvas.width / bounds.width, ratioY = canvas.height / bounds.height;
      const x = ((card.x + 1) * camera.zoom + camera.x - bounds.left) * ratioX;
      const y = ((card.y + 28) * camera.zoom + camera.y - bounds.top) * ratioY;
      const width = (card.width - 2) * camera.zoom * ratioX, height = 134 * camera.zoom * ratioY;
      ctx.fillStyle = '#20262d';
      ctx.fillRect(x, y, width, height);
      const cropRatio = (card.width - 2) / 134;
      const sw = Math.min(image.naturalWidth, image.naturalHeight * cropRatio);
      const sh = Math.min(image.naturalHeight, image.naturalWidth / cropRatio);
      ctx.drawImage(image, (image.naturalWidth - sw) / 2, (image.naturalHeight - sh) / 2, sw, sh, x, y, width, height);
      const points = [];
      for (const u of [0.2, 0.4, 0.6, 0.8]) for (const v of [0.2, 0.4, 0.6, 0.8]) {
        const px = Math.floor(x + width * u), py = Math.floor(y + height * v);
        if (px < 0 || py < 0 || px >= canvas.width || py >= canvas.height) continue;
        const found = [...actual.getImageData(px, py, 1, 1).data];
        const wanted = [...ctx.getImageData(px, py, 1, 1).data];
        const delta = Math.max(...wanted.map((value, index) => Math.abs(value - found[index])));
        points.push({ x: px, y: py, found, wanted, delta });
      }
      const opaque = points.filter(point => point.found[3] > 240).length;
      const matching = points.filter(point => point.delta <= 32).length;
      return {
        pass: points.length >= 12 && opaque === points.length && matching / points.length >= 0.8,
        matching, opaque, count: points.length, points,
        canvas: { width: canvas.width, height: canvas.height, bounds: bounds.toJSON(), style: canvas.getAttribute('style') },
        photoRect: { x, y, width, height }, sample,
      };
    });
    const screenshot = resolve(out, `${tag}-${view}.png`);
    await page.screenshot({ path: screenshot });
    receipt.samples.push({ view, ready: { pass: ready.pass, readyFraction: ready.readyFraction }, pixels, screenshot });
    console.log(JSON.stringify({ tag, view, readyFraction: ready.readyFraction, pixelPass: pixels.pass, matching: pixels.matching, opaque: pixels.opaque, points: pixels.count }));
  }
  receipt.pass = receipt.errors.length === 0 && receipt.samples.every(sample => sample.ready.pass && sample.pixels.pass);
} catch (error) {
  receipt.errors.push(String(error));
} finally {
  await browser?.close();
  await new Promise(resolve => setTimeout(resolve, 200));
  receipt.liveOwnedProcesses = (receipt.processIds || []).filter(pid => existsSync(`/proc/${pid}`));
  receipt.finished = new Date().toISOString();
  writeFileSync(resolve(out, `${tag}.json`), JSON.stringify(receipt, null, 2) + '\n');
  console.log(JSON.stringify({ tag, pass: receipt.pass, errors: receipt.errors, receipt: resolve(out, `${tag}.json`) }));
  if (!receipt.pass) process.exitCode = 1;
}
