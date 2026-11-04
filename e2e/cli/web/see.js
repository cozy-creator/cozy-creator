// See it, in a real browser on a real Calcifer (project-web-ui §4, §8; web-ui.md §2): a project as a live graph. Files from `cozy project
// import` and blocks written straight into project.yaml show with no reload: elements of every kind, edges from reads, each placed once,
// left to right, its row written. What `GET /v1/projects/{id}` says for it. Scrubbing opens no <video>; playing is Range with ?media=,
// at most two. A take landing shows with no reload, a failure stays across a reload and is gone when its text changes, a file that
// leaves is "On another computer" and comes back. Where this computer runs models: versions, looking at one, pin and unpin, on a real
// run. With the Hub unreachable it still answers. 300 elements with edges, measured: seen.scale.
//   node see.js <sign-in link from `cozy web --no-open`>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI; $SHOT: where to keep
// its screenshots; $ALONE: on a shared computer, so no run and no restart; $RESTART, $NO_HUB: shell commands that kill this Calcifer and
// bring it back, the second on a Hub nothing answers. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { exec, execFileSync, spawnSync } = require('child_process');
const { mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { MP4, STEP_MS, png, until, wav } = require('./page');

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
const tag = (page, name) => element(page, name).locator('.tag').textContent();
const edgeIds = (page) => page.evaluate(() => [...document.querySelectorAll('.ge')].map((edge) => edge.dataset.edge));
const shell = (command) => new Promise((done, fail) => exec(command, { shell: 'bash' }, (error) => (error ? fail(error) : done())));

function replaceFile(at, text) {
  writeFileSync(`${at}.part`, text);
  renameSync(`${at}.part`, at);
}

/** Somewhere along an edge that the pointer really lands on it, or null. */
const pointOn = (page, id) => page.evaluate((id) => {
  const hit = [...document.querySelectorAll('.ge')].find((edge) => edge.dataset.edge === id)?.querySelector('.ge-hit');
  if (!hit) return null;
  const screen = hit.getScreenCTM();
  for (let t = 0.05; t < 1; t += 0.05) {
    const at = hit.getPointAtLength(hit.getTotalLength() * t);
    const [x, y] = [at.x * screen.a + screen.e, at.y * screen.d + screen.f];
    if (document.elementFromPoint(x, y)?.closest?.('.ge')?.dataset.edge === id) return { x, y };
  }
  return null;
}, id);

/** Which edges have their path redrawn while `act` goes on. */
async function redrawn(page, act) {
  await page.evaluate(() => {
    window.touched = new Set();
    window.watching = new MutationObserver((list) => {
      for (const change of list) {
        const edge = change.target.closest?.('.ge');
        if (edge) window.touched.add(edge.dataset.edge);
      }
    });
    window.watching.observe(document.querySelector('.react-flow__viewport').parentElement, { subtree: true, attributes: true, attributeFilter: ['d'] });
  });
  await act();
  await page.waitForTimeout(300);
  return page.evaluate(() => {
    window.watching.disconnect();
    return [...window.touched];
  });
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
      // Whatever Calcifer refused, and in its words: its console line says only the status.
      if (answer.status() >= 400) {
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

  // An edit in an editor: a shot with beats, a cut, a frame, an entity with a sub-entity, what can't be made, and a warning.
  const local = (await api('/v1/status')).local_usable && !process.env.ALONE;
  const draw = join(__dirname, '..', 'project', 'draw.py');
  replaceFile(yamlAt, `${readFileSync(yamlAt, 'utf8').trimEnd()}
  shot:
    kind: video
    summary: |
      <hero> looks hard at <clip>.
    render: {with: see-no-such-package}
    beats:
      - duration: 4
        description: |
          <hero.older> waits on the shore.
      - duration: 3
        description: |
          It goes on.
  picture:
    kind: image
    video: <clip>
    at: 0.5
    render: {with: cozy}
  edit:
    kind: video
    render: {with: cozy}
    pieces:
      - <clip>
      - <clip_b>
  odd: {kind: widget, image: "${prefix}"}
  hollow_thing:
    description: Nothing makes this.
  needs:
    kind: image
    video: <hollow_thing>
    render: {with: cozy}
${local ? `  card:\n    kind: image\n    description: A still of one colour.\n    render: {with: ${draw}}\n` : ''}
entities:
  hero:
    kind: character
    description: |
      A hero of few words.
    image: <hero_art>
    older:
      description: |
        The same hero, older.
      image: <older_art>
      deeper:
        description: Sub-entities go one level.
`);
  const named = ['hero_art', 'older_art', 'sea_sound', 'clip', 'clip_b', 'shot', 'picture', 'edit', 'odd', 'hollow_thing', 'needs', 'hero', ...(local ? ['card'] : [])];

  // Every name is placed once and its row written, with no reload.
  const cards = async () => (await api(`/v1/projects/${id}/canvas`)).cards;
  const rows = await until('a row for every name', cards, (now) => named.every((name) => now[name]));
  check('each_placed_and_written', true);
  const left = (read, reader) => rows[reader].x > rows[read].x;
  check('reads_left_to_right', left('hero_art', 'hero') && left('hero', 'shot') && left('clip', 'shot') && left('clip', 'picture') && left('clip', 'edit') && left('older_art', 'hero'), rows);
  await page.click('.top button:has-text("Fit")');
  await page.waitForTimeout(400);
  await until('every element on screen', () => page.locator('.react-flow__node').count(), (n) => n === named.length);
  await shot('placed');

  // Its fields, on the wire.
  const read = await state();
  const of = (name) => read.assets.find((held) => held.name === name);
  const shotReads = Object.fromEntries((of('shot').reads ?? []).map((held) => [held.name, held.at]));
  check('wire_reads', shotReads.hero?.includes('summary') && shotReads.clip?.includes('summary') && shotReads['hero.older']?.includes('beats.0.description'), of('shot').reads);
  check('wire_fields', of('shot').fields?.summary?.includes('looks hard') && of('shot').fields?.beats?.[1]?.description?.includes('goes on'), of('shot').fields);
  check('wire_fields_no_platform_field', !('render' in (of('shot').fields ?? {})) && !('with' in (of('shot').fields ?? {})), of('shot').fields);
  check('wire_warning_named', read.warnings?.some((said) => said.name === 'odd' && said.message.includes('kind')) && read.warnings?.some((said) => said.name === 'hollow_thing'), read.warnings);
  check('wire_entity', read.entities?.[0]?.links?.image === 'hero_art' && read.entities?.[0]?.subs?.[0]?.links?.image === 'older_art', read.entities);

  // Elements of every kind, each by what it is.
  const kinds = { hero_art: 'file', sea_sound: 'file', clip: 'file', shot: 'shot', picture: 'frame', edit: 'cut', hero: 'character' };
  for (const [name, kind] of Object.entries(kinds)) check(`kind_${name}`, (await tag(page, name)) === kind, await tag(page, name));
  check('entity_sub_row', (await element(page, 'hero').locator('.sub-name').textContent()) === 'hero.older');
  check('entity_slots', (await element(page, 'hero').locator('.slot').count()) === 2);
  await until('an unmade shot shows its words', () => element(page, 'shot').locator('.words').textContent(), (text) => text.includes('looks hard'));
  check('beat_bar', (await element(page, 'shot').locator('.beats button').allTextContents()).join() === '4s,3s');
  check('warned_mark', (await element(page, 'odd').locator('.warned').count()) === 1);
  check('wire_warning_of_a_sub_entity', read.warnings?.some((said) => said.name === 'hero.older'), read.warnings);
  check('warned_mark_of_a_sub_entity', (await element(page, 'hero').locator('.warned').count()) === 1);
  check('dot_hollow_when_nothing_made', (await element(page, 'shot').locator('.dot.s-missing').count()) === 1);
  await until('hero_art measured', () => asset('hero_art'), (held) => held.alpha === true);
  await until('checkerboard under alpha', () => element(page, 'hero_art').locator('.media.alpha').count(), (n) => n === 1);
  check('no_checkerboard_without_alpha', (await element(page, 'older_art').locator('.media.alpha').count()) === 0);
  const thumbUp = (name) => until(`${name}'s thumbnail`, () => element(page, name).locator('.media img').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n > 0);
  await thumbUp('sea_sound');
  check('sound_plays', (await element(page, 'sea_sound').locator('.play').count()) === 1);
  await thumbUp('clip');

  // Edges from reads: solid, dotted.
  const want = {
    'mention:hero/>shot/': 'none',
    'mention:hero/from:hero.older>shot/': 'none',
    'mention:clip/>shot/': 'none',
    'mention:clip/>picture/': 'none',
    'mention:clip/>edit/': 'none',
    'link:hero_art/>hero/': 'dotted',
    'link:older_art/>hero/to:hero.older': 'dotted',
  };
  const drawn = await until('its edges', () => edgeIds(page), (ids) => Object.keys(want).every((edge) => ids.includes(edge)));
  const dashes = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.ge')].map((edge) => [edge.dataset.edge, getComputedStyle(edge.querySelector('.ge-line')).strokeDasharray])));
  const look = (dash) => (dash === 'none' ? 'none' : 'dotted');
  check('edge_looks', Object.entries(want).every(([edge, style]) => look(dashes[edge]) === style), dashes);
  check('edge_width_ignores_zoom', await page.evaluate(() => [...document.querySelectorAll('.ge-line')].every((line) => getComputedStyle(line).vectorEffect === 'non-scaling-stroke')));

  // Selecting lights its edges and dims the rest.
  await element(page, 'shot').locator('.nm').click();
  await until('the stage focused', () => page.getAttribute('.stage', 'data-focus'), (focus) => focus === 'on');
  const lit = await page.evaluate(() => [...document.querySelectorAll('.ge.lit')].map((edge) => edge.dataset.edge));
  const connected = drawn.filter((edge) => { const [from, to] = edge.split(':').slice(1).join(':').split('>'); return from.split('/')[0] === 'shot' || to.split('/')[0] === 'shot'; });
  check('selected_lights_its_edges', lit.length > 0 && lit.length === connected.length && connected.every((edge) => lit.includes(edge)), { lit, connected });
  await shot('selected');
  await page.keyboard.press('Escape');

  // Hovering an edge names the text that made it; clicking opens that text.
  const point = await pointOn(page, 'mention:hero/from:hero.older>shot/');
  check('edge_reachable', Boolean(point));
  if (point) {
    await page.mouse.move(point.x, point.y);
    await page.waitForSelector('.edge-tip');
    const tip = await page.textContent('.edge-tip');
    check('edge_names_its_text', tip.includes('shot') && tip.includes('beat 1') && tip.includes('waits on the shore'), tip);
    await page.mouse.click(point.x, point.y);
    await page.waitForSelector('.sheet');
    check('edge_opens_its_text', (await page.textContent('.sheet .here')).includes('waits on the shore'), await page.textContent('.sheet'));
    const onlyText = await page.evaluate(() => [...document.querySelectorAll('.sheet-body *')].every((node) => ['SECTION', 'H3', 'H4', 'P', 'CODE', 'SPAN'].includes(node.tagName)));
    check('words_are_text', onlyText);
    await page.keyboard.press('Escape');
  }

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
  await page.mouse.move(5, 300);

  // A take landing shows with no reload. The strip's + goes to POST runs; what Calcifer makes itself needs no machine.
  const picture = element(page, 'picture');
  const numberBefore = Math.max(0, ...((await asset('picture')).prefixes ?? []).map((version) => version.number));
  await picture.hover();
  await picture.locator('.take.add').click();
  await until('the frame landed', () => asset('picture'), (held) => held.standing === 'current' && Math.max(0, ...(held.prefixes ?? []).map((version) => version.number)) > numberBefore);
  await until('its picture shows, unreloaded', () => picture.locator('.media img').evaluateAll((images) => images.filter((image) => image.naturalWidth > 0).length), (n) => n > 0);
  await until('its dot', () => picture.locator('.dot.s-current').count(), (n) => n === 1);
  check('a_button_selects_nothing', (await page.locator('.react-flow__node.selected').count()) === 0, await page.getAttribute('.stage', 'data-focus'));
  check('landed_with_no_reload', await notReloaded());
  // A walk makes each tool whose reads settled, so the cut landed too: its pieces under it.
  await until('the cut landed', () => asset('edit'), (held) => held.standing === 'current');
  await until('its film shows', () => element(page, 'edit').locator('.media img').count(), (n) => n > 0);
  check('piece_strip', (await element(page, 'edit').locator('.beats button').allTextContents()).join() === 'clip,clip_b');
  const landedVersion = (await asset('picture')).prefixes?.at(-1);
  check('wire_version_main', landedVersion?.main?.field === 'image' && /^files\//.test(landedVersion?.main?.path ?? '') && landedVersion?.main?.sha256?.startsWith('sha256:'), landedVersion);
  await shot('landed');

  // Playing: a file here over Range with ?media=, two at most, and the third stops the first.
  await clip.locator('.play').click();
  await until('one video', () => page.locator('video').count(), (n) => n === 1);
  await until('it has its metadata', () => page.locator('video').first().evaluate((video) => video.readyState), (ready) => ready >= 1);
  await until('over Range with ?media=', () => Promise.resolve(fileAnswers), (all) => all.some((answer) => answer.status === 206 && answer.media));
  check('plays_by_range_and_media', true);
  await element(page, 'clip_b').locator('.play').click();
  await until('two videos', () => page.locator('video').count(), (n) => n === 2);
  await element(page, 'edit').locator('.play').click();
  await until('still two, the first stopped', () => clip.locator('video').count(), (n) => n === 0);
  await page.waitForTimeout(300);
  check('two_videos_at_most', (await page.locator('video').count()) === 2, await page.locator('video').count());
  await until('none playing', async () => {
    if (await page.locator('.stop').count()) await page.locator('.stop').first().click();
    return page.locator('video').count();
  }, (n) => n === 0);

  // A failure is named and stays: this asks for no run, as what it reads can never be made.
  const needs = element(page, 'needs');
  await needs.hover();
  await needs.locator('.take.add').click();
  const failed = await until('its reason', () => asset('needs'), (held) => held.failure);
  check('wire_failure', failed.failure.message.includes('cannot render') && Boolean(failed.failure.code), failed.failure);
  await until('its reason shows', () => needs.locator('.status.bad').count(), (n) => n === 1);
  check('failed_dot', (await needs.locator('.dot.s-failed').count()) === 1);
  check('try_again_offered', (await needs.locator('.status.bad .act').textContent()) === 'Try again');
  const told = spawnSync(COZY, ['project', '-p', folder, 'status'], { encoding: 'utf8' });
  check('terminal_hears_the_failure', `${told.stdout}${told.stderr}`.includes('cannot render'), `${told.stdout}${told.stderr}`.slice(0, 400));
  await page.reload();
  await page.waitForSelector('.react-flow__node');
  await page.evaluate(() => { window.notReloaded = true; });
  await until('still there after a reload', () => needs.locator('.status.bad').textContent(), (text) => text.includes('cannot render'));
  check('failure_survives_reload', true);
  const afterReload = await cards();
  check('rows_stand_after_reload', named.every((name) => afterReload[name].x === rows[name].x && afterReload[name].y === rows[name].y), afterReload);
  // Its text changes: the old reason no longer stands, with no reload.
  replaceFile(yamlAt, readFileSync(yamlAt, 'utf8').replace('video: <hollow_thing>', 'video: <clip>'));
  await until('the reason gone', () => asset('needs'), (held) => !held.failure);
  await until('and from the card', () => needs.locator('.status.bad').count(), (n) => n === 0);
  check('failure_gone_when_text_changes', await notReloaded());

  // A file that leaves is on another computer, and when it comes back it shows, with no reload: a pull writes neither file.
  const older = await asset('older_art');
  const olderAt = join(folder, older.path);
  renameSync(olderAt, `${olderAt}.away`);
  const away = await until('absent on the wire', () => asset('older_art'), (held) => held.absent === true);
  check('wire_absent_per_file', Object.values(away.files).every((file) => file.absent === true) && (await state()).absent > 0, away.files);
  await until('it says so', () => element(page, 'older_art').locator('.media-note').textContent(), (text) => text === 'On another computer');
  check('pull_offered', (await element(page, 'older_art').locator('.status .act').textContent()) === 'Pull');
  check('no_hash_when_absent', !(await page.evaluate(() => document.body.innerText)).includes(older.sha256.replace('sha256:', '').slice(0, 12)));
  await shot('absent');
  renameSync(`${olderAt}.away`, olderAt);
  await until('back', () => element(page, 'older_art').locator('.media-note').count(), (n) => n === 0);
  check('file_back_with_no_reload', await notReloaded());

  // Moving one: its row, no input of anything, and only its own edges.
  const inputs = async () => JSON.stringify((await state()).assets.map((held) => [held.name, held.inputs]));
  const inputsBefore = await inputs();
  const wasAt = (await cards()).hero_art;
  const touched = await redrawn(page, () => drag(page, element(page, 'hero_art').locator('.nm'), 90, 70));
  const movedTo = (await until('the moved row', cards, (now) => now.hero_art.x !== wasAt.x || now.hero_art.y !== wasAt.y)).hero_art;
  check('move_writes_its_row', movedTo.x > wasAt.x, { wasAt, movedTo });
  check('move_touches_no_input', (await inputs()) === inputsBefore);
  check('move_redraws_only_its_edges', touched.length > 0 && touched.every((edge) => edge.includes('hero_art/')), touched);

  // Versions, on real runs: a strip, looking at one changes only this page, pin writes the line and unpin removes it.
  seen.ran = local;
  if (local) {
    await until('a machine to send it to', () => page.locator('select[aria-label="The machine new takes go to"]').count(), (n) => n === 1);
    const card = element(page, 'card');
    const versions = async () => (await asset('card')).prefixes?.length ?? 0;
    const take = async (count) => {
      await card.hover();
      await card.locator('.take.add').click();
      await until(`version ${count}`, versions, (n) => n === count, FIRST_RUN_MS);
    };
    await take(1);
    await take(2);
    await until('its count', () => card.locator('.count').textContent(), (text) => text === '2/2');
    const made = await asset('card');
    check('wire_execution_ms', made.prefixes.every((version) => version.execution_ms > 0 && version.run > 0), made.prefixes);
    await card.hover();
    await card.locator('.take').first().click();
    await until('looking at the first', () => card.locator('.count').textContent(), (text) => text === '1/2');
    check('looking_is_only_this_page', (await asset('card')).version === made.version, (await asset('card')).version);
    await card.hover();
    await card.locator('button[aria-label=Pin]').click();
    // A hash on one of card's own lines: `image: <hero_art>` further down is hero's.
    await until('pinned in the file', () => Promise.resolve(readFileSync(yamlAt, 'utf8')), (text) => /\n {2}card:\n(?: {4}.*\n)*? {4}image: "?[0-9a-f]/.test(text));
    await until('pin lit', () => card.locator('button[aria-label=Unpin]').count(), (n) => n === 1);
    check('pin_is_that_version', (await asset('card')).approved === made.prefixes[0].number, (await asset('card')).approved);
    await card.locator('button[aria-label=Unpin]').click();
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

  // 300 elements with edges, on React Flow, by project-web-ui §8's three cases. What's measured is reported; what's checked is that each
  // case really happened, and that moving one redraws only its edges.
  const made = await api('/v1/projects', { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `see-${Date.now()}` }, body: JSON.stringify({ name: `web-see-scale-${Date.now()}` }) });
  try {
    const big = made.path;
    for (const [name, file] of [['a_picture', 'older.png'], ['a_video', 'clip.mp4']]) execFileSync(COZY, ['project', '-p', big, 'import', join(scratch, file), name]);
    const held = await api(`/v1/projects/${made.id}`);
    const hash = (name) => held.assets.find((row) => row.name === name).sha256.replace('sha256:', '').slice(0, 12);
    const COUNT = 300;
    const perRow = Math.ceil(Math.sqrt(COUNT));
    let text = 'version: 2\nassets:\n';
    let rowsText = '# Where cards sit. Only the page reads this. Nothing here is an input.\nversion: 1\ncards:\n';
    let edges = 0;
    for (let i = 0; i < COUNT; i++) {
      const reads = [1 + (i % 7), perRow + (i % 3)].filter((back) => i - back >= 0).map((back) => `<n${i - back}>`);
      edges += reads.length;
      text += `  n${i}:\n    ${i % 5 ? `image: "${hash('a_picture')}"` : `video: "${hash('a_video')}"`}\n    render:\n      with: see-no-such-package\n      prompt: from ${reads.join(' and ') || 'nothing'}\n`;
      rowsText += `  n${i}: {x: ${(i % perRow) * 304}, y: ${Math.floor(i / perRow) * 240}, w: 240}\n`;
    }
    replaceFile(join(big, 'project.yaml'), text);
    replaceFile(join(big, 'project.canvas.yaml'), rowsText);
    const began = Date.now();
    const bigState = await api(`/v1/projects/${made.id}`);
    const read_ms = Date.now() - began;
    check('scale_state', bigState.assets.length === COUNT, bigState.assets?.length);
    const open = async (zoom, x) => {
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
    const onScreen = (tab) => tab.evaluate(() => ({ cards: document.querySelectorAll('.react-flow__node').length, edges: document.querySelectorAll('.ge').length }));
    seen.scale = { elements: COUNT, edges, read_ms };

    let tab = await open(0.7, 40);
    const near = await panning(tab, 3000);
    seen.scale.pan_70 = { ...(await onScreen(tab)), fewest_cards: near.fewest, ...summary(near.frames) };
    check('scale_near_panned', near.places > 10, near.places);
    await tab.close();

    // At 11% the 18 columns are about 630 px of this 1800 px window. From here its pan, 480 px left and back, keeps them all on screen,
    // so every frame measured draws all 300.
    tab = await open(0.11, 800);
    const far = await panning(tab, 3000);
    seen.scale.pan_11 = { ...(await onScreen(tab)), fewest_cards: far.fewest, ...summary(far.frames) };
    check('scale_far_panned', far.places > 10, far.places);
    check('scale_far_shows_all', far.fewest === COUNT && seen.scale.pan_11.edges === edges, seen.scale.pan_11);
    await tab.close();

    tab = await open(0.7, 40);
    const name = element(tab, 'n39').locator('.nm');
    const box = await name.boundingBox();
    let frames;
    const moved = await redrawn(tab, async () => {
      await tab.mouse.move(box.x + 5, box.y + 5);
      await tab.mouse.down();
      await tab.mouse.move(box.x + 9, box.y + 5);
      const measuring = quiet(tab, 3000);
      const stopAt = Date.now() + 3000;
      for (let dx = 0; Date.now() < stopAt; dx = (dx + 7) % 280) await tab.mouse.move(box.x + 9 + dx, box.y + 5 + dx / 3);
      frames = await measuring;
      await tab.mouse.up();
    });
    seen.scale.move_70 = { ...(await onScreen(tab)), ...summary(frames), edges_redrawn: moved.length };
    check('scale_move_redraws_only_its_edges', moved.length > 0 && moved.every((edge) => /\bn39\//.test(edge)), moved);
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
})().catch((error) => {
  console.error(error);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
