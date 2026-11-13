// Place, in a real browser (web-ui.md §1): a PNG, an MP4 and a WAV dropped together; one chosen from the selector; one renamed; a
// move read back from project.db's rows; a reload that restores everything. Also: Esc cancels, a file that isn't media gets one line
// and the others land, the same file twice is two elements and one file, the hash shows nowhere on the page, the page's CSP holds.
//   node place.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// Last, where a box is let go or pulled to wins, and what it comes onto is pushed aside, down the line (Paul, 11-06).
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI, to check the
// terminal sees the same project and to lay boxes out; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, importFile, projectJson, calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, drop, openBox, openCanvas, png, launch, noStall, settled, until, wav, rowsOf, addBoxes, assetsOf, storedOf, fitCanvas, observeResizes } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const names = (page) => page.evaluate(() => [...document.querySelectorAll('.react-flow__node .nm, .react-flow__node .nm-field')]
  .map((el) => el.value ?? el.textContent).sort());
const element = boxOf;
/** Every box's fields as project.db stores them, as one text. */
const stored = (folder) => JSON.stringify(storedOf(folder));
/** The intentional denial owns a blank page under the server's actual policy. App errors stay on the app page. */
async function policyEvalProbe(context, base) {
  const native = await context.request.get(base);
  const policy = native.headers()['content-security-policy'];
  if (!native.ok() || !policy) return { ok: false, why: 'native page has no successful CSP response' };
  const probe = await context.newPage();
  const errors = [];
  const consoleErrors = [];
  probe.on('pageerror', (error) => errors.push(error.message));
  probe.on('console', (message) => { if (message.type() === 'error') consoleErrors.push(message.text()); });
  let evidence;
  try {
    const url = new URL(`/__cozy_e2e_policy_probe_${Date.now()}`, base).href;
    await probe.route(url, (route) => route.fulfill({ status: 200, contentType: 'text/html', headers: { 'Content-Security-Policy': policy }, body: '<!doctype html><html><head><link rel="icon" href="data:,"></head><body>Owned CSP negative control</body></html>' }));
    await probe.addInitScript(() => {
      window.probeViolations = [];
      document.addEventListener('securitypolicyviolation', (event) => window.probeViolations.push({ directive: event.effectiveDirective, blocked: event.blockedURI, policy: event.originalPolicy, disposition: event.disposition }));
    });
    const loaded = await probe.goto(url);
    // A timer runs as page code; the automation's own evaluation is allowed by the browser.
    const attempt = await probe.evaluate(() => new Promise((done) => setTimeout(() => {
      try { done({ refused: false, value: new Function('return 1')() }); }
      catch (error) { done({ refused: true, name: error.name }); }
    })));
    if (attempt.refused) await probe.waitForFunction(() => window.probeViolations.length > 0);
    const violations = await probe.evaluate(() => window.probeViolations);
    const expected = violations.length > 0 && violations.every((event) => event.directive === 'script-src' && event.blocked === 'eval' && event.policy === policy && event.disposition === 'enforce');
    evidence = { ok: attempt.refused && attempt.name === 'EvalError' && expected && errors.length === 0 && loaded.headers()['content-security-policy'] === policy, policy, attempt, violations, errors, consoleErrors };
  } finally {
    await probe.close();
  }
  return { ...evidence, closed: probe.isClosed() };
}
/** Its rows, as project.db holds them (cl-803). */
function rows(folder) {
  const found = {};
  for (const [name, x, y, w] of rowsOf(folder, 'SELECT name, x, y, w FROM canvas')) found[name] = { x, y, ...(w === null ? {} : { w }) };
  return found;
}
const FIELD = { png: 'image', jpg: 'image', mp4: 'video', wav: 'audio' };
const constants = (folder) => {
  const found = {};
  const files = readdirSync(folder).includes('files') ? readdirSync(join(folder, 'files')) : [];
  // A box that only approves a file: an import writes its version's number and its kind beside it.
  for (const [name, fields] of Object.entries(storedOf(folder))) {
    if (Object.keys(fields).filter((field) => !['selected_version', 'node_kind'].includes(field)).join() !== 'approved') continue;
    const hash = String(fields.approved);
    found[name] = { field: FIELD[files.find((file) => file.startsWith(hash))?.split('.').pop()], hash };
  }
  return found;
};

