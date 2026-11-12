// The Edit, in a real browser on a real Calcifer: at rest its picture, on a hover its filmstrip, with no <video> until it plays. Real short
// clips and a still dropped in from the canvas, trimmed, reordered, played in the page with no render, and rendered, free, with no machine.
// The film's frames are counted, its `.otio` read back with OpenTimelineIO's own reader. Two different edits make two versions, each with
// its own clips, and the same edit twice makes one; a version carries on and drags out to its own asset; a clip whose asset is deleted says
// so on the clip, and a render goes on without it.
//   node timeline.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI; $SHOT: where to keep its
// screenshots; ffmpeg, ffprobe and uv on PATH. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync, spawnSync } = require('child_process');
const { existsSync, mkdtempSync, readFileSync, rmSync, statSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { implementationRow, projectFromPage, projectJson, calcifer, carry, signIn, STEP_MS, boxOf, closeBox, drop, lastLook, lookAt, noStall, openBox, openCanvas, settled, fitCanvas, until, watchBoxes, pointAt, storedOf } = require('./page');

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
const looksLike = (pixel, want) => pixel.every((value, i) => Math.abs(value - want[i]) < 48);
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

  await signIn(page);
  await page.waitForSelector('.top');
  const projectName = `web-timeline-${Date.now()}`;
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', projectName);
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

  // Real clips, each one color and one tone, and a still with its left half transparent.
  const dir = mkdtempSync(join(tmpdir(), 'web-timeline-'));
  const red = plate(join(dir, 'red.mp4'), 'red', 2, 440);
  const blue = plate(join(dir, 'blue.mp4'), 'blue', 3, 880);
  const stillPath = join(dir, 'title.png');
  ffmpeg('-f', 'lavfi', '-i', "color=c=0x00ff00:size=400x400,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(lt(X,200),0,255)'", '-frames:v', '1', stillPath);
  const stage = await page.locator('.stage').boundingBox();
  await drop(page, [red, blue, { name: 'title.png', type: 'image/png', bytes: readFileSync(stillPath) }], stage.x + 508, stage.y + 104);
  await until('the three landed', async () => (await read()).assets.filter((asset) => asset.sha256).length, (n) => n === 3);
  await page.waitForSelector('.react-flow__node .nm:text-is("title")');

  // The Edit is the first tool, and dropping it makes a box a clip can go in.
  const tools = (await call('GET', '/v1/tools')).body.tools;
  const tool = tools.find((held) => held.id === 'cozy/edit');
  check('edit_is_a_tool', tool?.id === 'cozy/edit' && tool.makes === 'video' && tool.fields[0]?.type === 'timeline', tool);
  // By its id: a package's tool can be titled Edit too (minimax-h3/edit, 11-10).
  const beforeTimeline = new Set((await read()).assets.map((asset) => asset.name));
  await carry(page, await implementationRow(page, 'timeline', 'cozy/edit'), { x: 1208, y: 144 });
  timelineName = (await until('new Video Timeline asset is named', async () => (await read()).assets.find((asset) => asset.tool === 'cozy/edit' && !beforeTimeline.has(asset.name)), Boolean)).name;
  check('dropping_Video_Timeline_stays_in_Wander', await page.locator('[data-focused-asset]').count() === 0);
  const card = (name) => boxOf(page, name);
  await card(timelineName).waitFor();
  // A box shows as it's dropped, ahead of Calcifer's answer: it's the project that's asked.
  check('its_name', Boolean(await until('the Edit in the project', edit, Boolean)));
  // At rest it's its picture, or what it's for: on a hover, its filmstrip, in the same place, and still no video.
  const away = async () => {
    await closeBox(page);
    await page.mouse.move(2, 500);
  };
  const openEdit = async () => {
    await openBox(page, timelineName);
    const words = card(timelineName).locator('.take.add[aria-label="Its words"]:not(.on)');
    if (!(await card(timelineName).locator('.tl-clips').count()) && await words.count()) await words.click();
    await page.waitForSelector('.tl-clips');
  };
  await openBox(page, timelineName);
  await away();
  await until('an Edit at rest', () => card(timelineName).locator('.tl').count(), (n) => n === 0);
  check('at_rest_says_what_it_is_for', (await card(timelineName).locator('.media-note').textContent()) === 'Drag videos and stills here');
  // A box that just arrived is still landing, a little bigger than it rests.
  await until('it has landed', () => page.locator('.card.landing').count(), (n) => n === 0);
  const atRest = await card(timelineName).locator('.media').boundingBox();
  await openEdit();
  const onHover = await card(timelineName).locator('.tl').boundingBox();
  check('opened_it_has_the_room', onHover.width > atRest.width, { atRest, onHover });
  check('hover_opens_no_video', (await page.locator('video').count()) === 0);
  check('edit_has_no_project_picture_gallery', (await card(timelineName).locator('.pictures').count()) === 0);
  // Keep the source boxes and the open Edit in reach for real canvas-to-track drags.
  await page.click('.zoom-at');
  await page.getByRole('menuitem', { name: 'Zoom to 50%', exact: true }).click();
  await settled(page);
  const clips = () => page.$$eval('.tl-clip', (els) => els.map((el) => ({ name: el.dataset.name, in: Number(el.dataset.in), out: Number(el.dataset.out), frames: Number(el.dataset.frames), missing: el.dataset.missing })));
  const order = async () => (await clips()).map((clip) => clip.name).join(' ');
  // A real drag, by its head, on a real mouse: back where it started, and in the box.
  // A box goes from the canvas into a place on the Edit's track.
  const dragIn = async (name, x, y) => {
    await openEdit();
    await carry(page, card(name).locator('.nm'), { x, y });
    await until(`${name} on the track`, order, (now) => now.includes(name));
    return true;
  };
  // Clips close up with a little overshoot: a person takes hold of what has stopped.
  const closedUp = () => page.evaluate(() => Promise.all(document.querySelector('.tl').getAnimations({ subtree: true }).map((held) => held.finished.catch(() => {}))));
  const strip = async () => {
    await openEdit();
    await closedUp();
    return page.locator('.tl-clips').boundingBox();
  };
  let track = await strip();
  check('dragged_back', await dragIn('red', track.x + track.width / 2, track.y + 30));
  track = await strip();
  await dragIn('blue', track.x + track.width - 8, track.y + 30);
  track = await strip();
  // At the point it's dropped: before red.
  await dragIn('title', track.x + 4, track.y + 30);
  check('dropped_at_the_point', (await order()) === 'title red blue', await order());
  // Whole, once measured: a clip Calcifer hasn't read yet lasts as a still does, for as long as ffprobe takes.
  const whole = JSON.stringify([3 * FPS, 2 * FPS, 3 * FPS]);
  const start = await until('its clips measured', clips, (now) => JSON.stringify(now.map((clip) => clip.frames)) === whole).catch(() => clips());
  check('whole_takes', JSON.stringify(start.map((clip) => clip.frames)) === whole, start);
  // Its tracks as project.db stores them.
  const tracks = () => Promise.resolve(JSON.stringify(storedOf(folder)[timelineName]?.tracks ?? null));
  await until('the project holds them', tracks, (text) => /<title>[\s\S]*<red>[\s\S]*<blue>/.test(text));
  check('a_filmstrip_each', await until('its frames', () => page.locator('.tl-clip img').count(), (n) => n >= 3));
  await away();
  check('at_rest_its_first_clip', await until('its first clip\'s picture', () => card(timelineName).locator('.media img').count(), (n) => n === 1));
  // The built-in composition tool is removable, but its editor stays closed at rest.
  check('at_rest_timeline_is_closed_and_removable', await card(timelineName).locator('.tl').count() === 0 && await card(timelineName).getByRole('button', { name: new RegExp(`^Take .* off ${timelineName}$`) }).isVisible(), await card(timelineName).innerText());
  await openEdit();

  // Pull an end: the bubble says the timecode, the clips stay snapped. What's taken hold of has stopped: the canvas too, which draws in on
  // the box again when a version picked makes it another size, and moved under a pull it took it nowhere (macOS, 42113601659: `carried_on`).
  const pull = async (name, side, by, off = 0) => {
    // Render ends working on it: its clips are on it again once it's opened.
    await openEdit();
    await settled(page);
    await closedUp();
    const handle = await page.locator(`.tl-clip[data-name="${name}"] .tl-handle.${side}`).boundingBox();
    await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
    const under = await page.evaluate(({ x, y, name, side }) => {
      const held = document.elementFromPoint(x, y);
      return held?.closest('.tl-handle') === document.querySelector(`.tl-clip[data-name="${name}"] .tl-handle.${side}`) ? null : `${held?.className}`;
    }, { x: handle.x + handle.width / 2, y: handle.y + handle.height / 2, name, side });
    if (under !== null) throw new Error(`${name}'s ${side} handle is under ${under}`);
    await page.mouse.down();
    await page.mouse.move(handle.x + handle.width / 2 + by, handle.y + handle.height / 2 + off, { steps: 5 });
    const bubble = await page.locator('.tl-bubble').textContent();
    await page.mouse.up();
    return bubble;
  };
  const bubble = await pull('red', 'out', -30);
  check('timecode_while_trimming', /^Out \d\d:\d\d:\d\d:\d\d, \d+ f$/.test(bubble), bubble);
  check('timecode_gone_after', await page.locator('.tl-bubble').count() === 0);
  // An exact frame: the arrow keys move what was just pulled.
  const pulled = (await clips()).find((clip) => clip.name === 'red').out;
  await page.keyboard.press('ArrowLeft');
  await until('a frame off the out point', async () => (await clips()).find((clip) => clip.name === 'red').out, (out) => out === pulled - 1);
  await pull('blue', 'in', 40);
  // A press holds it open wherever the pointer goes: this one lets go well off the box.
  check('a_press_holds_it_open', /^\d\d:\d\d:\d\d:\d\d, \d+ f$/.test(await pull('title', 'out', -60, 300)));
  await until('three trims saved', tracks, (text) => (text.match(/"(frames|in|out)":/g) ?? []).length >= 3);
  // Reorder: blue goes first.
  const first = await strip();
  const body = await page.locator('.tl-clip[data-name="blue"]').boundingBox();
  await page.mouse.move(body.x + body.width / 2, body.y + 24);
  await page.mouse.down();
  await page.mouse.move(first.x + 4, body.y + 24, { steps: 12 });
  await page.mouse.up();
  await until('reordered', order, (now) => now === 'blue title red');
  const cut = await until('the edit reads back', async () => (await edit()).timeline?.tracks[0].clips, (now) => now?.[0]?.name === 'blue');
  const placed = await clips();
  const expected = placed.reduce((sum, clip) => sum + clip.frames, 0);
  check('trims_exact_in_the_read', placed.every((clip, i) => (clip.name === 'title' ? cut[i].frames === clip.frames : (cut[i].in ?? 0) === clip.in)), { placed, cut });

  // Scrub, on an exact frame, with the frame shown, holding one video; a pause lets go of its slot and keeps the frame.
  const rail = await page.locator('.tl-rail').boundingBox();
  await page.mouse.click(rail.x + 40, rail.y + rail.height / 2);
  const monitor = () => page.evaluate(() => {
    const el = document.querySelector('.tl-monitor');
    const video = el.querySelector('video');
    return { clip: el.dataset.clip, live: el.dataset.live !== undefined, videos: document.querySelectorAll('.tl video').length, src: video?.getAttribute('src'), t: video?.currentTime, frame: Number(document.querySelector('.tl-playhead').dataset.frame) };
  });
  const scrubbed = await until('a live video on that frame', monitor, (now) => now.live && now.clip === 'blue' && now.t > 0);
  check('one_video', scrubbed.videos === 1, scrubbed);
  check('frame_shown', Math.abs(scrubbed.t - (placed[0].in + scrubbed.frame + 0.5) / FPS) < 0.05, scrubbed);
  await until('it lets go, and no video is left', monitor, (now) => !now.live && now.videos === 0);

  // Plays in order in the page, with no render, every clip's turn.
  await page.evaluate(() => {
    window.played = [];
    const el = document.querySelector('.tl-monitor');
    new MutationObserver(() => { if (window.played.at(-1) !== el.dataset.clip) window.played.push(el.dataset.clip); }).observe(el, { attributes: true });
  });
  await page.mouse.click(rail.x + 1, rail.y + rail.height / 2);
  await page.locator('.tl-play').click();
  await until('it played to its end', () => page.locator('.tl-play').getAttribute('aria-label'), (label) => label === 'Play', STEP_MS);
  const played = await page.evaluate(() => window.played);
  check('played_in_order', played.join(' ').endsWith('blue title red'), played);
  check('no_render_to_play', (await edit()).versions === 0, (await edit()).versions);
  await page.screenshot({ path: process.env.SHOT ? `${process.env.SHOT}-edit.png` : undefined });

  // Render: free, with no machine, showing how far it is on the box it left, and a version like any other.
  const render = async () => {
    await openEdit();
    await card(timelineName).locator('button[aria-label="Render"]').click();
  };
  await page.evaluate(() => { window.rendering = 0; new MutationObserver(() => { if (document.querySelector('[role="progressbar"][aria-label="Rendering"]')) window.rendering += 1; }).observe(document.body, { subtree: true, childList: true, attributes: true }); });
  await render();
  const landed = (count) => until(`${count} versions`, async () => (await edit()).versions, (n) => n === count, 180_000);
  await landed(1);
  const film = join(folder, `${timelineName}.mp4`);
  await until('its film', () => existsSync(film), Boolean);
  check('frames_exact', countFrames(film) === expected, { film: countFrames(film), expected });
  const sound = streamSeconds(film, 'a'), picture = streamSeconds(film, 'v');
  check('sound_with_each_clip', Math.abs(sound - picture) < 0.1 && sound > 0, { sound, picture });
  const inClip = (at, want, name) => check(`film_frame_${name}`, looksLike(pixelAt(film, at), want), { at, pixel: pixelAt(film, at) });
  inClip(0, BLUE, 'blue_first');
  const titleClip = placed[1];
  inClip(placed[0].frames + Math.floor(titleClip.frames / 2), GREEN, 'still_center_cropped');
  inClip(placed[0].frames + titleClip.frames + 1, RED, 'red_last');
  // A still's transparent half is black.
  const edge = execFileSync('ffmpeg', ['-v', 'error', '-i', film, '-vf', `select=eq(n\\,${placed[0].frames + 2}),crop=2:2:4:ih/2,scale=1:1`, '-frames:v', '1', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], { stdio: ['ignore', 'pipe', 'inherit'] });
  check('transparent_is_black', looksLike([...edge], [0, 0, 0]), [...edge]);

  // The timeline, read back by OpenTimelineIO's own reader.
  const assets = (await read()).assets;
  const file = (name) => assets.find((asset) => asset.name === name).path;
  const otioClip = (clip) => (clip.name === 'title' ? `title:0:${clip.frames}:${file('title')}:still` : `${clip.name}:${clip.in}:${clip.out}:${file(clip.name)}`);
  const otio = spawnSync('uv', ['run', '-q', '--no-project', '--with', 'opentimelineio==0.17.0', 'python', '-I', join(__dirname, '..', 'project', 'otio_check.py'), join(folder, `${timelineName}.otio`), String(FPS), ...placed.map(otioClip)], { encoding: 'utf8' });
  check('otio_reads_back', otio.status === 0 && /otio PASS/.test(otio.stdout), `${otio.stdout}${otio.stderr}`.slice(-1500));

  // The same edit again renders nothing.
  const bytes = statSync(film).mtimeMs;
  await render();
  await until('asked and settled', async () => !(await read()).busy, Boolean);
  await page.waitForTimeout(1000);
  check('same_edit_one_version', (await edit()).versions === 1, (await edit()).versions);
  check('same_edit_no_render', statSync(film).mtimeMs === bytes, { before: bytes, after: statSync(film).mtimeMs });

  // A different edit is another version, with clips of its own.
  await pull('blue', 'out', -40);
  await until('the file moved', async () => (await edit()).standing, (standing) => standing !== 'current');
  const second = await clips();
  await render();
  await landed(2);
  const versions = (await edit()).prefixes;
  check('two_versions', versions.length === 2 && versions[0].main.sha256 !== versions[1].main.sha256, versions);
  const framesOf = (version) => version.timeline.tracks[0].clips.map((clip) => clip.out ?? clip.frames);
  check('each_keeps_its_clips', JSON.stringify(framesOf(versions[0])) !== JSON.stringify(framesOf(versions[1])), versions.map(framesOf));
  check('second_frames_exact', countFrames(film) === second.reduce((sum, clip) => sum + clip.frames, 0), { film: countFrames(film), second });

  // Back to the first: its arrangement shows, and an edit carries on from there.
  await openBox(page, timelineName);
  await card(timelineName).locator('.take[aria-label$=" 1"]').click();
  await until('version 1\'s clips', clips, (now) => now[0].out === placed[0].out);
  await pull('blue', 'in', 10);
  await until('carried on from it', async () => (await edit()).fields.timeline.tracks[0].clips[0].out, (out) => out === placed[0].out);
  check('carried_on', (await edit()).fields.timeline.tracks[0].clips[0].in > placed[0].in, (await edit()).fields.timeline);

  // Dragged out, a version is an asset of its own with the same file, and no new bytes.
  await openBox(page, timelineName);
  // Its takes are a row under it, which drags across: one leaves it up or down.
  await carry(page, card(timelineName).locator('.take[data-take][aria-label$=" 1"]'), await openCanvas(page), 'y');
  const dragged = await until('its own asset', async () => (await read()).assets.find((asset) => asset.sha256 === versions[0].main.sha256 && asset.name !== timelineName), Boolean);
  check('dragged_out', Boolean(dragged), (await read()).assets.map((asset) => asset.name));

  // Approved by a double tap, that version is what the Edit is, and its film follows.
  await openBox(page, timelineName);
  await card(timelineName).locator('.take[aria-label$=" 2"]').dblclick();
  await until('approved', async () => (await edit()).approved, (number) => number === versions[1].number);
  check('film_follows_the_approved', await until('its film', () => spawnSync('ffprobe', ['-v', 'error', film]).status === 0 && countFrames(film), (n) => n === second.reduce((sum, clip) => sum + clip.frames, 0)));
  await openBox(page, timelineName);
  // Approved, it shows its film, and only an Edit has + for its words.
  check('an_edit_has_its_words', (await card(timelineName).locator('.take.add[aria-label="Its words"]').count()) === 1, await card(timelineName).innerText());
  // Unapproved, it has no film of what it holds now, so it's cut there and then, and the call answers when that's done: what's counted below is what's asked below.
  // Again on it, unapproved.
  await Promise.all([page.waitForResponse((answer) => answer.url().endsWith('/unpin')), card(timelineName).locator('.take.approved').dblclick()]);
  await until('unapproved', async () => (await edit()).pinned, (pinned) => !pinned);
  await openEdit();

  // Its right click: the film's shape and size, as a render makes them, and a rate that keeps every clip's times.
  const setting = async (label, value) => {
    await openBox(page, timelineName);
    await card(timelineName).getByRole('button', { name: 'Rename node', exact: true }).click({ button: 'right' });
    await page.selectOption(`.asset-menu select[aria-label="${label}"]`, value);
    await page.keyboard.press('Escape');
  };
  await setting('Aspect ratio', '1:1');
  await setting('Resolution', '720p');
  await until('both in the file', async () => (await edit()).fields, (fields) => fields.aspect_ratio === '1:1' && fields.resolution === '720p');
  const shaped = (await edit()).versions;
  await render();
  await landed(shaped + 1);
  const size = probe(film, '-select_streams', 'v:0', '-show_entries', 'stream=width,height', '-of', 'csv=p=0');
  check('menu_shapes_the_film', size === '720,720', size);
  // By now a render long enough to see: the first, of a few small flat clips, can be over before a read comes back. Rendering leaves the
  // box, so it's where it stands that says how far: once only an open Edit did, and only the cut an Unapprove makes was seen (42113601659).
  check('progress_shown', (await page.evaluate(() => window.rendering)) > 0);
  const timed = (await edit()).fields.timeline.tracks[0].clips;
  await setting('Frame rate', String(FPS * 2));
  const retimed = await until('the rate in the file', async () => (await edit()).fields, (fields) => fields.fps === FPS * 2);
  check('rate_keeps_times', retimed.timeline.tracks[0].clips.every((clip, i) => ['in', 'out', 'frames'].every((point) => (timed[i][point] === undefined ? clip[point] === undefined : clip[point] === timed[i][point] * 2))), { timed, retimed: retimed.timeline });
  await setting('Frame rate', String(FPS));
  await setting('Aspect ratio', '');
  await setting('Resolution', '');
  await until('back as it was', async () => (await edit()).fields, (fields) => fields.fps === FPS && fields.aspect_ratio === undefined && fields.resolution === undefined);
  await openEdit();

  // A real incoming clip waits on its file. Hold only its transport request:
  // the previous decoded frame must stay visible, and pausing must not be undone
  // when the delayed response finally arrives. The rendered Edit above is unchanged.
  const handoff = await call('POST', `/v1/projects/${id}/assets`, {
    tool: 'cozy/edit', name: 'handoff_preview', exact: true,
    fields: { timeline: { tracks: [{ clips: [{ clip: '<blue>' }, { clip: '<red>' }] }] } },
  });
  if (handoff.status !== 201 || !handoff.body?.name) throw new Error(`preview fixture refused: ${handoff.status}`);
  const handoffName = handoff.body.name;
  const incomingPath = `/v1/projects/${id}/${file('red').split('/').map(encodeURIComponent).join('/')}`;
  const incoming = (url) => url.pathname === incomingPath;
  const heldRequests = [];
  let holdIncoming = true;
  const delayIncoming = (route) => holdIncoming ? heldRequests.push(route) : route.continue();
  const previewPixels = () => page.evaluate((name) => {
    const monitor = document.querySelector(`.card[data-asset="${name}"] .tl-monitor`);
    const canvas = monitor?.querySelector('canvas');
    const video = monitor?.querySelector('video');
    const visible = (element) => Boolean(element && getComputedStyle(element).visibility !== 'hidden' && element.getBoundingClientRect().width > 0);
    const sample = (source) => {
      const copy = document.createElement('canvas'); copy.width = 1; copy.height = 1;
      const context = copy.getContext('2d');
      context.drawImage(source, 0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data].slice(0, 3);
    };
    const videoReady = Boolean(video && !video.seeking && video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA);
    return {
      clip: monitor?.dataset.clip,
      canvas_visible: visible(canvas), canvas_pixel: canvas?.width ? sample(canvas) : null,
      video_visible: visible(video), video_ready: videoReady, video_paused: video?.paused ?? true,
      video_pixel: videoReady ? sample(video) : null,
    };
  }, handoffName);
  try {
    await away();
    await fitCanvas(page);
    await openBox(page, handoffName);
    await card(handoffName).locator('.tl-monitor').waitFor();
    await page.route(incoming, delayIncoming);
    await card(handoffName).getByRole('button', { name: 'Play', exact: true }).click();
    await until('first preview clip decoded', previewPixels, (now) => now.clip === 'blue' && now.video_visible && now.video_ready && looksLike(now.video_pixel, BLUE));
    await until('incoming preview request held', () => heldRequests.length, (count) => count > 0);
    await until('preview entered its delayed clip', previewPixels, (now) => now.clip === 'red');
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    const waitingFrame = await previewPixels();
    check('preview_handoff_keeps_last_decoded_frame', waitingFrame.canvas_visible && Array.isArray(waitingFrame.canvas_pixel) && looksLike(waitingFrame.canvas_pixel, BLUE) && !waitingFrame.video_visible, waitingFrame);
    await card(handoffName).getByRole('button', { name: 'Pause', exact: true }).click();
    const arrivals = heldRequests.map((route) => route.request());
    holdIncoming = false;
    for (const route of heldRequests.splice(0)) await route.continue();
    let transfersSettled = false, transferError;
    void Promise.all(arrivals.map(async (request) => {
      const response = await request.response();
      if (response) await response.finished();
    })).then(() => { transfersSettled = true; }, (error) => { transferError = error; });
    await until('delayed preview transfer settled', () => {
      if (transferError) throw transferError;
      return transfersSettled;
    }, Boolean);
    await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    check('preview_handoff_does_not_resume_after_pause', await card(handoffName).getByRole('button', { name: 'Play', exact: true }).count() === 1 && (await previewPixels()).video_paused);
    await card(handoffName).getByRole('button', { name: 'Play', exact: true }).click();
    const nextFrame = await until('incoming preview clip decoded', previewPixels, (now) => now.clip === 'red' && now.video_visible && now.video_ready && looksLike(now.video_pixel, RED));
    check('preview_handoff_reveals_decoded_next_frame', true, nextFrame);
  } finally {
    holdIncoming = false;
    for (const route of heldRequests.splice(0)) await route.continue().catch(() => {});
    await page.unroute(incoming, delayIncoming);
    await call('DELETE', `/v1/projects/${id}/assets/${encodeURIComponent(handoffName)}`);
  }
  // Deleted while open: the page leaves it once it hears so. Until then the address still names it, and a box asked for waits on it (cl-877).
  await page.waitForFunction((gone) => !location.pathname.endsWith(`/assets/${encodeURIComponent(gone)}`), handoffName);
  await openEdit();

  // Its asset deleted: the clip says so, plainly, and a render goes on without it.
  await away();
  // Back at rest its X may be past the window: F brings it all in, as for a person.
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  await pointAt(page, card('red').locator('.nm'));
  await card('red').locator('button[aria-label="Delete"]').click();
  await openEdit();
  await until('the clip says it\'s offline', clips, (now) => now.some((clip) => clip.name === 'red' && clip.missing === 'no longer in the project'));
  await page.screenshot({ path: process.env.SHOT ? `${process.env.SHOT}-offline.png` : undefined });
  check('offline_on_the_clip', /no longer in the project/.test(await page.locator('.tl-clip[data-name="red"]').textContent()));
  const before = (await edit()).versions;
  const left = (await clips()).filter((clip) => !clip.missing).reduce((sum, clip) => sum + clip.frames, 0);
  await render();
  await landed(before + 1);
  check('render_without_it', countFrames(film) === left, { film: countFrames(film), left });
  check('says_what_it_went_without', (await edit()).prefixes.at(-1).without?.some((line) => line.startsWith('red')), (await edit()).prefixes.at(-1));

  if (process.env.COZY) {
    const status = projectJson(folder);
    check('api_sees_its_versions', status.assets.find((asset) => asset.name === timelineName)?.versions === before + 1, status.assets.find((asset) => asset.name === timelineName));
  }
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
