// pieces (cl-1220): twelve videos playing at once in three windows of one browser, its calls timed and its sockets to Calcifer read.
// What it guards: a player's URL asks Calcifer for its ranges in pieces (`?pieces=true`, Transport.media), each answer a megabyte at most
// with the true total in `Content-Range`. No connection then stays open between pieces, but only because a browser (1) asks on from where
// a shorter 206 ended, and (2) takes no more than about 2 MiB off the socket of an element that has stopped reading (Chrome's network
// process; a megabyte leaves a factor of two). No standard promises either. If a browser changes either, twelve videos hold the six
// connections it opens to Calcifer and the page's calls stop answering, as the whole remainder did (44243310584: the page dead in Chrome,
// 5 of 12 playing in Firefox). It fails on a video that doesn't move, a call over a second, or one of this browser's sockets to Calcifer
// holding bytes 6 s after a pause; and a range asked without `pieces` (a download resuming) must come whole. Chrome then Firefox
// (`@engines`), the job to itself (web.sh's lane `media`).
const { readFileSync } = require('fs');
const { test, expect, focus, canvasVisible } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');
const { sockets } = require('./sockets');

const MB = 1 << 20;
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/** One of the page's own calls, timed: -1 if no answer in 2 s. */
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

/** `urls` each in a `<video>` along the page's bottom, muted and looping, played at once. */
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
const clocks = (page) => page.evaluate(() => window.__videos.map((el) => ({ at: el.currentTime, paused: el.paused, error: el.error?.code ?? null })));

/** What a range of `url` answers: its status, `Content-Range` and how many bytes came, counted as they come. */
const ranged = (page, url, range) => page.evaluate(async ([url, range]) => {
  const answer = await fetch(url, { headers: { Range: range }, cache: 'no-store' });
  let bytes = 0;
  for (const reader = answer.body.getReader(); ;) {
    const { done, value } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
  }
  return { status: answer.status, range: answer.headers.get('Content-Range'), bytes };
}, [url, range]);

test('pieces', { tag: '@engines' }, async ({ page, project, context, signIn, browserName }) => {
  test.setTimeout(600_000);
  // 120 s at a constant 8 Mbit/s (filler where the picture needs less), its index at its end: the bytes of a real take, cheap to decode.
  const clip = test.info().outputPath('long.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=24', '-t', '120', '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '24',
    '-b:v', '8M', '-minrate', '8M', '-maxrate', '8M', '-bufsize', '2M', '-x264-params', 'nal-hrd=cbr:force-cfr=1', '-pix_fmt', 'yuv420p', clip);
  const bytes = readFileSync(clip);
  expect(bytes.length, 'its file: 120 s at 8 Mbit/s').toBeGreaterThan(100e6);
  await project.import(bytes, { name: 'long', at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'long'), { timeout: 120_000 }).toBe(true);
  await fit(page);

  // The URL the page gives its own player in Focus, each of the twelve at an address of its own so none shares another's bytes.
  await focus(page, 'long');
  const live = page.getByRole('region', { name: 'Focused asset' }).locator('video.live');
  await expect(live).toHaveCount(1, { timeout: 60_000 });
  const played = new URL(await live.getAttribute('src'), project.url());
  expect(played.searchParams.get('pieces'), `the page's player asks Calcifer for pieces: ${played.pathname}${played.search}`).toBe('true');
  await leave(page);
  const port = played.port;
  const address = (n) => { const one = new URL(played); one.searchParams.set('n', String(n)); return one.pathname + one.search; };

  await test.step('a range asked in pieces comes a megabyte at a time; one asked without, as a download resuming, comes whole', async () => {
    const whole = new URL(played);
    whole.searchParams.delete('pieces');
    expect.soft(await ranged(page, address(0), 'bytes=0-'), 'in pieces').toEqual({ status: 206, range: `bytes 0-${MB - 1}/${bytes.length}`, bytes: MB });
    expect.soft(await ranged(page, whole.pathname + whole.search, `bytes=${MB}-`), 'whole').toEqual({ status: 206, range: `bytes ${MB}-${bytes.length - 1}/${bytes.length}`, bytes: bytes.length - MB });
  });

  await test.step('twelve playing in three windows: every one moves, every call answers within a second, no socket holds bytes once paused', async () => {
    // Every window open before any plays: one that came after could be the one a pinned connection starves.
    const windows = [page];
    for (let i = 0; i < 2; i++) {
      const tab = await context.newPage();
      await signIn(tab, project.url());
      await expect(tab.locator('.stage')).toBeVisible();
      windows.push(tab);
    }
    for (const [w, tab] of windows.entries()) await start(tab, [1, 2, 3, 4].map((n) => address(w * 4 + n)));
    const calls = windows.map(() => []), stuck = [], seen = [];
    let was = await Promise.all(windows.map(clocks));
    const began = Date.now();
    for (let second = 1; Date.now() - began < 30_000; second++) {
      await sleep(Math.max(0, began + second * 1000 - Date.now()));
      for (const [w, tab] of windows.entries()) calls[w].push(await call(tab, project.id));
      if (second % 5) continue;
      const now = await Promise.all(windows.map(clocks));
      now.forEach((videos, w) => videos.forEach((video, n) => {
        if (video.paused || video.error || video.at <= was[w][n].at) stuck.push(`window ${w + 1} video ${n + 1} at ${second} s: ${JSON.stringify(video)}`);
      }));
      was = now;
      seen.push(sockets(port));
    }
    for (const tab of windows) await tab.evaluate(() => window.__videos.forEach((el) => el.pause()));
    await sleep(6000);
    const paused = sockets(port);
    const after = [];
    for (const tab of windows) after.push(await call(tab, project.id));
    console.log(`NOTE pieces ${browserName}: calls ms ${JSON.stringify(calls.map((ms) => Math.max(...ms)))} slowest by window, after the pause ${JSON.stringify(after)}; sockets playing ${JSON.stringify(seen)}, paused 6 s ${JSON.stringify(paused)}`);
    expect.soft(stuck, 'every video moves at every look').toEqual([]);
    expect.soft(calls.flat().filter((ms) => ms < 0 || ms > 1000), 'every call while they play answers within a second').toEqual([]);
    expect.soft(after.filter((ms) => ms < 0 || ms > 1000), 'every call once they pause answers within a second').toEqual([]);
    expect.soft(Math.max(...seen.map((one) => one.open)), "this browser's sockets to Calcifer are seen").toBeGreaterThan(0);
    expect.soft({ unread: paused.unread, unsent: paused.unsent }, 'no socket holds bytes 6 s after a pause').toEqual({ unread: 0, unsent: 0 });
    for (const tab of windows) await tab.evaluate(() => window.__videos.forEach((el) => { el.removeAttribute('src'); el.load(); el.remove(); }));
    for (const tab of windows.slice(1)) await tab.close();
  });
});
