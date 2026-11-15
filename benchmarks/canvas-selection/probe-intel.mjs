// Sequential hardware/coherence qualification; do not overlap with timed trials or other probes.
import { chromium } from 'playwright';
import { mkdirSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';

const arg = (key, fallback) => process.argv.find(value => value.startsWith(`--${key}=`))?.slice(key.length + 3) ?? fallback;
const out = resolve(arg('out', '/home/fidika/cozy_v2/outputs/canvas-library-selection/intel-pipeline'));
const base = arg('base-url', 'http://127.0.0.1:5197');
mkdirSync(out, { recursive: true });
const env = { ...process.env, VK_DRIVER_FILES: '/usr/share/vulkan/icd.d/intel_icd.json' };
for (const key of ['__NV_PRIME_RENDER_OFFLOAD', '__GLX_VENDOR_LIBRARY_NAME', '__VK_LAYER_NV_optimus', '__EGL_VENDOR_LIBRARY_FILENAMES', 'VK_ICD_FILENAMES']) delete env[key];
const common = ['--no-sandbox', '--enable-unsafe-webgpu', '--ignore-gpu-blocklist', '--disable-software-rasterizer'];
const cases = [
  ['intel-vulkan-gl', ['--enable-features=Vulkan,WebGPUOnVulkanViaGLInterop', '--use-angle=gl']],
  ['intel-default-angle', []],
  ['intel-gl-interop', ['--enable-features=WebGPUOnVulkanViaGLInterop']],
  ['intel-vulkan-angle', ['--enable-features=Vulkan', '--use-angle=vulkan']],
];
const results = [];
for (const [name, extra] of cases) {
  let browser;
  const row = { name, headless: true, env: { VK_DRIVER_FILES: env.VK_DRIVER_FILES }, removedOffloadEnvironment: true,
    args: [...common, ...extra], errors: [], started: new Date().toISOString() };
  try {
    browser = await chromium.launch({
      headless: true,
      executablePath: process.env.BENCH_CHROME || '/home/fidika/cozy_v2/outputs/canvas-library-selection/browser/chrome-linux64/chrome',
      env, args: row.args, ignoreDefaultArgs: ['--enable-unsafe-swiftshader'], timeout: 15000,
    });
    const context = await browser.newContext({ viewport: { width: 1200, height: 700 }, deviceScaleFactor: 1 });
    const page = await context.newPage();
    page.on('console', message => { if (message.type() === 'error') row.errors.push(message.text()); });
    page.on('pageerror', error => row.errors.push(String(error)));
    await page.goto(`${base}/probe.html`);
    await page.setContent('<style>html,body{margin:0;background:#111820}canvas{position:absolute}</style>');
    row.probe = await page.evaluate(async () => {
      const makeCanvas = (width, height, x, y) => {
        const element = document.createElement('canvas');
        element.width = width; element.height = height;
        Object.assign(element.style, { left: `${x}px`, top: `${y}px` });
        document.body.append(element);
        return element;
      };
      const photo = new Image();
      photo.src = '/cards/0/256.webp';
      await photo.decode();
      const flat = makeCanvas(640, 360, 0, 0);
      const c = flat.getContext('2d'); // Deliberately default accelerated-capable Canvas2D path.
      c.fillStyle = '#111820'; c.fillRect(0, 0, 640, 360);
      c.fillStyle = '#ff0000'; c.fillRect(10, 10, 60, 60);
      c.drawImage(photo, 90, 10, 240, 134);
      c.fillStyle = '#ffffff'; c.font = '24px Arial'; c.fillText('Canvas2D café 山 12345', 10, 190);
      const solid = [...c.getImageData(20, 20, 1, 1).data];
      const photoPixels = c.getImageData(90, 10, 240, 134).data;
      const textPixels = c.getImageData(10, 155, 360, 45).data;
      let photoOpaque = 0, photoNonBlack = 0, textWhite = 0;
      for (let i = 0; i < photoPixels.length; i += 4) {
        if (photoPixels[i + 3] === 255) photoOpaque++;
        if (photoPixels[i] + photoPixels[i + 1] + photoPixels[i + 2] > 40) photoNonBlack++;
      }
      for (let i = 0; i < textPixels.length; i += 4) {
        if (textPixels[i] > 200 && textPixels[i + 1] > 200 && textPixels[i + 2] > 200 && textPixels[i + 3] === 255) textWhite++;
      }
      const canvas2d = { attributes: c.getContextAttributes(), solid, photoOpaque, photoNonBlack, textWhite,
        pass: solid.join(',') === '255,0,0,255' && photoOpaque === 240 * 134 && photoNonBlack > 10000 && textWhite > 100 };
      const glProbe = (type, x) => {
        const canvas = makeCanvas(128, 128, x, 10);
        const gl = canvas.getContext(type, { preserveDrawingBuffer: true, failIfMajorPerformanceCaveat: true });
        if (!gl) return { context: false, pass: false };
        const initialError = gl.getError();
        gl.viewport(0, 0, 128, 128); gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        gl.clearColor(1, 0, 0, 1); gl.clear(gl.COLOR_BUFFER_BIT);
        const pixel = new Uint8Array(4);
        gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel); // Before WebGPU await/frame boundary.
        const error = gl.getError();
        const debug = gl.getExtension('WEBGL_debug_renderer_info');
        const renderer = debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : null;
        return { context: true, initialError, pixel: [...pixel], error, renderer,
          pass: initialError === 0 && error === 0 && pixel.join(',') === '255,0,0,255' && /intel/i.test(renderer || '') && !/swiftshader|llvmpipe/i.test(renderer || '') };
      };
      const webgl2 = glProbe('webgl2', 700);
      const webgl1 = glProbe('webgl', 850);
      const adapter = await Promise.race([
        navigator.gpu?.requestAdapter({ powerPreference: 'high-performance' }),
        new Promise((_, reject) => setTimeout(() => reject(new Error('requestAdapter protocol budget exceeded')), 10000)),
      ]);
      let webgpu = { adapter: false, pass: false };
      if (adapter) {
        const device = await adapter.requestDevice();
        window.__pipelineProbeDevice = device;
        const element = makeCanvas(128, 128, 700, 180);
        const context = element.getContext('webgpu');
        device.pushErrorScope('validation');
        context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat(), alphaMode: 'opaque', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC });
        const texture = context.getCurrentTexture();
        const encoder = device.createCommandEncoder();
        const pass = encoder.beginRenderPass({ colorAttachments: [{ view: texture.createView(), clearValue: { r: 0, g: 1, b: 0, a: 1 }, loadOp: 'clear', storeOp: 'store' }] });
        pass.end();
        const buffer = device.createBuffer({ size: 256, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
        encoder.copyTextureToBuffer({ texture }, { buffer, bytesPerRow: 256, rowsPerImage: 1 }, { width: 1, height: 1, depthOrArrayLayers: 1 });
        device.queue.submit([encoder.finish()]);
        await buffer.mapAsync(GPUMapMode.READ);
        const pixel = [...new Uint8Array(buffer.getMappedRange()).slice(0, 4)];
        buffer.unmap(); buffer.destroy();
        const error = (await device.popErrorScope())?.message || null;
        webgpu = { adapter: true, vendor: adapter.info.vendor, architecture: adapter.info.architecture,
          isFallbackAdapter: adapter.info.isFallbackAdapter, pixel, error,
          pass: adapter.info.isFallbackAdapter === false && /intel/i.test(adapter.info.vendor) && pixel.join(',') === '0,255,0,255' && error === null };
      }
      return { canvas2d, webgl2, webgl1, webgpu, htmlInCanvas: 'drawElementImageToTexture' in (globalThis.GPUQueue?.prototype || {}) };
    });
    const imagePath = resolve(out, `${name}.png`);
    await page.screenshot({ path: imagePath });
    row.screenshot = imagePath;
    row.visiblePixels = JSON.parse(execFileSync('python3', ['-c', `
import json,sys
from PIL import Image
im=Image.open(sys.argv[1]).convert('RGBA')
photo=im.crop((90,10,330,144)); text=im.crop((10,155,370,200))
photo_opaque=sum(p[3]==255 for p in photo.getdata())
photo_nonblack=sum(sum(p[:3])>40 for p in photo.getdata())
text_white=sum(min(p[:3])>200 and p[3]==255 for p in text.getdata())
points={'solid':im.getpixel((20,20)),'webgl2':im.getpixel((710,20)),'webgl1':im.getpixel((860,20)),'webgpu':im.getpixel((710,190))}
print(json.dumps({'points':points,'photoOpaque':photo_opaque,'photoNonBlack':photo_nonblack,'textWhite':text_white,'pass':points['solid']==(255,0,0,255) and points['webgl2']==(255,0,0,255) and points['webgpu']==(0,255,0,255) and photo_opaque==240*134 and photo_nonblack>10000 and text_white>100}))
`, imagePath], { encoding: 'utf8' }));
    const cdp = await browser.newBrowserCDPSession();
    row.system = await cdp.send('SystemInfo.getInfo');
    row.pass = row.probe.canvas2d.pass && row.probe.webgl2.pass && row.probe.webgpu.pass && row.visiblePixels.pass;
  } catch (error) {
    row.error = String(error.stack || error);
    row.pass = false;
  } finally {
    await browser?.close();
    row.finished = new Date().toISOString();
    results.push(row);
    writeFileSync(resolve(out, 'result.json'), JSON.stringify(results, null, 2));
    console.log(JSON.stringify({ name, pass: row.pass, probe: row.probe, visiblePixels: row.visiblePixels, error: row.error }));
  }
  if (row.pass) break;
}
