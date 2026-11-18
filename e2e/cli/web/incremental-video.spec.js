// incremental-video (cl-1210; Paul, 11-18: "we could already see segments 1 and 2, but then they suddenly disappear and we are waiting for
// the entire video to download to see segment 3"): a shot rendered in three segments (project/segments.py, H3's stand-in) plays as one
// growing video. At rest its box plays what's made so far and each newer revision, then its take, never a blank box. In Focus its clock runs
// on through each new revision and through its take landing, and Focus always has a frame. Each revision after the first keeps the bytes of
// the one before but its index, so only its new segment and index are read.
const { mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, focus, fixtureTool, canvasCard, canvasVisible, screenshotPixels } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

const SEGMENTS = fixtureTool('segments.py/segments');
const SECONDS = 8;
// Each segment after the first renders in this long: faster than it plays, so playing never catches up with the render.
const WAIT = 5;
const RUN_MS = 120_000;
const LOOKS = [{ width: 1440, height: 900, theme: 'paper' }, { width: 1440, height: 900, theme: 'dark' }];
// The stand-in's sound and pictures (project/segments.py): a tone of 220 Hz times its number a clip, a colour a clip, a bar 4 px a frame.
const RATE = 48000;
const HUES = [[200, 40, 40], [40, 160, 60], [40, 70, 200], [200, 170, 40]];
const FPS = 24;
// A join of three clips (2, 3 and 2 s) as the Runtime's `join_video` wrote it (segments.py/joins on cozy-runtime 8bf4503e3, e2e run
// 44236913278): what WebKit seeks, on a Mac, where no machine runs to make one. `incremental-video joins` holds today's to its layout.
const JOIN = join(__dirname, 'incremental-video-join.mp4');
const JOIN_SECONDS = [2, 3, 2];

/** A file's top-level boxes, in order: a growing MP4 is `ftyp`, an `mdat` a piece, then its index. */
function boxes(file) {
  const bytes = readFileSync(file), out = [];
  for (let at = 0; at + 8 <= bytes.length;) {
    const size = bytes.readUInt32BE(at) || bytes.length - at;
    out.push(bytes.toString('latin1', at + 4, at + 8));
    at += size;
  }
  return out;
}

/** Each stream's start time: the same for every revision and the take, both 0 as libavformat writes them. */
const starts = (file) => Object.fromEntries(JSON.parse(ffmpeg('ffprobe')('-v', 'error', '-show_entries', 'stream=codec_type,start_time', '-of', 'json', file))
  .streams.map((stream) => [stream.codec_type, Number(stream.start_time)]));

/** The frames where the colour changes: where a clip ends and the next begins. */
function colourSwitches(file) {
  // A row of 32 points across the middle: the bar covers one or two of them, so the colour most of them have is the clip's.
  const points = 32;
  const raw = ffmpeg('ffmpeg')('-v', 'error', '-i', file, '-vf', `crop=iw:2:0:ih/2,scale=${points}:1:flags=neighbor,format=rgb24`, '-f', 'rawvideo', '-');
  const nearest = (p) => HUES.reduce((best, h, i) => (Math.hypot(...h.map((v, c) => v - p[c])) < Math.hypot(...HUES[best].map((v, c) => v - p[c])) ? i : best), 0);
  const hue = (frame) => {
    const votes = new Array(HUES.length).fill(0);
    for (let x = 0; x < points; x++) votes[nearest(raw.subarray((frame * points + x) * 3, (frame * points + x) * 3 + 3))]++;
    return votes.indexOf(Math.max(...votes));
  };
  const out = [];
  const frames = raw.length / (3 * points);
  for (let frame = 1; frame < frames; frame++) if (hue(frame) !== hue(frame - 1)) out.push({ frame, from: hue(frame - 1), to: hue(frame) });
  return out;
}

/** A tone's amplitude over the window of about 5 cycles of 220 Hz centred on `centre`: whole cycles of every clip's tone. */
function amplitude(pcm, f, centre) {
  const width = 1091, low = Math.round(centre - width / 2);
  let re = 0, im = 0;
  for (let n = Math.max(0, low); n < Math.min(pcm.length, low + width); n++) {
    const w = (2 * Math.PI * f * n) / RATE;
    re += pcm[n] * Math.cos(w); im += pcm[n] * Math.sin(w);
  }
  return (2 * Math.hypot(re, im)) / width;
}

