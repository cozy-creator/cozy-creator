// See it, in a real browser on a real Calcifer (project-web-ui §4, §8; web-ui.md §2): a project as a live graph. Files from `cozy project
// import` and blocks written straight into project.yaml show with no reload: elements of every kind, with no edges, each placed once, left
// to right of what it reads, its row written. What `GET /v1/projects/{id}` says for it. Scrubbing opens no <video>; playing is Range with ?media=,
// at most two. A take landing shows with no reload, a failure stays across a reload and is gone when its text changes, a file that
// leaves is "On another computer" and comes back. Where this computer runs models: versions, looking at one, pin and unpin, on a real
// run. With the Hub unreachable it still answers. 300 elements, measured: seen.scale.
//   node see.js <sign-in link from `cozy web --no-open`>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI; $SHOT: where to keep
// its screenshots; $ALONE: on a shared computer, so no run and no restart; $RESTART, $NO_HUB: shell commands that kill this Calcifer and
// bring it back, the second on a Hub nothing answers. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { exec, execFileSync, spawnSync } = require('child_process');
const { mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { MP4, boxLog, STEP_MS, lastLook, lookAt, png, until, watchBoxes, wav, pointAt } = require('./page');

const failures = [];
const seen = {};
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
const element = (page, name) => page.locator('.react-flow__node').filter({ has: page.locator(`.nm:text-is("${name}")`) });
const away = (page) => page.mouse.move(2, 500);
async function open(page, name) {
  await away(page);
  await pointAt(page, element(page, name).locator('.nm'));
}
async function tag(page, name) {
  await open(page, name);
  return element(page, name).locator('.card').getAttribute('data-kind');
}
const shell = (command) => new Promise((done, fail) => exec(command, { shell: 'bash' }, (error) => (error ? fail(error) : done())));

function replaceFile(at, text) {
  writeFileSync(`${at}.part`, text);
  renameSync(`${at}.part`, at);
}

async function drag(page, handle, dx, dy) {
  const box = await handle.boundingBox();
  const [x, y] = [box.x + box.width / 2, box.y + box.height / 2];
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 3, y);
  await page.mouse.move(x + dx, y + dy, { steps: 6 });
  await page.mouse.up();
}

/** The frames while a pan goes on, how many places the viewport went, as a pan that didn't move proves nothing, and the fewest cards on
 *  screen at any moment of it. It goes 480 px left and back. */
const panning = (page, ms) => page.evaluate((ms) => new Promise((done) => {
  const pane = document.querySelector('.react-flow__pane');
  const viewport = document.querySelector('.react-flow__viewport');
  const frames = [];
  const places = new Set();
  let [last, sign, count, fewest] = [performance.now(), 1, 0, Infinity];
  const end = last + ms;
  const step = (now) => {
    frames.push(now - last);
    last = now;
    places.add(getComputedStyle(viewport).transform);
    fewest = Math.min(fewest, document.querySelectorAll('.react-flow__node').length);
    if (++count % 40 === 0) sign = -sign;
    pane.dispatchEvent(new WheelEvent('wheel', { deltaX: sign * 12, deltaY: 0, bubbles: true, cancelable: true }));
    if (now < end) requestAnimationFrame(step);
    else done({ frames: frames.slice(1), places: places.size, fewest });
  };
  requestAnimationFrame(step);
}), ms);

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

function summary(frames) {
  const sorted = [...frames].sort((a, b) => a - b);
  return { median_ms: +sorted[sorted.length >> 1].toFixed(1), p95_ms: +sorted[Math.floor(sorted.length * 0.95)].toFixed(1), frames: sorted.length };
}

