// See it, in a real browser on a real Calcifer (project-web-ui §4, §8; web-ui.md §2): a project as a live graph. Files from `cozy
// import` and blocks of an export edited and imported (the import API) show with no reload: elements of every kind, with no edges, each placed once, left
// to right of what it reads, its row written. What `GET /v1/projects/{id}` says for it. A video's poster isn't its black first frame; a rest
// on its box, or on a take, plays its preview, made as it landed, which leaving or a pan stops; playing is Range with ?media=, at most two. A take landing shows with no reload, a failure stays across a reload and is gone when its text changes, a file that
// leaves is "On another computer" and comes back. Where this computer runs models: versions, looking at one, pin and unpin, on a real
// run. With the Hub unreachable it still answers. 300 elements, measured, and panned under a resting pointer: seen.scale. The project
// tabs: as many as fit, still while the window resizes, "N more" opening All projects with the rest first, a search there, and + ready
// for a new name.
//   node see.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI; $SHOT: where to keep
// its screenshots; $ALONE: on a shared computer, so no run and no restart; $RESTART, $NO_HUB: shell commands that kill this Calcifer and
// bring it back, the second on a Hub nothing answers. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { exec } = require('child_process');
const { existsSync, mkdtempSync, realpathSync, renameSync, rmSync, writeFileSync } = require('fs');
const { homedir, tmpdir } = require('os');
const { basename, join } = require('path');
const { importFile, projectJson, newProject, calcifer, signIn, DARK_OPEN, MP4, boxLog, boxOf, closeBox, openBox, STEP_MS, importYaml, lastLook, launch, lookAt, png, noStall, panning, removeGeneratedProject, renderOf, rowsOf, summary, until, watchBoxes, wav, yamlOf } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
// The last thing checked, so a refusal says when it happened.
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
// A session's media token stays out of what this prints.
const shownUrl = (url) => url.replace(/([?&]media=)[^&]+/, '$1…');

const FIRST_RUN_MS = 300_000;
const COZY = process.env.COZY;
const element = boxOf;
const away = async (page) => {
  await closeBox(page);
  await page.mouse.move(2, 500);
};
const open = openBox;
async function tag(page, name) {
  await open(page, name);
  return element(page, name).locator('.card').getAttribute('data-kind');
}
const shell = (command) => new Promise((done, fail) => exec(command, { shell: 'bash' }, (error) => (error ? fail(error) : done())));

async function drag(page, handle, dx, dy) {
  const box = await handle.boundingBox();
  const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 3, y);
  await page.mouse.move(x + dx, y + dy, { steps: 6 });
  await page.mouse.up();
}

const quiet = (page, ms) => page.evaluate((ms) => new Promise((done) => {
  const frames = [];
  let last = performance.now();
  const end = last + ms;
  const step = (now) => {
    frames.push(now - last);
    last = now;
    if (now < end) requestAnimationFrame(step);
    else done(frames.slice(1));
  };
  requestAnimationFrame(step);
}), ms);

/** How far an image's or a video's luma spreads, 0 to 255, on 64×36 of it, and its mean: near 0 for black. */
const spreadOf = (picture) => {
  const canvas = document.createElement('canvas');
  [canvas.width, canvas.height] = [64, 36];
  const drawn = canvas.getContext('2d');
  drawn.drawImage(picture, 0, 0, 64, 36);
  const pixels = drawn.getImageData(0, 0, 64, 36).data;
  const lumas = [];
  for (let i = 0; i < pixels.length; i += 4) lumas.push(0.299 * pixels[i] + 0.587 * pixels[i + 1] + 0.114 * pixels[i + 2]);
  const mean = lumas.reduce((sum, luma) => sum + luma, 0) / lumas.length;
  return { mean: Math.round(mean), spread: Math.round(Math.sqrt(lumas.reduce((sum, luma) => sum + (luma - mean) ** 2, 0) / lumas.length)) };
};

/** What each video on the page is doing. */
const videos = (page) => page.evaluate(() => [...document.querySelectorAll('video')].map((video) => ({ preview: video.classList.contains('preview'), on: video.classList.contains('on'), paused: video.paused, at: video.currentTime, muted: video.muted, loop: video.loop })));

/** A video's preview in Calcifer's cache, by its hash: made as the file landed. */
const previewOf = (sha256) => join(homedir(), '.cozy', 'cache', 'thumbs', `${sha256.replace('sha256:', '')}-preview.mp4`);

/** The pointer resting on `handle`, moving a little as a hand does. */
async function rest(page, handle) {
  const box = await handle.boundingBox();
  await page.mouse.move(box.x + box.width * 0.4, box.y + box.height * 0.5);
  await page.mouse.move(box.x + box.width * 0.5, box.y + box.height * 0.5, { steps: 3 });
}


