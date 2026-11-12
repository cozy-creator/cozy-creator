// A video in Focus has a scrubber, and Pick up from here under it (cl-933, Paul 11-12: "when you are viewing a video in focused mode, you
// can display the pick up from here, probably put that underneath the thumbnail. Also give us a scrubber of some sort. Also if you click on
// an empty image / empty video node, with nothing in it, there is no point in displaying 'instructions' or anything"), in a real browser on a
// real Calcifer. H3's Shot with a four-second take: in Focus, scrubbed a frame at a time, the time says where, and Pick up from here then
// Render asks to keep up to that frame (on the tool's grid). An empty picture and an empty video with no tool, in Focus, ask nothing.
//   node scrub.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, until, upload } = require('./page');

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

/** Where a pickup lands on a tool's frame grid (ranges.ts pickupAt). */
const pickupAt = (frame, last, snap) => (!snap ? Math.min(frame, last) : frame < snap.first ? null : Math.min(last, Math.max(snap.first, snap.offset + Math.round((frame - snap.offset) / snap.every) * snap.every)));

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|status of 5\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-scrub-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  // H3's Shot with a four-second take of its own.
  const folder = mkdtempSync(join(tmpdir(), 'web-scrub-'));
  const clip = join(folder, 'take.mp4');
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=4', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', clip]);
  const tools = (await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [];
  const shotTool = tools.find((tool) => tool.id === 'minimax-h3/generate') ?? tools.find((tool) => tool.package === 'minimax-h3' && tool.pickup);
  seen.tool = shotTool && { id: shotTool.id, pickup: shotTool.pickup };
  if (!shotTool?.pickup) throw new Error(`the Hub lists no H3 tool that picks up: ${JSON.stringify(tools.map((tool) => [tool.id, tool.pickup]))}`);
  await post(`${at}/assets`, { tool: shotTool.id, kind: 'video', name: 'shot', exact: true, fields: { summary: 'a lighthouse at dusk', beats: [{ description: 'Waves.', duration: 4 }] }, at: { x: 100, y: 100 } });
  await post(`${at}/assets/shot/versions`, { source: await upload(held?.token, clip) });
  // An empty picture and an empty video with no tool.
  await post(`${at}/assets`, { tool: '', kind: 'image', name: 'blank_pic', exact: true, fields: { node_kind: 'image' }, at: { x: 600, y: 100 } });
  await post(`${at}/assets`, { tool: '', kind: 'video', name: 'blank_clip', exact: true, fields: { node_kind: 'video' }, at: { x: 1000, y: 100 } });
  for (const name of ['shot', 'blank_pic', 'blank_clip']) await boxOf(page, name).waitFor();
  await until('the take', async () => (await api(at)).body?.assets?.find((row) => row.name === 'shot')?.versions, (n) => n >= 1);
  await fitCanvas(page);

  // In Focus: scrubbed a frame at a time, the time says where.
  await openBox(page, 'shot');
  const focus = page.locator('.focus-viewport');
  const scrub = focus.locator('.video-scrub');
  const range = scrub.locator('input[type=range]');
  await range.waitFor();
  const last = Number(await range.getAttribute('max'));
  await range.focus();
  for (let step = 0; step < 50; step++) await page.keyboard.press('ArrowRight');
  await page.waitForTimeout(400);
  const frame = Number(await range.inputValue());
  const says = await scrub.locator('[aria-label="Where it is"]').innerText();
  const time = await focus.locator('video').first().evaluate((video) => video.currentTime).catch(() => null);
  seen.scrubbed = { frame, last, says, time };
  check('it_scrubs_a_frame_at_a_time', frame === 50 && last >= 90, seen.scrubbed);
  check('it_says_where_it_is', /\d/.test(says) && says.includes('/'), says);
  check('the_video_is_there', time === null || Math.abs(time - frame / 24) < 0.2, seen.scrubbed);
  await scrub.locator('button[aria-label="A frame on"]').click();
  check('a_frame_on', Number(await range.inputValue()) === 51, await range.inputValue());
  await shot('scrubbed');

  // Pick up from here, under it: Render asks to keep up to that frame, on the tool's grid.
  const pickUp = scrub.locator('button:text-is("Pick up from here")');
  check('pick_up_is_under_the_scrubber', (await pickUp.count()) === 1 && (await focus.locator('.media .video-pickup-action').count()) === 0);
  const wanted = pickupAt(51, last, shotTool.pickup);
  seen.wanted_out_frame = wanted;
  const asked = page.waitForRequest((request) => request.method() === 'POST' && /\/v1\/projects\/[^/]+\/runs$/.test(new URL(request.url()).pathname), { timeout: STEP_MS });
  await page.route('**/runs', (route) => (route.request().method() === 'POST' ? route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: { kind: 'conflict', code: 'e2e_held', message: 'held by the cell' } }) }) : route.continue()));
  await pickUp.click();
  await focus.locator('button[aria-label=Render]').first().click();
  // A render this computer would do poorly is asked first (localfit): here, anyway.
  const anyway = page.getByRole('button', { name: 'Run here anyway' });
  await Promise.race([asked, anyway.first().waitFor({ timeout: STEP_MS }).then(() => anyway.first().click())]).catch(() => {});
  const body = JSON.parse((await asked).postData() ?? '{}');
  const pickup = body.pickup ?? body.asks?.[0]?.pickup ?? JSON.stringify(body).match(/"pickup":(\{[^}]*\})/)?.[1];
  seen.asked = body;
  const out = typeof pickup === 'string' ? JSON.parse(pickup).out_frame : pickup?.out_frame;
  check('pick_up_keeps_up_to_the_scrubbed_frame', out === wanted && String(JSON.stringify(body)).includes('<shot.1>'), { out, wanted, body });
  await page.unroute('**/runs');
  await shot('picking-up');
  await closeBox(page);

  // An empty picture and an empty video with no tool ask nothing.
  for (const name of ['blank_pic', 'blank_clip']) {
    await openBox(page, name);
    await page.waitForTimeout(400);
    const asks = await page.locator('.focus-viewport .mention-text, .focus-viewport [data-field], .focus-viewport button[aria-label=Render]').count();
    seen[name] = { asks, text: (await focus.innerText()).slice(0, 300) };
    check(`${name}_with_no_tool_asks_nothing`, asks === 0, seen[name]);
    await shot(name);
    await closeBox(page);
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
