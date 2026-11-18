// PROBE (cl-1220; this branch only, never merged). The app against itself: Focus opened on a video with its element on the whole file in
// the page's memory (cl-1215), and on its URL as before it (`__probeUrl`), each by how much Calcifer reads out at once (`probe_piece`)
// and the most a Range answer carries (`probe_cap`). From the press that opens Focus to the video's first frame, three times each, the
// HTTP cache off: Focus's own cost is in every row, so the rows differ only by how the video's bytes come.
const { readFileSync, statSync } = require('fs');
const { test, expect, focus, canvasVisible } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

const WAYS = [
  { name: 'memory, 4 KiB pieces (merged)', url: false, piece: null, cap: null },
  { name: 'memory, 256 KiB pieces', url: false, piece: 256 << 10, cap: null },
  { name: 'URL, whole remainder, 4 KiB pieces (before cl-1215)', url: true, piece: null, cap: null },
  { name: 'URL, whole remainder, 256 KiB pieces', url: true, piece: 256 << 10, cap: null },
  { name: 'URL, 1 MiB answers, 256 KiB pieces', url: true, piece: 256 << 10, cap: 1 << 20 },
  { name: 'URL, 256 KiB answers, 256 KiB pieces', url: true, piece: 256 << 10, cap: 256 << 10 },
];

test('first-frame: probe', async ({ page, project, guards }) => {
  test.setTimeout(900_000);
  guards.allow(/^console error:/);
  const out = {};
  const say = (key, value) => { out[key] = value; console.log(`first-frame, probe ${key}: ${JSON.stringify(value)}`); };
  const clips = {
    tiny: ['testsrc2=size=640x360:rate=24', '10', ['-preset', 'veryfast']],
    small: ['mandelbrot=size=640x360:rate=24', '40', ['-preset', 'veryfast']],
    big: ['testsrc2=size=1280x720:rate=24,noise=alls=60:allf=t', '20', ['-preset', 'ultrafast', '-b:v', '50M', '-minrate', '50M', '-maxrate', '50M', '-bufsize', '50M']],
  };
  const sizes = {};
  for (const [i, [name, [source, seconds, encode]]] of Object.entries(clips).entries()) {
    const path = test.info().outputPath(`${name}.mp4`);
    ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', source, '-t', seconds, '-c:v', 'libx264', ...encode, '-pix_fmt', 'yuv420p', path);
    await project.import(readFileSync(path), { name, at: { x: i * 520, y: 0 } });
    sizes[name] = `${(statSync(path).size / 1e6).toFixed(1)} MB`;
  }
  say('files', sizes);
  await page.setViewportSize({ width: 1280, height: 800 });
  for (const name of Object.keys(clips)) await expect.poll(() => canvasVisible(page, name), { timeout: 120_000 }).toBe(true);
  await fit(page);
  // Their thumbnails and previews made, off the measurements.
  await page.waitForTimeout(10_000);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.enable');
  await cdp.send('Network.setCacheDisabled', { cacheDisabled: true });
  const knobs = (way) => page.evaluate((way) => {
    const cookie = (name, value) => { document.cookie = value === null ? `${name}=; Max-Age=0; path=/` : `${name}=${value}; path=/`; };
    cookie('probe_piece', way.piece);
    cookie('probe_cap', way.cap);
    window.__probeUrl = way.url;
    window.__probe = { press: null, start: null, first: null };
    if (window.__probeOn) return;
    window.__probeOn = true;
    const live = (event) => event.target instanceof HTMLVideoElement && event.target.matches('video.live');
    document.addEventListener('pointerdown', () => { if (window.__probe.first === null) window.__probe.press = performance.now(); }, true);
    document.addEventListener('loadstart', (event) => { if (live(event) && event.target.getAttribute('src')) window.__probe.start ??= performance.now(); }, true);
    document.addEventListener('loadeddata', (event) => { if (live(event)) window.__probe.first ??= performance.now(); }, true);
  }, way);
  const region = page.getByRole('region', { name: 'Focused asset' });

  for (const way of WAYS) {
    const rows = {};
    for (const name of Object.keys(clips)) {
      const opens = [];
      for (let again = 0; again < 3; again++) {
        await knobs(way);
        await focus(page, name);
        await expect.poll(() => page.evaluate(() => window.__probe.first), { message: `${way.name}: ${name}'s first frame`, timeout: 120_000 }).not.toBeNull();
        const { press, start, first } = await page.evaluate(() => window.__probe);
        const src = await region.locator('video.live').evaluate((el) => el.currentSrc.slice(0, 5));
        opens.push({ press_to_first_frame: Math.round(first - press), press_to_source: start === null ? null : Math.round(start - press), source_to_first_frame: start === null ? null : Math.round(first - start), src });
        await leave(page);
      }
      rows[name] = { press_to_first_frame_ms: opens.map((one) => one.press_to_first_frame), press_to_source_ms: opens.map((one) => one.press_to_source), source_to_first_frame_ms: opens.map((one) => one.source_to_first_frame), src: opens[0].src };
    }
    say(way.name, rows);
  }
  test.info().annotations.push({ type: 'probe', description: JSON.stringify(out) });
});
