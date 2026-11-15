// Untimed live-video correctness probe. Run only after the coordinator stops other browsers.
import { chromium } from 'playwright';
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const arg = (key, fallback) => process.argv.find(value => value.startsWith(`--${key}=`))?.slice(key.length + 3) ?? fallback;
const out = resolve(arg('out', '/home/fidika/cozy_v2/outputs/canvas-library-selection/video-gpucanvas'));
const base = arg('base-url', 'http://127.0.0.1:5197');
const count = Number(arg('n', '16'));
const samples = Number(arg('samples', '8'));
const intervalMs = Number(arg('interval-ms', '350'));
mkdirSync(out, { recursive: true });
const env = {
  ...process.env,
  VK_DRIVER_FILES: '/usr/share/vulkan/icd.d/intel_icd.json',
};
for (const key of ['__NV_PRIME_RENDER_OFFLOAD', '__GLX_VENDOR_LIBRARY_NAME', '__VK_LAYER_NV_optimus', '__EGL_VENDOR_LIBRARY_FILENAMES', 'VK_ICD_FILENAMES']) delete env[key];
const flags = ['--no-sandbox', '--ignore-gpu-blocklist', '--enable-unsafe-webgpu',
  '--disable-software-rasterizer', '--use-angle=gl', '--enable-features=Vulkan,WebGPUOnVulkanViaGLInterop'];
