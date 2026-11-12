// A field that applies only while another has a value (cl-948, Paul 11-12: "for qwen image edit, if prompt sampling is turned off, the
// creativity of the prompt sampling selector should be dulled and turned off. There should be a general way for a tool to say 'this
// setting depends on that'"), in a real browser on a real Calcifer, with the fixture `project/when.py`, read by this machine's Runtime.
// `cozy run` given Creativity with prompt upsampling off, and an overlap with no batch size: each said, and the function gets its
// defaults; with them on it gets what was given; --describe says each condition. In Focus, Prompt upsampling turned off dulls Creativity
// at once, its value kept, its control disabled and why on hover; a Render then sends its default; turned on, it's back. Temporal overlap
// is dulled until Batch size is set. Where this computer runs no models it says so and skips: only a machine's Runtime reads `When`.
//   node when.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT, $CHROME, $COZY, $SHOT, $ALONE as tools.js. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { spawnSync } = require('child_process');
const { readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, STEP_MS, boxOf, lastLook, launch, lookAt, noStall, openBox, until } = require('./page');

const COZY = process.env.COZY;
const FIXTURE = join(__dirname, '..', 'project', 'when.py');
// The first run installs the fixture on this machine.
const FIRST_RUN_MS = 300_000;

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
  await page.fill('input[aria-label="New project\'s name"]', `web-when-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  if (!((await api('/v1/status')).body.local_usable && !process.env.ALONE)) {
    seen.skipped = 'this computer runs no models: only a machine Runtime reads When, and PyPI\'s refuses it';
    await browser.close();
    console.log(JSON.stringify({ failures, seen }, null, 2));
    return;
  }
  // New code, so this machine has never installed it.
  const fixture = join(folder, 'when.py');
  writeFileSync(fixture, `${readFileSync(FIXTURE, 'utf8')}\n# ${id}\n`);
  const cozy = (...args) => spawnSync(COZY, args, { encoding: 'utf8', cwd: folder, timeout: FIRST_RUN_MS });
  const runs = async () => ((await api(`/v1/runs?project=${id}&limit=100`)).body?.runs ?? []).filter((run) => run.target?.endsWith('/image') && run.status === 'completed').sort((a, b) => b.number - a.number);
  const given = async (number) => (await api(`/v1/runs/${number}`)).body?.result?.given;

  // `cozy run`: each value given where it doesn't apply is said and goes as its default; given where it does, as written.
  const off = cozy('--json', 'run', `${fixture}/image`, 'instructions=a lighthouse', 'prompt_upsampling=false', 'creativity=wild', 'temporal_overlap=4', '--on', 'local');
  seen.off = { status: off.status, said: off.stderr.split('\n').filter((line) => line.includes('ignored')) };
  check('run_says_creativity_is_ignored', off.stderr.includes('creativity: "wild" is ignored: it applies only with prompt_upsampling on'), off.stderr.slice(-800));
  check('run_says_overlap_is_ignored', off.stderr.includes('temporal_overlap: 4 is ignored: it applies only with batch_size set'), off.stderr.slice(-800));
  const [first] = await runs();
  check('the_function_got_the_defaults', first && (await given(first.number)) === 'creativity=balanced temporal_overlap=0', first && (await given(first.number)));
  const on = cozy('--json', 'run', `${fixture}/image`, 'instructions=a lighthouse', 'prompt_upsampling=true', 'creativity=wild', 'batch_size=8', 'temporal_overlap=4', '--on', 'local');
  const [second] = await runs();
  check('with_them_on_nothing_is_said', on.status === 0 && !on.stderr.includes('is ignored'), on.stderr.slice(-800));
  check('with_them_on_the_function_got_them', second && second.number !== first?.number && (await given(second.number)) === 'creativity=wild temporal_overlap=4', second && (await given(second.number)));
  const described = cozy('run', `${fixture}/image`, '--describe');
  check('describe_says_each_condition', described.stdout.includes('(only with prompt_upsampling on)') && described.stdout.includes('(only with batch_size set)'), described.stdout.slice(-800));

  // A box of it, and the tool as the page reads it: local code a box names, read by this machine.
  const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: `${fixture}/image`, kind: 'image', name: 'still', exact: true, fields: { instructions: 'a lighthouse at dusk', creativity: 'wild' }, at: { x: 200, y: 200 } }) });
  if (made.status >= 300) throw new Error(`the box: ${JSON.stringify(made)}`);
  const tool = await until('the fixture listed', async () => (await api(`/v1/tools?project=${id}`)).body?.tools?.find((one) => one.local && one.id === `${fixture}/image`), Boolean, FIRST_RUN_MS);
  const creativityField = tool.fields.find((field) => field.field === 'creativity');
  check('the_tool_says_when', JSON.stringify(creativityField?.when) === JSON.stringify({ field: 'prompt_upsampling' }), creativityField);
  await boxOf(page, 'still').waitFor();
  await openBox(page, 'still');
  const fieldsOf = async () => (await api(at)).body?.assets?.find((row) => row.name === 'still')?.fields ?? {};
  /** The control labelled `label` in Focus: why it's dulled (its hint), whether it's disabled, and what it holds. */
  const control = (label) => page.evaluate((label) => {
    const found = document.querySelector(`.card.here [aria-label="${label}"]`);
    const dulled = found?.closest('fieldset.when-off');
    const buttons = [...(found?.querySelectorAll('button') ?? [])];
    return found && { why: dulled?.title ?? null, disabled: found.matches(':disabled') || (buttons.length > 0 && buttons.every((b) => b.matches(':disabled'))), checked: found.querySelector('[aria-checked=true]')?.textContent ?? null, faded: Number(getComputedStyle(dulled ?? found).opacity) };
  }, label);
  const switchOf = page.locator('.card.here input[role=switch][aria-label="Prompt upsampling"]');

  const before = await control('Creativity');
  check('creativity_applies_with_upsampling_on', before && before.why === null && !before.disabled && before.checked === 'wild', before);
  const overlap = await control('Temporal overlap');
  check('overlap_dulled_without_a_batch_size', overlap?.why === 'Only with Batch size set' && overlap.disabled, overlap);
  await switchOf.click();
  const dulled = await until('Creativity dulled', () => control('Creativity'), (now) => now?.why !== null && now?.why !== undefined);
  check('creativity_dulled_with_why', dulled.why === 'Only with Prompt upsampling on' && dulled.disabled && dulled.faded < 0.6, dulled);
  check('its_value_is_kept', dulled.checked === 'wild', dulled);
  const written = await until('upsampling off in the file', fieldsOf, (fields) => fields.prompt_upsampling === false);
  check('the_box_keeps_creativity', written.creativity === 'wild', written);
  await shot('dulled');
  // A hover over it names why, and a press on it changes nothing.
  await page.locator('.card.here fieldset.when-off:has([aria-label="Creativity"])').hover();
  await page.locator('.card.here [aria-label="Creativity"] [role=radio]').first().click({ force: true }).catch(() => {});
  check('a_press_changes_nothing', (await fieldsOf()).creativity === 'wild', await fieldsOf());

  // Turned on again, it's back as it was; off again, dulled again.
  await switchOf.click();
  const back = await until('Creativity back', () => control('Creativity'), (now) => now?.why === null);
  check('creativity_back_with_upsampling_on', !back.disabled && back.checked === 'wild', back);
  await shot('back');
  await switchOf.click();
  await until('Creativity dulled again', () => control('Creativity'), (now) => now?.why === 'Only with Prompt upsampling on');

  // Rendered now, the box's function gets Creativity's default, said nowhere: the box shows why. A take shows the box's
  // picture, not its fields, so this is last.
  const last = (await runs())[0]?.number ?? 0;
  await page.locator('.card.here button[aria-label=Render]').first().click();
  const rendered = await until('the render', async () => (await runs()).find((run) => run.number > last), Boolean, FIRST_RUN_MS);
  check('the_render_got_the_default', (await given(rendered.number)) === 'creativity=balanced temporal_overlap=0', await given(rendered.number));
  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
