// Place, in a real browser (web-ui.md §1): a PNG, an MP4 and a WAV dropped together; one chosen from the selector; one renamed; a
// move read back from project.canvas.yaml; a reload that restores everything. Also: Esc cancels, a file that isn't media gets one line
// and the others land, the same file twice is two elements and one file, the hash shows nowhere but the yaml pane, the page's CSP holds.
//   node place.js <sign-in link from `cozy web --no-open`>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser (Google Chrome, which plays H.264); $COZY: the CLI, to check the
// terminal sees the same project; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { readFileSync, readdirSync, writeFileSync } = require('fs');
const { join } = require('path');
const { STEP_MS, drop, png, until, wav } = require('./page');

const MP4 = Buffer.from('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', 'base64');

const failures = [];
const seen = {};
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

const names = (page) => page.evaluate(() => [...document.querySelectorAll('.react-flow__node .nm, .react-flow__node .nm-field')]
  .map((el) => el.value ?? el.textContent).sort());
const element = (page, name) => page.locator('.react-flow__node').filter({ has: page.locator(`.nm:text-is("${name}")`) });
const yaml = (folder) => readFileSync(join(folder, 'project.yaml'), 'utf8');
function rows(folder) {
  let text;
  try { text = readFileSync(join(folder, 'project.canvas.yaml'), 'utf8'); } catch { return {}; }
  const found = {};
  for (const line of text.split('\n')) {
    const m = line.match(/^\s+(\S+):\s*\{x:\s*(-?[\d.]+),\s*y:\s*(-?[\d.]+)/);
    if (m) found[m[1]] = { x: Number(m[2]), y: Number(m[3]) };
  }
  return found;
}
const constants = (folder) => {
  const found = {};
  for (const line of yaml(folder).split('\n')) {
    const m = line.match(/^\s+(\S+):\s*\{(image|video|audio):\s*(\w+)\}/);
    if (m) found[m[1]] = { field: m[2], hash: m[3] };
  }
  return found;
};

(async () => {
  const link = process.argv[2];
  const base = new URL(link).origin;
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1500, height: 900 } });
  const page = await context.newPage();
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d/.test(m.text())) errors.push(`console: ${m.text()}`); });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });

  // Sign in: the link's code becomes a session, and leaves the URL.
  await page.goto(link);
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
  const written = '# Mine, and kept.\nentities:\n  coast:\n    kind: environment\n    # Kept too.\n    description: |\n      A volcanic coast,\n      under a bright sky.\n';
  writeFileSync(join(folder, 'project.yaml'), written);

  // A PNG, an MP4 and a WAV dropped together: each there at once under its stem, nothing focused.
  const pngBytes = png(60);
  await drop(page, [
    { name: 'Kyogre Art (1).png', type: 'image/png', bytes: pngBytes },
    { name: 'Clash Take.mp4', type: 'video/mp4', bytes: MP4 },
    { name: 'Sea.Sound.wav', type: 'audio/wav', bytes: wav() },
  ], 500, 300);
  const atOnce = await names(page);
  check('three_appear_at_once', atOnce.length === 3, atOnce);
  check('none_focused', await page.evaluate(() => document.activeElement?.tagName !== 'INPUT'));
  check('stems_are_names', JSON.stringify(atOnce) === JSON.stringify(['clash_take', 'kyogre_art_1', 'sea_sound']), atOnce);
  await until('three constants in project.yaml', () => Object.keys(constants(folder)).length, (n) => n === 3);
  const three = constants(folder);
  check('constants_by_kind', three.kyogre_art_1?.field === 'image' && three.clash_take?.field === 'video' && three.sea_sound?.field === 'audio', three);
  check('rows_written', ['kyogre_art_1', 'clash_take', 'sea_sound'].every((name) => rows(folder)[name]), rows(folder));
  // Calcifer may fetch its ffmpeg for the first of these.
  const thumbs = await until('thumbnails up', () => page.evaluate(() => ({
    up: [...document.querySelectorAll('.react-flow__node .media img')].filter((img) => img.naturalWidth > 0 && !img.src.startsWith('blob:')).length,
    none: [...document.querySelectorAll('.media-note')].map((note) => note.textContent),
  })), (now) => now.up === 3 || now.none.includes('No preview'), 5 * STEP_MS);
  check('thumbnails', thumbs.up === 3, thumbs);
  await page.screenshot({ path: process.env.SHOT ? `${process.env.SHOT}-dropped.png` : undefined });

  // One chosen from the selector, in the top bar's button: its field is focused with the stem selected; a name shows as it will be.
  const [chooser] = await Promise.all([page.waitForEvent('filechooser'), page.click('.top button:has-text("Add file")')]);
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
  await element(page, 'sea_sound').locator('.nm').dblclick();
  await page.waitForSelector('.nm-field');
  await page.keyboard.type('ocean');
  await page.keyboard.press('Enter');
  await until('renamed in project.yaml', () => constants(folder), (all) => all.ocean && !all.sea_sound);
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
  await until('the copy landed', () => constants(folder).kyogre_art_1_2, Boolean);
  const lines = await until('one line for the text file', () => page.locator('.note .line').allTextContents(), (all) => all.some((line) => line.includes('notes.txt')));
  check('one_line_for_non_media', lines.filter((line) => line.includes('notes.txt')).length === 1, lines);
  check('non_media_not_on_canvas', !(await names(page)).includes('notes'), await names(page));
  check('same_file_one_file', readdirSync(join(folder, 'files')).length === filesBefore);
  const all = constants(folder);
  check('same_file_same_hash', all.kyogre_art_1.hash === all.kyogre_art_1_2.hash, all);
  check('same_file_two_elements', (await names(page)).includes('kyogre_art_1_2'), await names(page));

  // Dragged: the row in project.canvas.yaml moves by what the mouse did, and no input changes.
  const box = await element(page, 'clash_take').locator('.nm').boundingBox();
  const was = rows(folder).clash_take;
  const zoom = await page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
  const yamlBeforeMove = yaml(folder);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 2, box.y + box.height / 2);
  await page.mouse.move(box.x + box.width / 2 + 180, box.y + box.height / 2 + 120, { steps: 5 });
  await page.mouse.up();
  const row = (await until('the moved row', () => rows(folder), (now) => now.clash_take && (now.clash_take.x !== was.x || now.clash_take.y !== was.y))).clash_take;
  check('move_in_canvas_file', Math.abs(row.x - was.x - 180 / zoom) <= 3 && Math.abs(row.y - was.y - 120 / zoom) <= 3, { was, row, zoom });
  check('move_touches_no_input', yaml(folder) === yamlBeforeMove);

  // The hash shows nowhere but the raw yaml pane.
  const hashes = Object.values(constants(folder)).map((constant) => constant.hash);
  const shown = await page.evaluate(() => document.body.innerText + [...document.querySelectorAll('[title],[aria-label]')].map((el) => el.title + el.ariaLabel).join(' '));
  check('hash_not_shown', hashes.every((hash) => !shown.includes(hash)));

  // A drawer is a route and loses nothing: same viewport, the dock re-centred beside it, and the yaml there holds the hash.
  const viewportNow = () => page.evaluate(() => getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  const dockCentre = () => page.evaluate(() => { const b = document.querySelector('.dock').getBoundingClientRect(); return b.left + b.width / 2; });
  const [viewportShut, dockShut] = [await viewportNow(), await dockCentre()];
  await page.click('.top a:has-text("project.yaml")');
  await page.waitForSelector('.drawer pre.yaml');
  await until('yaml in the pane', () => page.textContent('pre.yaml'), (text) => text.includes('Hero_Shot_v2'));
  check('drawer_keeps_viewport', (await viewportNow()) === viewportShut);
  check('dock_recentres', (await dockCentre()) < dockShut - 100, { dockShut, open: await dockCentre() });
  check('yaml_pane_is_the_file', (await page.textContent('pre.yaml')) === yaml(folder));
  await page.click('.drawer a[aria-label=Close]');

  // Reload restores everything: names, positions, viewport, thumbnails.
  const namesBefore = await names(page);
  const places = () => page.evaluate(() => Object.fromEntries([...document.querySelectorAll('.react-flow__node')].map((node) => [node.querySelector('.nm')?.textContent, getComputedStyle(node).transform])));
  const placesBefore = await places();
  await new Promise((done) => setTimeout(done, 300));
  const viewportBefore = await viewportNow();
  await page.reload();
  await until('names back', () => names(page), (now) => JSON.stringify(now) === JSON.stringify(namesBefore));
  await until('thumbnails back', () => page.evaluate(() => [...document.querySelectorAll('.react-flow__node .media img')].filter((img) => img.naturalWidth > 0).length), (n) => n === namesBefore.length);
  check('reload_positions', JSON.stringify(await places()) === JSON.stringify(placesBefore), { before: placesBefore, after: await places() });
  check('reload_viewport', (await viewportNow()) === viewportBefore, { before: viewportBefore, after: await viewportNow() });
  await page.screenshot({ path: process.env.SHOT ? `${process.env.SHOT}-reloaded.png` : undefined });

  check('comments_and_text_kept', yaml(folder).startsWith(written), yaml(folder));

  // The terminal sees the same project.
  if (process.env.COZY) {
    const status = JSON.parse(execFileSync(process.env.COZY, ['--json', 'project', '-p', folder, 'status'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }));
    const held = status.assets.map((asset) => asset.name);
    check('cli_sees_it', ['kyogre_art_1', 'kyogre_art_1_2', 'clash_take', 'ocean', 'Hero_Shot_v2'].every((name) => held.includes(name)), held);
  }

  // Everything above hashed in WebAssembly under the page's own CSP, with no violation; `eval` stays refused beside it.
  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
  // In a timer, as the page's own code runs: an evaluation itself is let past the CSP.
  check('eval_refused', await page.evaluate(() => new Promise((done) => setTimeout(() => { try { new Function('return 1')(); done(false); } catch { done(true); } }))));
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch((error) => {
  console.error(error);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
