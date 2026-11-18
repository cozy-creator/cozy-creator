// PROBE (cl-1215's follow-up; this branch only, never merged). Whether a video can keep a plain URL and still hold no connection: Calcifer
// answers a Range request with at most `probe_cap` bytes (the true total in Content-Range) and the answer completes; the element asks for
// the next range when it wants it. Measured against today's two ways, an open-ended Range answer and the whole file in the page's memory:
// where the whole-file read's time goes (by `probe_piece`, how much Calcifer reads out at once), first frame and seeks, nine videos
// playing at once in three windows (more than the six connections a browser opens) with the page's calls timed, and Chrome's memory.
const { createHash } = require('crypto');
const { readFileSync } = require('fs');
const { test, expect } = require('./fixtures');
const { ffmpeg } = require('./hands');

const MB = 1 << 20;
const CAPS = [null, 4 * MB, MB, 256 << 10];
const named = (cap) => (cap === null ? 'whole remainder' : cap >= MB ? `${cap / MB} MiB` : `${cap >> 10} KiB`);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/** A window's requests for a video file: how many, and how many are open at once, now and at most. */
function watch(page) {
  const open = new Set();
  const seen = { requests: 0, most: 0, finished: 0, failed: 0 };
  const media = (url) => /\/files\/[0-9a-f]{64}\.mp4/.test(url);
  page.on('request', (request) => {
    if (!media(request.url())) return;
    seen.requests++;
    open.add(request);
    seen.most = Math.max(seen.most, open.size);
  });
  page.on('requestfinished', (request) => { if (open.delete(request)) seen.finished++; });
  page.on('requestfailed', (request) => { if (open.delete(request)) seen.failed++; });
  return { now: () => ({ ...seen, open: open.size }), reset: () => Object.assign(seen, { requests: 0, finished: 0, failed: 0, most: open.size }) };
}

/** One of the page's own calls, timed; given up at 5 s. */
const call = (page, id) => page.evaluate(async (id) => {
  const began = performance.now();
  try {
    const answer = await fetch(`/v1/projects/${encodeURIComponent(id)}?assets=false`, { cache: 'no-store', signal: AbortSignal.timeout(5000) });
    await answer.text();
    return answer.status === 200 ? Math.round(performance.now() - began) : `status ${answer.status}`;
  } catch (error) {
    return `none in ${Math.round(performance.now() - began)} ms (${error.name})`;
  }
}, id);

/** The whole file read as the page reads it, then played from its blob: where the time goes. */
const anatomy = (page, url, how) => page.evaluate(async ([url, how]) => {
  const t0 = performance.now();
  const answer = await fetch(url, { cache: 'no-store' });
  const t1 = performance.now();
  const body = how === 'blob' ? await answer.blob() : await answer.arrayBuffer();
  const t2 = performance.now();
  const size = how === 'blob' ? body.size : body.byteLength;
  const out = { how, mb: Number((size / 1e6).toFixed(1)), headers_ms: Math.round(t1 - t0), body_ms: Math.round(t2 - t1), mb_per_s: Math.round(size / 1e6 / ((t2 - t0) / 1000)) };
  if (how !== 'blob') return out;
  const blobUrl = URL.createObjectURL(body);
  const el = document.createElement('video');
  Object.assign(el, { muted: true, preload: 'auto', playsInline: true });
  const marks = await new Promise((done) => {
    let metadata = null;
    el.addEventListener('loadedmetadata', () => { metadata = performance.now(); }, { once: true });
    el.addEventListener('loadeddata', () => done({ metadata, first: performance.now() }), { once: true });
    el.addEventListener('error', () => done({ error: `error ${el.error?.code}` }), { once: true });
    setTimeout(() => done({ error: 'no frame in 60 s' }), 60_000);
    el.src = blobUrl;
  });
  el.removeAttribute('src');
  el.load();
  URL.revokeObjectURL(blobUrl);
  return { ...out, element_metadata_ms: marks.metadata ? Math.round(marks.metadata - t2) : null, element_first_frame_ms: marks.first ? Math.round(marks.first - t2) : null, ...(marks.error ? { element: marks.error } : {}) };
}, [url, how]);

