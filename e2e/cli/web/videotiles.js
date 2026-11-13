// Every video box at rest plays its preview under a resting pointer, whatever made the video (cl-964, Paul 11-12: "shouldn't all videos have
// a playable thumbnail preview? I feel like some don't and some do, for the video nodes"), in a real browser on a real Calcifer. Each kind:
// an imported file; an Edit's cut, rendered free; a shot's take from its tool (project/shots.py) and one picked up from it; an H3 box's
// uploaded take; a take with no preview made yet (its file older than previews at landing); and a box whose take was played in Focus,
// whose playhead stays the open box's. Each shows its poster, then its preview plays muted and looping; each is kept as a picture.
//   node videotiles.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER. $PLAYWRIGHT, $CHROME, $COZY, $SHOT as
//   nodes.js. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require('fs');
const { homedir, tmpdir } = require('os');
const { join } = require('path');
const { boxOf, calcifer, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, projectAction, projectFromPage, signIn, STEP_MS, until, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
const RUN_MS = 300_000;
const SHOTS = join(__dirname, '..', 'project', 'shots.py');

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of [45]\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-videotiles-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  const asset = async (name) => (await api(at)).body.assets.find((held) => held.name === name);

  // Four seconds of a test pattern each, with sound, as a person's files are.
  const clips = mkdtempSync(join(tmpdir(), 'web-videotiles-'));
  const clip = (name, pattern) => {
    const file = join(clips, `${name}.mp4`);
    execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', `${pattern}=size=320x180:rate=24:duration=4`, '-f', 'lavfi', '-i', 'sine=frequency=330:duration=4',
      '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', file]);
    return file;
  };
  await projectAction(folder, 'import', clip('imported', 'testsrc2'), 'imported');
  await projectAction(folder, 'import', clip('old', 'smptebars'), 'old');
  await projectAction(folder, 'add', 'cozy/edit', 'cut', 'tracks:=[{"clips":["<imported>"]}]');
  // An H3 box with an uploaded take, as scrub.js has one.
  const tools = (await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [];
  const h3 = tools.find((tool) => tool.id === 'minimax-h3/generate');
  seen.h3_tool = h3?.id ?? null;
  // With no H3 listed, the box would be a plain video box, and its case not H3's.
  check('the_hub_lists_h3', Boolean(h3), tools.map((tool) => tool.id));
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: h3?.id ?? '', kind: 'video', name: 'h3', exact: true, fields: { node_kind: 'video', summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 4 }] } }) });
  await api(`${at}/assets/h3/versions`, { method: 'POST', body: JSON.stringify({ source: await upload(token, clip('h3', 'testsrc')) }) });
  await until('the h3 take', async () => (await asset('h3'))?.versions, (n) => n >= 1);

  const names = ['imported', 'old', 'cut', 'h3'];
  if (local) {
    // A shot's take from its tool, and one picked up from that take: the tool is new code, so its first render installs it.
    const shots = join(folder, 'shots.py');
    writeFileSync(shots, `${readFileSync(SHOTS, 'utf8')}\n# ${id}\n`);
    await projectAction(folder, 'add', `${shots}/shoot`, 'shot', 'node_kind=video', 'plate=<imported>', 'summary=Along the coast.');
    await projectAction(folder, 'render', 'shot', '--on', 'local');
    const first = await until('the shot take', async () => asset('shot'), (held) => held?.versions >= 1, RUN_MS);
    // Its number, whatever it is: a take's number is reserved when Render is pressed (cl-945).
    const from = first.prefixes.at(-1).number;
    const picked = await api(`${at}/runs`, { method: 'POST', body: JSON.stringify({ names: ['shot'], on: ['local'], pickup: { take: `<shot.${from}>`, out_frame: 24 } }) });
    seen.pickup_asked = picked.status;
    const taken = await until('the picked-up take', async () => asset('shot'), (held) => held?.versions >= 2, RUN_MS);
    check('a_take_is_picked_up', Boolean(taken.prefixes?.at(-1)?.pickup), taken.prefixes?.at(-1));
    names.push('shot');
  }
  await projectAction(folder, 'render', 'cut', '--on', 'local');
  await until('the cut', async () => (await asset('cut'))?.versions, (n) => n >= 1, RUN_MS);
  // `old`: made before previews were made at landing, so none is kept yet.
  const oldHex = (await asset('old')).prefixes?.at(-1)?.main?.sha256?.replace(/^sha256:/, '');
  const thumbs = join(homedir(), '.cozy', 'cache', 'thumbs');
  // The preview made at its landing, taken away: with none there to take, `old` would be an ordinary case, not this one.
  const kept = join(thumbs, `${oldHex}-preview.mp4`);
  const hadOne = await until('old has its landing preview', () => Promise.resolve(existsSync(kept)), Boolean, 30_000).then(() => true, () => false);
  check('old_had_a_preview_to_take_away', hadOne, kept);
  if (hadOne) rmSync(kept);
  seen.old_preview_removed = oldHex;

  // Laid out apart, all in view.
  const places = Object.fromEntries(names.map((name, i) => [name, { x: 80 + (i % 3) * 460, y: 80 + Math.floor(i / 3) * 360 }]));
  await api(`${at}/canvas`, { method: 'PATCH', body: JSON.stringify({ cards: places }) });
  // The canvas draws only the boxes in view: fitted to them first (again, until the page has their new places), then each is there.
  await until('each box in view', async () => {
    await fitCanvas(page);
    return (await Promise.all(names.map((name) => boxOf(page, name).isVisible()))).every(Boolean);
  }, Boolean);

  // A box whose take was played in Focus: its playhead is the open box's (Pick up from here), not what its tile shows at rest.
  await openBox(page, 'h3');
  const focus = page.locator('.focus-viewport');
  // Its transport, once drawn: Play is pressed unless it's already playing.
  const transport = focus.locator('button[aria-label="Play"], button[aria-label="Pause"]').first();
  await transport.waitFor();
  if ((await transport.getAttribute('aria-label')) === 'Play') await transport.click();
  const player = focus.locator('video.live').first();
  const played = await until('it plays in Focus', () => player.evaluate((el) => el.currentTime).catch(() => 0), (t) => t > 0.5, 30_000).catch(() => 0);
  seen.played_in_focus = played;
  await closeBox(page);
  await fitCanvas(page);

  // Under a resting pointer, each plays: its poster first, then its preview, muted and looping.
  const rest = async (name) => {
    await page.mouse.move(4, 500);
    // The last box's preview stopped with the pointer gone, before this one's is looked for.
    await until('no preview playing', () => page.locator('video.preview.on').count(), (n) => n === 0, 5000).catch(() => {});
    const media = boxOf(page, name).locator('.media').first();
    const poster = await until(`${name}'s poster`, () => media.locator('.scrub img').evaluate((el) => el.complete && el.naturalWidth > 0).catch(() => false), Boolean, 60_000).catch(() => false);
    const { x, y, width, height } = await media.boundingBox();
    await page.mouse.move(x + width / 2 - 12, y + height / 2, { steps: 2 });
    await page.mouse.move(x + width / 2, y + height / 2, { steps: 4 });
    const previewing = await until(`${name}'s preview plays`, () => media.locator('video.preview').evaluate((el) => ({ on: el.classList.contains('on'), at: el.currentTime, muted: el.muted, loop: el.loop, width: el.videoWidth })).catch(() => null), (now) => now?.on && now.at > 0.3, 45_000).catch((error) => error.message);
    seen[name] = { poster, previewing };
    check(`${name}_shows_its_poster`, poster === true, poster);
    check(`${name}_plays_its_preview`, typeof previewing === 'object' && previewing.on && previewing.muted && previewing.loop, previewing);
    await shot(`${name}-previewing`);
  };
  for (const name of names) await rest(name);
  check('played_in_focus_still_previews', seen.h3_plays_its_preview === true && played > 0.5, { played });

  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  process.exit(2);
}
