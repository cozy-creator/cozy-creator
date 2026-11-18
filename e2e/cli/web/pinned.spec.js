// PROBE (cl-1220; this branch only, never merged). Whether a capped Range answer frees its connection in the steady state: twelve videos
// of one 120 MB file (each its own address) playing at once in three windows, twice the six connections a browser opens to Calcifer, a
// file too large to buffer whole. By cap: do all twelve play, do the page's calls answer (one a second in each window for 30 s), and how
// many of the browser's sockets to Calcifer hold unread bytes (`ss`), playing and after 6 s paused.
const { createHash } = require('crypto');
const { execFileSync } = require('child_process');
const { readFileSync } = require('fs');
const { test, expect } = require('./fixtures');
const { ffmpeg } = require('./hands');

const MB = 1 << 20;
const CAPS = [null, 4 * MB, 2 * MB, MB, 512 << 10];
const named = (cap) => (cap === null ? 'whole remainder' : cap >= MB ? `${cap / MB} MiB` : `${cap >> 10} KiB`);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/** The sockets open to Calcifer's port: the browser's, and of them those holding bytes it hasn't read; Calcifer's with bytes it can't send. */
function sockets(port) {
  try {
    const lines = execFileSync('ss', ['-H', '-t', '-n', '-p', 'state', 'established'], { encoding: 'utf8' }).split('\n');
    const seen = { browser: 0, browser_unread: 0, unread_kb: 0, calcifer_unsent: 0, unsent_kb: 0 };
    for (const line of lines) {
      const parts = line.trim().split(/\s+/);
      const at = parts.findIndex((part) => /:\d+$/.test(part));
      if (at < 2 || !/:\d+$/.test(parts[at + 1] ?? '')) continue;
      const [recv, send] = [Number(parts[at - 2]), Number(parts[at - 1])];
      const [local, peer] = [parts[at].split(':').pop(), parts[at + 1].split(':').pop()];
      if (peer === port && /chrome|firefox|WebKit|MiniBrowser/i.test(line)) {
        seen.browser++;
        if (recv > 0) { seen.browser_unread++; seen.unread_kb += Math.round(recv / 1024); }
      }
      if (local === port && send > 0) { seen.calcifer_unsent++; seen.unsent_kb += Math.round(send / 1024); }
    }
    return seen;
  } catch (error) {
    return { error: String(error.message).slice(0, 120) };
  }
}

const call = (page, id) => page.evaluate(async (id) => {
  const began = performance.now();
  try {
    const answer = await fetch(`/v1/projects/${encodeURIComponent(id)}?assets=false`, { cache: 'no-store', signal: AbortSignal.timeout(2000) });
    await answer.text();
    return Math.round(performance.now() - began);
  } catch {
    return -1;
  }
}, id);
const start = (page, urls) => page.evaluate((urls) => {
  window.__videos = urls.map((url, i) => {
    const el = document.createElement('video');
    Object.assign(el, { muted: true, loop: true, preload: 'auto', playsInline: true });
    el.style.cssText = `position:fixed;left:${i * 170}px;bottom:0;width:160px;height:90px;z-index:2147483647`;
    document.body.append(el);
    el.src = url;
    el.play().catch(() => {});
    return el;
  });
}, urls);
const read = (page) => page.evaluate(() => (window.__videos ?? []).map((el) => ({ at: Number(el.currentTime.toFixed(2)), paused: el.paused, ready: el.readyState, error: el.error?.code ?? null,
  ahead: el.buffered.length ? Math.round(el.buffered.end(el.buffered.length - 1) - el.currentTime) : 0 })));
const pauseAll = (page) => page.evaluate(() => (window.__videos ?? []).forEach((el) => el.pause()));
const stop = (page) => page.evaluate(() => {
  for (const el of window.__videos ?? []) { el.pause(); el.removeAttribute('src'); el.load(); el.remove(); }
  window.__videos = [];
});