/** One `<video>` on a URL (or, `viaBlob`, on the whole file read first): its first frame, then five seeks. */
const plain = (page, url, viaBlob) => page.evaluate(async ([url, viaBlob]) => {
  const out = {};
  let src = url, blobUrl = null;
  const began = performance.now();
  if (viaBlob) {
    blobUrl = src = URL.createObjectURL(await (await fetch(url, { cache: 'no-store' })).blob());
    out.read_ms = Math.round(performance.now() - began);
  }
  const el = document.createElement('video');
  Object.assign(el, { muted: true, preload: 'auto', playsInline: true });
  el.style.cssText = 'position:fixed;left:0;bottom:0;width:160px;height:90px;z-index:2147483647';
  document.body.append(el);
  const once = (name, ms) => new Promise((done) => {
    const end = (how) => { el.removeEventListener(name, ok); el.removeEventListener('error', bad); clearTimeout(timer); done(how); };
    const ok = () => end('ok');
    const bad = () => end(`error ${el.error?.code}`);
    const timer = setTimeout(() => end(`none in ${ms / 1000} s`), ms);
    el.addEventListener(name, ok);
    el.addEventListener('error', bad);
  });
  const first = once('loadeddata', 30_000);
  el.src = src;
  const how = await first;
  out.first_frame_ms = how === 'ok' ? Math.round(performance.now() - began) : how;
  out.seeks_ms = [];
  if (how === 'ok') {
    for (const part of [0.25, 0.75, 0.95, 0.05, 0.5]) {
      const at = performance.now();
      const seeked = once('seeked', 20_000);
      el.currentTime = el.duration * part;
      const landed = await seeked;
      out.seeks_ms.push(landed === 'ok' ? Math.round(performance.now() - at) : landed);
    }
  }
  el.pause();
  el.removeAttribute('src');
  el.load();
  el.remove();
  if (blobUrl) URL.revokeObjectURL(blobUrl);
  return out;
}, [url, viaBlob]);

const start = (page, urls, viaBlob = false) => page.evaluate(async ([urls, viaBlob]) => {
  window.__blobs = [];
  window.__videos = [];
  for (const [i, url] of urls.entries()) {
    let src = url;
    if (viaBlob) {
      src = URL.createObjectURL(await (await fetch(url, { cache: 'no-store' })).blob());
      window.__blobs.push(src);
    }
    const el = document.createElement('video');
    Object.assign(el, { muted: true, loop: true, preload: 'auto', playsInline: true });
    el.style.cssText = `position:fixed;left:${i * 170}px;bottom:0;width:160px;height:90px;z-index:2147483647`;
    document.body.append(el);
    el.src = src;
    el.play().catch(() => {});
    window.__videos.push(el);
  }
}, [urls, viaBlob]);
const read = (page) => page.evaluate(() => (window.__videos ?? []).map((el) => ({ at: Number(el.currentTime.toFixed(2)), paused: el.paused, ready: el.readyState, error: el.error?.code ?? null })));
const pauseAll = (page) => page.evaluate(() => (window.__videos ?? []).forEach((el) => el.pause()));
const stop = (page) => page.evaluate(() => {
  for (const el of window.__videos ?? []) { el.pause(); el.removeAttribute('src'); el.load(); el.remove(); }
  for (const url of window.__blobs ?? []) URL.revokeObjectURL(url);
  window.__videos = [];
  window.__blobs = [];
});

/** Chrome's memory, every process: resident and anonymous. Null in another browser. */
async function chromeMemory(page) {
  try {
    const session = await page.context().browser().newBrowserCDPSession();
    const { processInfo } = await session.send('SystemInfo.getProcessInfo');
    await session.detach();
    const sum = { rss: 0, anon: 0 };
    for (const one of processInfo) {
      let status;
      try { status = readFileSync(`/proc/${one.id}/status`, 'utf8'); } catch { continue; }
      const kb = (field) => Number(new RegExp(`^${field}:\\s+(\\d+) kB`, 'm').exec(status)?.[1] ?? 0);
      sum.rss += kb('VmRSS');
      sum.anon += kb('RssAnon');
    }
    return { rss_mb: Math.round(sum.rss / 1024), anon_mb: Math.round(sum.anon / 1024) };
  } catch {
    return null;
  }
}

