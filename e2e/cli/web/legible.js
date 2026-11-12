// Typed words read over a picture (cl-901, Paul 11-12: "the text you type into a prompt should be more bold ... so it's easier to see against
// a background image"), in a real browser on a real Calcifer. Qwen's Image on a busy picture, in Edit, its prompt over the picture: what's typed
// is bold, plain (no halo or outline), on a surface of the panel's colour over the picture (Paul, 11-12, cl-893), on paper and on dark; a prompt
// anywhere else is bold too. A screenshot of each.
//   node legible.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, until, upload } = require('./page');

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
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-legible-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).body?.assets?.find((row) => row.name === name);
  seen.project = { id, folder: (await api(at)).body.path };
  const box = (name) => boxOf(page, name);

  // A busy picture: black, white and bright stripes and checks, where no one colour of words reads.
  const folder = mkdtempSync(join(tmpdir(), 'web-legible-'));
  const colours = [[0, 0, 0], [255, 255, 255], [255, 210, 0], [30, 90, 255], [240, 40, 40], [250, 245, 235]];
  writeFileSync(join(folder, 'busy.png'), png(0, 640, 400, false, (x, y) => colours[(((x >> 3) ^ (y >> 4)) + (x >> 5)) % colours.length]));
  await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(folder, 'busy.png'), id), name: 'busy', exact: true, at: { x: 100, y: 100 } }) });
  await box('busy').waitFor();

  // Qwen's Image on it starts from it, and Edit puts its prompt over the picture.
  const searched = (await api(`/v1/tools?project=${id}&q=qwen`)).body?.tools ?? [];
  const qwen = searched.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  seen.tool = qwen?.id ?? searched.map((tool) => tool.id);
  if (!qwen) throw new Error('the Hub lists no Qwen Image tool with a prompt');
  await api(`${at}/assets/busy`, { method: 'PATCH', body: JSON.stringify({ tool: qwen.id }) });
  await until('Qwen on it', () => asset('busy'), (row) => row?.tool === qwen.id);
  await fitCanvas(page);
  await openBox(page, 'busy');
  // Open, a picture fills the page (Focus): the box is where it's worked on, on the canvas or over it.
  const here = page.locator('.card.here[data-asset="busy"]').first();
  await here.getByRole('button', { name: 'Edit image', exact: true }).click();
  const prompt = here.locator('.in-frame .mention-text').first();
  await prompt.waitFor();
  // Behind what's written, the pictures it uses, the backdrop's spread (cl-949).
  await until('the picture behind the prompt', () => here.locator('.in-frame .backdrop .spread-picture img').first().evaluate((image) => image.complete && image.naturalWidth > 0).catch(() => false), Boolean);
  await prompt.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' a lighthouse at dusk, waves breaking white on black rocks, gulls over the lamp');
  await until('the words typed', () => prompt.innerText(), (text) => text.includes('gulls over the lamp'));

  /** How the prompt's words are drawn: weight, colour, each shadow round them, and the surface under them against the panel's colour (0–255, alpha 0–1). */
  const drawn = () => prompt.evaluate((element) => {
    const style = getComputedStyle(element);
    const rgba = (value) => {
      const srgb = value.match(/^color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)(?: \/ ([\d.]+))?\)$/);
      if (srgb) return [Number(srgb[1]) * 255, Number(srgb[2]) * 255, Number(srgb[3]) * 255, srgb[4] === undefined ? 1 : Number(srgb[4])];
      const rgb = value.match(/^rgba?\(([\d.]+), ([\d.]+), ([\d.]+)(?:, ([\d.]+))?\)$/);
      return rgb ? [Number(rgb[1]), Number(rgb[2]), Number(rgb[3]), rgb[4] === undefined ? 1 : Number(rgb[4])] : null;
    };
    const probe = document.body.appendChild(document.createElement('i'));
    probe.style.color = 'var(--panel)';
    const panel = rgba(getComputedStyle(probe).color);
    probe.remove();
    return { weight: Number(style.fontWeight), color: style.color, shadows: style.textShadow === 'none' ? [] : style.textShadow.split(/,(?![^(]*\))/).map((one) => one.trim()),
      surface: { background: style.backgroundColor, rgba: rgba(style.backgroundColor), panel } };
  });
  for (const theme of ['paper', 'dark']) {
    await page.evaluate((dark) => (dark ? (document.documentElement.dataset.theme = 'dark') : delete document.documentElement.dataset.theme), theme === 'dark');
    await page.waitForTimeout(200);
    const said = await drawn();
    seen[theme] = said;
    check(`${theme}_words_over_the_picture_are_bold`, said.weight >= 700, said);
    check(`${theme}_words_over_the_picture_are_plain`, said.shadows.length === 0, said);
    const { rgba: under, panel } = said.surface;
    check(`${theme}_words_over_the_picture_sit_on_the_panel`, Boolean(under && panel) && under[3] >= 0.8 && under.slice(0, 3).every((channel, i) => Math.abs(channel - panel[i]) <= 2), said.surface);
    await shot(`prompt-${theme}`);
  }
  await page.evaluate(() => delete document.documentElement.dataset.theme);

  // A prompt anywhere else is bold too: More's words, or what's under the frame.
  const others = await here.locator('.mention-text').evaluateAll((all) => all.filter((one) => !one.closest('.in-frame')).map((one) => ({ field: one.dataset.field, weight: Number(getComputedStyle(one).fontWeight) })));
  seen.others = others;
  check('every_prompt_is_bold', others.every((one) => one.weight >= 700), others);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
