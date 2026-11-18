// PROBE (cl-1220; this branch only, never merged). Capped Range answers in each browser, one cell for Chrome, Firefox and WebKit: a
// 120 MB video's first frame and seeks by cap and from memory; twelve of it playing at once in three windows (do all play, do the page's
// calls answer, how many sockets to Calcifer hold unread bytes, playing and paused); and a file still landing, where the element asks
// past the bytes in place and Calcifer holds the request, refuses it 416, or refuses it 503: does it stall and go on by itself.
const { createHash } = require('crypto');
const { execFileSync } = require('child_process');
const { readFileSync } = require('fs');
const { test, expect } = require('./fixtures');
const { ffmpeg } = require('./hands');

const MB = 1 << 20;
const named = (cap) => (cap === null ? 'whole remainder' : cap >= MB ? `${cap / MB} MiB` : `${cap >> 10} KiB`);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

/** The sockets open to Calcifer's port: its clients', and of them those holding bytes not yet read; Calcifer's with bytes it can't send. */
function sockets(port) {
  const seen = { clients: 0, clients_unread: 0, unread_kb: 0, calcifer_unsent: 0, unsent_kb: 0 };
  const count = (recv, send, local, peer) => {
    if (peer === port) {
      seen.clients++;
      if (recv > 0) { seen.clients_unread++; seen.unread_kb += Math.round(recv / 1024); }
    }
    if (local === port && send > 0) { seen.calcifer_unsent++; seen.unsent_kb += Math.round(send / 1024); }
  };
  try {
    if (process.platform === 'darwin') {
      for (const line of execFileSync('netstat', ['-anp', 'tcp'], { encoding: 'utf8' }).split('\n')) {
        const parts = line.trim().split(/\s+/);
        if (!/^tcp/.test(parts[0] ?? '') || parts[5] !== 'ESTABLISHED') continue;
        count(Number(parts[1]), Number(parts[2]), parts[3].split('.').pop(), parts[4].split('.').pop());
      }
    } else {
      for (const line of execFileSync('ss', ['-H', '-t', '-n', 'state', 'established'], { encoding: 'utf8' }).split('\n')) {
        const parts = line.trim().split(/\s+/);
        const at = parts.findIndex((part) => /:\d+$/.test(part));
        if (at < 2 || !/:\d+$/.test(parts[at + 1] ?? '')) continue;
        count(Number(parts[at - 2]), Number(parts[at - 1]), parts[at].split(':').pop(), parts[at + 1].split(':').pop());
      }
    }
    return seen;
  } catch (error) {
    return { error: String(error.message).slice(0, 120) };
  }
}

