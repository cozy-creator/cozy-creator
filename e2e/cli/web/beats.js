// The Shot tool's beats (cl-903, Paul 11-12: "the 'beats' box should also be much larger, in terms of more lines to type into ... the
// 'duration' bar doesn't need to go all the way to the right ... We also don't need a total counter"), in a real browser on a real Calcifer. H3's
// Shot with three beats: each beat's words start six lines tall and grow with what's typed, its length is a short slider beside its value, and
// nothing under them adds the lengths up. A screenshot of it.
//   node beats.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { projectFolder, projectFromPage, calcifer, signIn, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, openBox, until } = require('./page');

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
  const api = (path, init = {}) => page.evaluate(async ({ path, init }) => {
    const answer = await fetch(path, { ...init, headers: { 'cozy-project-storage': '3', 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-beats-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const box = (name) => boxOf(page, name);

  // H3's Shot with three beats of 10 s, the third with a paragraph of words.
  const searched = (await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [];
  const beated = searched.filter((tool) => tool.package === 'minimax-h3' && tool.fields.some((field) => field.field === 'beats'));
  const shotTool = beated.find((tool) => tool.id === 'minimax-h3/generate') ?? beated[0];
  seen.tool = shotTool?.id ?? searched.map((tool) => tool.id);
  if (!shotTool) throw new Error('the Hub lists no H3 tool with beats');
  const long = 'She steps out of the rain into the bar. The neon over the door hums and flickers. She shakes off her coat, looks along the counter, finds the man in grey at the end, and walks to him without a word. He doesn\'t look up from his glass.';
  const beats = [{ description: 'A wide street at night, rain on the stones.', duration: 10 }, { description: 'Closer: the door of the bar.', duration: 10 }, { description: long, duration: 10 }];
  const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: shotTool.id, kind: 'video', name: 'scene', exact: true, fields: { beats }, at: { x: 80, y: 80 } }) });
  if (made.status >= 300) throw new Error(`the Shot box: ${JSON.stringify(made)}`);
  await fitCanvas(page);
  await openBox(page, 'scene');
  // Open, the box is where it's worked on, on the canvas or over it (Focus).
  const scene = page.locator('.card.here[data-asset="scene"]').first();
  const rows = scene.locator('.beats-edit[data-field="beats"] > .beat');
  await rows.nth(2).waitFor();
  check('three_beats', (await rows.count()) === 3, await rows.count());

  // Each beat's words start six lines tall, and the long one grows to show all it says. Beats shown one at a time (an accordion) are each opened.
  const measured = [];
  for (let i = 0; i < 3; i++) {
    const row = rows.nth(i);
    const toggle = row.locator('.beat-toggle');
    if ((await toggle.count()) && (await toggle.getAttribute('aria-expanded')) !== 'true') {
      await toggle.click();
      await until(`beat ${i + 1} open`, () => toggle.getAttribute('aria-expanded'), (open) => open === 'true');
      // Measured once it has opened all the way, not part way through: its height the same two looks running. (Its animation can't be
      // waited on: headless Chrome's compositor may hold it at its start.)
      let was = -1;
      await until(`beat ${i + 1} opened all the way`, async () => {
        const now = await row.evaluate((one) => one.getBoundingClientRect().height);
        const still = now === was;
        was = now;
        return still;
      }, Boolean, 10_000).catch(() => {});
    }
    measured.push(await row.evaluate((one) => {
      const words = one.querySelector('.mention-text[data-field$=".description"]') ?? one.querySelector('.mention-text');
      const style = getComputedStyle(words);
      const line = Number.parseFloat(style.lineHeight) || Number.parseFloat(style.fontSize) * 1.4;
      const slider = one.querySelector('.beat-time input[type="range"]');
      return {
        lines: words.clientHeight / line,
        fits: words.scrollHeight <= words.clientHeight + 1,
        slider: slider?.getBoundingClientRect().width ?? null,
        row: one.querySelector('.beat-time')?.getBoundingClientRect().width ?? null,
        value: slider?.closest('.slide')?.querySelector('.num')?.textContent ?? null,
        accordion: one.classList.contains('beat-accordion'),
      };
    }));
    await shot(`beat-${i + 1}`);
  }
  seen.beats = measured;
  check('each_beat_starts_six_lines_tall', measured.every((one) => one.lines >= 6), measured);
  check('a_long_beat_grows_to_show_it_all', measured[2].fits && measured[2].lines >= measured[0].lines, measured[2]);
  check('the_length_is_a_short_slider_beside_its_value', measured.every((one) => one.slider !== null && one.row !== null && one.slider < one.row * 0.6 && /^10\b/.test(one.value ?? '')), measured);
  // Nowhere among the beats a sum of their lengths (3 × 10 s), however it would be written.
  const all = await scene.locator('.beats-edit[data-field="beats"]').innerText();
  check('no_running_total', !/\b30\s*(s|sec|secs|seconds)\b|\b0:30\b/i.test(all), all);
  await shot('three-beats');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
