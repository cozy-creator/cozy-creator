// The video editor (cl-957), in a real browser on a real Calcifer: an Edit opened is a full-size player over a standard timeline, its versions
// beside them, with no Render button and no panel under it. Three clips go in, one a box dropped on the Edit from the canvas and two files
// from the computer let go on the timeline; then a reorder, a trim, a split at the playhead and a piece deleted, with Undo. The player shows
// the cut live, and each change is joined here, free, into the next version: the newest one's film has the cut's frames exactly, its pieces
// where the timeline has them, its sound, and its `.otio` reads back with OpenTimelineIO's own reader.
//   node timeline.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $SHOT: where to keep its screenshots;
// ffmpeg, ffprobe and uv on PATH. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync, spawnSync } = require('child_process');
const { existsSync, mkdtempSync, readFileSync, rmSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { implementationRow, projectFromPage, calcifer, carry, signIn, STEP_MS, boxOf, closeBox, drop, lastLook, lookAt, noStall, openBox, settled, until, watchBoxes } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const FPS = 24;
const ffmpeg = (...args) => execFileSync('ffmpeg', ['-v', 'error', '-y', ...args], { stdio: ['ignore', 'ignore', 'inherit'] });
const probe = (file, ...args) => execFileSync('ffprobe', ['-v', 'error', ...args, file], { encoding: 'utf8' }).trim();
const countFrames = (file) => Number(probe(file, '-count_frames', '-select_streams', 'v:0', '-show_entries', 'stream=nb_read_frames', '-of', 'csv=p=0'));
const streamSeconds = (file, kind) => Number(probe(file, '-select_streams', `${kind}:0`, '-show_entries', 'stream=duration', '-of', 'csv=p=0'));
// One frame's middle pixel, as red, green and blue.
function pixelAt(file, frame) {
  const out = execFileSync('ffmpeg', ['-v', 'error', '-i', file, '-vf', `select=eq(n\\,${frame}),crop=2:2:iw/2:ih/2,scale=1:1`, '-frames:v', '1', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], { stdio: ['ignore', 'pipe', 'inherit'] });
  return [...out];
}
const looksLike = (pixel, want) => Array.isArray(pixel) && pixel.every((value, i) => Math.abs(value - want[i]) < 48);
const RED = [255, 0, 0], BLUE = [0, 0, 255], GREEN = [0, 255, 0];

function plate(path, color, seconds, hz) {
  ffmpeg('-f', 'lavfi', '-i', `color=c=${color}:size=320x180:rate=${FPS}:duration=${seconds}`, '-f', 'lavfi', '-i', `sine=frequency=${hz}:sample_rate=32000:duration=${seconds}`,
    '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-g', String(FPS), '-c:a', 'aac', '-shortest', path);
  return { name: path.split(/[\\/]/).pop(), type: 'video/mp4', bytes: readFileSync(path) };
}

(async () => {
  const base = calcifer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1900, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  await watchBoxes(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d/.test(m.text())) errors.push(`console: ${m.text()}`); });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (what) => page.screenshot({ path: process.env.SHOT ? `${process.env.SHOT}-${what}.png` : undefined });

  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-timeline-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const call = (method, path, body) => page.evaluate(async ({ method, path, body }) => {
    const { token } = JSON.parse(localStorage.getItem('cozy.session'));
    const response = await fetch(path, { method, headers: { Authorization: `Bearer ${token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) }, body: body ? JSON.stringify(body) : undefined });
    return { status: response.status, body: await response.json().catch(() => null) };
  }, { method, path, body });
  const read = async () => (await call('GET', `/v1/projects/${id}`)).body;
  const folder = (await read()).path;
  seen.project = { id, folder };
  let timelineName;
  const edit = async () => (await read()).assets.find((asset) => asset.name === timelineName);

  // Real clips, each one colour and one tone, and a green still.
  const dir = mkdtempSync(join(tmpdir(), 'web-timeline-'));
  const red = plate(join(dir, 'red.mp4'), 'red', 2, 440);
  const blue = plate(join(dir, 'blue.mp4'), 'blue', 3, 880);
  const stillPath = join(dir, 'title.png');
  ffmpeg('-f', 'lavfi', '-i', 'color=c=0x00ff00:size=400x400', '-frames:v', '1', stillPath);
  const title = { name: 'title.png', type: 'image/png', bytes: readFileSync(stillPath) };
  const stage = await page.locator('.stage').boundingBox();
  await drop(page, [red], stage.x + 508, stage.y + 104);
  // One file dropped asks for its name: as given.
  await page.locator('.nm-field').waitFor();
  await page.keyboard.press('Enter');
  await until('red landed', async () => (await read()).assets.find((asset) => asset.name === 'red')?.sha256, Boolean);

  // The Video Timeline, dropped from the tools: a box on the canvas.
  const tools = (await call('GET', '/v1/tools')).body.tools;
  check('edit_is_a_tool', tools.some((held) => held.id === 'cozy/edit' && held.makes === 'video'));
  const before = new Set((await read()).assets.map((asset) => asset.name));
  await carry(page, await implementationRow(page, 'timeline', 'cozy/edit'), { x: 1208, y: 160 });
  timelineName = (await until('the Video Timeline box', async () => (await read()).assets.find((asset) => asset.tool === 'cozy/edit' && !before.has(asset.name)), Boolean)).name;
  const card = (name) => boxOf(page, name);
  await card(timelineName).waitFor();
  await until('it has landed', () => page.locator('.card.landing').count(), (n) => n === 0);

  // From the canvas: a box let go on the Edit goes on its timeline.
  await page.keyboard.press('f');
  await settled(page);
  const tile = await card(timelineName).locator('.media').boundingBox();
  await carry(page, card('red').locator('.nm'), { x: tile.x + tile.width / 2, y: tile.y + tile.height / 2 });
  check('canvas_box_onto_the_edit', await until('red on its timeline', async () => JSON.stringify((await edit()).fields?.timeline ?? null), (text) => text.includes('<red>')).then(() => true, () => false));
  // A cut changed at rest is joined too: its first version, with nothing pressed.
  check('joined_at_rest', await until('its first version', async () => (await edit()).prefixes?.length ?? 0, (n) => n >= 1, 120_000).then(() => true, () => false));

  // Opened, it's a video editor: the cut full size in the frame, the timeline under it, its versions beside them; no Render, no panel under it.
  await openBox(page, timelineName);
  const focus = page.locator('.focus-viewport');
  await focus.locator('.tl-clip').first().waitFor();
  await settled(page);
  const viewport = page.viewportSize();
  const player = await focus.locator('.tl-player').boundingBox();
  const timeline = await focus.locator('.tl').boundingBox();
  const side = await focus.locator('.here-side').boundingBox();
  seen.layout = { player, timeline, side };
  check('player_full_size', player.width >= viewport.width * 0.45 && player.height >= viewport.height * 0.35, seen.layout);
  check('timeline_under_the_player', timeline.y >= player.y + player.height && timeline.width >= player.width * 0.9, seen.layout);
  check('versions_beside', Boolean(side) && side.x >= timeline.x + timeline.width - 1, seen.layout);
  check('no_render_button', (await focus.locator('button[aria-label="Render"]').count()) === 0);
  check('no_panel_under_it', (await focus.locator('.here-under, .take.add').count()) === 0, await focus.innerText());
  check('no_video_until_played', (await page.locator('video').count()) === 0);
  await shot('open');

  // From the computer: files let go on the timeline go in where they're let go, each a box of its own too.
  const lane = () => focus.locator('.tl-clips').boundingBox();
  const dropOnTimeline = async (file, where) => {
    const at = await lane();
    const x = where === 'start' ? at.x + 2 : at.x + at.width - 4;
    const transfer = await page.evaluateHandle((held) => {
      const data = new DataTransfer();
      data.items.add(new File([Uint8Array.from(atob(held.b64), (c) => c.charCodeAt(0))], held.name, { type: held.type }));
      return data;
    }, { name: file.name, type: file.type, b64: file.bytes.toString('base64') });
    for (const type of ['dragenter', 'dragover', 'drop']) await page.dispatchEvent('.focus-viewport .tl-clips', type, { dataTransfer: transfer, clientX: x, clientY: at.y + at.height / 2 });
  };
  const clips = () => page.$$eval('.focus-viewport .tl-clip', (els) => els.map((el) => ({ name: el.dataset.name, in: Number(el.dataset.in), out: Number(el.dataset.out), frames: Number(el.dataset.frames), missing: el.dataset.missing })));
  const order = async () => (await clips()).map((clip) => clip.name).join(' ');
  await dropOnTimeline(title, 'start');
  await until('the still first', order, (now) => now === 'title red');
  await dropOnTimeline(blue, 'end');
  check('files_where_let_go', await until('three clips', order, (now) => now === 'title red blue').then(() => true, async () => order()));
  const whole = JSON.stringify([3 * FPS, 2 * FPS, 3 * FPS]);
  const measured = await until('each as long as it lasts', clips, (now) => JSON.stringify(now.map((clip) => clip.frames)) === whole).catch(() => clips());
  check('whole_takes', JSON.stringify(measured.map((clip) => clip.frames)) === whole, measured);
  // As long as it lasts on the ruler: one scale for every clip, once they've slid into place.
  const still = () => page.evaluate(() => Promise.all(document.querySelector('.focus-viewport .tl').getAnimations({ subtree: true }).map((held) => held.finished.catch(() => {}))));
  await still();
  const widths = await page.$$eval('.focus-viewport .tl-clip', (els) => els.map((el) => el.getBoundingClientRect().width / Number(el.dataset.frames)));
  check('blocks_sized_by_duration', widths.every((w) => Math.abs(w - widths[0]) < 0.05), widths);
  check('a_filmstrip_each', await until('their frames', () => focus.locator('.tl-clip .tl-picture img').count(), (n) => n >= 3).then(() => true, () => false));
  check('sound_under_the_videos', await until('their sound', () => focus.locator('.tl-clip:not([data-name="title"]) .tl-sound img').count(), (n) => n === 2).then(() => true, () => false));
  check('ruler_marks', (await focus.locator('.tl-mark').count()) >= 3, await focus.locator('.tl-mark').allTextContents());
  await shot('three');

  // The ruler takes the playhead to a frame, as the transport says; what the player shows is that frame of the cut, live.
  // Pixels a frame, as the ruler is drawn: it never slides.
  const scale = async () => (await focus.locator('.tl-ruler').boundingBox()).width / (await clips()).reduce((sum, clip) => sum + clip.frames, 0);
  const seek = async (frame) => {
    const rail = await focus.locator('.tl-rail').boundingBox();
    await page.mouse.click(rail.x + (frame + 0.5) * (await scale()), rail.y + rail.height / 2);
    return until(`the playhead on ${frame}`, () => focus.locator('.tl-playhead').getAttribute('data-frame').then(Number), (at) => at === frame);
  };
  const showing = () => page.evaluate(() => {
    const monitor = document.querySelector('.focus-viewport .tl-monitor');
    const visible = (el) => Boolean(el && getComputedStyle(el).visibility !== 'hidden' && el.getBoundingClientRect().width > 0);
    const sample = (source) => {
      const copy = document.createElement('canvas');
      copy.width = 1;
      copy.height = 1;
      const context = copy.getContext('2d');
      context.drawImage(source, source.videoWidth ? source.videoWidth / 2 - 1 : (source.naturalWidth || source.width) / 2 - 1, source.videoHeight ? source.videoHeight / 2 - 1 : (source.naturalHeight || source.height) / 2 - 1, 2, 2, 0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data].slice(0, 3);
    };
    const video = monitor.querySelector('video');
    const canvas = monitor.querySelector('canvas');
    const image = [...monitor.querySelectorAll('img')].find((img) => img.complete && img.naturalWidth && visible(img));
    const ready = video && visible(video.parentElement) && video.readyState >= 2 && !video.seeking;
    const drawn = ready ? video : visible(canvas) && canvas.width ? canvas : image;
    return { clip: monitor.dataset.clip, pixel: drawn ? sample(drawn) : null, time: document.querySelector('.focus-viewport .tl-time')?.textContent };
  });
  const live = async (frame, name, colour) => {
    await seek(frame);
    return until(`${name} in the player`, showing, (now) => now.clip === name && looksLike(now.pixel, colour)).then(() => true, async () => showing());
  };

  // Reorder: blue goes first.
  const lift = await focus.locator('.tl-clip[data-name="blue"]').boundingBox();
  const start = await lane();
  await page.mouse.move(lift.x + lift.width / 2, lift.y + 20);
  await page.mouse.down();
  await page.mouse.move(start.x + 4, lift.y + 20, { steps: 12 });
  await page.mouse.up();
  check('reordered', await until('blue first', order, (now) => now === 'blue title red').then(() => true, async () => order()));

  // Trim: red's end pulled in, the bubble saying where; an arrow key a frame more.
  const pull = async (at, side, by) => {
    await settled(page);
    await still();
    const handle = await focus.locator(`.tl-clip >> nth=${at} >> .tl-handle.${side}`).boundingBox();
    await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
    await page.mouse.down();
    await page.mouse.move(handle.x + handle.width / 2 + by, handle.y + handle.height / 2, { steps: 6 });
    const bubble = await focus.locator('.tl-bubble').textContent();
    await page.mouse.up();
    return bubble;
  };
  const bubble = await pull(2, 'out', -Math.round(16 * (await scale())));
  check('timecode_while_trimming', /^Out \d\d:\d\d:\d\d:\d\d, \d+ f$/.test(bubble), bubble);
  check('bubble_gone_after', (await focus.locator('.tl-bubble').count()) === 0);
  const pulled = (await clips())[2].out;
  check('trimmed', pulled < 2 * FPS, await clips());
  await page.keyboard.press('ArrowLeft');
  check('a_frame_by_the_arrow', await until('a frame off its end', async () => (await clips())[2].out, (out) => out === pulled - 1).then(() => true, () => false));
  // A moment after the cut stops changing it's a version, with nothing pressed.
  check('joined_unasked', await until('a version of the cut so far', async () => (await edit()).prefixes?.length ?? 0, (n) => n >= 2, 120_000).then(() => true, () => false));

  // The player shows the cut as it is now, at each frame the playhead goes to.
  const now = await clips();
  check('player_shows_blue', await live(10, 'blue', BLUE), await showing());
  check('player_shows_the_still', await live(now[0].frames + 10, 'title', GREEN), await showing());
  check('player_shows_red', await live(now[0].frames + now[1].frames + 5, 'red', RED), await showing());
  check('transport_time', /^00:00:\d\d:\d\d \/ 00:00:\d\d:\d\d$/.test((await showing()).time ?? ''), (await showing()).time);

  // Split at the playhead (S): blue in two, the pieces meeting where it was.
  await seek(36);
  await page.keyboard.press('s');
  const halves = await until('blue in two', clips, (held) => held.map((clip) => clip.name).join(' ') === 'blue blue title red').catch(() => clips());
  check('split_at_the_playhead', halves.length === 4 && halves[0].out === 36 && halves[1].in === 36 && halves[0].in === 0, halves);

  // Delete the second piece with its trash can: it goes, and Undo brings it back; then the Delete key.
  const second = await focus.locator('.tl-clip >> nth=1').boundingBox();
  await page.mouse.click(second.x + second.width / 2, second.y + 20);
  await focus.getByRole('button', { name: 'Delete clip', exact: true }).click();
  check('deleted', await until('a piece gone', order, (held) => held === 'blue title red').then(() => true, async () => order()));
  const undo = page.locator('.note button', { hasText: 'Undo' }).last();
  await undo.click();
  // The pointer off the notes: one under it stays up until it's let go.
  await page.mouse.move(2, 500);
  check('undo_brings_it_back', await until('the piece back', order, (held) => held === 'blue blue title red').then(() => true, async () => order()));
  const again = await focus.locator('.tl-clip >> nth=1').boundingBox();
  await page.mouse.click(again.x + again.width / 2, again.y + 20);
  await page.keyboard.press('Delete');
  check('deleted_by_the_key', await until('the piece gone again', order, (held) => held === 'blue title red').then(() => true, async () => order()));
  check('box_stays', (await page.locator('.focus-viewport').count()) === 1);
  const cut = await clips();
  const expected = cut.reduce((sum, clip) => sum + clip.frames, 0);
  seen.cut = { cut, expected };
  await shot('edited');

  // Joined here, free, with nothing pressed: the newest version is this cut, its film exactly as long, its pieces where they are.
  const newest = await until('a version of this cut', async () => {
    const held = (await edit()).prefixes?.at(-1);
    const film = held?.main?.path && join(folder, held.main.path);
    return film && existsSync(film) && countFrames(film) === expected ? { held, film } : null;
  }, Boolean, 180_000).catch(async () => ({ held: (await edit()).prefixes?.at(-1) }));
  check('version_with_the_cuts_duration', Boolean(newest.film), { newest: newest.held, expected });
  const versions = (await edit()).prefixes ?? [];
  seen.versions = versions.length;
  check('every_edit_kept', versions.length >= 3, versions.map((version) => version.number));
  check('versions_in_the_takes_column', await until('its versions beside it', () => focus.locator('.here-side .take[data-take]').count(), (n) => n === versions.length).then(() => true, () => false));
  if (newest.film) {
    const film = newest.film;
    const sound = streamSeconds(film, 'a'), picture = streamSeconds(film, 'v');
    check('sound_with_each_clip', Math.abs(sound - picture) < 0.1 && sound > 0, { sound, picture });
    check('film_blue_first', looksLike(pixelAt(film, 2), BLUE), pixelAt(film, 2));
    check('film_still_next', looksLike(pixelAt(film, cut[0].frames + Math.floor(cut[1].frames / 2)), GREEN), pixelAt(film, cut[0].frames + Math.floor(cut[1].frames / 2)));
    check('film_red_last', looksLike(pixelAt(film, expected - 2), RED), pixelAt(film, expected - 2));
    // The timeline, read back by OpenTimelineIO's own reader.
    const assets = (await read()).assets;
    const file = (name) => assets.find((asset) => asset.name === name).path;
    const otioClip = (clip) => (clip.name === 'title' ? `title:0:${clip.frames}:${file('title')}:still` : `${clip.name}:${clip.in}:${clip.out}:${file(clip.name)}`);
    const otio = spawnSync('uv', ['run', '-q', '--no-project', '--with', 'opentimelineio==0.17.0', 'python', '-I', join(__dirname, '..', 'project', 'otio_check.py'), join(folder, `${timelineName}.otio`), String(FPS), ...cut.map(otioClip)], { encoding: 'utf8' });
    check('otio_reads_back', otio.status === 0 && /otio PASS/.test(otio.stdout), `${otio.stdout}${otio.stderr}`.slice(-1500));
  }

  // Zoomed in, the track is wider than the view; Fit brings the whole cut back.
  const wide = () => focus.locator('.tl-track').evaluate((el) => el.getBoundingClientRect().width);
  const fitted = await wide();
  await focus.getByRole('button', { name: 'Zoom in timeline', exact: true }).click();
  await focus.getByRole('button', { name: 'Zoom in timeline', exact: true }).click();
  check('zooms', await until('wider', wide, (w) => w > fitted * 1.5).then(() => true, () => false));
  await focus.getByRole('button', { name: 'Fit timeline', exact: true }).click();
  check('fits', await until('the whole cut', wide, (w) => Math.abs(w - fitted) < 2).then(() => true, () => false));

  // Looked at (Paul, 11-12): each size people use, on paper and on dark, with a clip selected and the playhead in a clip.
  const theme = async (to) => {
    await page.click('summary[aria-label="Menu"]');
    await page.getByRole('button', { name: to, exact: true }).click();
    await settled(page);
  };
  // The notes said so far, dismissed: the page's notes sit at its bottom left, over whatever's there, for as long as they're up.
  while (await page.locator('.note button[aria-label="Dismiss"]').count()) await page.locator('.note button[aria-label="Dismiss"]').first().click();
  await seek(20);
  await page.mouse.click((await focus.locator('.tl-clip >> nth=1').boundingBox()).x + 30, (await focus.locator('.tl-clip >> nth=1').boundingBox()).y + 20);
  for (const [width, height] of [[1440, 900], [1920, 1080]]) {
    await page.setViewportSize({ width, height });
    await settled(page);
    await shot(`${width}x${height}-paper`);
    await theme('Dark');
    await shot(`${width}x${height}-dark`);
    // Small text and controls, read where they're drawn.
    const small = await page.evaluate(() => [...document.querySelectorAll('.focus-viewport .tl *, .focus-viewport .tl-transport *')]
      .filter((el) => [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim()) && el.getBoundingClientRect().width)
      .map((el) => ({ text: el.textContent.trim().slice(0, 20), px: parseFloat(getComputedStyle(el).fontSize) }))
      .filter((held) => held.px < 16));
    check(`${width}_text_readable`, small.length === 0, small);
    const tiny = await page.evaluate(() => [...document.querySelectorAll('.focus-viewport .tl button, .focus-viewport .tl input, .focus-viewport .tl-transport button, .focus-viewport .tl-transport input')]
      .map((el) => ({ label: el.getAttribute('aria-label'), h: el.getBoundingClientRect().height })).filter((held) => held.h < 28));
    check(`${width}_controls_28px`, tiny.length === 0, tiny);
    await theme('Paper');
  }
  await page.setViewportSize({ width: 1900, height: 1000 });

  // At rest it's its film, as any video box is.
  await closeBox(page);
  await page.keyboard.press('f');
  await settled(page);
  check('at_rest_its_film', await until('its film', () => card(timelineName).locator('.media img').count(), (n) => n >= 1).then(() => true, () => false));
  await shot('rest');

  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  check('no_page_errors', errors.length === 0, errors);
  rmSync(dir, { recursive: true, force: true });
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