(async () => {
  const link = process.argv[2];
  const base = new URL(link).origin;
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
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
      if (url.pathname.includes('/thumbs/')) thumbs.push({ strip: url.searchParams.get('strip'), status: answer.status() });
      if (url.pathname.includes('/files/') && answer.request().resourceType() === 'media') fileAnswers.push({ status: answer.status(), media: url.searchParams.has('media') });
    });
  };
  watch(page);
  await context.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await page.goto(link);
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
  const yamlAt = join(folder, 'project.yaml');
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
  ]) {
    writeFileSync(join(scratch, file), bytes);
    execFileSync(COZY, ['project', '-p', folder, 'import', join(scratch, file), name], { encoding: 'utf8' });
    imported[name] = (await asset(name)).sha256;
  }
  const prefix = imported.hero_art.replace('sha256:', '').slice(0, 12);
  const olderPrefix = imported.older_art.replace('sha256:', '').slice(0, 12);

  // An edit in an editor: a shot with beats, a cut, a frame, a character, what can't be made, and a warning.
  const local = (await api('/v1/status')).local_usable && !process.env.ALONE;
  const draw = join(__dirname, '..', 'project', 'draw.py');
  replaceFile(yamlAt, `${readFileSync(yamlAt, 'utf8').trimEnd()}
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
  odd: {kind: widget, approved: "${prefix}"}
  hollow_thing:
    instructions: Nothing makes this.
  needs:
    video: <hollow_thing>
    tool: cozy/frame
${local ? `  card:\n    instructions: A still of one colour.\n    tool: ${draw}/draw\n` : ''}
  hero:
    approved: "${prefix}"
    description: A hero of few words.
    asset_type: character
  older:
    approved: "${olderPrefix}"
    description: The same hero, older.
    asset_type: villain
`);
  const named = ['hero_art', 'older_art', 'sea_sound', 'clip', 'clip_b', 'shot', 'picture', 'edit', 'odd', 'hollow_thing', 'needs', 'hero', 'older', ...(local ? ['card'] : [])];

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
  check('wire_warning_named', read.warnings?.some((said) => said.name === 'odd' && said.message.includes('kind')) && read.warnings?.some((said) => said.name === 'hollow_thing'), read.warnings);
  check('wire_asset_type', of('hero').fields?.asset_type === 'character' && of('hero').fields?.description?.includes('few words'), of('hero').fields);

  // Elements of every kind, each by what it is.
  const kinds = { hero_art: 'file', sea_sound: 'file', clip: 'file', picture: 'frame', edit: 'edit' };
  for (const [name, kind] of Object.entries(kinds)) check(`kind_${name}`, (await tag(page, name)) === kind, await tag(page, name));
  await away(page);
  await until('an unmade shot shows its words', () => element(page, 'shot').locator('.words').textContent(), (text) => text.includes('looks hard'));
  await open(page, 'shot');
  await until('its beats, each a text and a slider', () => element(page, 'shot').locator('.beat input[type=range]').evaluateAll((all) => all.map((input) => input.value).join()), (text) => text === '4,3');
  await open(page, 'odd');
  check('warned_mark', (await element(page, 'odd').locator('.warned').count()) === 1);
  check('wire_warning_of_an_asset_type', read.warnings?.some((said) => said.name === 'older' && said.message.includes('asset_type')), read.warnings);
  check('dot_hollow_when_nothing_made', (await element(page, 'shot').locator('.dot.s-missing').count()) === 1);
  await until('hero_art measured', () => asset('hero_art'), (held) => held.alpha === true);
  await until('checkerboard under alpha', () => element(page, 'hero_art').locator('.media.alpha').count(), (n) => n === 1);
  check('no_checkerboard_without_alpha', (await element(page, 'older_art').locator('.media.alpha').count()) === 0);
  const thumbUp = (name) => until(`${name}'s thumbnail`, () => element(page, name).locator('.media img').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n > 0);
  await thumbUp('sea_sound');
  check('sound_plays', (await element(page, 'sea_sound').locator('.play').count()) === 1);
  await thumbUp('clip');

  // No edges at all: what reads what stays inside Calcifer.
  check('no_edges', (await page.locator('.react-flow__edge, .ge').count()) === 0);

  // A video costs nothing until clicked: hovering scrubs a strip, with a timecode, and opens no <video>.
  await until('clip measured', () => asset('clip'), (held) => held.duration_ms > 0);
  const clip = element(page, 'clip');
  const media = await clip.locator('.media').boundingBox();
  await page.mouse.move(media.x + media.width * 0.6, media.y + media.height * 0.6);
  await until('a strip of 8 asked', () => Promise.resolve(thumbs), (all) => all.some((thumb) => thumb.strip === '8' && thumb.status === 200));
  await until('the strip up', () => clip.locator('img.strip').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n === 1);
  // Eight frames of a clip of ten, all there: the last seek of a short clip lands past its last frame.
  const strip = await clip.locator('img.strip').evaluate((image) => ({ width: image.naturalWidth, height: image.naturalHeight, moved: image.style.transform }));
  check('strip_is_8_frames', strip.width === 8 * 160 && strip.height === 90, strip);
  check('scrub_moves_the_strip', strip.moved === 'translateX(-50%)', strip);
  check('timecode', /^\d+:\d\d$/.test(await clip.locator('.tc').textContent()), await clip.locator('.tc').textContent());
  check('hover_opens_no_video', (await page.locator('video').count()) === 0);
  await away(page);

  // A take landing shows with no reload. The strip's + goes to POST runs; what Calcifer makes itself needs no machine.
  const picture = element(page, 'picture');
  await away(page);
  const numberBefore = Math.max(0, ...((await asset('picture')).prefixes ?? []).map((version) => version.number));
  await open(page, 'picture');
  await picture.locator('.take.add').click();
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
  // A walk makes each tool whose reads settled, so the cut landed too: its pieces under it.
  await until('the cut landed', () => asset('edit'), (held) => held.standing === 'current');
  await until('its film shows', () => element(page, 'edit').locator('.media img').count(), (n) => n > 0);
  await open(page, 'edit');
  await element(page, 'edit').locator('.tl').waitFor();
  check('its_clips_on_a_hover', (await element(page, 'edit').locator('.tl-clip').evaluateAll((clips) => clips.map((held) => held.dataset.name))).join() === 'clip,clip_b');
  check('hover_on_an_edit_opens_no_video', (await page.locator('video').count()) === 0);
  const landedVersion = (await asset('picture')).prefixes?.at(-1);
  check('wire_version_main', landedVersion?.main?.field === 'image' && /^files\//.test(landedVersion?.main?.path ?? '') && landedVersion?.main?.sha256?.startsWith('sha256:'), landedVersion);
  await shot('landed');

  // Playing: a file here over Range with ?media=, two at most, and the third stops the first.
  await open(page, 'clip');
  // A clip lasts a second, and one that ends lets go: paused, each keeps its place in the count.
  const held = () => page.evaluate(() => document.querySelectorAll('video').forEach((video) => video.pause()));
  await clip.locator('.play').click();
  await until('one video', () => page.locator('video').count(), (n) => n === 1);
  await until('it has its metadata', () => page.locator('video').first().evaluate((video) => video.readyState), (ready) => ready >= 1);
  await held();
  await until('over Range with ?media=', () => Promise.resolve(fileAnswers), (all) => all.some((answer) => answer.status === 206 && answer.media));
  check('plays_by_range_and_media', true);
  await open(page, 'clip_b');
  await element(page, 'clip_b').locator('.play').click();
  await until('two videos', () => page.locator('video').count(), (n) => n === 2);
  await held();
  await open(page, 'edit');
  await element(page, 'edit').locator('.tl-play').click();
  await until('still two, the first stopped', () => clip.locator('video').count(), (n) => n === 0);
  await page.waitForTimeout(300);
  check('two_videos_at_most', (await page.locator('video').count()) === 2, await page.locator('video').count());
  await until('none playing', async () => {
    // One that ends takes its Stop away as this reaches for it.
    if (await page.locator('.stop').count()) await page.locator('.stop').first().click({ timeout: 2000 }).catch(() => {});
    return page.locator('video').count();
  }, (n) => n === 0);

  // A failure is named and stays: this asks for no run, as what it reads can never be made.
  const needs = element(page, 'needs');
  await open(page, 'needs');
  // Its controls open under it, and it sits low: the canvas goes up as far as they need, as a person would take it.
  const under = await needs.locator('.take.add').boundingBox();
  await page.mouse.wheel(0, Math.max(0, under.y + under.height + 40 - page.viewportSize().height));
  await page.waitForTimeout(200);
  await open(page, 'needs');
  await needs.locator('.take.add').click();
  const failed = await until('its reason', () => asset('needs'), (held) => held.failure);
  check('wire_failure', failed.failure.message.includes('cannot render') && Boolean(failed.failure.code), failed.failure);
  // Its reason shows on the box the pointer is still on. On Windows (41909619528) it had closed, and its neighbour opened.
  await page.waitForTimeout(300);
  check('a_box_stays_open_under_the_pointer', await needs.locator('.card').evaluate((el) => el.classList.contains('active')), await boxLog(page));
  await until('its reason shows', async () => {
    await open(page, 'needs');
    return needs.locator('.status.bad').count();
  }, (n) => n === 1);
  check('failed_dot', (await needs.locator('.dot.s-failed').count()) === 1);
  check('try_again_offered', (await needs.locator('.status.bad .act').textContent()) === 'Try again');
  const told = spawnSync(COZY, ['project', '-p', folder, 'status'], { encoding: 'utf8' });
  check('terminal_hears_the_failure', `${told.stdout}${told.stderr}`.includes('cannot render'), `${told.stdout}${told.stderr}`.slice(0, 400));
  await page.reload();
  await page.waitForSelector('.react-flow__node');
  await page.evaluate(() => { window.notReloaded = true; });
  await open(page, 'needs');
  await until('still there after a reload', () => needs.locator('.status.bad').textContent(), (text) => text.includes('cannot render'));
  check('failure_survives_reload', true);
  const afterReload = await cards();
  check('rows_stand_after_reload', named.every((name) => afterReload[name].x === rows[name].x && afterReload[name].y === rows[name].y), afterReload);
  // Its text changes: the old reason no longer stands, with no reload.
  replaceFile(yamlAt, readFileSync(yamlAt, 'utf8').replace('video: <hollow_thing>', 'video: <clip>'));
  await until('the reason gone', () => asset('needs'), (held) => !held.failure);
  await until('and from the card', () => needs.locator('.status.bad').count(), (n) => n === 0);
  check('failure_gone_when_text_changes', await notReloaded());
  await page.keyboard.press('f');
  await page.waitForTimeout(400);

  // A file that leaves is on another computer, and when it comes back it shows, with no reload: a pull writes neither file.
  const older = await asset('older_art');
  const olderAt = join(folder, older.path);
  // Out of files/, where any name starting with its hash is it.
  const olderAway = join(folder, 'older_art.away');
  renameSync(olderAt, olderAway);
  const absent = await until('absent on the wire', () => asset('older_art'), (held) => held.absent === true);
  check('wire_absent_per_file', Object.values(absent.files).every((file) => file.absent === true) && (await state()).absent > 0, absent.files);
  await until('it says so', () => element(page, 'older_art').locator('.media-note').textContent(), (text) => text === 'On another computer');
  await open(page, 'older_art');
  check('pull_offered', (await element(page, 'older_art').locator('.status .act').textContent()) === 'Pull');
  check('no_hash_when_absent', !(await page.evaluate(() => document.body.innerText)).includes(older.sha256.replace('sha256:', '').slice(0, 12)));
  await shot('absent');
  renameSync(olderAway, olderAt);
  await until('back', () => element(page, 'older_art').locator('.media-note').count(), (n) => n === 0);
  check('file_back_with_no_reload', await notReloaded());

  // Moving one: its row, and no input of anything.
  const inputs = async () => JSON.stringify((await state()).assets.map((held) => [held.name, held.inputs]));
  const inputsBefore = await inputs();
  const wasAt = (await cards()).hero_art;
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
      await card.locator('button[aria-label=Render]').click();
      await until(`version ${count}`, versions, (n) => n === count, FIRST_RUN_MS);
    };
    await take(1);
    await take(2);
    await open(page, 'card');
    await until('its count', () => card.locator('.count').textContent(), (text) => text === '2/2');
    const made = await asset('card');
    check('wire_execution_ms', made.prefixes.every((version) => version.execution_ms > 0 && version.run > 0), made.prefixes);
    await card.locator('.take').first().click();
    await until('looking at the first', () => card.locator('.count').textContent(), (text) => text === '1/2');
    check('looking_is_only_this_page', (await asset('card')).version === made.version, (await asset('card')).version);
    await card.locator('button:has-text("Approve")').click();
    // A hash on one of card's own lines: `image: <hero_art>` further down is hero's.
    await until('pinned in the file', () => Promise.resolve(readFileSync(yamlAt, 'utf8')), (text) => /\n {2}card:\n(?: {4}.*\n)*? {4}image: "?[0-9a-f]/.test(text));
    await open(page, 'card');
    await until('it can be unapproved', () => card.locator('button:has-text("Unapprove")').count(), (n) => n === 1);
    check('approved_is_that_version', (await asset('card')).approved === made.prefixes[0].number, (await asset('card')).approved);
    await card.locator('button:has-text("Unapprove")').click();
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
    const lockAt = join(folder, 'project.lock');
    await shell(process.env.NO_HUB).catch(() => {});
    await until('Calcifer back with no Hub', () => api('/v1/status').catch(() => null), Boolean);
    const lockBefore = readFileSync(lockAt);
    const began = Date.now();
    const answered = await state();
    seen.hub_down_read_ms = Date.now() - began;
    check('reads_with_no_hub', answered.assets?.length === read.assets.length, answered.assets?.length);
    check('read_with_no_hub_is_quick', seen.hub_down_read_ms < 2000, seen.hub_down_read_ms);
    await page.reload();
    await until('the page with no Hub', () => element(page, 'shot').count(), (n) => n === 1);
    check('page_with_no_hub', true);
    check('read_writes_no_lock', readFileSync(lockAt).equals(lockBefore));
    await shell(process.env.RESTART).catch(() => {});
    await until('Calcifer back as it was', () => api('/v1/status').catch(() => null), Boolean);
  }

  // 300 elements, on React Flow, by project-web-ui §8's three cases. What's measured is reported; what's checked is that each case really happened.
  const made = await api('/v1/projects', { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `see-${Date.now()}` }, body: JSON.stringify({ name: `web-see-scale-${Date.now()}` }) });
  try {
    const big = made.path;
    for (const [name, file] of [['a_picture', 'older.png'], ['a_video', 'clip.mp4']]) execFileSync(COZY, ['project', '-p', big, 'import', join(scratch, file), name]);
    const held = await api(`/v1/projects/${made.id}`);
    const hash = (name) => held.assets.find((row) => row.name === name).sha256.replace('sha256:', '').slice(0, 12);
    const COUNT = 300;
    const perRow = Math.ceil(Math.sqrt(COUNT));
    let text = 'version: 3\nassets:\n';
    let rowsText = '# Where cards sit. Only the page reads this. Nothing here is an input.\nversion: 1\ncards:\n';
    for (let i = 0; i < COUNT; i++) {
      const reads = [1 + (i % 7), perRow + (i % 3)].filter((back) => i - back >= 0).map((back) => `<n${i - back}>`);
      text += `  n${i}:\n    ${i % 5 ? `approved: "${hash('a_picture')}"` : `approved: "${hash('a_video')}"`}\n    tool: see-no-such-package/draw\n    instructions: from ${reads.join(' and ') || 'nothing'}\n`;
      rowsText += `  n${i}: {x: ${(i % perRow) * 304}, y: ${Math.floor(i / perRow) * 240}, w: 240}\n`;
    }
    replaceFile(join(big, 'project.yaml'), text);
    replaceFile(join(big, 'project.canvas.yaml'), rowsText);
    const began = Date.now();
    const bigState = await api(`/v1/projects/${made.id}`);
    const read_ms = Date.now() - began;
    check('scale_state', bigState.assets.length === COUNT, bigState.assets?.length);
    const openTab = async (zoom, x) => {
      const tab = await context.newPage();
      watch(tab);
      tab.setDefaultTimeout(STEP_MS);
      await tab.addInitScript(([id, zoom, x]) => localStorage.setItem(`cozy.view:here:${id}`, JSON.stringify({ x, y: 60, zoom })), [made.id, zoom, x]);
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

    // At 11% the 18 columns are about 630 px of this 1800 px window. From here its pan, 480 px left and back, keeps them all on screen,
    // so every frame measured draws all 300.
    tab = await openTab(0.11, 800);
    const far = await panning(tab, 3000);
    seen.scale.pan_11 = { ...(await onScreen(tab)), fewest_cards: far.fewest, ...summary(far.frames) };
    check('scale_far_panned', far.places > 10, far.places);
    check('scale_far_shows_all', far.fewest === COUNT && seen.scale.pan_11.edges === 0, seen.scale.pan_11);
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
    await api(`/v1/projects/${made.id}`, { method: 'DELETE' }).catch(() => {});
    if (made.path?.includes('web-see-scale-')) rmSync(made.path, { recursive: true, force: true });
  }

  // Each refusal's words have had their moment to arrive.
  await new Promise((done) => setTimeout(done, 500));
  check('no_page_errors', errors.length === 0, { errors, refused });
  rmSync(scratch, { recursive: true, force: true });
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(async (error) => {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
