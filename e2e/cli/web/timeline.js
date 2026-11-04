// The Edit, in a real browser on a real Calcifer: at rest its picture, on a hover its filmstrip, with no <video> until it plays. Real short
// clips and a still dropped in from the canvas, trimmed, reordered, played in the page with no render, and rendered, free, with no machine.
// The film's frames are counted, its `.otio` read back with OpenTimelineIO's own reader. Two different edits make two versions, each with
// its own clips, and the same edit twice makes one; a version carries on and drags out to its own asset; a clip whose asset is deleted says
// so on the clip, and a render goes on without it.
//   node timeline.js <sign-in link from `cozy web --no-open`>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI; $SHOT: where to keep its
// screenshots; ffmpeg, ffprobe and uv on PATH. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync, spawnSync } = require('child_process');
const { existsSync, mkdtempSync, readFileSync, rmSync, statSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { STEP_MS, drop, lastLook, lookAt, until } = require('./page');

const failures = [];
const seen = {};
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
  const link = process.argv[2];
  const base = new URL(link).origin;
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1900, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d/.test(m.text())) errors.push(`console: ${m.text()}`); });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });

  await page.goto(link);
  await page.waitForSelector('.top');
  const projectName = `web-timeline-${Date.now()}`;
  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', projectName);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const call = (method, path, body) => page.evaluate(async ({ method, path, body }) => {
    const { token } = JSON.parse(localStorage.getItem('cozy.session'));
    const response = await fetch(path, { method, headers: { Authorization: `Bearer ${token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) }, body: body ? JSON.stringify(body) : undefined });
    return { status: response.status, body: await response.json().catch(() => null) };
  }, { method, path, body });
  const read = async () => (await call('GET', `/v1/projects/${id}`)).body;
  const folder = (await read()).path;
  seen.project = { id, folder };
  const edit = async () => (await read()).assets.find((asset) => asset.name === 'Edit');

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
  check('edit_is_a_tool', tool?.label === 'Edit' && tool.kind === 'video' && tool.controls[0]?.type === 'timeline', tool);
  await page.locator('.tools .tool').filter({ has: page.locator('b:text-is("Edit")') }).dragTo(page.locator('.stage'), { targetPosition: { x: 1208, y: 144 } });
  const card = (name) => page.locator('.react-flow__node').filter({ has: page.locator(`.nm:text-is("${name}")`) });
  await card('Edit').waitFor();
  check('its_name', Boolean(await edit()), (await read()).assets.map((asset) => asset.name));
  // At rest it's its picture, or what it's for: on a hover, its filmstrip, in the same place, and still no video.
  const away = () => page.mouse.move(2, 500);
  const openEdit = async () => {
    await card('Edit').locator('.head').hover();
    await page.waitForSelector('.tl-clips');
  };
  await card('Edit').locator('.head').hover();
  await away();
  await until('an Edit at rest', () => card('Edit').locator('.tl').count(), (n) => n === 0);
  check('at_rest_says_what_it_is_for', (await card('Edit').locator('.media-note').textContent()) === 'Drag videos and stills here');
  const atRest = await card('Edit').locator('.card').boundingBox();
  await openEdit();
  const onHover = await card('Edit').locator('.tl').boundingBox();
  check('a_hover_moves_nothing', Math.abs(onHover.height - (atRest.height - (onHover.y - atRest.y))) < 2, { atRest, onHover });
  check('hover_opens_no_video', (await page.locator('video').count()) === 0);
  const clips = () => page.$$eval('.tl-clip', (els) => els.map((el) => ({ name: el.dataset.name, in: Number(el.dataset.in), out: Number(el.dataset.out), frames: Number(el.dataset.frames), missing: el.dataset.missing })));
  const order = async () => (await clips()).map((clip) => clip.name).join(' ');
  // A real drag, by its head, on a real mouse: back where it started, and in the box.
  const dragIn = async (name, x, y) => {
    const before = await card(name).boundingBox();
    const head = await card(name).locator('.head').boundingBox();
    await page.mouse.move(head.x + head.width / 2, head.y + head.height / 2);
    await page.mouse.down();
    await page.mouse.move(head.x + head.width / 2 + 12, head.y + head.height / 2 + 12, { steps: 3 });
    await page.mouse.move(x, y, { steps: 10 });
    await page.mouse.up();
    await until(`${name} on the track`, order, (now) => now.includes(name));
    const after = await card(name).boundingBox();
    return Math.abs(before.x - after.x) < 1 && Math.abs(before.y - after.y) < 1;
  };
  // Clips close up with a little overshoot: a person takes hold of what has stopped.
  const settled = () => page.evaluate(() => Promise.all(document.querySelector('.tl').getAnimations({ subtree: true }).map((held) => held.finished.catch(() => {}))));
  const strip = async () => {
    await openEdit();
    await settled();
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
  const start = await clips();
  check('whole_takes', JSON.stringify(start.map((clip) => clip.frames)) === JSON.stringify([3 * FPS, 2 * FPS, 3 * FPS]), start);
  await until('the file holds them', () => readFileSync(join(folder, 'project.yaml'), 'utf8'), (text) => /<title>[\s\S]*<red>[\s\S]*<blue>/.test(text));
  check('a_filmstrip_each', await until('its frames', () => page.locator('.tl-clip img').count(), (n) => n >= 3));
  await away();
  check('at_rest_its_first_clip', await until('its first clip\'s picture', () => card('Edit').locator('.media img').count(), (n) => n === 1));
  check('at_rest_nothing_to_press', (await card('Edit').locator('.tl, button').count()) === 0, await card('Edit').innerText());
  await openEdit();

  // Pull an end: the bubble says the timecode, the clips stay snapped.
  const pull = async (name, side, by) => {
    await settled();
    const handle = await page.locator(`.tl-clip[data-name="${name}"] .tl-handle.${side}`).boundingBox();
    await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
    const under = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.className, { x: handle.x + handle.width / 2, y: handle.y + handle.height / 2 });
    if (!`${under}`.includes('tl-handle')) throw new Error(`${name}'s ${side} handle is under ${under}`);
    await page.mouse.down();
    await page.mouse.move(handle.x + handle.width / 2 + by, handle.y + handle.height / 2, { steps: 5 });
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
  await pull('title', 'out', -60);
  await until('three trims saved', () => readFileSync(join(folder, 'project.yaml'), 'utf8'), (text) => (text.match(/frames:|start:|end:/g) ?? []).length >= 3);
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

  // Render: free, with no machine, showing how far it is, and a version like any other.
  const render = async () => {
    await card('Edit').locator('.head').hover();
    await card('Edit').locator('button[aria-label="Render"]').click();
  };
  await page.evaluate(() => { window.rendering = 0; new MutationObserver(() => { if (document.querySelector('.tl-rendering')) window.rendering += 1; }).observe(document.body, { subtree: true, childList: true, attributes: true }); });
  await render();
  const landed = (count) => until(`${count} versions`, async () => (await edit()).versions, (n) => n === count, 180_000);
  await landed(1);
  check('progress_shown', (await page.evaluate(() => window.rendering)) > 0);
  const film = join(folder, 'Edit.mp4');
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
  const otio = spawnSync('uv', ['run', '-q', '--no-project', '--with', 'opentimelineio==0.17.0', 'python', '-I', join(__dirname, '..', 'project', 'otio_check.py'), join(folder, 'Edit.otio'), String(FPS), ...placed.map(otioClip)], { encoding: 'utf8' });
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
  await card('Edit').locator('.head').hover();
  await card('Edit').locator('.take').first().click();
  await until('version 1\'s clips', clips, (now) => now[0].out === placed[0].out);
  await pull('blue', 'in', 10);
  await until('carried on from it', async () => (await edit()).fields.timeline.tracks[0].clips[0].out, (out) => out === placed[0].out);
  check('carried_on', (await edit()).fields.timeline.tracks[0].clips[0].in > placed[0].in, (await edit()).fields.timeline);

  // Dragged out, a version is an asset of its own with the same file, and no new bytes.
  await card('Edit').locator('.head').hover();
  await card('Edit').locator('.take[draggable=true]').first().dragTo(page.locator('.stage'), { targetPosition: { x: 508, y: 744 } });
  const dragged = await until('its own asset', async () => (await read()).assets.find((asset) => asset.sha256 === versions[0].main.sha256 && asset.name !== 'Edit'), Boolean);
  check('dragged_out', Boolean(dragged), (await read()).assets.map((asset) => asset.name));

  // Approved, that version is what the Edit is, and its film follows.
  await card('Edit').locator('.head').hover();
  await card('Edit').locator('.take').nth(1).click();
  await card('Edit').locator('button:text-is("Approve")').click();
  await until('approved', async () => (await edit()).approved, (number) => number === versions[1].number);
  check('film_follows_the_approved', await until('its film', () => spawnSync('ffprobe', ['-v', 'error', film]).status === 0 && countFrames(film), (n) => n === second.reduce((sum, clip) => sum + clip.frames, 0)));
  await card('Edit').locator('.head').hover();
  await card('Edit').locator('button:text-is("Unapprove")').click();
  await until('unapproved', async () => (await edit()).pinned, (pinned) => !pinned);
  await openEdit();

  // Its right click: the film's shape and size, as a render makes them, and a rate that keeps every clip's times.
  const setting = async (label, value) => {
    await card('Edit').locator('.nm').click({ button: 'right' });
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
  const timed = (await edit()).fields.timeline.tracks[0].clips;
  await setting('Frame rate', String(FPS * 2));
  const retimed = await until('the rate in the file', async () => (await edit()).fields, (fields) => fields.fps === FPS * 2);
  check('rate_keeps_times', retimed.timeline.tracks[0].clips.every((clip, i) => ['in', 'out', 'frames'].every((point) => (timed[i][point] === undefined ? clip[point] === undefined : clip[point] === timed[i][point] * 2))), { timed, retimed: retimed.timeline });
  await setting('Frame rate', String(FPS));
  await setting('Aspect ratio', '');
  await setting('Resolution', '');
  await until('back as it was', async () => (await edit()).fields, (fields) => fields.fps === FPS && fields.aspect_ratio === undefined && fields.resolution === undefined);
  await openEdit();

  // Its asset deleted: the clip says so, plainly, and a render goes on without it.
  await card('red').locator('.head').hover();
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
    const status = JSON.parse(execFileSync(process.env.COZY, ['--json', 'project', '-p', folder, 'status'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }));
    check('cli_sees_its_versions', status.assets.find((asset) => asset.name === 'Edit')?.versions === before + 1, status.assets.find((asset) => asset.name === 'Edit'));
  }
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  check('no_page_errors', errors.length === 0, errors);
  rmSync(dir, { recursive: true, force: true });
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(async (error) => {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