/** The exact view in which a row may be culled; this reads no API or client data. */
function placementState(expected) {
  const viewport = document.querySelector('.react-flow__viewport');
  const pane = document.querySelector('.react-flow__pane')?.getBoundingClientRect();
  const node = [...document.querySelectorAll('.react-flow__node')].find((one) => one.querySelector('.card[data-asset="raced"]'));
  const own = node && new DOMMatrix(getComputedStyle(node).transform);
  const matrix = viewport && new DOMMatrix(getComputedStyle(viewport).transform);
  const point = matrix?.transformPoint(new DOMPoint(expected.x, expected.y));
  const projected = point && pane ? { x: point.x + pane.left, y: point.y + pane.top, width: expected.w * Math.hypot(matrix.a, matrix.b) } : null;
  return {
    node: own ? { x: Math.round(own.e), y: Math.round(own.f) } : null,
    projected, expected_visible: Boolean(projected && projected.x >= pane.left && projected.x < pane.right && projected.y >= pane.top && projected.y < pane.bottom),
    viewport: viewport?.style.transform, matrix: matrix?.toString(), zoom: document.querySelector('.zoom-at')?.textContent,
    pane: pane && [pane.left, pane.top, pane.right, pane.bottom], mounted: document.querySelectorAll('.react-flow__node').length,
    open: document.querySelector('.card.here')?.dataset.asset, focus: `${document.activeElement?.tagName}.${document.activeElement?.className}`,
  };
}