test('pinned: probe', { tag: '@engines' }, async ({ page, project, context, signIn, guards, browserName }) => {
  test.setTimeout(1_200_000);
  guards.allow(/^console error:/, /^page error:/);
  const say = (key, value) => console.log(`pinned, probe ${browserName}, ${key}: ${JSON.stringify(value)}`);
  const path = test.info().outputPath('long.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=24,noise=alls=20:allf=t', '-t', '120', '-c:v', 'libx264', '-preset', 'ultrafast',
    '-b:v', '8000k', '-minrate', '8000k', '-maxrate', '8000k', '-bufsize', '8000k', '-g', '24', '-pix_fmt', 'yuv420p', path);
  const bytes = readFileSync(path);
  await project.import(bytes, { name: 'long', at: { x: 0, y: 0 } });
  const file = `/v1/projects/${encodeURIComponent(project.id)}/files/${createHash('sha256').update(bytes).digest('hex')}.mp4`;
  await expect.poll(() => page.evaluate((file) => fetch(file, { method: 'HEAD' }).then((answer) => answer.status, () => 0), file), { message: 'long served', timeout: 120_000 }).toBe(200);
  const port = new URL(project.url()).port;
  say('file', { mb: Number((bytes.length / 1e6).toFixed(1)), seconds: 120, port, sockets_at_rest: sockets(port) });
  await sleep(15_000);
  const windows = [{ page }];
  for (let i = 0; i < 2; i++) {
    const one = { page: await context.newPage() };
    await signIn(one.page, project.url());
    await expect(one.page.locator('.stage')).toBeVisible();
    windows.push(one);
  }
  let fresh = 0;
  const knob = (name, value) => page.evaluate(([name, value]) => {
    document.cookie = value === null ? `${name}=; Max-Age=0; path=/` : `${name}=${value}; path=/`;
  }, [name, value]);
  await knob('probe_piece', 256 << 10);

  for (const cap of CAPS) {
    try {
      await knob('probe_cap', cap);
      for (const one of windows) {
        one.calls = [];
        one.moved = [0, 0, 0, 0];
        await start(one.page, Array.from({ length: 4 }, () => `${file}?n=${++fresh}`));
        one.was = await read(one.page);
      }
      const seen = [];
      const began = Date.now();
      for (let second = 1; Date.now() - began < 30_000; second++) {
        await sleep(Math.max(0, began + second * 1000 - Date.now()));
        for (const one of windows) one.calls.push(await call(one.page, project.id));
        if (second % 5 === 0) {
          for (const one of windows) {
            const now = await read(one.page);
            now.forEach((video, n) => { if (!video.paused && Math.abs(video.at - one.was[n].at) > 0.05) one.moved[n]++; });
            one.was = now;
          }
          seen.push(sockets(port));
        }
      }
      const looks = seen.length;
      const playing = windows.map((one) => ({
        videos_moving_of_looks: `${one.moved.join(',')} of ${looks}`, ahead_s: one.was.map((video) => video.ahead), errors: one.was.map((video) => video.error).filter(Boolean),
        calls: one.calls.length, unanswered_in_2_s: one.calls.filter((ms) => ms < 0).length, over_500_ms: one.calls.filter((ms) => ms > 500).length, slowest_ms: Math.max(...one.calls),
      }));
      for (const one of windows) await pauseAll(one.page);
      await sleep(6000);
      const paused = { sockets: sockets(port), calls: [] };
      for (const one of windows) paused.calls.push(await call(one.page, project.id));
      for (const one of windows) await stop(one.page);
      await sleep(2500);
      say(`twelve at once, ${named(cap)}`, { playing, sockets_playing: seen, paused_6_s: paused, sockets_after_stop: sockets(port) });
    } catch (error) {
      say(`twelve at once, ${named(cap)}, FAILED`, String(error.message).slice(0, 400));
      for (const one of windows) await stop(one.page).catch(() => {});
    }
  }
});
