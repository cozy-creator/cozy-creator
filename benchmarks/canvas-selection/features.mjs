import { chromium } from 'playwright';
import { getBrowserSettings } from './hardware.mjs';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const arg = (key, fallback) => process.argv.find(value => value.startsWith(`--${key}=`))?.slice(key.length + 3) ?? fallback;
const modes = arg('modes', 'dom,reactflow,pixi,leafer,pixi-webgpu,gpucanvas').split(',').filter(Boolean);
const n = Number(arg('n', '1000'));
const dpr = Number(arg('dpr', '1'));
const config = arg('gpu', 'intel-coherent');
const threeBackend = arg('three-backend', 'webgl');
const overheadMs = Number(arg('overhead-ms', '0'));
const baseUrl = arg('url', 'http://127.0.0.1:5197');
const query = arg('query', 'floor=256&view=working&retention=budget');
const runId = `${new Date().toISOString().replace(/[^0-9TZ]/g, '')}-${process.pid}`;
const out = resolve(arg('out', `results/features-${runId}`));
const fixtures = process.env.CANVAS_BENCH_ASSETS || '/home/fidika/cozy_v2/outputs/canvas-library-selection/fixtures';
const pixelGate = fileURLToPath(new URL('./pixel-gate.py', import.meta.url));
const settings = { ...getBrowserSettings(config), headless: true };
if (!['webgl', 'webgpu'].includes(threeBackend)) throw new Error('--three-backend must be webgl or webgpu');
if (!Number.isInteger(n) || n < 1 || !(dpr > 0) || overheadMs < 0) throw new Error('Invalid count, DPR, or overhead duration');
mkdirSync(out, { recursive: true });