async function placedInView(page, expected) {
  const evidence = { before: await page.evaluate(placementState, expected), samples: [], attempts: 0, fit_requested: true };
  await closeBox(page);
  await fitCanvas(page);
  let last;
  try {
    await until('the fitted page presents the CLI placement', async () => {
      last = await page.evaluate(placementState, expected);
      evidence.attempts++;
      evidence.samples.push(last);
      if (evidence.samples.length > 16) evidence.samples.shift();
      return last;
    }, (state) => state.expected_visible && state.node?.x === expected.x && state.node?.y === expected.y);
  } catch (error) { evidence.wait_error = error.message; }
  return { ...evidence, last };
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1500, height: 900 } });
  const page = await context.newPage();
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => {
    errors.push(`pageerror: ${error.message}`);
    (seen.page_error_details ??= []).push({ message: error.message, stack: error.stack });
  });
  await page.addInitScript(observeResizes);
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d/.test(m.text())) errors.push(`console: ${m.text()}`); });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI} ${e.sourceFile}:${e.lineNumber} ${e.sample}`));
  });

  // Sign in: the link's code becomes a session, and leaves the URL.
  await signIn(page);
  await page.waitForSelector('.top');
  check('code_left_the_url', !page.url().includes('code='), page.url());
  const session = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  check('session_kept', Boolean(session?.token && session?.media), session);

  // A project of its own, by name, from the Projects drawer.
  const projectName = `web-place-${Date.now()}`;
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', projectName);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = await page.evaluate(async (id) => {
    const { token } = JSON.parse(localStorage.getItem('cozy.session'));
    return (await fetch(`/v1/projects/${id}`, { headers: { Authorization: `Bearer ${token}` } })).json();
  }, id);
  const folder = state.path;
  seen.project = { id, folder };
  // A box another writer made: its words must outlast the page.
  const coast = 'A volcanic coast,\nunder a bright sky.';
  addBoxes(folder, [{ name: 'coast', kind: 'image', fields: { description: coast } }]);

  // A PNG, an MP4 and a WAV dropped together: each there at once under its stem, nothing focused.
  const pngBytes = png(60);
  await drop(page, [
    { name: 'Kyogre Art (1).png', type: 'image/png', bytes: pngBytes },
    { name: 'Clash Take.mp4', type: 'video/mp4', bytes: MP4 },
    { name: 'Sea.Sound.wav', type: 'audio/wav', bytes: wav() },
  ], 500, 300);
  // The coast box can show before or after these. A picture is placed once it's decoded, all of a drop together (cl-918).
  const atOnce = await until('three placed', async () => (await names(page)).filter((name) => name !== 'coast'), (now) => now.length === 3, 3000).catch(async () => (await names(page)).filter((name) => name !== 'coast'));
  check('three_appear_at_once', atOnce.length === 3, atOnce);
  check('none_focused', await page.evaluate(() => document.activeElement?.tagName !== 'INPUT'));
  check('stems_are_names', JSON.stringify(atOnce) === JSON.stringify(['clash_take', 'kyogre_art_1', 'sea_sound']), atOnce);
  await until('three constants in project.db', () => Object.keys(constants(folder)).length, (n) => n === 3);
  const three = constants(folder);
  check('constants_by_kind', three.kyogre_art_1?.field === 'image' && three.clash_take?.field === 'video' && three.sea_sound?.field === 'audio', three);
  // An import writes the constant, then the row, so the last row can come a moment after the third constant.
  await until('three rows in project.db', () => rows(folder), (now) => ['kyogre_art_1', 'clash_take', 'sea_sound'].every((name) => now[name]));
  check('rows_written', true);
  // Calcifer may fetch its ffmpeg for the first of these. The picture shows the file dropped, and no copy of it takes its place (cl-918); the video
  // and the sound show Calcifer's thumbnails.
  const thumbs = await until('thumbnails up', () => page.evaluate(() => ({
    up: [...document.querySelectorAll('.react-flow__node .media')].filter((media) => [...media.querySelectorAll('img')].some((img) => img.naturalWidth > 0)).length,
    made: [...document.querySelectorAll('.react-flow__node .media img')].filter((img) => img.naturalWidth > 0 && !img.src.startsWith('blob:')).length,
    own: [...document.querySelectorAll('.react-flow__node .card[data-asset="kyogre_art_1"] .media img')].map((img) => img.getAttribute('src')),
    none: [...document.querySelectorAll('.media-note')].map((note) => note.textContent),
  })), (now) => (now.up === 3 && now.made === 2) || now.none.includes('No preview'), 5 * STEP_MS);
  check('thumbnails', thumbs.up === 3 && thumbs.made === 2 && thumbs.own.length === 1 && thumbs.own[0].startsWith('blob:'), thumbs);
  if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-dropped.png` });

  // The top-bar plus opens the existing native selector and uploads the chosen file.
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), page.locator('.top .history [aria-label="Add files"]').click()]);
  check('selector_accepts_media', (await chooser.element().getAttribute('accept')) === 'image/*,video/*,audio/*');
  await chooser.setFiles({ name: 'Hero Shot.png', mimeType: 'image/png', buffer: png(120) });
  await page.waitForSelector('.nm-field');
  const focus = await page.evaluate(() => {
    const el = document.activeElement;
    return { field: el?.className.includes('nm-field'), value: el?.value, from: el?.selectionStart, to: el?.selectionEnd };
  });
  check('name_focused_stem_selected', focus.field && focus.value === 'hero_shot' && focus.from === 0 && focus.to === 'hero_shot'.length, focus);
  await page.keyboard.type('Hero Shot.v2');
  check('name_as_it_will_be', (await page.inputValue('.nm-field')) === 'Hero_Shot_v2', await page.inputValue('.nm-field'));
  await page.keyboard.press('Enter');
  await until('the chosen file named', () => constants(folder).Hero_Shot_v2, Boolean);

  // Renamed from its name in Focus's heading (cl-893).
  await openBox(page, 'sea_sound');
  await page.locator('[data-focused-asset="sea_sound"] .focus-name').click();
  await page.waitForSelector('.nm-field');
  await page.keyboard.type('ocean');
  await page.keyboard.press('Enter');
  await until('renamed in project.db', () => constants(folder), (all) => all.ocean && !all.sea_sound);
  await closeBox(page);
  check('renamed_row_moved', Boolean((await until('renamed row', () => rows(folder), (all) => all.ocean)).ocean));
  check('rename_shown', (await names(page)).includes('ocean'), await names(page));

  // Esc cancels an import: nothing in the project.
  const before = stored(folder);
  await drop(page, [{ name: 'Never.png', type: 'image/png', bytes: png(200) }], 300, 600);
  await page.waitForSelector('.nm-field');
  await page.keyboard.press('Escape');
  await until('Esc removed it', () => names(page), (all) => !all.includes('never'));
  await new Promise((done) => setTimeout(done, 1000));
  check('esc_leaves_nothing', stored(folder) === before);

  // A file that isn't media gets one line; the one beside it lands. The same file twice is two elements and one file.
  // Let go on bare canvas, the nearest to 900,650: on a box of pictures they'd be its next version (place.ts join). Where the coast comes
  // races the first drop, and Hero_Shot pushes it down onto 900,650 from its home right of clash_take (Windows 43288869140).
  const bare = await page.evaluate(() => {
    const open = (x, y) => [[0, 0], [-40, -40], [40, -40], [-40, 40], [40, 40]].every(([dx, dy]) => document.elementFromPoint(x + dx, y + dy)?.classList.contains('react-flow__pane'));
    const spots = [];
    for (let y = 120; y <= innerHeight - 120; y += 20) {
      for (let x = 320; x <= innerWidth - 120; x += 20) if (open(x, y)) spots.push({ x, y });
    }
    return spots.sort((a, b) => Math.hypot(a.x - 900, a.y - 650) - Math.hypot(b.x - 900, b.y - 650))[0] ?? null;
  });
  if (!bare) throw new Error('no bare canvas to let the copy go on');
  seen.copy_let_go_at = bare;
  const filesBefore = readdirSync(join(folder, 'files')).length;
  await drop(page, [
    { name: 'notes.txt', type: 'text/plain', bytes: Buffer.from('these are not pictures\n'.repeat(4)) },
    { name: 'Kyogre Art (1).png', type: 'image/png', bytes: pngBytes },
  ], bare.x, bare.y);
  await until('the copy landed', () => constants(folder).kyogre_art_2, Boolean);
  const lines = await until('one line for the text file', () => page.locator('.note .line').allTextContents(), (all) => all.some((line) => line.includes('notes.txt')));
  check('one_line_for_non_media', lines.filter((line) => line.includes('notes.txt')).length === 1, lines);
  check('non_media_not_on_canvas', !(await names(page)).includes('notes'), await names(page));
  check('same_file_one_file', readdirSync(join(folder, 'files')).length === filesBefore);
  const all = constants(folder);
  check('same_file_same_hash', all.kyogre_art_1.hash === all.kyogre_art_2.hash, all);
  check('same_file_two_elements', (await names(page)).includes('kyogre_art_2'), await names(page));

  // Dragged: its row in project.db moves by what the mouse did, and no input changes.
  const box = await element(page, 'clash_take').locator('.nm').boundingBox();
  const was = rows(folder).clash_take;
  const zoom = await page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
  const storedBeforeMove = stored(folder);
  // Somewhere clear, the nearest at least 120 off: let go on another box, it would stay there a little smaller and push that aside.
  const { dx, dy } = await page.evaluate((name) => {
    const rects = [...document.querySelectorAll('.react-flow__node')].map((node) => ({ name: node.querySelector('.nm')?.textContent, rect: node.getBoundingClientRect() }));
    const mine = rects.find((held) => held.name === name).rect;
    const others = rects.filter((held) => held.name !== name).map((held) => held.rect);
    const gap = 32;
    const clear = [];
    for (let y = -600; y <= 600; y += 30) {
      for (let x = -900; x <= 900; x += 30) {
        const [left, top, right, bottom] = [mine.left + x - gap, mine.top + y - gap, mine.right + x + gap, mine.bottom + y + gap];
        const onScreen = top > 60 && left > 230 && bottom < innerHeight - 20 && right < innerWidth - 20;
        if (Math.hypot(x, y) >= 120 && onScreen && !others.some((other) => other.left < right && other.right > left && other.top < bottom && other.bottom > top)) clear.push({ dx: x, dy: y });
      }
    }
    if (!clear.length) throw new Error(`nowhere clear to move it: ${JSON.stringify({ mine, others })}`);
    return clear.sort((a, b) => Math.hypot(a.dx, a.dy) - Math.hypot(b.dx, b.dy))[0];
  }, 'clash_take');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 2, box.y + box.height / 2);
  await page.mouse.move(box.x + box.width / 2 + dx, box.y + box.height / 2 + dy, { steps: 5 });
  await page.mouse.up();
  const row = (await until('the moved row', () => rows(folder), (now) => now.clash_take && (now.clash_take.x !== was.x || now.clash_take.y !== was.y))).clash_take;
  check('move_in_canvas_file', Math.abs(row.x - was.x - dx / zoom) <= 3 && Math.abs(row.y - was.y - dy / zoom) <= 3, { was, row, zoom, dx, dy });
  check('move_touches_no_input', stored(folder) === storedBeforeMove);

  // A drag on the empty canvas moves it and selects nothing; with Shift it selects instead.
  const empty = await page.evaluate(() => {
    const pane = document.querySelector('.react-flow__pane').getBoundingClientRect();
    for (let y = pane.top + 140; y < pane.bottom - 140; y += 40) {
      for (let x = pane.left + 340; x < pane.right - 140; x += 40) {
        if (document.elementFromPoint(x, y)?.classList.contains('react-flow__pane')) return { x, y };
      }
    }
    return null;
  });
  check('empty_canvas_found', empty !== null);
  const viewport = () => page.evaluate(() => document.querySelector('.react-flow__viewport').style.transform);
  const held = await viewport();
  const selected = () => page.locator('.react-flow__node.selected').count();
  const selectedBefore = await selected();
  await page.mouse.move(empty.x, empty.y);
  await page.mouse.down();
  await page.mouse.move(empty.x + 90, empty.y + 60, { steps: 6 });
  await page.mouse.up();
  check('dragging_the_canvas_moves_it', (await viewport()) !== held);
  check('dragging_the_canvas_selects_nothing', (await selected()) === selectedBefore);
  const moved = await viewport();
  await page.keyboard.down('Shift');
  await page.mouse.move(empty.x, empty.y);
  await page.mouse.down();
  await page.mouse.move(empty.x + 40, empty.y + 40, { steps: 4 });
  const drawing = await page.locator('.react-flow__selection').count();
  await page.mouse.up();
  await page.keyboard.up('Shift');
  check('shift_draws_a_selection', drawing === 1 && (await viewport()) === moved);

  // The hash shows nowhere on the page.
  const hashes = Object.values(constants(folder)).map((constant) => constant.hash);
  const shown = await page.evaluate(() => document.body.innerText + [...document.querySelectorAll('[title],[aria-label]')].map((el) => el.title + el.ariaLabel).join(' '));
  check('hash_not_shown', hashes.every((hash) => !shown.includes(hash)));

  const viewportNow = () => page.evaluate(() => getComputedStyle(document.querySelector('.react-flow__viewport')).transform);

  // Reload restores everything: names, positions, viewport, thumbnails.
  const namesBefore = await names(page);
  // By name, each element with a file has its picture back, whatever was up before. The coast is an element with none.
  const withFiles = Object.keys(constants(folder)).sort();
  const picturesUp = () => page.evaluate(() => [...document.querySelectorAll('.react-flow__node')].filter((node) => [...node.querySelectorAll('.media img')].some((img) => img.naturalWidth > 0)).map((node) => node.querySelector('.nm')?.textContent).sort());
  check('elements_are_its_files_and_its_words', JSON.stringify(namesBefore) === JSON.stringify([...withFiles, 'coast'].sort()), { namesBefore, withFiles });
  const places = () => page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.react-flow__node')].map((node) => [node.querySelector('.nm')?.textContent, getComputedStyle(node).transform]).sort()));
  const placesBefore = await places();
  await new Promise((done) => setTimeout(done, 300));
  const viewportBefore = await viewportNow();
  await page.reload();
  await until('names back', () => names(page), (now) => JSON.stringify(now) === JSON.stringify(namesBefore));
  await until('thumbnails back', picturesUp, (now) => JSON.stringify(now) === JSON.stringify(withFiles));
  check('reload_positions', JSON.stringify(await places()) === JSON.stringify(placesBefore), { before: placesBefore, after: await places() });
  check('reload_viewport', (await viewportNow()) === viewportBefore, { before: viewportBefore, after: await viewportNow() });
  if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-reloaded.png` });

  check('text_kept', assetsOf(folder).find((asset) => asset.name === 'coast')?.fields.description === coast, assetsOf(folder).find((asset) => asset.name === 'coast'));

  // Where a box is put wins (Paul, 11-06: "the placement position of the new object should be primary, and then other objects should move around it
  // to make room, although the new object can also of course shrink in size if needed slightly"; "Resizing it will push the other surrounding objects
  // out of the way"). On boxes the CLI lays out past everything else, each let go by hand straight down onto its spot, or pulled by its corner:
  // - let go on another, it stays and the other is pushed aside, the least way, and nothing else moves;
  // - in a gap a little short of it, what's either side goes a little apart and it stays as it was;
  // - in a crowd, it stays, maybe a little smaller, and what it lies on goes aside;
  // - in a gap short of it between two tight rows, it shrinks a little rather than shove both rows;
  // - A B C D E in a row, B pulled wider by its corner: B stays, C goes right just enough, then D, then E, and nothing else moves
  //   (Paul: "A simpler change would be to move C a bit to make room for the now-larger B"; "C displaces D, which displaces E, and so on").
  //   One Undo puts all of it back.
  if (process.env.COZY) {
    const pictures = mkdtempSync(join(tmpdir(), 'web-place-'));
    const at = (name, x, y, w) => projectJson(folder, 'PATCH', '/canvas', { cards: { [name]: { x, y, w } } });
    const lay = {};
    const left = Math.max(...Object.values(rows(folder)).map((row) => row.x)) + 1500;
    const put = (name, x, y, w) => { lay[name] = { x: left + x, y, w }; };
    // Rows of the grid meant to touch: a picture 300 wide (its name 32, the picture 169, its row 40) and the room it keeps under it, 92
    // (shown.ts heightFor and underOf). That room has changed with the box three times; where the page keeps more, it gives way and the
    // layout is the one it settles to (`laid`, below), which every drop and pull is judged against.
    const ROW = 32 + 169 + 40 + 92;
    for (const [i, j] of [[0, 0], [1, 0], [2, 0], [0, 1], [2, 1], [0, 2], [1, 2], [2, 2]]) put(`grid_${i}_${j}`, 320 * i, ROW * j, 300);
    // The chain beside the grid and the rest under it, each clear of the one above as laid out (a 240 picture keeps 299, the crowded 360 one
    // 367): fitted, it all shows at a zoom of 0.2 or more, where a box has its corner (CanvasHost's far band; all under the grid, 0.197).
    const BELOW = 3 * ROW + 14;
    put('crowded', 290, BELOW, 360);
    put('beside', 0, BELOW + 400, 240);
    put('free', 200, BELOW + 700, 240);
    put('left', 0, BELOW + 1000, 240);
    put('right', 468, BELOW + 1000, 240);
    put('nudging', 234, BELOW + 1300, 240);
    const CHAIN = ['chain_a', 'chain_b', 'chain_c', 'chain_d', 'chain_e'];
    CHAIN.forEach((name, i) => put(name, 1100 + 260 * i, 0, 240));
    // Two rows four long, 10 apart, with a gap 4 short of the box between them, and room beyond.
    for (let i = 0; i < 4; i++) put(`row_l${i}`, 250 * i, BELOW + 1700, 240);
    for (let i = 0; i < 4; i++) put(`row_r${i}`, 990 + 236 + 250 * i, BELOW + 1700, 240);
    put('squeezed', 990 + 118 - 120, BELOW + 2100, 240);
    Object.keys(lay).forEach((name, i) => {
      const file = join(pictures, `${name}.png`);
      writeFileSync(file, png(40 + i * 9, 320, 180));
      importFile(folder, file, name);
    });
    // Each placed by the page as it arrives first: placed after its `at`, it would be put where the page found room (the page's read of where
    // things sit, then its write of a new one's place, can straddle another writer's).
    const placed = await until('each placed', () => rows(folder), (now) => Object.keys(lay).every((name) => now[name]));
    for (const [name, { x, y, w }] of Object.entries(lay)) at(name, x, y, w);
    rmSync(pictures, { recursive: true, force: true });
    // Each written where it was put (or given way from it, below): moved off where the page first put it, unless that was already there.
    await until('laid out', () => rows(folder), (now) => Object.entries(lay).every(([name, row]) => (now[name]?.x !== placed[name].x || now[name]?.y !== placed[name].y) || (placed[name].x === row.x && placed[name].y === row.y)));
    const away = await openCanvas(page);
    await page.mouse.click(away.x, away.y);
    await page.keyboard.press('f');
    await settled(page);
    // A picture box's height at a width: its name, and the picture at 16:9.
    const tall = (w) => 32 + Math.round((w * 9) / 16);
    const middle = (row) => ({ x: row.x + row.w / 2, y: row.y + tall(row.w) / 2 });
    const apart = (a, b) => Math.round(Math.hypot(middle(a).x - middle(b).x, middle(a).y - middle(b).y));
    const lying = () => page.evaluate(() => {
      const cards = [...document.querySelectorAll('.react-flow__node .card')].map((card) => ({ name: card.dataset.asset, rect: card.getBoundingClientRect() }));
      const on = (a, b) => a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
      return cards.flatMap((a, i) => cards.slice(i + 1).filter((b) => on(a.rect, b.rect)).map((b) => `${a.name} on ${b.name}`));
    });
    const zoomNow = () => page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
    // Rows that have stopped changing, both what the page wrote and what it shows.
    const settledRows = (name) => until(`${name} settled`, async () => {
      const one = JSON.stringify(rows(folder));
      await page.waitForTimeout(400);
      return one === JSON.stringify(rows(folder)) ? rows(folder) : null;
    }, Boolean);
    const movedSince = (before, after, name) => Object.fromEntries(Object.keys(lay).filter((other) => other !== name && apart(after[other], before[other]) > 0).map((other) => [other, apart(after[other], before[other])]));
    /** Let go `dy` below or above where it stands, held by its picture's middle: where it was let go is where it stood as the button came up. */
    const letGo = async (name, dy) => {
      const before = rows(folder);
      const zoom = await zoomNow();
      const media = await element(page, name).locator('.media').first().boundingBox();
      const [x, y] = [media.x + media.width / 2, media.y + media.height / 2];
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x, y - 3);
      await page.mouse.move(x, y + dy * zoom, { steps: 10 });
      const held = await element(page, name).evaluate((node) => {
        const { e, f } = new DOMMatrix(getComputedStyle(node).transform);
        return { e, f };
      });
      await page.mouse.up();
      // It lands, and what it lies on goes aside, in one write.
      // A row not there (between writes, or gone: let go on a picture, a box joins it) is not yet its write.
      await until(`${name} written`, () => Promise.resolve(rows(folder)[name]), (now) => Boolean(now) && (now.x !== before[name].x || now.y !== before[name].y)).catch(() => {});
      const after = await settledRows(name);
      if (!after[name]) throw new Error(`${name} left the canvas when let go: no row for it (${Object.keys(after).length} rows), as when a box is let go on a picture and joins it`);
      return { dropped: { ...before[name], x: Math.round(held.e), y: Math.round(held.f) }, now: after[name], moved: movedSince(before, after, name), lying: await lying() };
    };
    const stays = (one) => apart(one.now, one.dropped);
    // The layout as the page settled it, nothing on another: each drop below is let go onto its target as it lies here.
    const laid = await settledRows('the layout');
    seen.given_way = Object.keys(lay).filter((name) => laid[name].x !== lay[name].x || laid[name].y !== lay[name].y);
    check('laid_out_on_nothing', (await lying()).length === 0, await lying());

    const free = await letGo('free', laid.beside.y - laid.free.y);
    seen.pushed = { ...free, by: stays(free) };
    check('a_box_let_go_on_another_stays', free.now.w === 240 && seen.pushed.by <= 1, seen.pushed);
    // `beside` lay 40 under its left side: it goes left, a gap clear, and nothing else moves.
    const pushedTo = rows(folder).beside;
    check('what_it_lies_on_is_pushed_aside', Object.keys(free.moved).join() === 'beside' && pushedTo.y === laid.beside.y && pushedTo.x === free.now.x - 16 - 240, { moved: free.moved, beside: pushedTo, free: free.now });
    check('a_push_lies_on_nothing', free.lying.length === 0, free.lying);

    const nudging = await letGo('nudging', laid.left.y - laid.nudging.y);
    seen.nudging = { ...nudging, by: stays(nudging) };
    const outward = rows(folder);
    check('small_pushes_absorb_a_drop', nudging.now.w === 240 && seen.nudging.by <= 1 && outward.left.x === nudging.now.x - 16 - 240 && outward.right.x === nudging.now.x + 240 + 16 && Object.keys(nudging.moved).sort().join() === 'left,right', seen.nudging);
    check('a_nudged_drop_lies_on_nothing', nudging.lying.length === 0, nudging.lying);

    const crowded = await letGo('crowded', laid.grid_0_1.y + 3 - laid.crowded.y);
    seen.crowded = { ...crowded, by: stays(crowded) };
    check('a_crowded_drop_stays_where_it_was_let_go', crowded.now.w >= 360 * 0.8 && seen.crowded.by <= 3 && Object.keys(crowded.moved).length > 0, seen.crowded);
    check('a_crowded_drop_lies_on_nothing', crowded.lying.length === 0, crowded.lying);

    const squeezed = await letGo('squeezed', laid.row_l0.y - laid.squeezed.y);
    seen.squeezed = { ...squeezed, by: stays(squeezed) };
    const pushedRows = Object.values(squeezed.moved).reduce((sum, by) => sum + by, 0);
    check('a_tight_gap_shrinks_it_a_little', squeezed.now.w < 240 && squeezed.now.w >= 240 * 0.8 && seen.squeezed.by <= 3 && pushedRows < 8 * 18, seen.squeezed);
    check('a_shrunk_drop_lies_on_nothing', squeezed.lying.length === 0, squeezed.lying);

    // The chain in view at a zoom where a box has its corner (0.2 and up, CanvasHost's far band), whatever the whole layout fits at: the five
    // selected, zoomed to (Shift+2), and let go of.
    for (const [i, name] of CHAIN.entries()) {
      const media = await element(page, name).locator('.media').first().boundingBox();
      if (i) await page.keyboard.down('Shift');
      await page.mouse.click(media.x + media.width / 2, media.y + media.height / 2);
      if (i) await page.keyboard.up('Shift');
    }
    await page.keyboard.press('Shift+Digit2');
    await settled(page);
    await page.keyboard.press('Escape');
    // B pulled 100 wider by its corner, live: each after it goes right as little as keeps a gap of 16, or what it had if less.
    const before = rows(folder);
    const zoom = await zoomNow();
    await element(page, 'chain_b').locator('.media').first().hover();
    const corner = await element(page, 'chain_b').locator('.corner').boundingBox();
    const originalFrom = { x: corner.x + corner.width / 2, y: corner.y + corner.height / 2 };
    const originalTo = { x: originalFrom.x + 100 * zoom, y: originalFrom.y };
    // Native mouse coordinates are integers in WebKit: choose the nearest representable 100-wide pull.
    const from = { x: Math.round(originalFrom.x), y: Math.round(originalFrom.y) };
    const to = { x: from.x + Math.round(100 * zoom), y: from.y };
    const input = { zoom, corner, originalFrom, originalTo, from, to, steps: 10, nearestWidth: before.chain_b.w + (to.x - from.x) / zoom };
    seen.chain = { input };
    if (!Number.isFinite(input.nearestWidth) || Math.abs(Math.round(input.nearestWidth) - 340) > 2) throw new Error('resize input precondition: nearest integer pointer endpoints cannot reach 340 ±2; see chain.input');
    await page.evaluate(() => {
      const events = [];
      const record = (event) => {
        const matrix = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
        events.push({ type: event.type, at_ms: performance.now(), x: event.clientX, y: event.clientY, pointerId: event.pointerId, buttons: event.buttons, trusted: event.isTrusted, asset: event.target.closest('.card')?.dataset.asset, camera: { x: matrix.e, y: matrix.f, zoom: matrix.a } });
      };
      const types = ['pointerdown', 'pointermove', 'pointerup', 'pointercancel', 'lostpointercapture'];
      types.forEach((type) => document.addEventListener(type, record, true));
      window.chainPull = { events, stop: () => types.forEach((type) => document.removeEventListener(type, record, true)) };
    });
    let live;
    try {
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      await page.mouse.move(to.x, to.y, { steps: 10 });
      await page.waitForTimeout(300);
      live = await page.evaluate((names) => Object.fromEntries(names.map((name) => {
        const node = [...document.querySelectorAll('.react-flow__node')].find((one) => one.querySelector(`.card[data-asset="${name}"]`));
        return [name, Math.round(new DOMMatrix(getComputedStyle(node).transform).e)];
      })), CHAIN);
    } finally {
      try { await page.mouse.up(); }
      finally {
        input.delivered = await page.evaluate(() => {
          const { events, stop } = window.chainPull;
          stop();
          delete window.chainPull;
          return events;
        });
      }
    }
    await until('chain_b written', () => Promise.resolve(rows(folder).chain_b), (now) => now.w !== before.chain_b.w);
    const after = await settledRows('chain_b');
    const b = after.chain_b;
    const want = { chain_a: before.chain_a.x, chain_b: before.chain_b.x };
    for (let i = 2; i < CHAIN.length; i++) {
      const [prev, name] = [CHAIN[i - 1], CHAIN[i]];
      const gap = Math.min(16, before[name].x - before[prev].x - 240);
      want[name] = Math.max(before[name].x, want[prev] + (prev === 'chain_b' ? b.w : 240) + gap);
    }
    seen.chain = { before: Object.fromEntries(CHAIN.map((name) => [name, before[name]])), after: Object.fromEntries(CHAIN.map((name) => [name, after[name]])), want, live, input };
    check('a_box_pulled_wider_stays', b.x === before.chain_b.x && b.y === before.chain_b.y && Math.abs(b.w - 340) <= 2, seen.chain);
    check('its_neighbours_go_right_just_enough', CHAIN.every((name) => after[name].x === want[name] && after[name].y === before[name].y) && after.chain_c.x > before.chain_c.x, seen.chain);
    check('the_push_goes_down_the_line', after.chain_c.x - before.chain_c.x >= after.chain_d.x - before.chain_d.x && after.chain_d.x - before.chain_d.x >= after.chain_e.x - before.chain_e.x && after.chain_e.x > before.chain_e.x, seen.chain);
    check('nothing_off_the_line_moves', Object.keys(movedSince(before, after, 'chain_b')).sort().join() === 'chain_c,chain_d,chain_e', movedSince(before, after, 'chain_b'));
    check('pushed_while_pulled', CHAIN.slice(2).every((name) => live[name] > before[name].x + 40), seen.chain);
    check('a_pull_lies_on_nothing', (await lying()).length === 0, await lying());
    // One Undo, all of it.
    await page.keyboard.press('Control+z');
    const undone = await until('all back', () => Promise.resolve(rows(folder)), (now) => CHAIN.every((name) => now[name].x === before[name].x && now[name].y === before[name].y && now[name].w === before[name].w)).catch(() => rows(folder));
    check('one_undo_puts_the_pull_back', CHAIN.every((name) => undone[name].x === before[name].x && undone[name].w === before[name].w), Object.fromEntries(CHAIN.map((name) => [name, undone[name]])));
    if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-given-way.png` });
  }

  // A box the page places as it arrives, and the CLI's `at` for it written between the page's read of where things sit and its write of
  // the place it found: the CLI's stands (cl-800). The page's read is held until the CLI has written, so the two cross every time.
  if (process.env.COZY) {
    const pictures = mkdtempSync(join(tmpdir(), 'web-place-race-'));
    const file = join(pictures, 'raced.png');
    writeFileSync(file, png(222, 320, 180));
    let [read, release] = [false, () => {}];
    const held = new Promise((done) => { release = done; });
    const canvasRead = `**/v1/projects/${id}/canvas`;
    await page.route(canvasRead, async (route) => {
      if (route.request().method() !== 'GET' || read) return route.fallback();
      read = true;
      const answer = await route.fetch();
      await held;
      await route.fulfill({ response: answer }).catch(() => {});
    });
    importFile(folder, file, 'raced');
    await until('the page reads where things sit', () => Promise.resolve(read), Boolean);
    // Clear of everything, so nothing moves it off another.
    const cli = { x: Math.max(...Object.values(rows(folder)).map((row) => row.x + (row.w ?? 240))) + 2000, y: 0, w: 240 };
    projectJson(folder, 'PATCH', '/canvas', { cards: { raced: { x: cli.x, y: cli.y, w: cli.w } } });
    release();
    // The page's place goes now, after the CLI's: what's written is still the CLI's, once and for good.
    const settled = await until('its row settled', async () => {
      const one = JSON.stringify(rows(folder).raced);
      await page.waitForTimeout(1500);
      return one === JSON.stringify(rows(folder).raced) ? rows(folder).raced : null;
    }, Boolean);
    await page.unroute(canvasRead);
    check('a_placement_never_overwrites_an_at', settled.x === cli.x && settled.y === cli.y && settled.w === cli.w, { settled, cli });
    const presentation = await placedInView(page, cli);
    seen.cli_placement = presentation;
    const shown = presentation.last?.node;
    check('the_page_shows_the_cli_place', presentation.last?.expected_visible && shown?.x === cli.x && shown?.y === cli.y, { shown, cli, presentation });
    rmSync(pictures, { recursive: true, force: true });
  }

  // The terminal sees the same project.
  if (process.env.COZY) {
    const status = projectJson(folder);
    const held = status.assets.map((asset) => asset.name);
    check('cli_sees_it', ['kyogre_art_1', 'kyogre_art_2', 'clash_take', 'ocean', 'Hero_Shot_v2'].every((name) => held.includes(name)), held);
  }

  // Everything above hashed in WebAssembly under the page's own CSP, with no violation; `eval` stays refused beside it.
  const violations = await page.evaluate(() => window.violations);
  check('csp_holds', violations.length === 0, { violations, styles: await page.evaluate(() => [...document.querySelectorAll('style')].map((held) => held.textContent.slice(0, 80))) });
  seen.eval_probe = await policyEvalProbe(context, base);
  check('eval_refused', seen.eval_probe.ok && seen.eval_probe.closed, seen.eval_probe);
  seen.resize_observers = await page.evaluate(() => window.resizeEvidence ?? null);
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