(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1800, height: 1200 } });
  const page = await context.newPage();
  lookAt(page);
  await watchBoxes(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  const thumbs = [];
  const fileAnswers = [];
  const refused = [];
  seen.refused = refused;
  const watch = (tab) => {
    tab.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
    // A connection a restart cut isn't a fault of the page. Anything Calcifer answered is: a route it lacks shows only here.
    tab.on('console', (m) => { if (m.type() === 'error' && !/net::ERR_/.test(m.text())) errors.push(`console: ${m.text()} ${shownUrl(m.location().url)}`); });
    tab.on('response', (answer) => {
      const url = new URL(answer.url());
      // Whatever Calcifer refused, and in its words: its console line says only the status. HEAD asks whether a file is held, and no is an answer.
      if (answer.status() >= 400 && answer.request().method() !== 'HEAD') {
        const refusal = { status: answer.status(), url: shownUrl(url.pathname + url.search), during };
        refused.push(refusal);
        answer.json().then(({ error }) => Object.assign(refusal, { code: error?.code, message: error?.message })).catch(() => {});
      }
      if (url.pathname.includes('/thumbs/')) thumbs.push({ strip: url.searchParams.get('strip'), preview: url.searchParams.has('preview'), status: answer.status() });
      if (url.pathname.includes('/files/') && answer.request().resourceType() === 'media') fileAnswers.push({ status: answer.status(), media: url.searchParams.has('media') });
    });
  };
  watch(page);
  await context.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, ...init.headers } });
    return answer.json();
  };

  // A project of its own, by name, from the Projects drawer: the page is open on it from here on, and nothing below reloads it unless it says so.
  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-see-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const state = () => api(`/v1/projects/${id}`);
  const folder = (await state()).path;
  seen.project = { id, folder };
  // The project as the YAML export API writes it; a hand's edit of it goes back by the import API (cl-803: its state is project.db).
  const yaml = () => yamlOf(folder);
  await page.evaluate(() => { window.notReloaded = true; });
  const notReloaded = () => page.evaluate(() => window.notReloaded === true);
  const asset = async (name) => (await state()).assets.find((held) => held.name === name);

  // Files from a terminal: each an element in this page, with nothing asked of it.
  const scratch = mkdtempSync(join(tmpdir(), 'web-see-'));
  const imported = {};
  for (const [name, file, bytes] of [
    ['hero_art', 'hero.png', png(40, 320, 200, true)],
    ['older_art', 'older.png', png(90)],
    ['sea_sound', 'sea.wav', wav()],
    ['clip', 'clip.mp4', MP4],
    ['clip_b', 'clip_b.mp4', MP4],
    ['dark_open', 'dark_open.mp4', DARK_OPEN],
  ]) {
    writeFileSync(join(scratch, file), bytes);
    importFile(folder, join(scratch, file), name);
    imported[name] = (await asset(name)).sha256;
  }
  const prefix = imported.hero_art.replace('sha256:', '').slice(0, 12);
  const olderPrefix = imported.older_art.replace('sha256:', '').slice(0, 12);

  // An edit in an editor: a shot with beats, a cut, a frame, a character, what can't be made, and a warning.
  const local = (await api('/v1/status')).local_usable && !process.env.ALONE;
  const draw = join(__dirname, '..', 'project', 'draw.py');
  importYaml(folder, `${yaml().trimEnd()}
  shot:
    summary: |
      <hero> looks hard at <clip>.
    tool: see-no-such-package/shoot
    beats:
      - duration: 4
        description: |
          <older> waits on the shore.
      - duration: 3
        description: |
          It goes on.
  picture:
    video: <clip>
    at: 0.5
    tool: cozy/frame
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - <clip>
          - <clip_b>
  odd: {approved: "${prefix}"}
  hollow_thing:
    instructions: Nothing makes this.
  needs:
    video: <hollow_thing>
    tool: cozy/frame
${local ? `  card:\n    instructions: A still of one colour.\n    tool: ${draw}/draw\n` : ''}
  hero:
    approved: "${prefix}"
    description: A hero of few words.
  older:
    approved: "${olderPrefix}"
    description: The same hero, older.
`);
  const named = ['hero_art', 'older_art', 'sea_sound', 'clip', 'clip_b', 'dark_open', 'shot', 'picture', 'edit', 'odd', 'hollow_thing', 'needs', 'hero', 'older', ...(local ? ['card'] : [])];

  // Every name is placed once and its row written, with no reload.
  const cards = async () => (await api(`/v1/projects/${id}/canvas`)).cards;
  const rows = await until('a row for every name', cards, (now) => named.every((name) => now[name]));
  check('each_placed_and_written', true);
  const left = (read, reader) => rows[reader].x > rows[read].x;
  check('reads_left_to_right', left('hero', 'shot') && left('clip', 'shot') && left('clip', 'picture') && left('clip', 'edit'), rows);
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  await until('every element on screen', () => page.locator('.react-flow__node').count(), (n) => n === named.length);
  await shot('placed');

  // Its fields, on the wire.
  const read = await state();
  const of = (name) => read.assets.find((held) => held.name === name);
  const shotReads = Object.fromEntries((of('shot').reads ?? []).map((held) => [held.name, held.at]));
  check('wire_reads', shotReads.hero?.includes('summary') && shotReads.clip?.includes('summary') && shotReads.older?.includes('beats.0.description'), of('shot').reads);
  check('wire_fields', of('shot').fields?.summary?.includes('looks hard') && of('shot').fields?.beats?.[1]?.description?.includes('goes on'), of('shot').fields);
  check('wire_fields_no_platform_field', !('tool' in (of('shot').fields ?? {})) && !('kind' in (of('shot').fields ?? {})), of('shot').fields);
  // `hollow_thing` is its words since 27c5db3f5: a box with no tool, which warns of nothing.
  check('wire_warns_of_nothing_here', !read.warnings?.some((said) => said.name === 'hollow_thing'), read.warnings);
  check('wire_description', of('hero').fields?.description?.includes('few words'), of('hero').fields);

  // Elements of every kind, each by what it is.
  const kinds = { hero_art: 'file', sea_sound: 'file', clip: 'file', picture: 'frame', edit: 'edit' };
  for (const [name, kind] of Object.entries(kinds)) check(`kind_${name}`, (await tag(page, name)) === kind, await tag(page, name));
  await away(page);
  await until('an unmade shot shows its words', () => element(page, 'shot').locator('.words').textContent(), (text) => text.includes('looks hard'));
  await open(page, 'shot');
  await until('its beats, each a text and a slider', () => element(page, 'shot').locator('.beat input[type=range]').evaluateAll((all) => all.map((input) => input.value).join()), (text) => text === '4,3');
  // Nothing made yet is nothing wrong: its dot is plain, as is one out of date.
  check('dot_plain_when_nothing_made', (await element(page, 'shot').locator('.dot.s-current').count()) === 1 && (await element(page, 'shot').locator('.dot.s-missing, .dot.s-ood').count()) === 0, await element(page, 'shot').locator('.dot').getAttribute('class'));
  // Drawn in on one box, the canvas holds only what's near it: the rest is read at rest.
  await away(page);
  await until('hero_art measured', () => asset('hero_art'), (held) => held.alpha === true);
  await until('checkerboard under alpha', () => element(page, 'hero_art').locator('.media.alpha').count(), (n) => n === 1);
  const checker = await element(page, 'hero_art').locator('.media.alpha').evaluate((el) => ({ under: getComputedStyle(el).backgroundColor, over: getComputedStyle(el).getPropertyValue('--check2').trim() }));
  check('checkerboard_white_and_light_grey', checker.under === 'rgb(251, 251, 251)' && checker.over === '#e8e8e8', checker);
  check('no_checkerboard_without_alpha', (await element(page, 'older_art').locator('.media.alpha').count()) === 0);
  const thumbUp = (name) => until(`${name}'s thumbnail`, () => element(page, name).locator('.media img').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n > 0);
  await thumbUp('sea_sound');
  await open(page, 'sea_sound');
  check('sound_plays', (await element(page, 'sea_sound').locator('.play').count()) === 1);
  await away(page);
  await thumbUp('clip');

  // No edges at all: what reads what stays inside Calcifer.
  check('no_edges', (await page.locator('.react-flow__edge, .ge').count()) === 0);

  // A video costs nothing until the pointer rests on it. Its poster is a frame that shows something, not its black first one. Its preview,
  // made as it was imported, plays from that frame, muted and looping, and goes when the pointer leaves or its box leaves the screen.
  await until('dark_open measured', () => asset('dark_open'), (held) => held.duration_ms > 0);
  const dark = element(page, 'dark_open');
  await thumbUp('dark_open');
  const poster = await dark.locator('.media img').first().evaluate(spreadOf);
  check('poster_not_black', poster.spread >= 12, poster);
  await until('its preview made as it was imported', () => Promise.resolve(existsSync(previewOf(imported.dark_open))), Boolean);
  check('preview_made_unasked', !thumbs.some((thumb) => thumb.preview), thumbs);
  check('no_video_before_a_hover', (await page.locator('video').count()) === 0, await videos(page));
  await rest(page, dark.locator('.media'));
  const playing = await until('its preview plays', () => videos(page), (all) => all.length === 1 && all[0].preview && all[0].on && !all[0].paused && all[0].at > 0);
  check('hover_plays_the_preview', playing[0].muted && playing[0].loop, playing);
  const frame = await dark.locator('video.preview').evaluate(spreadOf);
  check('preview_from_the_poster_frame', frame.spread >= 12, frame);
  await shot('preview');
  await page.mouse.move(2, 500);
  await until('leaving stops it', () => page.locator('video').count(), (n) => n === 0);
  check('leaving_stops_it', true);
  await rest(page, dark.locator('.media'));
  await until('it plays again', () => dark.locator('video.preview.on').count(), (n) => n === 1);
  // Panned away under the pointer, by a wheel that never reaches its box.
  await page.evaluate(() => document.querySelector('.react-flow__pane').dispatchEvent(new WheelEvent('wheel', { deltaX: 6000, deltaY: 0, bubbles: true, cancelable: true })));
  await until('its box off screen', () => dark.count(), (n) => n === 0);
  await page.waitForTimeout(400);
  check('nothing_plays_off_screen', (await page.locator('video').count()) === 0, await videos(page));
  await page.mouse.move(2, 500);
  await page.keyboard.press('f');
  await until('every element on screen again', () => page.locator('.react-flow__node').count(), (n) => n === named.length);
  await away(page);

  // A take landing shows with no reload. The strip's + goes to POST runs; what Calcifer makes itself needs no machine.
  const picture = element(page, 'picture');
  await away(page);
  const numberBefore = Math.max(0, ...((await asset('picture')).prefixes ?? []).map((version) => version.number));
  await open(page, 'picture');
  // Render is with its words: on a take, its + goes to them.
  await (await renderOf(page, 'picture')).click();
  await until('the frame landed', () => asset('picture'), (held) => held.standing === 'current' && Math.max(0, ...(held.prefixes ?? []).map((version) => version.number)) > numberBefore);
  await until('its picture shows, unreloaded', () => picture.locator('.media img').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n > 0);
  await until('its dot', () => picture.locator('.dot.s-current').count(), (n) => n === 1).catch(async (error) => {
    // Which of them is behind: the page, or what Calcifer says is still to do.
    const now = await state();
    seen.at_its_dot = { dot: await picture.locator('.dot').getAttribute('class'), queue: now.queue, busy: now.busy, standing: (await asset('picture')).standing };
    throw error;
  });
  check('a_button_selects_nothing', (await page.locator('.react-flow__node.selected').count()) === 0);
  check('landed_with_no_reload', await notReloaded());
  // A walk makes each tool whose reads settled, so the cut landed too: its pieces under it. Render kept picture open, and drawn in on it the canvas draws only what's near.
  await away(page);
  await until('the cut landed', () => asset('edit'), (held) => held.standing === 'current');
  await until('its film shows', () => element(page, 'edit').locator('.media img').count(), (n) => n > 0);
  await open(page, 'edit');
  await element(page, 'edit').locator('.tl').waitFor();
  check('its_clips_on_a_hover', (await element(page, 'edit').locator('.tl-clip').evaluateAll((clips) => clips.map((held) => held.dataset.name))).join() === 'clip,clip_b');
  check('hover_on_an_edit_opens_no_video', (await page.locator('video').count()) === 0);
  // A take on its row plays its preview too: the film's, made as it landed from the cut.
  const film = (await asset('edit')).prefixes.at(-1).main.sha256;
  await until('the film\'s preview made as it landed', () => Promise.resolve(existsSync(previewOf(film))), Boolean);
  const take = element(page, 'edit').locator('.take-thumb').first();
  await rest(page, take);
  await until('the take\'s preview plays', () => take.locator('video.preview').evaluateAll((all) => all.map((video) => !video.paused && video.currentTime > 0)), (all) => all.length === 1 && all[0]);
  check('take_hover_plays_its_preview', true);
  await shot('take-preview');
  await page.mouse.move(2, 500);
  await until('it stops off the take', () => page.locator('video').count(), (n) => n === 0);
  const landedVersion = (await asset('picture')).prefixes?.at(-1);
  check('wire_version_main', landedVersion?.main?.field === 'image' && /^files\//.test(landedVersion?.main?.path ?? '') && landedVersion?.main?.sha256?.startsWith('sha256:'), landedVersion);
  await shot('landed');

  // Playing: a file here over Range with ?media=. One box is open at a time, and what it plays goes with it.
  await open(page, 'clip');
  // A clip lasts a second, and one that ends lets go: paused, each keeps its place in the count.
  const held = () => page.evaluate(() => document.querySelectorAll('video').forEach((video) => video.pause()));
  await element(page, 'clip').locator('.play').click();
  await until('one video', () => page.locator('video').count(), (n) => n === 1);
  await until('it has its metadata', () => page.locator('video').first().evaluate((video) => video.readyState), (ready) => ready >= 1);
  await held();
  // WebKit on a Mac loads a video through AVFoundation, where Playwright hears nothing: that it's loaded, off ?media=, is what shows there.
  if (process.env.ENGINE === 'webkit') {
    check('plays_by_range_and_media', (await page.locator('video').first().evaluate((video) => video.currentSrc)).includes('media='));
  } else {
    await until('over Range with ?media=', () => Promise.resolve(fileAnswers), (all) => all.some((answer) => answer.status === 206 && answer.media));
    check('plays_by_range_and_media', true);
  }
  await open(page, 'clip_b');
  await until('what was playing went with its box', () => page.locator('video').count(), (n) => n === 0);
  await element(page, 'clip_b').locator('.play').click();
  await until('its own', () => page.locator('video').count(), (n) => n === 1);
  check('one_box_plays', (await page.locator('video').count()) === 1, await page.locator('video').count());
  await away(page);
  await until('none playing', () => page.locator('video').count(), (n) => n === 0);

  // A failure is named in a toast, and its mark stays: this asks for no run, as what it reads can never be made.
  const needs = element(page, 'needs');
  await open(page, 'needs');
  await needs.locator('button[aria-label=Render]').click();
  const failed = await until('its reason', () => asset('needs'), (held) => held.failure);
  check('wire_failure', failed.failure.message.includes('cannot render') && Boolean(failed.failure.code), failed.failure);
  const said = await until('a toast says why', () => page.locator('.note .line').allTextContents(), (lines) => lines.some((line) => line.includes("needs didn't render") && line.includes('cannot render'))).catch((why) => String(why));
  check('failure_is_a_toast', Array.isArray(said), said);
  // Render stays Render, and the box stays open, saying nothing at rest of what failed.
  await page.waitForTimeout(300);
  check('render_keeps_the_box', (await page.locator('.card.here[data-asset="needs"]').count()) === 1 && (await needs.locator('button[aria-label=Render]').innerText()).trim() === 'Render', await boxLog(page));
  await until('its dot', () => needs.locator('.dot.s-failed').count(), (n) => n === 1);
  check('failed_dot', true);
  check('no_line_at_rest', (await needs.locator('.status').count()) === 0, await needs.innerText());
  const told = projectJson(folder);
  check('api_hears_the_failure', JSON.stringify(told).includes('cannot render'), told);
  // A box takes its tool's shape once its tool is known, and a box it then lay on moved clear: what stands is the rows as they are now.
  const beforeReload = await cards();
  seen.moved_since_placed = named.filter((name) => beforeReload[name].x !== rows[name].x || beforeReload[name].y !== rows[name].y);
  await page.reload();
  await page.waitForSelector('.react-flow__node');
  await page.evaluate(() => { window.notReloaded = true; });
  await open(page, 'needs');
  await until('still marked after a reload', () => needs.locator('.dot.s-failed').count(), (n) => n === 1);
  check('failure_survives_reload', (await asset('needs')).failure?.message.includes('cannot render'), (await asset('needs')).failure);
  const afterReload = await cards();
  const moved = named.filter((name) => afterReload[name].x !== beforeReload[name].x || afterReload[name].y !== beforeReload[name].y);
  check('rows_stand_after_reload', moved.length === 0, moved.map((name) => ({ name, was: beforeReload[name], now: afterReload[name] })));
  // Its text changes: the old reason no longer stands, with no reload.
  importYaml(folder, yaml().replace('video: <hollow_thing>', 'video: <clip>'));
  await until('the reason gone', () => asset('needs'), (held) => !held.failure);
  await until('and from the card', () => needs.locator('.dot.s-failed').count(), (n) => n === 0);
  check('failure_gone_when_text_changes', await notReloaded());
  await away(page);
  await page.keyboard.press('f');
  await page.waitForTimeout(400);

  // A file that leaves is on another computer, and when it comes back it shows, with no reload: restoring it writes neither project record.
  const older = await asset('older_art');
  const olderAt = join(folder, older.path);
  // Out of files/, where any name starting with its hash is it.
  const olderAway = join(folder, 'older_art.away');
  renameSync(olderAt, olderAway);
  const absent = await until('absent on the wire', () => asset('older_art'), (held) => held.absent === true);
  check('wire_absent_per_file', Object.values(absent.files).every((file) => file.absent === true) && (await state()).absent > 0, absent.files);
  await until('it says so', () => element(page, 'older_art').locator('.media-note').textContent(), (text) => text === 'On another computer');
  await open(page, 'older_art');
  check('no_manual_pull', (await element(page, 'older_art').getByRole('button', { name: 'Pull', exact: true }).count()) === 0);
  check('no_hash_when_absent', !(await page.evaluate(() => document.body.innerText)).includes(older.sha256.replace('sha256:', '').slice(0, 12)));
  await shot('absent');
  renameSync(olderAway, olderAt);
  await until('back', () => element(page, 'older_art').locator('.media-note').count(), (n) => n === 0);
  check('file_back_with_no_reload', await notReloaded());

  // Moving one: its row, and no input of anything.
  const inputs = async () => JSON.stringify((await state()).assets.map((held) => [held.name, held.inputs]));
  const inputsBefore = await inputs();
  const wasAt = (await cards()).hero_art;
  await away(page);
  await drag(page, element(page, 'hero_art').locator('.nm'), 90, 70);
  const movedTo = (await until('the moved row', cards, (now) => now.hero_art.x !== wasAt.x || now.hero_art.y !== wasAt.y)).hero_art;
  check('move_writes_its_row', movedTo.x > wasAt.x, { wasAt, movedTo });
  check('move_touches_no_input', (await inputs()) === inputsBefore);

  // Versions, on real runs: side by side, looking at one changes only this page, Approve writes the line and Unapprove removes it.
  seen.ran = local;
  if (local) {
    const card = element(page, 'card');
    const versions = async () => (await asset('card')).prefixes?.length ?? 0;
    const take = async (count) => {
      await open(page, 'card');
      await (await renderOf(page, 'card')).click();
      await until(`version ${count}`, versions, (n) => n === count, FIRST_RUN_MS);
    };
    await take(1);
    await take(2);
    await open(page, 'card');
    await until('its count', () => card.locator('.count').textContent(), (text) => text === '2/2');
    const made = await asset('card');
    check('wire_execution_ms', made.prefixes.every((version) => version.execution_ms > 0 && version.run > 0), made.prefixes);
    await card.locator('.take[aria-label$=" 1"]').click();
    await until('looking at the first', () => card.locator('.count').textContent(), (text) => text === '1/2');
    check('looking_is_only_this_page', (await asset('card')).version === made.version, (await asset('card')).version);
    // A double tap on it approves it, and again unapproves it.
    await card.locator('.take[aria-label$=" 1"]').dblclick();
    // On one of card's own lines: hero has an `approved` of its own further down.
    await until('pinned in the file', () => Promise.resolve(yaml()), (text) => /\n {2}card:\n(?: {4}.*\n)*? {4}approved: "?[0-9a-f]/.test(text));
    await open(page, 'card');
    await until('it shows approved', () => card.locator('.take.approved').count(), (n) => n === 1);
    check('approved_is_that_version', (await asset('card')).approved === made.prefixes[0].number, (await asset('card')).approved);
    await card.locator('.take.approved').dblclick();
    await until('unpinned', () => asset('card'), (held) => !held.pinned);
    check('landed_real_with_no_reload', await notReloaded());
    await shot('versions');
  }

  // The hash shows nowhere.
  const shown = await page.evaluate(() => document.body.innerText + [...document.querySelectorAll('[title],[aria-label]')].map((node) => node.title + node.ariaLabel).join(' '));
  const hashes = Object.values(imported).map((sha256) => sha256.replace('sha256:', '').slice(0, 12));
  check('hash_not_shown', hashes.every((hash) => !shown.includes(hash)), hashes.filter((hash) => shown.includes(hash)));
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));

  // The Hub unreachable: a read answers at once and writes nothing, and the page shows it. Then Calcifer as it was.
  seen.hub_checked = Boolean(process.env.NO_HUB);
  if (process.env.NO_HUB && process.env.RESTART) {
    // project.db's revision, one more each commit: a read commits nothing.
    const revision = () => rowsOf(folder, "SELECT value FROM meta WHERE key = 'seq'")[0]?.[0];
    await shell(process.env.NO_HUB).catch(() => {});
    await until('Calcifer back with no Hub', () => api('/v1/status').catch(() => null), Boolean);
    const revisionBefore = revision();
    const began = Date.now();
    const answered = await state();
    seen.hub_down_read_ms = Date.now() - began;
    check('reads_with_no_hub', answered.assets?.length === read.assets.length, answered.assets?.length);
    check('read_with_no_hub_is_quick', seen.hub_down_read_ms < 2000, seen.hub_down_read_ms);
    await page.reload();
    await until('the page with no Hub', () => element(page, 'shot').count(), (n) => n === 1);
    check('page_with_no_hub', true);
    check('read_writes_nothing', revision() === revisionBefore, { before: revisionBefore, after: revision() });
    await shell(process.env.RESTART).catch(() => {});
    await until('Calcifer back as it was', () => api('/v1/status').catch(() => null), Boolean);
  }

  // 300 elements, on React Flow, by project-web-ui §8's three cases. What's measured is reported; what's checked is that each case really happened.
  const made = await api('/v1/projects', { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `see-${Date.now()}` }, body: JSON.stringify({ name: `web-see-scale-${Date.now()}` }) });
  const scalePages = new Set();
  try {
    const big = made.path;
    for (const [name, file] of [['a_picture', 'older.png'], ['a_video', 'clip.mp4']]) importFile(big, join(scratch, file), name);
    const held = await api(`/v1/projects/${made.id}`);
    const hash = (name) => held.assets.find((row) => row.name === name).sha256.replace('sha256:', '').slice(0, 12);
    const COUNT = 300;
    const perRow = Math.ceil(Math.sqrt(COUNT));
    // Its boxes by the import API, and each one's place by the canvas's PATCH, as the page moves them.
    let text = 'version: 4\nassets:\n';
    const placed = {};
    for (let i = 0; i < COUNT; i++) {
      const reads = [1 + (i % 7), perRow + (i % 3)].filter((back) => i - back >= 0).map((back) => `<n${i - back}>`);
      text += `  n${i}:\n    ${i % 5 ? `approved: "${hash('a_picture')}"` : `approved: "${hash('a_video')}"`}\n    tool: see-no-such-package/draw\n    instructions: from ${reads.join(' and ') || 'nothing'}\n`;
      placed[`n${i}`] = { x: (i % perRow) * 304, y: Math.floor(i / perRow) * 240, w: 240 };
    }
    importYaml(big, text);
    await api(`/v1/projects/${made.id}/canvas`, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ cards: placed }) });
    const began = Date.now();
    const bigState = await api(`/v1/projects/${made.id}`);
    const read_ms = Date.now() - began;
    check('scale_state', bigState.assets.length === COUNT, bigState.assets?.length);
    const openTab = async (zoom, x) => {
      const tab = await context.newPage();
      scalePages.add(tab);
      watch(tab);
      tab.setDefaultTimeout(STEP_MS);
      await tab.addInitScript(([id, zoom, x]) => localStorage.setItem(`cozy.view:${id}`, JSON.stringify({ x, y: 60, zoom })), [made.id, zoom, x]);
      await tab.goto(`${base}/#/projects/${made.id}`);
      await tab.waitForSelector('.react-flow__node');
      await until('every card its thumbnail', () => tab.evaluate(() => {
        const images = [...document.querySelectorAll('.react-flow__node .media img')];
        return images.length >= document.querySelectorAll('.react-flow__node').length && images.every((image) => image.complete);
      }), Boolean);
      await tab.waitForTimeout(500);
      return tab;
    };
    const onScreen = (tab) => tab.evaluate(() => ({ cards: document.querySelectorAll('.react-flow__node').length, edges: document.querySelectorAll('.react-flow__edge').length }));
    seen.scale = { elements: COUNT, read_ms };

    let tab = await openTab(0.7, 40);
    const near = await panning(tab, 3000);
    seen.scale.pan_70 = { ...(await onScreen(tab)), fewest_cards: near.fewest, ...summary(near.frames) };
    check('scale_near_panned', near.places > 10, near.places);
    await tab.close();

    tab = await openTab(0.7, 40);
    // A hand's pan: wheels under a pointer resting on a video mid-screen (n40), whose preview plays. The first wheel stops it, and as boxes,
    // a fifth of them videos, slide under the pointer, none starts.
    await rest(tab, element(tab, 'n40').locator('.media'));
    await until('a rest plays its preview', () => tab.locator('video.preview.on').count(), (n) => n === 1);
    const counting = tab.evaluate(() => new Promise((done) => {
      let [last, added] = [performance.now(), 0];
      const [frames, end] = [[], last + 3000];
      const watching = new MutationObserver((changes) => {
        for (const change of changes) for (const node of change.addedNodes) if (node.nodeName === 'VIDEO' || node.querySelector?.('video')) added += 1;
      });
      watching.observe(document.body, { childList: true, subtree: true });
      const step = (now) => {
        frames.push(now - last);
        last = now;
        if (now < end) return requestAnimationFrame(step);
        watching.disconnect();
        done({ frames: frames.slice(1), added, left: document.querySelectorAll('video').length });
      };
      requestAnimationFrame(step);
    }));
    for (let i = 0, stop = Date.now() + 3000; Date.now() < stop; i++) await tab.mouse.wheel(i % 80 < 40 ? 12 : -12, 0);
    const under = await counting;
    seen.scale.pan_under_pointer = { videos_added: under.added, videos_left: under.left, ...summary(under.frames) };
    check('scale_pan_starts_no_preview', under.added === 0 && under.left === 0, seen.scale.pan_under_pointer);
    await tab.close();

    // At 11% the 18 columns are about 630 px of this 1800 px window. From here its pan, 480 px left and back, keeps them all on screen,
    // so every frame measured draws all 300.
    tab = await openTab(0.11, 800);
    const far = await panning(tab, 3000);
    seen.scale.pan_11 = { ...(await onScreen(tab)), fewest_cards: far.fewest, ...summary(far.frames) };
    check('scale_far_panned', far.places > 10, far.places);
    check('scale_far_shows_all', far.fewest === COUNT && seen.scale.pan_11.edges === 0, seen.scale.pan_11);
    await tab.close();

    // Far out, words stay near one size (Paul, 11-06): at 15%, no name, chip, ratio or wait lies on another box. A name sits above its box
    // (d88da410a) and what hangs from it in the room under it (e139a61c6).
    tab = await openTab(0.15, 800);
    const strays = await tab.evaluate(() => {
      const boxes = [...document.querySelectorAll('.react-flow__node')].map((node) => ({ node, at: node.getBoundingClientRect(), name: node.querySelector('.card')?.dataset.asset }));
      const meets = (a, b) => a.left < b.right - 1 && b.left < a.right - 1 && a.top < b.bottom - 1 && b.top < a.bottom - 1;
      const out = [];
      let labels = 0;
      for (const { node, at, name } of boxes) {
        for (const label of node.querySelectorAll('.nm, .add-on:not(.away), .ratio-rest, .rest-waiting, .dot')) {
          const seen = label.getBoundingClientRect();
          if (!seen.width || !seen.height || getComputedStyle(label).visibility === 'hidden') continue;
          labels += 1;
          for (const other of boxes) if (other.node !== node && meets(seen, other.at)) out.push({ name, label: label.className, on: other.name });
        }
      }
      return { zoom: new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a, labels, count: out.length, out: out.slice(0, 8) };
    });
    seen.scale.labels_at_15 = { zoom: strays.zoom, labels: strays.labels };
    check('far_words_stay_in_their_box', Math.abs(strays.zoom - 0.15) < 0.01 && strays.labels >= COUNT && strays.count === 0, strays);
    await tab.close();

    tab = await openTab(0.7, 40);
    const name = element(tab, 'n39').locator('.nm');
    const box = await name.boundingBox();
    let frames;
    await tab.mouse.move(box.x + 5, box.y + 5);
    await tab.mouse.down();
    await tab.mouse.move(box.x + 9, box.y + 5);
    const measuring = quiet(tab, 3000);
    const stopAt = Date.now() + 3000;
    for (let dx = 0; Date.now() < stopAt; dx = (dx + 7) % 280) await tab.mouse.move(box.x + 9 + dx, box.y + 5 + dx / 3);
    frames = await measuring;
    await tab.mouse.up();
    seen.scale.move_70 = { ...(await onScreen(tab)), ...summary(frames) };
    await tab.close();
  } finally {
    await Promise.all([...scalePages].filter((tab) => !tab.isClosed()).map((tab) => tab.close()));
    await removeGeneratedProject(made.path, 'web-see-scale-');
  }

  // The tabs: as many as fit the bar, each where it was and as wide while the window resizes, the rest behind "N more", which opens All
  // projects with them first; a search there, by name and by folder; and + ready for a new one's name.
  const trees = ['alder', 'birch', 'cedar', 'dogwood', 'elm', 'fir', 'ginkgo', 'hazel', 'ironwood', 'juniper', 'larch', 'maple', 'oak', 'pine', 'rowan', 'spruce'];
  // As Calcifer has it: macOS's temp folder is a link (/var → /private/var).
  const forest = realpathSync(mkdtempSync(join(tmpdir(), 'web-tabs-')));
  const listed = async () => (await api('/v1/projects?limit=200')).projects;
  let ours = [];
  let treeTab;
  try {
    const created = await Promise.all(trees.map((tree) => newProject(join(forest, tree))));
    // The native answer identifies each project; Windows and Node need not spell its parent path alike.
    const createdIds = new Set(created.map((row) => row.id));
    const listing = await api('/v1/projects?limit=200');
    ours = listing.projects.filter((row) => createdIds.has(row.id));
    seen.tabs_creation = { forest, created, projects: listing.projects, older: listing.older, missing: [...createdIds].filter((id) => !ours.some((row) => row.id === id)) };
    const made = createdIds.size === trees.length && ours.length === trees.length && seen.tabs_creation.missing.length === 0;
    check('tabs_projects_made', made, seen.tabs_creation);
    if (!made) throw new Error('the project list does not hold all 16 newly created project IDs; see tabs_creation');
    const tab = treeTab = await context.newPage();
    watch(tab);
    tab.setDefaultTimeout(STEP_MS);
    const snap = (name) => (process.env.SHOT ? tab.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());
    await tab.setViewportSize({ width: 1000, height: 800 });
    // By name: an earlier pass's forest is deleted, so its names are free (cl-842).
    const spruce = `${base}/#/projects/${ours.find((row) => row.name === 'spruce').id}`;
    await tab.goto(spruce);
    await tab.waitForSelector('nav.ptabs a.ptab.on');
    // What the bar is as each frame is painted: a ResizeObserver made after the page's hears of a resize after the page has answered it.
    await tab.evaluate(() => {
      window.frames_seen = [];
      new ResizeObserver(() => window.frames_seen.push([...document.querySelectorAll('nav.ptabs a.ptab:not(.more):not(.new)')].map((link) => {
        const { x, width } = link.getBoundingClientRect();
        return { id: link.getAttribute('href').split('/').pop(), on: link.classList.contains('on'), x, width };
      }))).observe(document.querySelector('.top'));
    });
    const frame = () => tab.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
    const bar = () => tab.evaluate(() => ({
      tabs: [...document.querySelectorAll('nav.ptabs a.ptab:not(.more):not(.new)')].map((link) => link.getAttribute('href').split('/').pop()),
      more: document.querySelector('nav.ptabs .ptab.more')?.textContent ?? null,
    }));
    const narrow = await bar();
    const all = await listed();
    const hidden = all.map((row) => row.id).filter((held) => !narrow.tabs.includes(held));
    await snap('tabs-narrow');
    check('tabs_overflow', hidden.length > 0 && narrow.more === `${hidden.length} more`, { narrow, projects: all.length });
    for (let width = 1000; width <= 1800; width += 40) {
      await tab.setViewportSize({ width, height: 800 });
      await frame();
    }
    const wide = await bar();
    await snap('tabs-wide');
    check('tabs_more_when_wider', wide.tabs.length > narrow.tabs.length && wide.tabs.slice(0, narrow.tabs.length).join() === narrow.tabs.join(), { narrow, wide });
    check('tabs_wide_counts_hidden', wide.more === (all.length > wide.tabs.length ? `${all.length - wide.tabs.length} more` : null), wide);
    for (let width = 1800; width >= 1000; width -= 40) {
      await tab.setViewportSize({ width, height: 800 });
      await frame();
    }
    const frames = await tab.evaluate(() => window.frames_seen);
    const first = new Map();
    const moved = [];
    for (const seen of frames) for (const one of seen) {
      const was = first.get(one.id) ?? (first.set(one.id, one), one);
      if (Math.abs(was.width - one.width) > 0.5 || (!one.on && Math.abs(was.x - one.x) > 0.5)) moved.push({ was, now: one });
    }
    check('tabs_still_while_resizing', frames.length >= 40 && moved.length === 0, { frames: frames.length, moved: moved.slice(0, 5) });
    check('tabs_back_when_narrower', (await bar()).tabs.join() === narrow.tabs.join(), { narrow, now: await bar() });

    // With no project open its "untitled" tab is measured too, so what has a tab keeps its width.
    await tab.goto(`${base}/#/projects`);
    await tab.waitForSelector('nav.ptabs span.ptab.on');
    const untitled = await tab.evaluate(() => [...document.querySelectorAll('nav.ptabs a.ptab:not(.more):not(.new)')].map((link) => ({ id: link.getAttribute('href').split('/').pop(), width: link.getBoundingClientRect().width })));
    const squeezed = untitled.filter((one) => first.has(one.id) && !first.get(one.id).on && Math.abs(first.get(one.id).width - one.width) > 0.5);
    check('tabs_untitled_measured', untitled.length > 0 && squeezed.length === 0, { untitled, squeezed });
    await tab.goto(spruce);
    await tab.waitForSelector('nav.ptabs a.ptab.on');

    // "N more": All projects, those with no tab first.
    await tab.click('nav.ptabs .ptab.more');
    await tab.waitForSelector('.drawer .list li');
    const drawn = () => tab.evaluate(() => [...document.querySelectorAll('.drawer .list li a')].map((link) => link.getAttribute('href').split('/').pop()));
    const order = await drawn();
    await snap('tabs-more');
    const line = await tab.evaluate(() => [...document.querySelectorAll('.drawer .list li')].findIndex((item) => item.classList.contains('tabbed')));
    check('tabs_more_lists_hidden_first', order.slice(0, hidden.length).sort().join() === [...hidden].sort().join() && line === hidden.length, { hidden, order, line });

    // Found by name, and by a folder no name holds.
    const find = tab.locator('.drawer input[aria-label="Search projects"]');
    await find.fill('ginkgo');
    const ginkgo = ours.find((row) => row.name === 'ginkgo').id;
    await until('one found by name', drawn, (now) => now.join() === ginkgo).then(() => check('tabs_find_by_name', true), (error) => check('tabs_find_by_name', false, error.message));
    await find.fill(basename(forest));
    const ids = ours.map((row) => row.id).sort().join();
    await until('ours found by folder', drawn, (now) => [...now].sort().join() === ids).then(() => check('tabs_find_by_folder', true), (error) => check('tabs_find_by_folder', false, error.message));
    await snap('tabs-found');
    await find.fill('');

    // +: the New field, ready, from the drawer open and from the canvas.
    const naming = () => tab.evaluate(() => document.activeElement?.getAttribute('aria-label'));
    await tab.click('nav.ptabs .ptab.new');
    await until('+ in the drawer names a new one', naming, (label) => label === "New project's name").then(() => check('tabs_plus_in_drawer', true), (error) => check('tabs_plus_in_drawer', false, error.message));
    await tab.click('.drawer [aria-label="Close"]');
    await tab.waitForSelector('.drawer', { state: 'detached' });
    await tab.click('nav.ptabs .ptab.new');
    await until('+ names a new one', naming, (label) => label === "New project's name").then(() => check('tabs_plus', true), (error) => check('tabs_plus', false, error.message));
    await tab.close();
  } finally {
    if (treeTab && !treeTab.isClosed()) await treeTab.close();
    await removeGeneratedProject(forest, 'web-tabs-');
  }

  // Each refusal's words have had their moment to arrive.
  await new Promise((done) => setTimeout(done, 500));
  check('no_page_errors', errors.length === 0, { errors, refused });
  rmSync(scratch, { recursive: true, force: true });
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