function procMem(pid) {
  try {
    const text = readFileSync(`/proc/${pid}/smaps_rollup`, 'utf8');
    const value = name => Number(text.match(new RegExp(`^${name}:\\s+(\\d+)`, 'm'))?.[1] || 0) * 1024;
    return { pid, rssBytes: value('Rss'), pssBytes: value('Pss'), privateBytes: value('Private_Clean') + value('Private_Dirty') };
  } catch { return { pid, unavailable: true }; }
}
async function snapshot(browserCdp, pageCdp) {
  const { processInfo } = await browserCdp.send('SystemInfo.getProcessInfo');
  const performance = await pageCdp.send('Performance.getMetrics');
  return {
    at: Date.now(), processes: processInfo.map(process => ({ ...process, ...procMem(process.id) })),
    performance: Object.fromEntries(performance.metrics.map(metric => [metric.name, metric.value])),
  };
}
function memory(sample) {
  return {
    pssMiB: sample.processes.reduce((sum, process) => sum + (process.pssBytes || 0), 0) / 1048576,
    rssMiB: sample.processes.reduce((sum, process) => sum + (process.rssBytes || 0), 0) / 1048576,
    privateMiB: sample.processes.reduce((sum, process) => sum + (process.privateBytes || 0), 0) / 1048576,
    jsHeapUsedMiB: sample.performance.JSHeapUsedSize / 1048576,
    processes: sample.processes,
  };
}
function summarizePhase(before, after, wallMs) {
  const prior = new Map(before.processes.map(process => [process.id, process.cpuTime]));
  const cpuSecondsByType = {};
  let seconds = 0;
  for (const process of after.processes) {
    const delta = Math.max(0, process.cpuTime - (prior.get(process.id) || 0));
    seconds += delta;
    cpuSecondsByType[process.type] = (cpuSecondsByType[process.type] || 0) + delta;
  }
  return {
    wallMs, browserCpuSeconds: seconds, cpuOneCorePercent: seconds / (wallMs / 1000) * 100, cpuSecondsByType,
    rendererTaskSeconds: after.performance.TaskDuration - before.performance.TaskDuration,
    rendererScriptSeconds: after.performance.ScriptDuration - before.performance.ScriptDuration,
    rendererLayoutSeconds: after.performance.LayoutDuration - before.performance.LayoutDuration,
    rendererRecalcStyleSeconds: after.performance.RecalcStyleDuration - before.performance.RecalcStyleDuration,
    beforeMemory: memory(before), afterMemory: memory(after),
  };
}
async function budget(promise, milliseconds, label) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} protocol budget exceeded (${milliseconds}ms)`)), milliseconds);
    })]);
  } finally { clearTimeout(timer); }
}
const twoFrames = page => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
const threeStats = page => page.evaluate(() => window.bench.sample().three ?? null);
async function measuredTour(page, browserCdp, pageCdp, milliseconds) {
  const before = await snapshot(browserCdp, pageCdp);
  const started = Date.now();
  const tour = await budget(page.evaluate(ms => window.bench.tour(ms), milliseconds), milliseconds + 30000, 'Camera tour');
  const after = await snapshot(browserCdp, pageCdp);
  return { tour, ...summarizePhase(before, after, Date.now() - started) };
}

const summary = [];
for (const mode of modes) {
  const key = `${config}-${mode}-features-n${n}-dpr${dpr}-${runId}`;
  const result = {
    key, mode, n, dpr, config, threeBackend, query, headless: true,
    started: new Date().toISOString(), status: 'running',
    settings: { args: [...settings.args], gpuConfiguration: config, headless: true },
    checks: {}, pageErrors: [], consoleErrors: [],
    boundary: 'Perspective plane input and an independent transparent 400x250 Three model-viewer canvas. No shared GPU-resource or integrated 3D-background claim.',
  };
  const check = (name, pass, detail = {}) => { result.checks[name] = { pass: Boolean(pass), ...detail }; };
  let browser;
  let page;
  try {
    browser = await chromium.launch(settings);
    const context = await browser.newContext({ viewport: { width: 1600, height: 900 }, deviceScaleFactor: dpr });
    page = await context.newPage();
    page.setDefaultTimeout(30000);
    page.on('pageerror', error => result.pageErrors.push(error.stack || String(error)));
    page.on('crash', () => result.pageErrors.push('PAGE_CRASH'));
    page.on('console', message => { if (message.type() === 'error') result.consoleErrors.push(message.text()); });
    const browserCdp = await browser.newBrowserCDPSession();
    const pageCdp = await context.newCDPSession(page);
    await pageCdp.send('Performance.enable');
    result.system = await browserCdp.send('SystemInfo.getInfo');
    const compositor = result.system.gpu.auxAttributes.glRenderer || '';
    check('hardwareCompositor', !/SwiftShader|llvmpipe|software/i.test(compositor), { renderer: compositor });
    if (!result.checks.hardwareCompositor.pass) throw new Error('Software compositor excluded');

    await page.goto(`${baseUrl}/?mode=${encodeURIComponent(mode)}&n=${n}&${query}`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.bench?.ready || window.bench?.error, {}, { timeout: 120000 });
    const initializationError = await page.evaluate(() => window.bench.error);
    if (initializationError) throw new Error(initializationError);
    result.initialReady = await budget(page.evaluate(() => window.bench.waitReady(120000)), 125000, 'Initial image readiness');
    check('thumbnailReadiness', result.initialReady.pass, { readyFraction: result.initialReady.readyFraction });
    if (!result.initialReady.pass) throw new Error('Initial thumbnail readiness failed');
    result.systemAfterInit = await browserCdp.send('SystemInfo.getInfo');
    const initializedCompositor = result.systemAfterInit.gpu.auxAttributes.glRenderer || '';
    check('hardwareCompositor', Boolean(initializedCompositor) && !/SwiftShader|llvmpipe|software/i.test(initializedCompositor), { renderer: initializedCompositor });
    if (!result.checks.hardwareCompositor.pass) throw new Error('Hardware compositor identity unavailable or software');
    if (mode === 'gpucanvas' || mode === 'pixi-webgpu' || threeBackend === 'webgpu') {
      result.webgpu = await budget(page.evaluate(async () => {
        const adapter = await navigator.gpu?.requestAdapter();
        return adapter ? { vendor: adapter.info.vendor, architecture: adapter.info.architecture, isFallbackAdapter: adapter.info.isFallbackAdapter } : null;
      }), 15000, 'WebGPU adapter');
      check('hardwareWebGPU', result.webgpu && result.webgpu.isFallbackAdapter === false, { adapter: result.webgpu });
      if (!result.checks.hardwareWebGPU.pass) throw new Error('Hardware WebGPU unavailable');
    }

    const initialPng = resolve(out, `${key}-initial.png`);
    await page.screenshot({ path: initialPng });
    result.initialPixelGate = JSON.parse(execFileSync('uv', ['run', '--with', 'pillow', 'python', pixelGate], {
      encoding: 'utf8', input: JSON.stringify({
        screenshot: initialPng, n, camera: result.initialReady.camera, dpr, width: 1600, height: 900,
        tier: result.initialReady.tier, pool: result.initialReady.assetPool, fixtures,
      }),
    }));
    check('displayedThumbnails', result.initialPixelGate.pass, { screenshot: initialPng, gate: result.initialPixelGate });
    if (!result.initialPixelGate.pass) throw new Error('Displayed-thumbnail pixel gate failed');

    await page.evaluate(() => {
      window.__featureInputs = [];
      for (const type of ['pointerdown', 'pointerup', 'wheel']) document.addEventListener(type, event => {
        window.__featureInputs.push({ type, trusted: event.isTrusted, x: event.clientX, y: event.clientY });
      }, true);
      window.bench.centerCard('0', 0.8);
      window.bench.setTilt(15, -18);
    });
    await page.evaluate(() => window.bench.waitReady(30000));
    await twoFrames(page);
    result.perspective = await page.evaluate(() => {
      let seed = 0x1234abcd;
      const random = () => ((seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0) / 0x100000000);
      const samples = [];
      let maxError = 0;
      for (let i = 0; i < 100; i++) {
        const point = { x: random() * innerWidth, y: random() * innerHeight };
        const projected = window.bench.projectPoint(point.x, point.y);
        const roundtrip = window.bench.unprojectPoint(projected.x, projected.y);
        const error = Math.hypot(roundtrip.x - point.x, roundtrip.y - point.y);
        maxError = Math.max(maxError, error);
        samples.push({ point, projected, roundtrip, error });
      }
      return { tilt: { x: 15, y: -18 }, seed: '0x1234abcd', count: samples.length, maxError, samples };
    });
    check('perspectiveRoundtrip', result.perspective.maxError <= 1e-6, { maxError: result.perspective.maxError, tolerance: 1e-6, samples: 100 });
    const projectedCard = await page.evaluate(() => window.bench.projectCard('0'));
    await page.mouse.click(projectedCard.x, projectedCard.y);
    await twoFrames(page);
    result.projectedClick = await page.evaluate(() => ({ selected: window.bench.sample().selected, inputs: [...window.__featureInputs] }));
    check('projectedTrustedClick', result.projectedClick.selected === '0'
      && result.projectedClick.inputs.some(event => event.type === 'pointerdown' && event.trusted)
      && result.projectedClick.inputs.some(event => event.type === 'pointerup' && event.trusted), result.projectedClick);

    const drag = await page.evaluate(() => {
      const card = { ...window.bench.getCard('0') };
      const start = window.bench.projectCard('0');
      const local = window.bench.unprojectPoint(start.x, start.y);
      return { card, start, local, end: window.bench.projectPoint(local.x + 32, local.y + 24) };
    });
    await page.mouse.move(drag.start.x, drag.start.y);
    await page.mouse.down();
    await page.mouse.move(drag.end.x, drag.end.y, { steps: 12 });
    await page.mouse.up();
    await twoFrames(page);
    const moved = await page.evaluate(() => ({ ...window.bench.getCard('0') }));
    const delta = { x: moved.x - drag.card.x, y: moved.y - drag.card.y };
    // The algebra gate above is exact; native pointer coordinates may be quantized to screen pixels.
    const inputTolerance = 0.5;
    check('projectedDrag', Math.abs(delta.x - 40) <= inputTolerance && Math.abs(delta.y - 30) <= inputTolerance,
      { ...drag, moved, delta, expectedWorldDelta: { x: 40, y: 30 }, nativeInputToleranceWorldUnits: inputTolerance });

    const anchor = await page.evaluate(() => {
      const screen = window.bench.projectCard('0');
      const local = window.bench.unprojectPoint(screen.x, screen.y);
      const camera = window.bench.sample().camera;
      return { screen, local, camera, world: { x: (local.x - camera.x) / camera.zoom, y: (local.y - camera.y) / camera.zoom } };
    });
    await page.mouse.move(anchor.screen.x, anchor.screen.y);
    await page.mouse.wheel(0, -120);
    await page.waitForTimeout(150);
    const afterWheel = await page.evaluate(screen => {
      const local = window.bench.unprojectPoint(screen.x, screen.y);
      const camera = window.bench.sample().camera;
      return { camera, world: { x: (local.x - camera.x) / camera.zoom, y: (local.y - camera.y) / camera.zoom }, inputs: [...window.__featureInputs] };
    }, anchor.screen);
    const anchorError = Math.hypot(afterWheel.world.x - anchor.world.x, afterWheel.world.y - anchor.world.y);
    check('projectedWheelAnchor', afterWheel.camera.zoom > anchor.camera.zoom && anchorError <= inputTolerance
      && afterWheel.inputs.some(event => event.type === 'wheel' && event.trusted),
      { before: anchor, after: afterWheel, anchorErrorWorldUnits: anchorError, toleranceWorldUnits: inputTolerance });
    result.tiltedReady = await page.evaluate(() => window.bench.waitReady(30000));
    await twoFrames(page);
    result.tiltedScreenshot = resolve(out, `${key}-tilted.png`);
    await page.screenshot({ path: result.tiltedScreenshot });
    await page.evaluate(() => window.bench.setTilt(0, 0));
    await twoFrames(page);

    if (overheadMs > 0) {
      result.overhead = { milliseconds: overheadMs, order: 'warmup, baseline, then animated separate 3D overlay', boundary: 'Single ordered paired sample, not a general renderer ranking or integrated-background benchmark.' };
      result.overhead.warmup = await budget(page.evaluate(ms => window.bench.tour(ms), overheadMs), overheadMs + 30000, 'Warmup tour');
      await page.evaluate(() => window.bench.waitReady(30000));
      result.overhead.baseline = await measuredTour(page, browserCdp, pageCdp, overheadMs);
      await page.evaluate(() => window.bench.waitReady(30000));
    }

    result.canvasBefore3D = await page.locator('canvas').count();
    result.threeOpened = await budget(page.evaluate(value => window.bench.open3D(value), threeBackend), 60000, 'Real glTF model load');
    await twoFrames(page);
    const opened = await threeStats(page);
    result.threeOpened = opened;
    result.canvasWith3D = await page.locator('canvas').count();
    check('realModel', opened?.modelReady === true && opened?.modelRendered === true && opened.meshCount > 0
      && opened.framesWithModel > 0 && opened.errors.length === 0 && opened.modelUrl.endsWith('/duck.glb'), { stats: opened });
    check('separate3DCanvas', result.canvasWith3D === result.canvasBefore3D + 1 && opened?.separateCanvas === true
      && opened?.sharesOverviewResources === false, { before: result.canvasBefore3D, with3D: result.canvasWith3D });
    check('actual3DBackend', opened?.actualBackend === (threeBackend === 'webgl' ? 'Three WebGLRenderer' : 'Three WebGPUBackend'), { actual: opened?.actualBackend });
    if (!result.checks.realModel.pass) throw new Error('Real glTF model did not load and render cleanly');

    const panel = page.locator('.bench-three canvas');
    const box = await panel.boundingBox();
    if (!box) throw new Error('Model viewer has no visible canvas bounds');
    result.threeScreenshot = resolve(out, `${key}-three.png`);
    await page.screenshot({ path: result.threeScreenshot });
    result.modelScreenshot = resolve(out, `${key}-duck.png`);
    await panel.screenshot({ path: result.modelScreenshot });
    result.modelPixelGate = JSON.parse(execFileSync('uv', ['run', '--with', 'pillow', 'python', '-c',
      'import json,sys\nfrom PIL import Image\nim=Image.open(sys.argv[1]).convert("RGB")\nwarm=sum(1 for r,g,b in im.getdata() if r>110 and g>70 and b<130 and r>b*1.3 and g>b*1.2)\nprint(json.dumps({"pass":warm>=100,"warmDuckPixels":warm,"pixels":im.width*im.height,"meaning":"Coarse yellow Duck presence gate, not full model pixel parity"}))',
      result.modelScreenshot], { encoding: 'utf8' }));
    check('displayedModelPixels', result.modelPixelGate.pass, { screenshot: result.modelScreenshot, gate: result.modelPixelGate });
    const beforeOrbit = await threeStats(page);
    await page.mouse.move(box.x + box.width * 0.5, box.y + box.height * 0.5);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.5 + 70, box.y + box.height * 0.5 + 25, { steps: 12 });
    await page.mouse.up();
    await twoFrames(page);
    const afterOrbit = await threeStats(page);
    const orbitDistance = Math.hypot(...afterOrbit.cameraPosition.map((value, index) => value - beforeOrbit.cameraPosition[index]));
    check('nativeOrbit', afterOrbit.orbitChanges > beforeOrbit.orbitChanges && orbitDistance > 1e-6,
      { before: beforeOrbit, after: afterOrbit, cameraDistance: orbitDistance });

    await page.evaluate(() => window.bench.stop3D());
    await twoFrames(page);
    await page.waitForTimeout(200);
    const pausedBefore = await threeStats(page);
    await page.waitForTimeout(1000);
    const pausedAfter = await threeStats(page);
    check('pausedNoIdleRaf', pausedAfter.frames === pausedBefore.frames && !pausedBefore.playing && !pausedAfter.playing
      && !pausedBefore.rafPending && !pausedAfter.rafPending && !pausedBefore.dirty && !pausedAfter.dirty,
      { intervalMs: 1000, before: pausedBefore, after: pausedAfter });
    await page.evaluate(() => window.bench.start3D());
    await page.waitForTimeout(1000);
    const animated = await threeStats(page);
    check('animatedReal3D', animated.playing && animated.animatedFrames > pausedAfter.animatedFrames
      && animated.framesWithModel > pausedAfter.framesWithModel && animated.errors.length === 0,
      { intervalMs: 1000, before: pausedAfter, after: animated });

    if (overheadMs > 0) {
      result.overhead.animated3D = await measuredTour(page, browserCdp, pageCdp, overheadMs);
      const baseline = result.overhead.baseline;
      const live = result.overhead.animated3D;
      result.overhead.delta = {
        cpuOneCorePercentagePoints: live.cpuOneCorePercent - baseline.cpuOneCorePercent,
        browserCpuSeconds: live.browserCpuSeconds - baseline.browserCpuSeconds,
        settledProcessPssMiB: live.afterMemory.pssMiB - baseline.afterMemory.pssMiB,
        p95FrameMs: live.tour.frameMs.p95 - baseline.tour.frameMs.p95,
      };
    }
    await page.evaluate(() => window.bench.stop3D());
    await twoFrames(page);
    const stoppedBefore = await threeStats(page);
    await page.waitForTimeout(1000);
    const stoppedAfter = await threeStats(page);
    check('stopFreezes3D', stoppedAfter.frames === stoppedBefore.frames && stoppedAfter.animatedFrames === stoppedBefore.animatedFrames
      && !stoppedAfter.playing && !stoppedAfter.rafPending, { intervalMs: 1000, before: stoppedBefore, after: stoppedAfter });
    result.beforeClose3D = stoppedAfter;
    await page.evaluate(() => window.bench.close3D());
    await twoFrames(page);
    const afterClose = await page.evaluate(() => ({
      three: window.bench.sample().three ?? null, canvases: document.querySelectorAll('canvas').length,
      viewerHosts: document.querySelectorAll('.bench-three').length, viewerCanvases: document.querySelectorAll('[data-three-backend]').length,
    }));
    check('closeRemoves3D', afterClose.three === null && afterClose.canvases === result.canvasBefore3D
      && afterClose.viewerHosts === 0 && afterClose.viewerCanvases === 0,
      { beforeCanvasCount: result.canvasBefore3D, after: afterClose, boundary: 'Canvas/host removal and dropped live API stats; does not independently prove driver VRAM reclamation.' });
    result.final = await page.evaluate(() => window.bench.sample());
    result.trustedInputs = await page.evaluate(() => [...window.__featureInputs]);
    result.finalMemory = memory(await snapshot(browserCdp, pageCdp));
    check('noPageErrors', result.pageErrors.length === 0, { errors: [...result.pageErrors] });
    check('noConsoleErrors', result.consoleErrors.length === 0, { errors: [...result.consoleErrors] });
    await page.evaluate(() => window.bench.destroy());
    result.status = Object.values(result.checks).every(value => value.pass) ? 'pass' : 'feature-failed';
  } catch (error) {
    result.status = 'failed';
    result.error = error.stack || String(error);
    try { result.failureSample = await page?.evaluate(() => window.bench?.sample()); } catch {}
    try {
      if (page && !page.isClosed()) {
        result.failureScreenshot = resolve(out, `${key}-failure.png`);
        await page.screenshot({ path: result.failureScreenshot });
      }
    } catch {}
  } finally {
    await browser?.close().catch(error => { result.closeError = String(error); });
    result.finished = new Date().toISOString();
    writeFileSync(resolve(out, `${key}.json`), `${JSON.stringify(result, null, 2)}\n`);
    const row = { key, mode, status: result.status, checks: Object.fromEntries(Object.entries(result.checks).map(([name, value]) => [name, value.pass])), error: result.error };
    summary.push(row);
    writeFileSync(resolve(out, `summary-${runId}.json`), `${JSON.stringify(summary, null, 2)}\n`);
    console.log(JSON.stringify(row));
  }
}
if (summary.some(result => result.status !== 'pass')) process.exitCode = 1;