/** Where, in ms from where the pictures switch, the tone switches: each tone's share of the window there against its own level nearby. */
function toneOffset(pcm, from, to, frame) {
  const width = 1091, at = (frame * RATE) / FPS;
  const share = (f, centre, steady) => Math.min(1, amplitude(pcm, f, centre) / Math.max(1e-6, amplitude(pcm, f, steady)));
  const after = at + width / 2 - share(220 * (to + 1), at, at + 2 * width) * width;
  const before = at - width / 2 + share(220 * (from + 1), at, at - 2 * width) * width;
  return Math.round(((after + before) / 2 - at) / RATE * 1e5) / 100;
}

/** A video's sound against its pictures at every join: its streams' starts, and how far (ms) its tone switches from its colour. */
function synced(file) {
  const pcm = new Float32Array(new Uint8Array(ffmpeg('ffmpeg')('-v', 'error', '-i', file, '-vn', '-ac', '1', '-ar', String(RATE), '-f', 'f32le', '-')).buffer);
  return { file, starts: starts(file), joins: colourSwitches(file).map(({ frame, from, to }) => ({ frame, offset_ms: toneOffset(pcm, from, to, frame) })) };
}

/** Every 100 ms, what Focus shows: the take's player once it plays, else the glimpse's shown element; its clock, and whether it has a frame. */
function sample() {
  const region = document.querySelector('[aria-label="Focused asset"]');
  const live = region?.querySelector('video.live');
  const glimpse = region?.querySelector('.glimpse video.shown');
  const take = live && live.readyState >= 2 && !live.paused && !live.seeking ? live : null;
  const el = take ?? glimpse ?? null;
  window.__shown.push({
    which: take ? 'take' : glimpse ? 'glimpse' : 'none',
    rev: glimpse?.closest('.glimpse')?.getAttribute('data-rev') ?? null,
    at: el ? el.currentTime : null,
    drawn: Boolean(el && el.readyState >= 2),
  });
}

