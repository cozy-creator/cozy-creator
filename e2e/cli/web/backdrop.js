// An edit's backdrop shows every picture it uses, as large as they can be (cl-949, Paul 11-12: "on qwen image 2.1, when editing an image,
// right now it only displays one of the images, rather than all 3 of them ... all 3, as large as possible in the background, full-bleed
// where possible"), in a real browser on a real Calcifer. Qwen Image's box, a take of its own and its words mentioning three pictures of
// three shapes: in Focus, Edit, all three behind the frame, each a real share of it, the frame full. A tool whose picture is an input (Qwen
// Image's Upscale) shows its input there too. Screenshots of both.
//   node backdrop.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, settled, until, upload } = require('./page');

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
  failures.push(`${during}: ${why instanceof Error ? why.stack ?? why.message : why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-backdrop-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id } = await projectFromPage(page);
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const folder = mkdtempSync(join(tmpdir(), 'web-backdrop-'));
  // Three pictures of three shapes, and the take being edited.
  const files = { wide: png(60, 320, 160), square: png(140, 200, 200), tall: png(220, 160, 280), take: png(100, 300, 200) };
  for (const [name, bytes] of Object.entries(files)) writeFileSync(join(folder, `${name}.png`), bytes);

  const tools = (await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? [];
  const image = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.label === 'Image') ?? tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image');
  const upscale = tools.find((tool) => tool.package === 'qwen-image-2' && tool.label === 'Upscale');
  if (!image) throw new Error(`the Hub lists no Qwen Image: ${JSON.stringify(tools.map((tool) => tool.id))}`);
  seen.tools = { image: image.id, upscale: upscale?.id ?? null };
  for (const [name, x] of [['wide', 0], ['square', 380], ['tall', 760]]) {
    await post(`${at}/imports`, { source: await upload(held?.token, join(folder, `${name}.png`)), name, exact: true, at: { x, y: 0 } });
  }
  await post(`${at}/assets`, { tool: image.id, kind: 'image', name: 'edit', exact: true, fields: { instructions: '<wide>, <square> and <tall> in one picture' }, at: { x: 0, y: 420 } });
  await post(`${at}/assets/edit/versions`, { source: await upload(held?.token, join(folder, 'take.png')) });
  if (upscale) await post(`${at}/assets`, { tool: upscale.id, kind: 'image', name: 'bigger', exact: true, fields: { image: '<wide>' }, at: { x: 420, y: 420 } });
  for (const name of ['wide', 'square', 'tall', 'edit', ...(upscale ? ['bigger'] : [])]) await boxOf(page, name).waitFor();
  await fitCanvas(page);
  await settled(page);

  /** What's behind the frame being written in: each picture, loaded, and its share of the frame; and how much of the frame they cover. */
  const behind = (name) => page.locator(`.card.here[data-asset="${name}"]`).evaluate((card) => {
    const backdrop = card.querySelector('.backdrop');
    if (!backdrop) return { pictures: [], covered: 0 };
    const frame = backdrop.getBoundingClientRect();
    const pictures = [...backdrop.querySelectorAll('.spread-picture')].map((cell) => {
      const r = cell.getBoundingClientRect();
      const img = cell.querySelector('img');
      return { key: cell.dataset.picture, share: (r.width * r.height) / (frame.width * frame.height), loaded: Boolean(img?.complete && img.naturalWidth > 0) };
    });
    return { pictures, covered: pictures.reduce((sum, one) => sum + one.share, 0), rows: Number(backdrop.querySelector('.spread')?.dataset.rows ?? 0) };
  });

  // Editing Qwen's picture with three mentioned: all three behind it, each a real share, together the frame.
  await openBox(page, 'edit');
  await page.locator('.card.here[data-asset="edit"] button[aria-label="Edit image"]').click();
  await page.locator('.card.here[data-asset="edit"] .in-frame').waitFor();
  const three = await until('three pictures behind the edit', () => behind('edit'), (now) => ['wide', 'square', 'tall'].every((key) => now.pictures.some((one) => one.key === key && one.loaded)), 20_000).catch(() => behind('edit'));
  seen.edit = three;
  const mentioned = three.pictures.filter((one) => ['wide', 'square', 'tall'].includes(one.key));
  check('all_three_behind_the_edit', mentioned.length === 3 && mentioned.every((one) => one.loaded), three);
  check('each_a_real_share', mentioned.every((one) => one.share >= 0.12), three);
  check('the_frame_full', three.covered >= 0.93, three);
  await shot('edit');
  await closeBox(page);

  // A picture given as an input is behind its frame too.
  if (upscale) {
    await openBox(page, 'bigger');
    const input = await until('its input behind it', () => behind('bigger'), (now) => now.pictures.some((one) => one.key === 'wide' && one.loaded), 20_000).catch(() => behind('bigger'));
    seen.upscale = input;
    check('an_input_behind_its_frame', input.pictures.some((one) => one.key === 'wide' && one.loaded && one.share >= 0.9), input);
    await shot('input');
    await closeBox(page);
  } else seen.upscale = 'no Qwen Upscale listed';

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
