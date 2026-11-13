// The palette offers every tool the Hub lists (cl-896, Paul 11-12: "Where did all of the other image models go?"), in a real browser on a
// real Calcifer: each package `cozy package ls` names with a release has its tools under what they make, the image models under Image and
// the video upscalers under Upscale Video, and nothing of a release locked to an older Runtime; one dragged out is a box of it, and one
// dropped on a video box goes on it; a tool taken off is off, and back when its package is added; with no Hub the palette says it holds
// only what's installed.
//   node palette.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER. $PLAYWRIGHT: playwright-core's directory;
// $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $RESTART, $NO_HUB: shell commands that bring this Calcifer
// back, the second on no Hub (web.sh's web_restart); $CATALOG: the Hub holds every first-party package. Prints what it saw as JSON and exits
// 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { exec, execFileSync } = require('child_process');
const { readFileSync } = require('fs');
const { projectFolder, projectFromPage, calcifer, signIn, STEP_MS, boxOf, carry, fitCanvas, implementationRow, lastLook, launch, lookAt, noStall, openCanvas, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
// What this cell took off this computer's tool list, put back if it dies first: the cells after it read the same list.
let putBack;
async function died(why) {
  failures.push(`${during}: ${why}`);
  if (putBack) await putBack().catch(() => {});
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}
const shell = (command) => new Promise((done, fail) => exec(command, { shell: 'bash' }, (error) => (error ? fail(error) : done())));

// What each first-party package offers, by the operation the palette puts it under: the packages repo's `@app.tool`s.
const OFFERS = {
  image: [...['anima', 'fibo-2', 'flux-2', 'flux.3-klein', 'hidream-o1', 'hunyuan-image-3', 'ideogram-4', 'krea-2', 'moss-image', 'qwen-image', 'qwen-image-2', 'sdxl'].map((p) => `${p}/image`), 'p-h3-image/generate'],
  edit_image: ['birefnet/remove_background'],
  upscale_image: ['seedvr2/upscale_image'],
  video: ['minimax-h3/generate', 'orbit/orbit'],
  edit_video: ['minimax-h3/edit'],
  upscale_video: ['minimax-h3/upscale', 'relayvsr/upscale', 'seedvr2/upscale'],
};
const OPERATIONS = ['image', 'edit_image', 'upscale_image', 'video', 'edit_video', 'upscale_video'];

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
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-palette-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const state = async () => (await api(at)).body;
  const asset = async (name) => (await state())?.assets?.find((row) => row.name === name);
  seen.project = { id, folder: (await state()).path };
  const box = (name) => boxOf(page, name);
  // Nothing open: a box opened in Focus is there twice, on the canvas and in Focus.
  const leave = async () => {
    if (!(await page.locator('.card.here').count())) return;
    await page.keyboard.press('Escape');
    await until('nothing open', () => page.locator('.card.here').count(), (n) => n === 0);
  };

  // The Hub's own word on what it holds, read by the CLI: each package with a release.
  const ls = JSON.parse(execFileSync(process.env.COZY, ['--json', 'package', 'ls', '--limit', '200'], { encoding: 'utf8' }));
  const released = ls.packages.filter((row) => row.latest_release).map((row) => row.package);
  seen.hub_packages = released;
  const wanted = Object.fromEntries(Object.entries(OFFERS).map(([operation, ids]) => [operation, ids.filter((tool) => released.includes(tool.split('/')[0]))]));
  seen.wanted = wanted;
  // On a Hub holding every first-party package ($CATALOG: e2e/isolated.sh's web-palette), every image model and the three video upscalers;
  // on the other shards' (qwen-image-2, minimax-h3, sdxl, birefnet), two image models and H3's upscaler.
  const whole = !process.env.CATALOG || (wanted.image.length === OFFERS.image.length && wanted.upscale_video.length === OFFERS.upscale_video.length);
  check('the_hub_holds_image_and_video_tools', whole && wanted.image.length >= 2 && wanted.upscale_video.length >= 1, wanted);

  // Calcifer's list at rest: every one of them, with no search and nothing installed, once the Hub's list is read.
  const listed = await until('the Hub read', async () => (await api(`/v1/tools?project=${id}`)).body, (body) => body && !body.note && Object.values(wanted).flat().every((tool) => body.tools.some((row) => row.id === tool)), 2 * STEP_MS).catch(() => null);
  const ids = (await api(`/v1/tools?project=${id}`)).body;
  seen.listed = { note: ids?.note, tools: ids?.tools?.map((tool) => `${tool.id}:${tool.makes}`) };
  check('every_hub_tool_listed_at_rest', Boolean(listed), seen.listed);
  check('no_tool_of_a_package_without_tools', !ids.tools.some((tool) => ['qwen3.5', 'qwen3.6'].includes(tool.package)), seen.listed);

  // The palette, as the page reads that list: each operation's models, as a person opens them.
  await page.reload();
  await until('the palette read', () => page.locator('.tools [data-operation="image"]').getAttribute('aria-disabled').catch(() => null), (disabled) => disabled === 'false');
  const palette = {};
  for (const operation of OPERATIONS) {
    const category = page.locator(`.tools [data-operation="${operation}"]`);
    if (await category.getAttribute('aria-disabled') === 'true') { palette[operation] = []; continue; }
    if (await category.getAttribute('aria-expanded') !== 'true') await category.click();
    palette[operation] = await page.locator(`.tools [data-implementation][data-tool-operation="${operation}"]`).evaluateAll((rows) => rows.map((row) => row.dataset.tool));
  }
  seen.palette = palette;
  // An image model that takes pictures is under Edit Image too, and one that needs one only there: under what it makes, either is its place.
  const under = (operation) => (operation === 'image' || operation === 'edit_image' ? [...palette.image, ...palette.edit_image] : palette[operation]);
  for (const [operation, tools] of Object.entries(wanted)) {
    const missing = tools.filter((tool) => !under(operation).includes(tool));
    check(`palette_${operation}_lists_every_hub_tool`, missing.length === 0, { missing, shown: palette[operation] });
  }
  // A release locked to the Runtime before this one is on the Hub, and offered nowhere (run 46790: flux-3-klein@0.7.0, Runtime 0.44).
  if (process.env.CATALOG) {
    const OLD = 'e2e-runtime-044';
    check('an_older_runtime_release_is_on_the_hub', released.includes(OLD), released);
    check('an_older_runtime_release_is_not_offered', !ids.tools.some((tool) => tool.package === OLD) && !Object.values(palette).flat().some((tool) => tool.startsWith(`${OLD}/`)), ids.tools.filter((tool) => tool.package === OLD).map((tool) => tool.id));
  }
  const media = ids.tools.filter((tool) => ['image', 'video'].includes(tool.makes) && tool.id !== 'cozy/edit');
  const unshown = media.filter((tool) => !Object.values(palette).flat().includes(tool.id)).map((tool) => tool.id);
  check('every_listed_media_tool_in_the_palette', unshown.length === 0, unshown);
  await shot('upscale-video');

  // A Hub image model dragged out is a box of it, read as a run of it would be; nothing is installed to list it.
  const image = wanted.image.find((tool) => tool.startsWith('sdxl/')) ?? wanted.image[0];
  const names = async () => (await state()).assets.map((row) => row.name);
  const before = await names();
  const imageRow = await implementationRow(page, 'image', image);
  await imageRow.scrollIntoViewIfNeeded();
  await carry(page, imageRow, await openCanvas(page));
  const made = await until('a box of it', async () => (await state()).assets.find((row) => !before.includes(row.name)), Boolean).catch(() => null);
  seen.image_box = made && { name: made.name, tool: made.tool, kind: made.kind };
  check('a_hub_image_tool_makes_its_box', made?.tool === image && made.kind === 'image', seen.image_box);
  await leave();

  // A video upscaler dropped on a video box goes on it.
  await carry(page, page.locator('.tools [data-palette="nodes"] li.tool[data-node="video"]'), await openCanvas(page));
  const video = await until('a video box', async () => (await state()).assets.find((row) => !before.includes(row.name) && row.kind === 'video'), Boolean);
  await leave();
  const upscaler = wanted.upscale_video.find((tool) => !tool.startsWith('minimax-h3/')) ?? wanted.upscale_video[0];
  await fitCanvas(page);
  await box(video.name).waitFor();
  const row = await implementationRow(page, 'upscale_video', upscaler);
  await row.scrollIntoViewIfNeeded();
  const target = await box(video.name).locator('.card').boundingBox();
  await carry(page, row, { x: target.x + target.width / 2, y: target.y + target.height / 2 });
  const upscaled = await until('the upscaler on it', () => asset(video.name), (now) => now?.tool === upscaler).catch(() => null);
  seen.video_box = upscaled && { name: upscaled.name, tool: upscaled.tool, kind: upscaled.kind };
  check('a_hub_video_upscaler_goes_on_a_video_box', upscaled?.tool === upscaler && upscaled.kind === 'video', { upscaler, asset: await asset(video.name) });
  await leave();
  await shot('boxes');

  // Taken off, it's off the list at rest and the palette; its package added, it's back.
  putBack = () => fetch(`${calcifer()}/v1/tools/list`, { method: 'POST', headers: { Authorization: `Bearer ${JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).token}`, 'Content-Type': 'application/json' }, body: JSON.stringify({ add: image.split('/')[0] }) });
  const off = await api('/v1/tools/list', { method: 'POST', body: JSON.stringify({ remove: [image] }) });
  check('taken_off', off.status === 200 && off.body?.without?.includes(image), off);
  await page.reload();
  const offRows = await implementationRow(page, 'image').then(() => page.locator(`.tools [data-implementation][data-tool-operation="image"]`).evaluateAll((rows) => rows.map((one) => one.dataset.tool)));
  check('off_the_list_and_the_palette', !(await api(`/v1/tools?project=${id}`)).body.tools.some((tool) => tool.id === image) && !offRows.includes(image), offRows);
  const back = await api('/v1/tools/list', { method: 'POST', body: JSON.stringify({ add: image.split('/')[0] }) });
  putBack = undefined;
  check('back_when_added', back.status === 200 && !back.body?.without?.includes(image) && (await api(`/v1/tools?project=${id}`)).body.tools.some((tool) => tool.id === image), back);

  // No Hub: what's installed, and the palette says so. Then Calcifer as it was, and the Hub's tools again.
  seen.hub_checked = Boolean(process.env.NO_HUB && process.env.RESTART);
  if (seen.hub_checked) {
    // Each restart done, or said: a failed one leaves the old Calcifer answering, on its Hub, and the cells after on the wrong one.
    const noHub = await shell(process.env.NO_HUB).then(() => true, (why) => String(why));
    check('restarted_with_no_hub', noHub === true, noHub);
    await until('Calcifer back with no Hub', () => api('/v1/status').catch(() => null), (answer) => answer?.status === 200);
    const away = (await api(`/v1/tools?project=${id}`)).body;
    seen.no_hub = { note: away?.note, tools: away?.tools?.length };
    check('no_hub_says_so', /^only what's installed/.test(away?.note ?? '') && away.tools.some((tool) => tool.id === 'cozy/edit'), seen.no_hub);
    await page.reload();
    const said = await until('the palette says so', () => page.locator('.tools [data-palette="tools"] p').allInnerTexts(), (texts) => texts.some((text) => text.includes("only what's installed"))).catch((why) => String(why));
    check('the_palette_says_so', Array.isArray(said), said);
    await shot('no-hub');
    const onHub = await shell(process.env.RESTART).then(() => true, (why) => String(why));
    check('restarted_on_its_hub', onHub === true, onHub);
    await until('Calcifer back as it was', () => api('/v1/status').catch(() => null), (answer) => answer?.status === 200);
    const again = await until('the Hub read again', async () => (await api(`/v1/tools?project=${id}`)).body, (body) => body && !body.note && body.tools.some((tool) => tool.id === image)).catch(() => null);
    check('the_hub_tools_back', Boolean(again));
  }

  // cl-975: local on a released Runtime older than the Hub's newest releases need (0.45.0, this shard's Hub pin: e2e.yaml) moves up for
  // one as a run of it would (cl-952), so each is still offered: e2e-runtime-046 (cozy-runtime>=0.46.0,<0.47) and every catalog tool,
  // never e2e-runtime-044 (<0.45). Held on 0.45.0 by hand, it moves for nothing, and 046 is off. Restarted, the first list is the Hub's
  // as last read, never only what's installed.
  if (process.env.CATALOG && process.env.RESTART) {
    const NEXT = 'e2e-runtime-046', OLD = 'e2e-runtime-044';
    const cozy = (...args) => execFileSync(process.env.COZY, ['--json', ...args], { encoding: 'utf8' });
    const on = async (runtime) => {
      const deadline = Date.now() + 10 * STEP_MS;
      let last;
      while (Date.now() < deadline) {
        try { last = JSON.parse(cozy('machine', 'show', 'local')); } catch (why) { last = String(why); }
        if (last?.state === 'ready' && last?.status?.runtime === runtime) return last;
        await new Promise((done) => setTimeout(done, 3000));
      }
      throw new Error(`local on Runtime ${runtime}: still ${JSON.stringify(last?.status?.runtime ?? last)}`);
    };
    const listing = async () => (await api(`/v1/tools?project=${id}`)).body;
    const lists = (body, name) => Boolean(body?.tools?.some((tool) => tool.package === name));
    const everyOne = (body) => Object.values(wanted).flat().filter((tool) => !body?.tools?.some((row) => row.id === tool));
    check('a_newer_runtime_release_is_on_the_hub', released.includes(NEXT), released);
    cozy('machine', 'update', 'local', '--runtime', '0.45.0', '-y');
    await on('0.45.0');
    const held = await until('the list held on 0.45.0', listing, (body) => body && !lists(body, NEXT)).catch(() => null);
    check('held_on_0_45_it_offers_no_046', Boolean(held), (await listing())?.tools?.map((tool) => tool.id));
    cozy('machine', 'update', 'local', '-y');
    await on('0.45.0');
    const moves = await until('the list on 0.45.0, not held', listing, (body) => body && !body.note && lists(body, NEXT) && everyOne(body).length === 0).catch(() => null);
    const now = await listing();
    seen.on_045 = { note: now?.note, missing: everyOne(now), tools: now?.tools?.map((tool) => tool.id) };
    check('on_0_45_it_offers_what_needs_0_46', Boolean(moves), seen.on_045);
    check('on_0_45_it_offers_no_044', !lists(now, OLD), seen.on_045);
    await shell(process.env.RESTART).catch(() => {});
    await until('Calcifer back', () => api('/v1/status').catch(() => null), (answer) => answer?.status === 200);
    const first = await listing();
    seen.after_restart = { note: first?.note, missing: everyOne(first), tools: first?.tools?.length };
    check('restarted_its_first_list_is_the_hubs', !/^only what's installed/.test(first?.note ?? '') && lists(first, NEXT) && everyOne(first).length === 0, seen.after_restart);
  }

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