test('incremental-video', async ({ page, project, api, cli }) => {
  await cli.json('machine', 'use', 'local');
  const beats = [1, 2, 3].map((part) => ({ description: `Part ${part} of the coast.`, duration: SECONDS }));
  await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast, segment after segment.', beats, wait: WAIT }, at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'shot')).toBe(true);
  await fit(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  const render = async () => {
    await focus(page, 'shot');
    const before = (await project.asset('shot'))?.prefixes?.length ?? 0;
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    let run;
    await expect.poll(async () => (run = (await project.asset('shot'))?.making?.[0]), { message: 'its run', timeout: RUN_MS }).toBeTruthy();
    return { run, before };
  };

  await test.step('at rest, its box plays what is made so far, each newer revision and then its take: never a blank box', async () => {
    const { run, before } = await render();
    const id = (await api.get(project.at(`/runs/${run}`))).id;
    await leave(page);
    const seen = [];
    let shots = 0;
    const kept = test.info().outputPath('revisions');
    mkdirSync(kept, { recursive: true });
    await expect.poll(async () => {
      // Each revision as Calcifer serves it, kept to check its sound against its pictures.
      const answer = await api.raw('GET', project.at(`/runs/${encodeURIComponent(id)}/outputs/video`));
      const rev = (answer.headers.get('etag') ?? '').replace(/\D/g, '');
      if (answer.ok && rev) writeFileSync(join(kept, `rev-${rev.padStart(3, '0')}.mp4`), Buffer.from(await answer.arrayBuffer()));
      else await answer.body?.cancel();
      const card = await canvasCard(page, 'shot');
      const glimpse = card?.rendered?.glimpse ?? null;
      const landed = ((await project.asset('shot'))?.prefixes?.length ?? 0) > before;
      seen.push({ glimpse, landed, poster: Boolean(card?.rendered?.media?.ready) });
      // Two revisions in: the box at rest, as Paul sees it, in paper and dark.
      if (glimpse?.ready && glimpse.rev >= 2 && !shots) {
        shots++;
        for (const { width, height, theme } of LOOKS) {
          await page.setViewportSize({ width, height });
          await project.look(theme);
          await fit(page);
          const body = (await canvasCard(page, 'shot')).body;
          const path = test.info().outputPath(`at-rest-${width}x${height}-${theme}.png`);
          const pixels = screenshotPixels(await page.screenshot({ path, clip: body }));
          const colours = new Set();
          for (let y = 2; y < pixels.height - 2; y += 8) for (let x = 2; x < pixels.width - 2; x += 8) colours.add(pixels(x, y).join());
          expect.soft(colours.size, `its box at rest (${theme}) shows a picture, not an empty frame`).toBeGreaterThan(8);
        }
        await project.look(null);
      }
      return landed && seen.some((one) => one.glimpse?.landed);
    }, { message: 'its take lands on the box it grew in', timeout: RUN_MS, intervals: [250] }).toBe(true);
    test.info().attach('at-rest.json', { body: JSON.stringify(seen, null, 1), contentType: 'application/json' });
    const during = seen.slice(seen.findIndex((one) => one.glimpse?.ready));
    expect(during.filter((one) => !one.glimpse?.ready && !one.poster).length, 'polls with the box drawing nothing').toBe(0);
    expect(new Set(during.map((one) => one.glimpse?.rev).filter(Boolean)).size, 'revisions played at rest').toBeGreaterThanOrEqual(2);
    // Each revision after the first keeps the bytes of the one before but its index (its last box, `moov`): what lets Calcifer read
    // only the new ones (TensorD's `appended_from`, measured on the pod by release-gate/incremental-video.sh).
    const files = readdirSync(kept).sort().map((name) => join(kept, name));
    for (const [at, file] of files.entries()) {
      if (!at) continue;
      const last = readFileSync(files[at - 1]), next = readFileSync(file);
      let box = 0;
      while (box + 8 <= last.length && box + last.readUInt32BE(box) < last.length) box += last.readUInt32BE(box) || last.length;
      expect.soft(last.toString('latin1', box + 4, box + 8), `${files[at - 1]} ends in its index`).toBe('moov');
      expect.soft(next.subarray(0, box).equals(last.subarray(0, box)), `${file} keeps ${files[at - 1]}'s first ${box} bytes`).toBe(true);
    }
    // Every revision and the take: both tracks start at 0, and at every join the tone switches within 1 ms of the colour.
    const sync = files.map(synced);
    test.info().attach('sync.json', { body: JSON.stringify(sync, null, 1), contentType: 'application/json' });
    expect(sync.length, 'revisions kept').toBeGreaterThanOrEqual(3);
    for (const one of sync) {
      expect.soft(one.starts, `${one.file}: its streams start at 0`).toEqual({ video: 0, audio: 0 });
      for (const at of one.joins) expect.soft(Math.abs(at.offset_ms), `${one.file}: the tone switches with the colour at frame ${at.frame} (ms)`).toBeLessThanOrEqual(1);
    }
    expect(sync.at(-1).joins.map((one) => one.frame), 'the take joins where its segments meet').toEqual([SECONDS * FPS, 2 * SECONDS * FPS]);
    // Calcifer's own account of it, on the run's record (`measurements.transfer`): it kept bytes of the revisions before, read at least
    // the take's own and fewer than the revisions hold together, and read none of them again whole.
    const record = await api.get(project.at(`/runs/${run}?detail=full`));
    const moved = record.measurements?.transfer?.video;
    const sizes = files.map((file) => statSync(file).size), together = sizes.reduce((sum, size) => sum + size, 0);
    test.info().attach('transfer.json', { body: JSON.stringify({ moved, sizes }, null, 1), contentType: 'application/json' });
    expect(moved, "the run's record says what it read of its video and what it kept").toMatchObject({ read: expect.any(Number), reused: expect.any(Number) });
    expect(moved.reused, 'bytes kept of the revisions before').toBeGreaterThan(0);
    expect(moved.read, "at least the take's own bytes were read").toBeGreaterThanOrEqual(sizes.at(-1));
    expect(moved.read, 'fewer bytes read than its revisions hold together').toBeLessThan(together);
    expect(moved.read + moved.reused, 'read and kept make up every revision that landed').toBeGreaterThanOrEqual(together);
    expect((record.warnings ?? []).map((one) => one.code), 'no revision read again whole').not.toContain('output_reread');
  });

  await test.step('in Focus, through segments 2 and 3 and its take landing, its clock runs on and it always has a frame', async () => {
    const { before } = await render();
    await region.locator('.here-side .take.making').first().click();
    await expect.poll(() => region.locator('.glimpse video.shown').evaluate((el) => el.readyState >= 2).catch(() => false), { message: 'segment 1 here', timeout: RUN_MS }).toBe(true);
    await region.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
    await expect.poll(() => region.locator('.glimpse video.shown').evaluate((el) => !el.paused && el.currentTime > 0.2), { message: 'segment 1 plays' }).toBe(true);
    await page.addScriptTag({ content: sample.toString() });
    await page.evaluate(() => { window.__shown = []; window.__sampler = setInterval(() => sample(), 100); });
    await expect.poll(async () => (await project.asset('shot'))?.prefixes?.length ?? 0, { message: 'its take', timeout: RUN_MS }).toBe(before + 1);
    await expect.poll(() => page.evaluate(() => window.__shown.filter((one) => one.which === 'take').length), { message: 'the take plays on', timeout: 60_000 }).toBeGreaterThan(20);
    const shown = await page.evaluate(() => { clearInterval(window.__sampler); return window.__shown; });
    test.info().attach('focus.json', { body: JSON.stringify(shown, null, 1), contentType: 'application/json' });
    const playing = shown.slice(shown.findIndex((one) => one.at !== null));
    expect(playing.filter((one) => one.which === 'none' || !one.drawn).length, 'samples with nothing drawn').toBe(0);
    const back = playing.slice(1).map((one, at) => ({ from: playing[at], to: one, step: one.at - playing[at].at })).filter(({ step }) => step < -0.1);
    expect(back, 'its clock never steps back (s)').toEqual([]);
    const revs = [...new Set(playing.filter((one) => one.which === 'glimpse').map((one) => one.rev))];
    expect(revs.length, `revisions played: ${revs.join(', ')}`).toBeGreaterThanOrEqual(2);
    expect(playing.at(-1).at, 'played on, past segment 1 (s)').toBeGreaterThan(SECONDS);
    await expect(region.locator('.glimpse'), 'nothing of the glimpse once the take plays').toHaveCount(0);
  });
});

// The same clips, each its own `save_video` with x264's B-frames, joined by `save_video_concat` and by `join_video` (cl-1210): the pictures of
// both are the clips' own, frame for frame, their streams start at 0, and each clip's tone comes in with its colour.
test('incremental-video joins', async ({ project, cli }) => {
  const dir = test.info().outputPath('joins');
  const said = await cli('run', fixtureTool('segments.py/joins'), '--on', 'local', '-o', dir);
  expect(said.code, `cozy run segments.py/joins: ${said.stderr.slice(-500)}`).toBe(0);
  const files = readdirSync(dir, { recursive: true }).map(String).filter((name) => name.endsWith('.mp4')).map((name) => join(dir, name));
  const of = (field) => files.filter((file) => file.includes(field)).sort();
  const md5 = (file) => ffmpeg('ffmpeg')('-v', 'error', '-i', file, '-map', '0:v', '-f', 'framemd5', '-').toString().split('\n').filter((line) => line && !line.startsWith('#')).map((line) => line.split(',').at(-1).trim());
  const [concat] = of('concat'), [joined] = of('joined'), pieces = of('pieces');
  expect(Boolean(concat && joined) && pieces.length === 3, `its outputs: ${files.join(', ')}`).toBe(true);
  const want = pieces.flatMap(md5);
  for (const file of [concat, joined]) {
    expect(md5(file), `${file}: the clips' own pictures, frame for frame`).toEqual(want);
    const sync = synced(file);
    expect(sync.starts, `${file}: its streams start at 0`).toEqual({ video: 0, audio: 0 });
    expect(sync.joins.map((one) => one.frame), `${file}: its joins`).toEqual([2 * FPS, 5 * FPS]);
    for (const at of sync.joins) expect(Math.abs(at.offset_ms), `${file}: the tone switches with the colour at frame ${at.frame} (ms)`).toBeLessThanOrEqual(1);
  }
  // The join WebKit seeks (JOIN) is laid out as today's: a writer that changes it makes that one anew.
  expect(boxes(joined), `${joined}: laid out as ${JOIN}, which web-webkit seeks`).toEqual(boxes(JOIN));
  expect(md5(JOIN).length, `${JOIN}: as many frames`).toBe(want.length);
});

// Seeks across each join of a take made in three segments land on the frame asked, in Focus: a frame before the join, the join and a frame
// mid-GOP after it, read from where the bar is and the clip's colour. `@engines`: in WebKit too (web-webkit on macOS: AVFoundation parses
// the index, where everything else goes through ffmpeg).
test('incremental-video seeks', { tag: '@engines' }, async ({ page, project, cli, browserName }) => {
  // Rendered here where a machine runs; in WebKit, on a Mac, the join the Runtime wrote (JOIN), as a box of its own.
  const rendered = browserName !== 'webkit';
  const seconds = rendered ? [4, 4, 4] : JOIN_SECONDS;
  const region = page.getByRole('region', { name: 'Focused asset' });
  if (rendered) {
    await cli.json('machine', 'use', 'local');
    const beats = seconds.map((duration, part) => ({ description: `Part ${part + 1}.`, duration }));
    await project.add({ name: 'shot', tool: SEGMENTS, kind: 'video', fields: { summary: 'Three parts.', beats, wait: 0 }, at: { x: 0, y: 0 } });
  } else await project.import(readFileSync(JOIN), { name: 'shot', at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'shot')).toBe(true);
  await fit(page);
  await focus(page, 'shot');
  if (rendered) {
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => (await project.asset('shot'))?.prefixes?.length ?? 0, { message: 'its take', timeout: RUN_MS }).toBe(1);
  }
  const live = region.locator('video.live');
  await expect.poll(() => live.evaluate((el) => el.readyState >= 2 && !el.classList.contains('taking')).catch(() => false), { message: 'its take open in Focus', timeout: 60_000 }).toBe(true);
  // Where each clip starts, in frames: a seek is a frame before a join, the join, and a frame mid-GOP after it.
  const clips = seconds.map((_, part) => seconds.slice(0, part).reduce((sum, one) => sum + one, 0) * FPS);
  const seen = [];
  for (const join of clips.slice(1)) {
    for (const frame of [join - 1, join, join + 12]) {
      await live.evaluate((el, t) => new Promise((done) => { el.pause(); el.addEventListener('seeked', () => requestAnimationFrame(() => requestAnimationFrame(done)), { once: true }); el.currentTime = t; }), (frame + 0.5) / FPS);
      const pixels = screenshotPixels(await live.screenshot());
      // The row through the picture's middle: where its colour runs is the picture; the white run in it is the bar.
      const y = Math.floor(pixels.height / 2), row = [];
      for (let x = 0; x < pixels.width; x++) row.push(pixels(x, y));
      const near = (p, c) => Math.hypot(p[0] - c[0], p[1] - c[1], p[2] - c[2]) < 60;
      const lit = row.map((p, x) => [x, p]).filter(([, p]) => [...HUES, [255, 255, 255]].some((c) => near(p, c)));
      const left = lit[0]?.[0] ?? 0, right = (lit.at(-1)?.[0] ?? pixels.width - 1) + 1;
      const white = row.map((p, x) => [x, p]).filter(([, p]) => p[0] > 220 && p[1] > 220 && p[2] > 220).map(([x]) => x);
      const segment = clips.findLastIndex((start) => start <= frame), own = frame - clips[segment];
      const middle = white.length ? (white[0] + white.at(-1) + 1) / 2 : -1;
      const bar = Math.round(((middle - left) / (right - left)) * 160 - 3);
      const centre = row[Math.floor((left + right) / 2)];
      const hue = HUES.reduce((best, h, i) => (Math.hypot(...h.map((v, c) => v - centre[c])) < Math.hypot(...HUES[best].map((v, c) => v - centre[c])) ? i : best), 0);
      seen.push({ frame, bar, want: (own * 4) % 160, hue, want_hue: segment % HUES.length });
    }
  }
  test.info().attach('seeks.json', { body: JSON.stringify(seen, null, 1), contentType: 'application/json' });
  for (const one of seen) {
    expect.soft(one.hue, `frame ${one.frame}: its clip's colour`).toBe(one.want_hue);
    expect.soft(Math.abs(one.bar - one.want), `frame ${one.frame}: the bar where that frame draws it (px of 160)`).toBeLessThanOrEqual(2);
  }
});