const result = {
  started: new Date().toISOString(), headless: true, status: 'running', count, samples, intervalMs,
  gpuConfiguration: 'intel-vulkan-gl', environment: { VK_DRIVER_FILES: env.VK_DRIVER_FILES }, removedOffloadEnvironment: true,
  source: '/motion.mp4', flags, phases: [], errors: [],
  boundary: 'Changing video pixels inside gpucanvas, not its separate focused DOM editor. Screenshot sampling is not video-FPS qualification.',
};
let browser;
let page;
try {
  browser = await chromium.launch({
    executablePath: process.env.BENCH_CHROME || '/home/fidika/cozy_v2/outputs/canvas-library-selection/browser/chrome-linux64/chrome',
    headless: true, env, args: flags, ignoreDefaultArgs: ['--enable-unsafe-swiftshader'],
  });
  const context = await browser.newContext({ viewport: { width: 1600, height: 900 }, deviceScaleFactor: 1 });
  page = await context.newPage();
  page.on('pageerror', error => result.errors.push(String(error)));
  await page.goto(`${base}/?mode=gpucanvas&n=${count}&videoProbe=1`);
  await page.waitForFunction(() => window.bench?.ready || window.bench?.error, {}, { timeout: 120000 });
  const initializationError = await page.evaluate(() => window.bench.error);
  if (initializationError) throw new Error(initializationError);
  result.capabilities = await page.evaluate(async () => {
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
    return adapter ? { vendor: adapter.info.vendor, architecture: adapter.info.architecture, isFallbackAdapter: adapter.info.isFallbackAdapter } : null;
  });
  if (result.capabilities?.isFallbackAdapter !== false || !/intel/i.test(result.capabilities?.vendor || '')) throw new Error('Qualified Intel hardware WebGPU unavailable; other/software adapters excluded');
  await page.evaluate(() => window.bench.centerCard('0', 1));
  result.initialReady = await page.evaluate(() => window.bench.waitReady(30000));
  if (!result.initialReady.pass) throw new Error('Initial image readiness protocol budget exceeded');
  if (!await page.evaluate(() => !!window.__gpucanvasVideoProbe)) throw new Error('Missing opt-in video hook; rebuild the harness before this separate probe');

  const clip = await page.evaluate(() => {
    const rect = window.__gpucanvasVideoProbe.screenRect('0');
    return { x: rect.x + 3 * rect.zoom, y: rect.y + 30 * rect.zoom,
      width: rect.width - 6 * rect.zoom, height: 130 * rect.zoom };
  });
  // Pointer/focus stay away before injection; no interaction pins the tested page.
  await page.mouse.move(8, 880);
  await page.evaluate(() => document.activeElement?.blur());
  await page.evaluate(async () => {
    const hook = window.__gpucanvasVideoProbe;
    const image = hook.image('0');
    const original = { hidden: image.hidden, style: image.style.cssText };
    const video = document.createElement('video');
    video.dataset.benchInlineVideo = '0';
    video.style.cssText = image.style.cssText;
    video.muted = true;
    video.loop = true;
    video.playsInline = true;
    video.controls = false;
    video.tabIndex = 0;
    video.preload = 'auto';
    video.src = '/motion.mp4';
    image.hidden = true;
    image.style.display = 'none';
    image.parentElement.append(video);
    const state = { callbacks: 0, metadata: null, callback: 0, video };
    const tick = (_now, metadata) => {
      state.callbacks++;
      state.metadata = { mediaTime: metadata.mediaTime, presentedFrames: metadata.presentedFrames };
      if (video.isConnected && !video.paused) state.callback = video.requestVideoFrameCallback(tick);
    };
    state.callback = video.requestVideoFrameCallback(tick);
    window.__inlineVideoState = state;
    window.__restoreInlineVideo = () => {
      video.cancelVideoFrameCallback(state.callback);
      video.pause();
      video.removeAttribute('src');
      video.load();
      video.remove();
      image.hidden = original.hidden;
      image.style.cssText = original.style;
      delete window.__inlineVideoState;
      delete window.__restoreInlineVideo;
    };
    await Promise.race([
      video.play(),
      new Promise((_, reject) => setTimeout(() => reject(new Error('Inline-video play protocol budget exceeded')), 15000)),
    ]);
  });

  for (const phase of ['unattended', 'hovered', 'focused']) {
    if (phase === 'hovered') await page.mouse.move(clip.x + clip.width / 2, clip.y + clip.height / 2);
    if (phase === 'focused') {
      await page.evaluate(() => window.__inlineVideoState.video.focus({ preventScroll: true }));
      await page.mouse.move(8, 880);
    }
    await page.waitForTimeout(500);
    const rows = [];
    for (let sample = 0; sample < samples; sample++) {
      // Source readback is detached 2D canvas work. It does not mutate the DOM or request a
      // gpucanvas snapshot/invalidate, which would change the condition being tested.
      const source = await page.evaluate(capture => {
        const state = window.__inlineVideoState;
        const video = state.video;
        const scratch = document.createElement('canvas');
        scratch.width = 96;
        scratch.height = 54;
        const context = scratch.getContext('2d', { willReadFrequently: true });
        context.drawImage(video, 0, 0, 96, 54);
        const pixels = context.getImageData(0, 0, 96, 54).data;
        let hash = 2166136261;
        for (const byte of pixels) hash = Math.imul(hash ^ byte, 16777619) >>> 0;
        const quality = video.getVideoPlaybackQuality();
        return {
          hash: hash.toString(16), currentTime: video.currentTime, paused: video.paused,
          readyState: video.readyState, width: video.videoWidth, height: video.videoHeight,
          callbacks: state.callbacks, frameMetadata: state.metadata,
          totalVideoFrames: quality.totalVideoFrames, droppedVideoFrames: quality.droppedVideoFrames,
          focus: document.activeElement === video,
          pageContentVisibility: getComputedStyle(video.closest('[data-gpucanvas-page]')).contentVisibility,
          gpucanvas: window.__gpucanvasVideoProbe.stats(),
          png: capture ? scratch.toDataURL('image/png') : null,
        };
      }, sample === 0 || sample === samples - 1);
      if (source.png) {
        const name = `${phase}-${sample}-source.png`;
        writeFileSync(resolve(out, name), Buffer.from(source.png.split(',')[1], 'base64'));
        source.sourceImage = name;
      }
      delete source.png;
      const screenshot = await page.screenshot({ clip, caret: 'hide', animations: 'allow' });
      const name = `${phase}-${sample}.png`;
      writeFileSync(resolve(out, name), screenshot);
      rows.push({ sample, source, screenshot: name,
        screenshotHash: createHash('sha256').update(screenshot).digest('hex') });
      if (sample + 1 < samples) await page.waitForTimeout(intervalMs);
    }
    // Fresh source snapshots come AFTER the displayed-pixel series. Taking one earlier would
    // promote a parked page and could conceal stale cached atlas pixels.
    const fresh = await page.evaluate(async () => {
      const image = await window.__gpucanvasVideoProbe.snapshot('0');
      const scratch = document.createElement('canvas');
      scratch.width = image.width;
      scratch.height = image.height;
      scratch.getContext('2d').putImageData(image, 0, 0);
      return { width: image.width, height: image.height, png: scratch.toDataURL('image/png') };
    });
    writeFileSync(resolve(out, `${phase}-fresh-package-snapshot.png`), Buffer.from(fresh.png.split(',')[1], 'base64'));
    const sourceDistinct = new Set(rows.map(row => row.source.hash)).size;
    const screenshotDistinct = new Set(rows.map(row => row.screenshotHash)).size;
    result.phases.push({ phase, sourceDistinct, screenshotDistinct,
      sourceMoving: sourceDistinct > 1,
      displayedPixelsMoving: screenshotDistinct > 1,
      staleDisplayWithMovingSource: sourceDistinct > 1 && screenshotDistinct === 1,
      freshPackageSnapshot: { width: fresh.width, height: fresh.height, file: `${phase}-fresh-package-snapshot.png` },
      samples: rows,
    });
  }
  result.status = 'complete';
} catch (error) {
  result.status = 'failed';
  result.error = String(error.stack || error);
} finally {
  if (page) {
    try {
      result.restoration = await page.evaluate(() => {
        window.__restoreInlineVideo?.();
        return { inlineVideos: document.querySelectorAll('[data-bench-inline-video]').length,
          imageRestored: !window.__gpucanvasVideoProbe?.image('0').hidden };
      });
    } catch (error) { result.restorationError = String(error); }
  }
  await browser?.close();
  result.finished = new Date().toISOString();
  writeFileSync(resolve(out, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ status: result.status, error: result.error,
    phases: result.phases.map(({ phase, sourceDistinct, screenshotDistinct, staleDisplayWithMovingSource }) =>
      ({ phase, sourceDistinct, screenshotDistinct, staleDisplayWithMovingSource })), out }));
}