/** 120 s of video at about 8 Mbit/s, its index at its end: with whichever encoder this computer's ffmpeg has. */
async function long(path) {
  const ways = [
    ['-i', 'testsrc2=size=640x360:rate=24,noise=alls=20:allf=t', '-t', '120', '-c:v', 'libx264', '-preset', 'ultrafast', '-b:v', '8000k', '-minrate', '8000k', '-maxrate', '8000k', '-bufsize', '8000k', '-g', '24'],
    ['-i', 'life=s=640x360:r=24:ratio=0.5:mold=0', '-t', '120', '-c:v', 'h264_videotoolbox', '-b:v', '8000k', '-g', '24'],
    ['-i', 'mandelbrot=size=640x360:rate=24', '-t', '120', '-c:v', 'h264_videotoolbox', '-b:v', '8000k', '-g', '24'],
  ];
  const said = [];
  for (let attempt = 0; attempt < 24; attempt++) {
    for (const [n, way] of ways.entries()) {
      try {
        ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', ...way, '-pix_fmt', 'yuv420p', path);
        return { way: n, attempt, bytes: readFileSync(path) };
      } catch (error) {
        said[n] = String(error.stderr || error.message).slice(-200);
      }
    }
    await sleep(5000);
  }
  throw new Error(`no ffmpeg made the clip: ${JSON.stringify(said)}`);
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
  el.load();
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

/** `urls` each in a `<video>` at the page's bottom left, played by the next press on the page (a browser may want one). */
const start = (page, urls) => page.evaluate((urls) => {
  window.__said = [];
  window.__videos = urls.map((url, i) => {
    const el = document.createElement('video');
    Object.assign(el, { muted: true, loop: true, preload: 'auto', playsInline: true });
    el.style.cssText = `position:fixed;left:${i * 170}px;bottom:0;width:160px;height:90px;z-index:2147483647`;
    for (const name of ['waiting', 'stalled', 'playing', 'error', 'ended']) el.addEventListener(name, () => window.__said.push(`${Math.round(performance.now() - window.__began)} ms ${name}${name === 'error' ? ` ${el.error?.code}` : ''}`));
    document.body.append(el);
    el.src = url;
    return el;
  });
  window.__began = performance.now();
  const play = () => window.__videos.forEach((el) => el.play().catch(() => {}));
  document.addEventListener('pointerdown', play, { capture: true, once: true });
  play();
}, urls);
const press = async (page) => { const { height } = page.viewportSize(); await page.mouse.click(40, height - 40); };
const read = (page) => page.evaluate(() => (window.__videos ?? []).map((el) => ({ at: Number(el.currentTime.toFixed(2)), paused: el.paused, ready: el.readyState, network: el.networkState, error: el.error?.code ?? null,
  ahead: el.buffered.length ? Math.round(el.buffered.end(el.buffered.length - 1) - el.currentTime) : 0 })));
const pauseAll = (page) => page.evaluate(() => (window.__videos ?? []).forEach((el) => el.pause()));
const stop = (page) => page.evaluate(() => {
  for (const el of window.__videos ?? []) { el.pause(); el.removeAttribute('src'); el.load(); el.remove(); }
  window.__videos = [];
});

test('capped: probe', { tag: '@engines' }, async ({ page, project, context, signIn, guards, browserName }) => {
  test.setTimeout(1_500_000);
  guards.allow(/^console error:/, /^page error:/);
  const say = (key, value) => console.log(`capped, probe ${browserName}, ${key}: ${JSON.stringify(value)}`);
  const step = async (name, body) => {
    try { await body(); } catch (error) { say(`${name}, FAILED`, String(error.message).slice(0, 400)); }
  };
  const made = await long(test.info().outputPath('long.mp4'));
  const bytes = made.bytes;
  await project.import(bytes, { name: 'long', at: { x: 0, y: 0 } });
  const file = `/v1/projects/${encodeURIComponent(project.id)}/files/${createHash('sha256').update(bytes).digest('hex')}.mp4`;
  await expect.poll(() => page.evaluate((file) => fetch(file, { method: 'HEAD' }).then((answer) => answer.status, () => 0), file), { message: 'long served', timeout: 180_000 }).toBe(200);
  const port = new URL(project.url()).port;
  say('file', { mb: Number((bytes.length / 1e6).toFixed(1)), seconds: 120, encoder: made.way, tries: made.attempt, platform: process.platform, sockets_at_rest: sockets(port) });
  await sleep(15_000);
  let fresh = 0;
  const url = () => `${file}?n=${++fresh}`;
  const knob = (name, value) => page.evaluate(([name, value]) => {
    document.cookie = value === null ? `${name}=; Max-Age=0; path=/` : `${name}=${value}; path=/`;
  }, [name, value]);
  await knob('probe_piece', 256 << 10);

  await step('a Range answer', async () => {
    await knob('probe_cap', MB);
    say('a Range answer, capped 1 MiB', await page.evaluate((url) => fetch(url, { headers: { Range: 'bytes=0-' }, cache: 'no-store' }).then(async (answer) => ({ status: answer.status, range: answer.headers.get('Content-Range'), bytes: (await answer.arrayBuffer()).byteLength })), url()));
    await knob('probe_cap', null);
  });

  await step('a file still landing', async () => {
    // Its first 6 MB (about 6 s) and its last 1 MiB (its index) are in place. The element plays from the start and soon asks past them.
    for (const past of [9000, 416, 503]) {
      await knob('probe_cap', MB);
      await knob('probe_edge', 6 * MB);
      await knob('probe_past', past);
      await start(page, [url()]);
      await press(page);
      const looks = [];
      for (let second = 1; second <= 20; second++) {
        await sleep(1000);
        // The rest lands 8 s in: from then every range is there.
        if (second === 8) { await knob('probe_edge', null); await knob('probe_past', null); }
        const [video] = await read(page);
        looks.push(`${second}s at ${video.at}${video.paused ? ' paused' : ''} ready ${video.ready}${video.error ? ` error ${video.error}` : ''}`);
      }
      const said = await page.evaluate(() => window.__said);
      // As the page does when Calcifer comes back: loaded again where it was.
      const again = await page.evaluate(async () => {
        const [el] = window.__videos;
        const at = el.currentTime;
        el.load();
        el.currentTime = at;
        el.play().catch(() => {});
        await new Promise((done) => setTimeout(done, 4000));
        return { asked_at: Number(at.toFixed(2)), now: Number(el.currentTime.toFixed(2)), paused: el.paused, ready: el.readyState, error: el.error?.code ?? null };
      });
      await stop(page);
      say(`landing, past the bytes in place ${past > 600 ? `held ${past} ms` : `answered ${past}`}`, { looks, said, loaded_again: again });
      await sleep(1500);
    }
    await knob('probe_cap', null);
  });

  await step('first frame and seeks', async () => {
    const rows = [];
    for (const cap of [null, 4 * MB, MB, 512 << 10]) {
      await knob('probe_cap', cap);
      rows.push({ way: `URL, ${named(cap)}`, ...(await plain(page, url(), false)) });
    }
    await knob('probe_cap', null);
    rows.push({ way: 'memory', ...(await plain(page, url(), true)) });
    say('first frame and seeks', rows);
  });

  await step('twelve at once', async () => {
    const windows = [{ page }];
    for (let i = 0; i < 2; i++) {
      const one = { page: await context.newPage() };
      await signIn(one.page, project.url());
      await expect(one.page.locator('.stage')).toBeVisible();
      windows.push(one);
    }
    // Firefox: only the cap the page will ask for (all five took it past its lane's 20 minutes in 44246295237).
    for (const cap of browserName === 'firefox' ? [MB] : [null, 4 * MB, 2 * MB, MB, 512 << 10]) {
      try {
        await knob('probe_cap', cap);
        for (const one of windows) {
          one.calls = [];
          one.moved = [0, 0, 0, 0];
          await one.page.bringToFront();
          await start(one.page, Array.from({ length: 4 }, url));
          await press(one.page);
          one.was = await read(one.page);
        }
        const seen = [];
        const began = Date.now();
        let looks = 0;
        for (let second = 1; Date.now() - began < 30_000; second++) {
          await sleep(Math.max(0, began + second * 1000 - Date.now()));
          for (const one of windows) one.calls.push(await call(one.page, project.id));
          if (second % 5 === 0) {
            looks++;
            for (const one of windows) {
              const now = await read(one.page);
              now.forEach((video, n) => { if (!video.paused && Math.abs(video.at - one.was[n].at) > 0.05) one.moved[n]++; });
              one.was = now;
            }
            seen.push(sockets(port));
          }
        }
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
});
