// Place, in a real browser (web-ui.md §1): a PNG, an MP4 and a WAV dropped together; one chosen from the selector; one renamed; a
// move read back from project.canvas.yaml; a reload that restores everything. Also: Esc cancels, a file that isn't media gets one line
// and the others land, the same file twice is two elements and one file, the hash shows nowhere but the yaml pane, the page's CSP holds.
//   node place.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// Last, a box let go on others gives way at the least cost: a short move, a small shrink, or a little nudge of what it lies on (Paul, 11-06).
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI, to check the
// terminal sees the same project and to lay boxes out; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, drop, openBox, png, launch, noStall, until, wav } = require('./page');

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
const yaml = (folder) => readFileSync(join(folder, 'project.yaml'), 'utf8');
function rows(folder) {
  let text;
  try { text = readFileSync(join(folder, 'project.canvas.yaml'), 'utf8'); } catch { return {}; }
  const found = {};
  for (const line of text.split('\n')) {
    const m = line.match(/^\s+(\S+):\s*\{x:\s*(-?[\d.]+),\s*y:\s*(-?[\d.]+)(?:,\s*w:\s*(-?[\d.]+))?/);
    if (m) found[m[1]] = { x: Number(m[2]), y: Number(m[3]), ...(m[4] === undefined ? {} : { w: Number(m[4]) }) };
  }
  return found;
}
const FIELD = { png: 'image', jpg: 'image', mp4: 'video', wav: 'audio' };
const constants = (folder) => {
  const found = {};
  const files = readdirSync(folder).includes('files') ? readdirSync(join(folder, 'files')) : [];
  for (const line of yaml(folder).split('\n')) {
    const m = line.match(/^\s+(\S+):\s*\{approved:\s*"?(\w+)"?\}/);
    if (m) found[m[1]] = { field: FIELD[files.find((name) => name.startsWith(m[2]))?.split('.').pop()], hash: m[2] };
  }
  return found;
};

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1500, height: 900 } });
  const page = await context.newPage();
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
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
  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', projectName);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const state = await page.evaluate(async (id) => {
    const { token } = JSON.parse(localStorage.getItem('cozy.session'));
    return (await fetch(`/v1/projects/${id}`, { headers: { Authorization: `Bearer ${token}` } })).json();
  }, id);
  const folder = state.path;
  seen.project = { id, folder };
  // A file a person wrote: every comment and `|` of it must outlast the page.
  const written = '# Mine, and kept.\nassets:\n  coast:\n    # Kept too.\n    description: |\n      A volcanic coast,\n      under a bright sky.\n';
  writeFileSync(join(folder, 'project.yaml'), written);

  // A PNG, an MP4 and a WAV dropped together: each there at once under its stem, nothing focused.
  const pngBytes = png(60);
  await drop(page, [
    { name: 'Kyogre Art (1).png', type: 'image/png', bytes: pngBytes },
    { name: 'Clash Take.mp4', type: 'video/mp4', bytes: MP4 },
    { name: 'Sea.Sound.wav', type: 'audio/wav', bytes: wav() },
  ], 500, 300);
  // The asset in the file above can show before or after these.
  const atOnce = (await names(page)).filter((name) => name !== 'coast');
  check('three_appear_at_once', atOnce.length === 3, atOnce);
  check('none_focused', await page.evaluate(() => document.activeElement?.tagName !== 'INPUT'));
  check('stems_are_names', JSON.stringify(atOnce) === JSON.stringify(['clash_take', 'kyogre_art_1', 'sea_sound']), atOnce);
  await until('three constants in project.yaml', () => Object.keys(constants(folder)).length, (n) => n === 3);
  const three = constants(folder);
  check('constants_by_kind', three.kyogre_art_1?.field === 'image' && three.clash_take?.field === 'video' && three.sea_sound?.field === 'audio', three);
  // An import writes the constant, then the row, so the last row can come a moment after the third constant.
  await until('three rows in project.canvas.yaml', () => rows(folder), (now) => ['kyogre_art_1', 'clash_take', 'sea_sound'].every((name) => now[name]));
  check('rows_written', true);
  // Calcifer may fetch its ffmpeg for the first of these.
  const thumbs = await until('thumbnails up', () => page.evaluate(() => ({
    up: [...document.querySelectorAll('.react-flow__node .media img')].filter((img) => img.naturalWidth > 0 && !img.src.startsWith('blob:')).length,
    none: [...document.querySelectorAll('.media-note')].map((note) => note.textContent),
  })), (now) => now.up === 3 || now.none.includes('No preview'), 5 * STEP_MS);
  check('thumbnails', thumbs.up === 3, thumbs);
  if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-dropped.png` });

  // One chosen from the selector, in the tools' button: its field is focused with the stem selected; a name shows as it will be.
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), page.click('.tools button:has-text("Add file")')]);
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

  // Renamed by double-click.
  await openBox(page, 'sea_sound');
  await element(page, 'sea_sound').locator('.nm').dblclick();
  await page.waitForSelector('.nm-field');
  await page.keyboard.type('ocean');
  await page.keyboard.press('Enter');
  await until('renamed in project.yaml', () => constants(folder), (all) => all.ocean && !all.sea_sound);
  await closeBox(page);
  check('renamed_row_moved', Boolean((await until('renamed row', () => rows(folder), (all) => all.ocean)).ocean));
  check('rename_shown', (await names(page)).includes('ocean'), await names(page));

  // Esc cancels an import: nothing in the project.
  const before = yaml(folder);
  await drop(page, [{ name: 'Never.png', type: 'image/png', bytes: png(200) }], 300, 600);
  await page.waitForSelector('.nm-field');
  await page.keyboard.press('Escape');
  await until('Esc removed it', () => names(page), (all) => !all.includes('never'));
  await new Promise((done) => setTimeout(done, 1000));
  check('esc_leaves_nothing', yaml(folder) === before);

  // A file that isn't media gets one line; the one beside it lands. The same file twice is two elements and one file.
  const filesBefore = readdirSync(join(folder, 'files')).length;
  await drop(page, [
    { name: 'notes.txt', type: 'text/plain', bytes: Buffer.from('these are not pictures\n'.repeat(4)) },
    { name: 'Kyogre Art (1).png', type: 'image/png', bytes: pngBytes },
  ], 900, 650);
  await until('the copy landed', () => constants(folder).kyogre_art_2, Boolean);
  const lines = await until('one line for the text file', () => page.locator('.note .line').allTextContents(), (all) => all.some((line) => line.includes('notes.txt')));
  check('one_line_for_non_media', lines.filter((line) => line.includes('notes.txt')).length === 1, lines);
  check('non_media_not_on_canvas', !(await names(page)).includes('notes'), await names(page));
  check('same_file_one_file', readdirSync(join(folder, 'files')).length === filesBefore);
  const all = constants(folder);
  check('same_file_same_hash', all.kyogre_art_1.hash === all.kyogre_art_2.hash, all);
  check('same_file_two_elements', (await names(page)).includes('kyogre_art_2'), await names(page));

  // Dragged: the row in project.canvas.yaml moves by what the mouse did, and no input changes.
  const box = await element(page, 'clash_take').locator('.nm').boundingBox();
  const was = rows(folder).clash_take;
  const zoom = await page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
  const yamlBeforeMove = yaml(folder);
  // Somewhere clear: let go on another box, it goes to the nearest clear place instead (30fc7cbba).
  const { dx, dy } = await page.evaluate((name) => {
    const rects = [...document.querySelectorAll('.react-flow__node')].map((node) => ({ name: node.querySelector('.nm')?.textContent, rect: node.getBoundingClientRect() }));
    const mine = rects.find((held) => held.name === name).rect;
    const others = rects.filter((held) => held.name !== name).map((held) => held.rect);
    const gap = 32;
    for (const y of [120, -120, 240, -240, 360, -360]) {
      for (const x of [180, -180, 360, -360, 540, -540]) {
        const [left, top, right, bottom] = [mine.left + x - gap, mine.top + y - gap, mine.right + x + gap, mine.bottom + y + gap];
        const onScreen = top > 60 && left > 230 && bottom < innerHeight - 20 && right < innerWidth - 20;
        if (onScreen && !others.some((other) => other.left < right && other.right > left && other.top < bottom && other.bottom > top)) return { dx: x, dy: y };
      }
    }
    throw new Error(`nowhere clear to move it: ${JSON.stringify({ mine, others })}`);
  }, 'clash_take');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 2, box.y + box.height / 2);
  await page.mouse.move(box.x + box.width / 2 + dx, box.y + box.height / 2 + dy, { steps: 5 });
  await page.mouse.up();
  const row = (await until('the moved row', () => rows(folder), (now) => now.clash_take && (now.clash_take.x !== was.x || now.clash_take.y !== was.y))).clash_take;
  check('move_in_canvas_file', Math.abs(row.x - was.x - dx / zoom) <= 3 && Math.abs(row.y - was.y - dy / zoom) <= 3, { was, row, zoom, dx, dy });
  check('move_touches_no_input', yaml(folder) === yamlBeforeMove);

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

  // The hash shows nowhere but the raw yaml pane.
  const hashes = Object.values(constants(folder)).map((constant) => constant.hash);
  const shown = await page.evaluate(() => document.body.innerText + [...document.querySelectorAll('[title],[aria-label]')].map((el) => el.title + el.ariaLabel).join(' '));
  check('hash_not_shown', hashes.every((hash) => !shown.includes(hash)));

  // A drawer is a route and loses nothing: same viewport, and the yaml there holds the hash.
  const viewportNow = () => page.evaluate(() => getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  const viewportShut = await viewportNow();
  await page.click('.main-menu summary');
  await page.click('.main-menu a:has-text("project.yaml")');
  await page.waitForSelector('.drawer pre.yaml');
  await until('yaml in the pane', () => page.textContent('pre.yaml'), (text) => text.includes('Hero_Shot_v2'));
  check('drawer_keeps_viewport', (await viewportNow()) === viewportShut);
  check('yaml_pane_is_the_file', (await page.textContent('pre.yaml')) === yaml(folder));
  await page.click('.drawer a[aria-label=Close]');

  // Reload restores everything: names, positions, viewport, thumbnails.
  const namesBefore = await names(page);
  // By name, each element with a file has its picture back, whatever was up before. The file's coast is an element with none.
  const withFiles = Object.keys(constants(folder)).sort();
  const picturesUp = () => page.evaluate(() => [...document.querySelectorAll('.react-flow__node')].filter((node) => [...node.querySelectorAll('.media img')].some((img) => img.naturalWidth > 0)).map((node) => node.querySelector('.nm')?.textContent).sort());
  check('elements_are_its_files_and_its_words', JSON.stringify(namesBefore) === JSON.stringify([...withFiles, 'coast'].sort()), { namesBefore, withFiles });
  // Its text was written with no version line, so that's the one thing said of the file, and the top bar says it.
  check('warning_is_the_missing_version', ((await page.getAttribute('.top .sum.ood', 'title')) ?? '').includes('version: 4'), await page.getAttribute('.top .sum.ood', 'title'));
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

  check('comments_and_text_kept', yaml(folder).startsWith(written), yaml(folder));

  // Giving way (Paul, 11-06), on boxes the CLI lays out past everything else, each let go by hand straight down onto its spot:
  // - on a crowded spot, where only a shrink fits near, it stays where it was let go, smaller, rather than going far;
  // - with a free spot beside it, it only moves, and the other stays;
  // - in a gap a little short of it, between boxes with room beyond, they're nudged a little apart and it stays as it was.
  if (process.env.COZY) {
    const pictures = mkdtempSync(join(tmpdir(), 'web-place-'));
    const at = (name, x, y, w) => execFileSync(process.env.COZY, ['project', '-p', folder, 'set', name, `at=${x},${y},${w}`], { stdio: 'ignore' });
    const lay = {};
    const left = Math.max(...Object.values(rows(folder)).map((row) => row.x)) + 1500;
    const put = (name, x, y, w) => { lay[name] = { x: left + x, y, w }; };
    // Rows of the grid touch: a picture 300 wide and the room it keeps under it for the name of a box below (e139a61c6, shown.ts).
    const ROW = 262;
    for (const [i, j] of [[0, 0], [1, 0], [2, 0], [0, 1], [2, 1], [0, 2], [1, 2], [2, 2]]) put(`grid_${i}_${j}`, 320 * i, ROW * j, 300);
    put('crowded', 290, 800, 360);
    put('beside', 0, 1200, 240);
    put('free', 200, 1500, 240);
    put('left', 0, 1800, 240);
    put('right', 468, 1800, 240);
    put('nudging', 234, 2100, 240);
    Object.keys(lay).forEach((name, i) => {
      const file = join(pictures, `${name}.png`);
      writeFileSync(file, png(40 + i * 9, 320, 180));
      execFileSync(process.env.COZY, ['project', '-p', folder, 'import', file, name], { stdio: 'ignore' });
    });
    // Each placed by the page as it arrives first: placed after its `at`, it would be put where the page found room (the page's read of where
    // things sit, then its write of a new one's place, can straddle another writer's).
    await until('each placed', () => rows(folder), (now) => Object.keys(lay).every((name) => now[name]));
    for (const [name, { x, y, w }] of Object.entries(lay)) at(name, x, y, w);
    rmSync(pictures, { recursive: true, force: true });
    await until('laid out', () => rows(folder), (now) => Object.entries(lay).every(([name, row]) => now[name]?.x === row.x && now[name]?.y === row.y));
    await page.mouse.click(1450, 860);
    await page.keyboard.press('f');
    await page.waitForTimeout(800);
    // A picture box's height at a width: its name, and the picture at 16:9.
    const tall = (w) => 32 + Math.round((w * 9) / 16);
    const middle = (row) => ({ x: row.x + row.w / 2, y: row.y + tall(row.w) / 2 });
    const apart = (a, b) => Math.round(Math.hypot(middle(a).x - middle(b).x, middle(a).y - middle(b).y));
    const lying = () => page.evaluate(() => {
      const cards = [...document.querySelectorAll('.react-flow__node .card')].map((card) => ({ name: card.dataset.asset, rect: card.getBoundingClientRect() }));
      const on = (a, b) => a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
      return cards.flatMap((a, i) => cards.slice(i + 1).filter((b) => on(a.rect, b.rect)).map((b) => `${a.name} on ${b.name}`));
    });
    /** Let go `dy` below or above where it stands, held by its picture's middle: where it was let go is where it stood as the button came up. */
    const letGo = async (name, dy) => {
      const before = rows(folder);
      const zoom = await page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
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
      // It lands, then gives way: two rows, the second written at once.
      await page.waitForTimeout(600);
      const after = await until(`${name} settled`, async () => {
        const one = JSON.stringify(rows(folder));
        await page.waitForTimeout(400);
        return one === JSON.stringify(rows(folder)) ? rows(folder) : null;
      }, Boolean);
      const moved = Object.fromEntries(Object.keys(lay).filter((other) => other !== name && apart(after[other], before[other]) > 0).map((other) => [other, apart(after[other], before[other])]));
      return { dropped: { ...before[name], x: Math.round(held.e), y: Math.round(held.f) }, now: after[name], moved, lying: await lying() };
    };
    const crowded = await letGo('crowded', ROW + 3 - 800);
    seen.crowded = { ...crowded, by: apart(crowded.now, crowded.dropped) };
    check('a_crowded_drop_shrinks_where_it_was_let_go', crowded.now.w < 360 * 0.97 && crowded.now.w >= 180 && seen.crowded.by <= 40, seen.crowded);
    check('a_crowded_drop_nudges_little', Object.values(crowded.moved).every((by) => by <= 40), crowded.moved);
    check('a_crowded_drop_lies_on_nothing', crowded.lying.length === 0, crowded.lying);
    const free = await letGo('free', -300);
    seen.free = { ...free, by: apart(free.now, free.dropped) };
    check('a_drop_with_room_beside_only_moves', free.now.w === 240 && seen.free.by > 0 && seen.free.by <= 60 && Object.keys(free.moved).length === 0, seen.free);
    check('a_drop_with_room_lies_on_nothing', free.lying.length === 0, free.lying);
    const nudging = await letGo('nudging', -300);
    seen.nudging = { ...nudging, by: apart(nudging.now, nudging.dropped) };
    const outward = rows(folder);
    check('small_nudges_absorb_a_drop', nudging.now.w === 240 && seen.nudging.by <= 3 && outward.left.x < lay.left.x && outward.right.x > lay.right.x && Object.values(nudging.moved).every((by) => by <= 40), seen.nudging);
    check('a_nudged_drop_lies_on_nothing', nudging.lying.length === 0, nudging.lying);
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
    execFileSync(process.env.COZY, ['project', '-p', folder, 'import', file, 'raced'], { stdio: 'ignore' });
    await until('the page reads where things sit', () => Promise.resolve(read), Boolean);
    // Clear of everything, so nothing moves it off another.
    const cli = { x: Math.max(...Object.values(rows(folder)).map((row) => row.x + (row.w ?? 240))) + 2000, y: 0, w: 240 };
    execFileSync(process.env.COZY, ['project', '-p', folder, 'set', 'raced', `at=${cli.x},${cli.y},${cli.w}`], { stdio: 'ignore' });
    release();
    // The page's place goes now, after the CLI's: what's written is still the CLI's, once and for good.
    const settled = await until('its row settled', async () => {
      const one = JSON.stringify(rows(folder).raced);
      await page.waitForTimeout(1500);
      return one === JSON.stringify(rows(folder).raced) ? rows(folder).raced : null;
    }, Boolean);
    await page.unroute(canvasRead);
    check('a_placement_never_overwrites_an_at', settled.x === cli.x && settled.y === cli.y && settled.w === cli.w, { settled, cli });
    await page.keyboard.press('f');
    await page.waitForTimeout(600);
    const shown = await page.evaluate(() => {
      const node = [...document.querySelectorAll('.react-flow__node')].find((one) => one.querySelector('.card[data-asset="raced"]'));
      const m = node && new DOMMatrix(getComputedStyle(node).transform);
      return m ? { x: Math.round(m.e), y: Math.round(m.f) } : null;
    });
    check('the_page_shows_the_cli_place', shown?.x === cli.x && shown?.y === cli.y, { shown, cli });
    rmSync(pictures, { recursive: true, force: true });
  }

  // The terminal sees the same project.
  if (process.env.COZY) {
    const status = JSON.parse(execFileSync(process.env.COZY, ['--json', '--full', 'project', '-p', folder, 'status'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }));
    const held = status.assets.map((asset) => asset.name);
    check('cli_sees_it', ['kyogre_art_1', 'kyogre_art_2', 'clash_take', 'ocean', 'Hero_Shot_v2'].every((name) => held.includes(name)), held);
  }

  // Everything above hashed in WebAssembly under the page's own CSP, with no violation; `eval` stays refused beside it.
  const violations = await page.evaluate(() => window.violations);
  check('csp_holds', violations.length === 0, { violations, styles: await page.evaluate(() => [...document.querySelectorAll('style')].map((held) => held.textContent.slice(0, 80))) });
  // In a timer, as the page's own code runs: an evaluation itself is let past the CSP.
  check('eval_refused', await page.evaluate(() => new Promise((done) => setTimeout(() => { try { new Function('return 1')(); done(false); } catch { done(true); } }))));
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