test('ranges: probe', { tag: '@engines' }, async ({ page, project, context, signIn, guards, browserName }) => {
  test.setTimeout(1_200_000);
  guards.allow(/^console error:/, /^page error:/);
  const out = {};
  const say = (key, value) => { out[key] = value; console.log(`ranges, probe ${browserName}, ${key}: ${JSON.stringify(value)}`); };
  const step = async (name, body) => {
    try { await test.step(name, body); } catch (error) { say(`${name}, FAILED`, String(error.message).slice(0, 400)); }
  };
  const make = (name, size, seconds, kbps, seed) => {
    const path = test.info().outputPath(`${name}.mp4`);
    ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=${size}:rate=24,noise=alls=20:allf=t:all_seed=${seed}`, '-t', String(seconds), '-c:v', 'libx264', '-preset', 'ultrafast',
      '-b:v', `${kbps}k`, '-minrate', `${kbps}k`, '-maxrate', `${kbps}k`, '-bufsize', `${kbps}k`, '-g', '24', '-pix_fmt', 'yuv420p', path);
    return readFileSync(path);
  };
  let fresh = 0;
  const files = {};
  const add = async (name, bytes, i) => {
    await project.import(bytes, { name, at: { x: (i % 4) * 520, y: Math.floor(i / 4) * 420 } });
    files[name] = { mb: Number((bytes.length / 1e6).toFixed(1)), path: `/v1/projects/${encodeURIComponent(project.id)}/files/${createHash('sha256').update(bytes).digest('hex')}.mp4` };
  };
  /** A file's address, new each time: no cache answers it, and no two elements share its bytes. */
  const url = (name) => `${files[name].path}?n=${++fresh}`;
  const clips = Array.from({ length: 9 }, (_, i) => `clip${i}`);
  for (const [i, name] of clips.entries()) await add(name, make(name, '640x360', 40, 2000, i + 1), i);
  await add('big', make('big', '1280x720', 20, 50_000, 99), 9);
  say('files', Object.fromEntries(Object.entries(files).map(([name, file]) => [name, `${file.mb} MB`])));
  for (const name of Object.keys(files)) {
    await expect.poll(() => page.evaluate((path) => fetch(path, { method: 'HEAD' }).then((answer) => answer.status, () => 0), files[name].path), { message: `${name} served`, timeout: 60_000 }).toBe(200);
  }
  // Their thumbnails and previews made, off the measurements.
  await sleep(15_000);
  const knob = (name, value) => page.evaluate(([name, value]) => {
    document.cookie = value === null ? `${name}=; Max-Age=0; path=/` : `${name}=${value}; path=/`;
  }, [name, value]);

  await step('what a capped Range answer says', async () => {
    const asked = async (cap) => {
      await knob('probe_cap', cap);
      return page.evaluate((path) => fetch(path, { headers: { Range: 'bytes=0-' }, cache: 'no-store' }).then(async (answer) => ({ status: answer.status, range: answer.headers.get('Content-Range'), bytes: (await answer.arrayBuffer()).byteLength })), url('clip0'));
    };
    say('a Range answer', { whole: await asked(null), capped_1MiB: await asked(MB) });
    await knob('probe_cap', null);
  });

  await step('the whole-file read: where its time goes, by how much Calcifer reads out at once', async () => {
    const rows = [];
    for (const piece of [null, 64 << 10, 256 << 10, MB]) {
      await knob('probe_piece', piece);
      for (const name of ['clip0', 'big']) rows.push({ piece: piece === null ? '4 KiB (today)' : named(piece), file: name, ...(await anatomy(page, url(name), 'blob')) });
      rows.push({ piece: piece === null ? '4 KiB (today)' : named(piece), file: 'big', ...(await anatomy(page, url('big'), 'buffer')) });
    }
    await knob('probe_piece', null);
    say('whole-file read', rows);
  });

  await step('first frame and seeks: a plain URL by cap, and the whole file in memory', async () => {
    const rows = [];
    for (const cap of CAPS) {
      await knob('probe_cap', cap);
      for (const name of ['clip0', 'big']) rows.push({ way: `URL, ${named(cap)}`, file: name, ...(await plain(page, url(name), false)) });
    }
    await knob('probe_cap', null);
    for (const name of ['clip0', 'big']) rows.push({ way: 'memory', file: name, ...(await plain(page, url(name), true)) });
    say('first frame and seeks', rows);
  });

  const windows = [{ page }];
  await step('nine videos at once in three windows, by cap: do all play, and do calls answer', async () => {
    for (const one of windows.concat([{}, {}]).slice(windows.length)) {
      one.page = await context.newPage();
      await signIn(one.page, project.url());
      await expect(one.page.locator('.stage')).toBeVisible();
      windows.push(one);
    }
    for (const one of windows) one.media = watch(one.page);
    const rows = [];
    for (const way of [...CAPS.map((cap) => ({ cap })), { memory: true }]) {
      await knob('probe_cap', way.memory ? null : way.cap);
      for (const [i, one] of windows.entries()) {
        one.media.reset();
        one.calls = [];
        one.moved = [0, 0, 0];
        await start(one.page, clips.slice(i * 3, i * 3 + 3).map(url), Boolean(way.memory));
        one.was = await read(one.page);
      }
      for (let round = 0; round < 5; round++) {
        await sleep(3300);
        for (const one of windows) {
          const now = await read(one.page);
          now.forEach((video, n) => { if (!video.paused && Math.abs(video.at - one.was[n].at) > 0.05) one.moved[n]++; });
          one.was = now;
          one.calls.push(await call(one.page, project.id));
        }
      }
      const playing = windows.map((one) => ({ videos_moving_of_5_looks: one.moved, errors: one.was.map((video) => video.error).filter(Boolean), calls: one.calls, media: one.media.now() }));
      for (const one of windows) await pauseAll(one.page);
      await sleep(5000);
      const paused = [];
      for (const one of windows) paused.push({ open: one.media.now().open, call: await call(one.page, project.id) });
      for (const one of windows) await stop(one.page);
      await sleep(1500);
      rows.push({ way: way.memory ? 'memory' : `URL, ${named(way.cap)}`, playing, paused_5_s: paused });
      say(`nine at once, ${way.memory ? 'memory' : `URL, ${named(way.cap)}`}`, rows.at(-1));
    }
    await knob('probe_cap', null);
  });

  await step("Chrome's memory: a large file playing from a URL by cap, and from memory", async () => {
    if (!(await chromeMemory(page))) return say('memory', 'not measured in this browser');
    const rows = [];
    for (const way of [{ cap: null }, { cap: MB }, { memory: true }]) {
      await knob('probe_cap', way.memory ? null : way.cap);
      await sleep(3000);
      const rest = await chromeMemory(page);
      await start(page, [url('big')], Boolean(way.memory));
      await sleep(8000);
      const playing = { ...(await chromeMemory(page)), video: (await read(page))[0] };
      await stop(page);
      await sleep(4000);
      rows.push({ way: way.memory ? 'memory' : `URL, ${named(way.cap)}`, rest, playing, after: await chromeMemory(page) });
    }
    await knob('probe_cap', null);
    say('memory, 127 MB playing 8 s', rows);
  });

  await step('the link at 1 MB/s (Chrome): first frame and seeks by cap', async () => {
    if (browserName !== 'chromium') return say('1 MB/s', 'not measured in this browser');
    const cdp = await context.newCDPSession(page);
    await cdp.send('Network.enable');
    await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 20, downloadThroughput: 1_000_000, uploadThroughput: 1_000_000 });
    const rows = [];
    for (const cap of CAPS) {
      await knob('probe_cap', cap);
      rows.push({ way: `URL, ${named(cap)}`, file: 'clip0', ...(await plain(page, url('clip0'), false)) });
    }
    await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1 });
    await knob('probe_cap', null);
    say('1 MB/s, first frame and seeks', rows);
  });

  test.info().annotations.push({ type: 'probe', description: JSON.stringify(out) });
  expect(Object.keys(out).length, 'something measured').toBeGreaterThan(1);
});
