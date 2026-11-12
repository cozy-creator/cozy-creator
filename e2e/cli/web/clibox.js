// What the page renders, `cozy box` renders, through the page's own routes, and the page then shows it as its own (cl-956). With no page
// open: a clip and a picture made boxes as a dropped file is, shots.py's Shot of the clip rendered (no model; its run says its seam, frame
// 6, as H3 does), Redo of its segment 2, another box continuing its take from the end with the picture in its prompt, and shaped.py's shot
// taken up by `up` from that take's latents, as H3 Upscale is. Then the page: each box, each take, and what each take went on from, said as
// for a take the page made (web/scrub.js).
//   node clibox.js: on the Calcifer whose lock record is $HOLDER; $COZY: the cozy under test; $ARGV_LOG: each cozy call, for coverage;
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { appendFileSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } = require('fs');
const { homedir, tmpdir } = require('os');
const { join } = require('path');
const { boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, projectFromPage, projectUrl, signIn, STEP_MS, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

/** Seconds as the page says a cut, "0:04.00". */
const clock = (seconds) => `${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(2).padStart(5, '0')}`;
// The first render here installs its tool's environment.
const RUN_MS = 10 * 60_000;
const SHOTS = join(__dirname, '..', 'project', 'shots.py');
const SHAPED = join(__dirname, '..', 'project', 'shaped.py');
const SEAM = 6;
const folder = join(homedir(), `web-clibox-${Date.now()}`);
const scratch = mkdtempSync(join(tmpdir(), 'web-clibox-'));
seen.project = { folder };

/** One call of the cozy under test, from the project unless `cwd` says, logged for coverage: what it printed. Synchronous, so a first
 *  render's install is waited out whole (noStall counts what's seen between calls). */
function cozyAt(cwd, ...words) {
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  return execFileSync(process.env.COZY, words, { cwd, encoding: 'utf8', maxBuffer: 1 << 26, timeout: RUN_MS, stdio: ['ignore', 'pipe', 'pipe'] });
}
const cozy = (...words) => cozyAt(folder, ...words);
const json = (...words) => JSON.parse(cozy('--json', ...words));
/** A `cozy --json box render`'s lines, and its takes. */
function render(...words) {
  const lines = cozy('--json', 'box', 'render', ...words, '--on', 'local').trim().split('\n').map((line) => JSON.parse(line));
  const ended = lines.find((line) => line.type === 'ended');
  return { asked: lines.find((line) => line.type === 'asked'), ended: ended?.ended, takes: ended?.takes ?? [] };
}
/** A take's warnings that say its pickup moved: off a seam, or off its tool's grid. */
const moved = (take) => (take?.warnings ?? []).filter((said) => said.includes('keeps up to frame') || said.includes("isn't one a pickup lands on"));
/** A refused call's error code. */
function refused(...words) {
  try {
    cozy('--json', ...words);
    return null;
  } catch (error) {
    try { return JSON.parse(error.stdout).error?.code; } catch { return String(error.stdout ?? error.message).slice(0, 300); }
  }
}

(async () => {
  // A 4 s clip and a picture, each a box as a file dropped on the canvas makes one.
  const clip = join(scratch, 'look.mp4');
  const picture = join(scratch, 'mara.png');
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=4', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', clip]);
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'color=c=red:size=64x64', '-frames:v', '1', picture]);
  cozyAt(homedir(), 'new', folder, '--unencrypted');
  const look = json('box', 'add', 'look', clip);
  const mara = json('box', 'add', 'mara', picture);
  check('files_are_boxes', look.name === 'look' && look.field === 'video' && mara.name === 'mara' && mara.field === 'image', { look, mara });
  check('a_taken_name_is_refused', refused('box', 'add', 'look', '--tool', `${SHOTS}/shoot`) !== null);

  // shots.py's Shot of the clip, rendered: its take says its seam.
  cozy('box', 'add', 'shot', '--tool', `${SHOTS}/shoot`, 'summary=On <look>.');
  const first = render('shot');
  const one = first.takes.find((take) => take.name === 'shot');
  seen.first = first;
  check('a_box_renders', one?.number === 1 && /\.mp4$/.test(one?.path ?? '') && JSON.stringify(one?.seams) === `[${SEAM}]` && !one?.pickup, first);

  // Redo segment 2 of take 1, as Focus's Redo does: segment 1 kept, so it picks up at the seam, the box's beats from there.
  check('segment_1_is_a_new_take', refused('box', 'render', 'shot', '--redo', '1', '--on', 'local') === 'redo_first_segment');
  check('take_1_has_no_segment_3', refused('box', 'render', 'shot', '--redo', '3', '--take', '1', '--on', 'local') === 'redo_no_segment');
  const redo = render('shot', '--redo', '2', '--take', '1');
  const two = redo.takes.find((take) => take.name === 'shot');
  seen.redo = redo;
  check('redo_keeps_the_segments_before_it', two?.number === 2 && two.pickup?.take === '<shot.1>' && two.pickup?.out_frame === SEAM
    && Math.abs(two.pickup?.beats_from - SEAM / 24) < 1e-9 && two.seams?.[0] === SEAM && !moved(two).length, two);

  // Continue from: another box goes on from the end of shot's take 1, its words from their start, the picture in its prompt.
  cozy('box', 'add', 'next', '--tool', `${SHOTS}/shoot`, 'summary=Then <look> again, with <mara>.');
  const continued = render('next', '--continue', 'shot.1');
  const after = continued.takes.find((take) => take.name === 'next');
  seen.continued = continued;
  const downloads = join(scratch, 'next');
  cozy('--json', 'run', 'watch', String(after?.run), '--once', '-o', downloads);
  const given = JSON.parse(readFileSync(join(downloads, readdirSync(downloads).find((file) => file.includes('-given'))), 'utf8'));
  seen.given = given;
  check('it_continues_from_the_end', after?.pickup?.take === '<shot.1>' && after.pickup.beats_from === 0 && after.pickup.out_frame > SEAM
    && given.pickup?.out_frame === after.pickup.out_frame && !moved(after).length, { after, given: given.pickup });
  check('a_picture_is_a_reference', given.summary?.references?.mara === 'image' && given.summary?.references?.look === 'video', given.summary);

  // Its words set as its controls set them, from the command line and from a file.
  cozy('box', 'set', 'next', 'summary=Then <look> once more, with <mara>.', 'seed=7');
  writeFileSync(join(scratch, 'words.json'), JSON.stringify({ summary: 'Then <look>, with <mara>, at last.' }));
  cozy('box', 'set', 'next', '--input', join(scratch, 'words.json'));
  const set = json('box', 'ls', 'next');
  check('set_writes_its_fields', set.fields?.summary === 'Then <look>, with <mara>, at last.' && set.fields?.seed === 7, set.fields);

  // An upscale from a take's latents, as H3 Upscale reads them: shaped.py's shot, two takes, and `up` of its first.
  writeFileSync(join(scratch, 'grey.yaml'), 'beats:\n  - description: A grey room.\n    duration: 1\n');
  cozy('box', 'add', 'grey', '--tool', `${SHAPED}/shot`, '--input', join(scratch, 'grey.yaml'));
  const greys = render('grey', '-n', '2').takes.filter((take) => take.name === 'grey');
  check('n_takes', greys.length === 2, greys);
  cozy('box', 'add', 'big', '--tool', `${SHAPED}/up`, 'video=<grey.1>');
  const big = render('big').takes.find((take) => take.name === 'big');
  const asked = json('--full', 'run', 'watch', String(big?.run), '--once');
  seen.upscale = { big, video: asked.request?.input?.video };
  check('an_upscale_reads_its_takes_latents', Boolean(asked.request?.input?.video?.latents && asked.request.input.video.file), seen.upscale);

  const boxes = Object.fromEntries(json('box', 'ls').boxes.map((one) => [one.name, one.versions]));
  seen.boxes = boxes;
  check('ls_lists_each_box', JSON.stringify(Object.keys(boxes).sort()) === JSON.stringify(['big', 'grey', 'look', 'mara', 'next', 'shot'])
    && boxes.shot === 2 && boxes.grey === 2 && boxes.next === 1 && boxes.big === 1, boxes);
  const takes = json('box', 'ls', 'shot').takes;
  check('ls_lists_its_takes', takes.length === 2 && takes[1].pickup?.out_frame === SEAM && takes[0].seams?.[0] === SEAM, takes);

  // The page: those boxes and takes, as it shows its own.
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|status of 5\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path) => page.evaluate(async ({ path, token }) => {
    const answer = await fetch(path, { headers: token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {} });
    return answer.json().catch(() => null);
  }, { path, token: held?.token });
  await page.goto(await projectUrl(folder));
  const id = (await projectFromPage(page)).id;
  for (const name of ['look', 'mara', 'shot', 'next', 'grey', 'big']) await boxOf(page, name).waitFor();
  await fitCanvas(page);
  seen.shown = true;

  // What the page would send for the same gestures: the seam, and the take's end by its length.
  const main = (await api(`/v1/projects/${id}`)).assets.find((asset) => asset.name === 'shot').prefixes.find((one) => one.number === 1).main;
  const fps = main.fps || 24;
  const last = Math.max(1, Math.round((main.duration_ms / 1000) * fps));
  check('it_continues_where_the_page_would', after?.pickup?.out_frame === last, { sent: after?.pickup, last, main });

  const focus = page.locator('.focus-viewport');
  const says = focus.locator('.tl-take .continues');
  const said = (prefix) => until(`"${prefix}" said`, () => says.innerText().catch(() => ''), (text) => text.startsWith(prefix)).catch(() => says.innerText().catch(() => ''));
  await openBox(page, 'shot');
  check('focus_shows_both_takes', (await focus.locator('.take[aria-label="take 1"]').count()) === 1 && (await focus.locator('.take[aria-label="take 2"]').count()) === 1);
  await focus.locator('.take[aria-label="take 2"]').first().click();
  seen.redone_said = await said('Redone');
  check('focus_shows_the_redo', seen.redone_said === `Redone from ${clock(SEAM / fps)}`, seen.redone_said);
  await closeBox(page);
  await openBox(page, 'next');
  await focus.locator('.take[aria-label="take 1"]').first().click().catch(() => {});
  seen.continued_said = await said('Continues');
  check('focus_shows_its_source', seen.continued_said === `Continues <shot.1> from ${clock(last / fps)}`, seen.continued_said);
  if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-continued.png` });
  await closeBox(page);
  seen.tile_said = await boxOf(page, 'next').locator('.continues-rest').innerText().catch(() => '');
  check('its_tile_shows_its_source', seen.tile_said === 'from shot', seen.tile_said);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
