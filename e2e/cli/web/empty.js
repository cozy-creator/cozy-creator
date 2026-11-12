// An empty box offers no New (cl-925, Paul 11-12: "when focused on a node, next to its name, do not have 'new' next to it if there are no
// images or media to display there"), in a real browser on a real Calcifer. Qwen's Image and H3's Shot, nothing made: in Focus, no New take
// and no New beside the name; once each has its first take, New is there. And a box's face never falls back to its stored description
// (cl-900): a blank Edit described by hand shows nothing of it. New stays while the first take is queued and running, and starts a second
// beside it (cl-941).
//   node empty.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, renderOf, until, upload } = require('./page');

/** A slow still, local code this computer runs (`project/tool.py`). */
const TOOL = join(__dirname, '..', 'project', 'tool.py');

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

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-empty-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).body?.assets?.find((row) => row.name === name);
  seen.project = { id, folder: (await api(at)).body.path };
  const folder = mkdtempSync(join(tmpdir(), 'web-empty-'));

  // Two empty boxes: Qwen's Image and H3's Shot, their words written, nothing made.
  const tools = [...((await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? []), ...((await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [])];
  const image = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  const video = tools.find((tool) => tool.id === 'minimax-h3/generate') ?? tools.find((tool) => tool.package === 'minimax-h3' && tool.makes === 'video');
  seen.tools = { image: image?.id, video: video?.id };
  if (!image || !video) throw new Error(`the Hub lists no Qwen Image or H3 Shot: ${JSON.stringify(tools.map((tool) => tool.id))}`);
  const boxes = [
    { name: 'still', tool: image.id, kind: 'image', fields: { instructions: 'a lighthouse at dusk' }, at: { x: 100, y: 100 }, take: () => { writeFileSync(join(folder, 'take.png'), png(70)); return join(folder, 'take.png'); } },
    { name: 'shot', tool: video.id, kind: 'video', fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 5 }] }, at: { x: 600, y: 100 }, take: () => { writeFileSync(join(folder, 'take.mp4'), MP4); return join(folder, 'take.mp4'); } },
  ];
  for (const box of boxes) {
    const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: box.tool, kind: box.kind, name: box.name, exact: true, fields: box.fields, at: box.at }) });
    if (made.status >= 300) throw new Error(`${box.name}: ${JSON.stringify(made)}`);
    await boxOf(page, box.name).waitFor();
  }
  await fitCanvas(page);

  /** What an open box offers as New: its next-take tile, and the label beside its name. */
  const offers = async (name) => {
    const here = page.locator(`.card.here[data-asset="${name}"]`).first();
    await here.waitFor();
    await page.waitForTimeout(400);
    const take = await page.locator(`.card.here[data-asset="${name}"] .take.add`).count();
    const label = await page.locator('.focus-viewport .focus-version-label').allInnerTexts();
    const said = await page.locator('.focus-viewport .focus-heading').innerText().catch(() => '');
    return { take, label, said };
  };
  for (const box of boxes) {
    await openBox(page, box.name);
    const empty = await offers(box.name);
    seen[`${box.name}_empty`] = empty;
    check(`${box.name}_empty_offers_no_new`, empty.take === 0 && !empty.label.includes('New') && !/\bNew\b/.test(empty.said), empty);
    await shot(`${box.name}-empty`);
    await closeBox(page);

    // Its first take: New is there.
    const added = await api(`${at}/assets/${box.name}/versions`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, box.take()) }) });
    if (added.status >= 300) throw new Error(`${box.name}'s first take: ${JSON.stringify(added)}`);
    await until(`${box.name}'s first take`, () => asset(box.name), (row) => row?.versions >= 1 || row?.prefixes?.length >= 1);
    await openBox(page, box.name);
    const one = await until(`${box.name}'s New`, () => offers(box.name), (now) => now.take === 1).catch(() => null);
    seen[`${box.name}_with_a_take`] = one;
    check(`${box.name}_with_a_take_offers_new`, one?.take === 1, one);
    await shot(`${box.name}-with-a-take`);
    await closeBox(page);
  }

  // New stays while the first take is on its way, queued and then running, and starts a second beside it (cl-941, Paul 11-12: "it should always
  // be there, if I have another image that is pending"). Where this computer runs models: a slow still, its first take rendered from Focus.
  if ((await api('/v1/status')).body?.local_usable) {
    const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: `${TOOL}/slow`, kind: 'image', name: 'pending', exact: true, fields: { instructions: 'the first', seconds: 90 }, at: { x: 1100, y: 100 } }) });
    if (made.status >= 300) throw new Error(`pending: ${JSON.stringify(made)}`);
    await boxOf(page, 'pending').waitFor();
    await openBox(page, 'pending');
    await (await renderOf(page, 'pending')).click();
    const runs = async () => ((await api(`${at}/runs?limit=100`)).body?.runs ?? []).filter((run) => run.project_id === id && run.kind === 'call');
    const phase = async () => ({ ...(await offers('pending')), runs: (await runs()).map((run) => run.status), takes: (await asset('pending'))?.versions ?? 0 });
    const queued = await until('the first take on its way', phase, (now) => now.runs.length === 1 && now.takes === 0, 60_000).catch(phase);
    seen.pending_queued = queued;
    check('new_while_the_first_take_is_on_its_way', queued.take === 1 && queued.runs.length === 1 && queued.takes === 0, queued);
    await shot('pending-queued');
    const running = await until('the first take running', phase, (now) => now.runs[0] === 'in_progress', 400_000).catch(phase);
    seen.pending_running = running;
    check('new_while_the_first_take_runs', running.take === 1 && running.runs[0] === 'in_progress' && running.takes === 0, running);
    await shot('pending-running');
    // New starts a second take beside it.
    await page.locator('.card.here[data-asset="pending"] .take.add').click();
    await page.locator('.card.here[data-asset="pending"] .in-frame .mention-text').waitFor();
    await page.locator('.card.here[data-asset="pending"] button[aria-label="Render"]').click();
    const two = await until('a second take beside it', phase, (now) => now.runs.length === 2, 60_000).catch(phase);
    seen.pending_two = two;
    check('new_starts_a_second_take_beside_it', two.runs.length === 2 && two.runs.some((status) => status === 'in_progress'), two);
    await shot('pending-second');
    for (const run of await runs()) if (!['completed', 'failed', 'canceled'].includes(run.status)) await api(`${at}/runs/${run.id}/cancel`, { method: 'POST', body: '{}' });
    await closeBox(page);
  } else check('this_computer_runs_the_slow_still', false, (await api('/v1/status')).body);

  // A blank Edit with a description written by hand shows nothing of it on its face (cl-900): only what it's for.
  const blank = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: 'cozy/edit', kind: 'video', name: 'cut', exact: true, fields: { description: 'HIDDEN-DESCRIPTION' }, at: { x: 100, y: 700 } }) });
  if (blank.status >= 300) throw new Error(`the Edit: ${JSON.stringify(blank)}`);
  await boxOf(page, 'cut').waitFor();
  await until('its description in the file', () => asset('cut'), (row) => row?.fields?.description === 'HIDDEN-DESCRIPTION');
  await page.waitForTimeout(500);
  const face = await boxOf(page, 'cut').innerText();
  seen.blank_face = face;
  check('a_blank_box_never_shows_its_description', !face.includes('HIDDEN-DESCRIPTION'), face);
  await shot('blank-face');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
