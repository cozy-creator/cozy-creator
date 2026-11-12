// Pasted words mark their mentions (cl-902, Paul 11-12: "when I copy-paste into a 'beats' box ... it should be auto-highlighting the terms like
// <Azul_3> but I feel like sometimes it doesn't"), in a real browser on a real Calcifer. Lines pasted into a beat of H3's Shot, as plain text and
// as a rich-text program pastes them: each mention of a box here is a chip at once (`_` and digits, a version, a video's range, an orbit's angle),
// and the file holds the text as pasted. Typed, and loaded again, the same text is marked the same, `<nobody>` (no box) included.
//   node paste.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectUrl, calcifer, signIn, MP4, STEP_MS, boxOf, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, png, until, upload } = require('./page');

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
  await page.fill('input[aria-label="New project\'s name"]', `web-paste-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).body?.assets?.find((row) => row.name === name);
  seen.project = { id, folder: (await api(at)).body.path };
  const box = (name) => boxOf(page, name);

  // A picture named with `_` and a digit, and two videos.
  const folder = mkdtempSync(join(tmpdir(), 'web-paste-'));
  writeFileSync(join(folder, 'azul.png'), png(60));
  writeFileSync(join(folder, 'walk.mp4'), MP4);
  writeFileSync(join(folder, 'cat.mp4'), MP4);
  for (const [name, file, x] of [['Azul_3', 'azul.png', 80], ['walk', 'walk.mp4', 480], ['cat', 'cat.mp4', 880]]) {
    await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(folder, file)), name, exact: true, at: { x, y: 80 } }) });
    await box(name).waitFor();
  }

  // H3's Shot, its beats' words the prompts Paul pastes into.
  const searched = (await api(`/v1/tools?project=${id}&q=minimax`)).body?.tools ?? [];
  const beated = searched.filter((tool) => tool.package === 'minimax-h3' && tool.fields.some((field) => field.field === 'beats'));
  const shotTool = beated.find((tool) => tool.id === 'minimax-h3/generate') ?? beated[0];
  seen.tool = shotTool?.id ?? searched.map((tool) => tool.id);
  if (!shotTool) throw new Error('the Hub lists no H3 tool with beats');
  const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: shotTool.id, kind: 'video', name: 'scene', exact: true, fields: { beats: [{ description: '', duration: 5 }, { description: '', duration: 5 }] }, at: { x: 80, y: 520 } }) });
  if (made.status >= 300) throw new Error(`the Shot box: ${JSON.stringify(made)}`);
  await fitCanvas(page);
  await openBox(page, 'scene');
  // Open, the box is where it's worked on, on the canvas or over it (Focus).
  const here = () => page.locator('.card.here[data-asset="scene"]').first();
  const beat = (n) => here().locator(`.mention-text[data-field="beats.${n}.description"]`);
  /** Beat `n` open to write in: beats shown one at a time (an accordion) open by their heading. */
  const openBeat = async (n) => {
    const toggle = here().locator(`.beat[data-beat-index="${n}"] .beat-toggle`);
    if (!(await toggle.count()) || (await toggle.getAttribute('aria-expanded')) === 'true') return;
    await toggle.click();
    await until(`beat ${n + 1} open`, () => toggle.getAttribute('aria-expanded'), (open) => open === 'true');
    await page.waitForTimeout(300);
  };
  await beat(0).waitFor();

  const words = '<Azul_3> runs past <walk:48-120>,\nthen <cat:120deg> turns\n\nand <Azul_3.1> looks back at <nobody>.';
  const marked = ['Azul_3', 'walk:48-120', 'cat:120deg', 'Azul_3.1'];
  /** The names its chips stand for, in order. */
  const chips = (field) => field.locator('[data-mention]').evaluateAll((all) => all.map((one) => one.dataset.mention));
  /** A paste as the browser hands it over: plain text, and with `html` as a rich-text program copies it. */
  const paste = (field, text, html) => field.evaluate((element, { text, html }) => {
    const data = new DataTransfer();
    data.setData('text/plain', text);
    if (html) data.setData('text/html', html);
    element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  }, { text, html });
  const fieldOf = async (n) => (await asset('scene'))?.fields?.beats?.[n]?.description;

  const known = (names) => names.filter((name) => name !== 'nobody');
  /** Its chips once nothing more is coming: what Calcifer says of `<nobody>` has come back. */
  const settled = async (field) => {
    let last = '';
    return until('its marks settled', async () => {
      await page.waitForTimeout(700);
      const now = JSON.stringify(await chips(field));
      const same = now === last;
      last = now;
      return same ? JSON.parse(now) : null;
    }, Boolean);
  };

  // Pasted as plain text, then as rich text: each box's mention marked at once, and the file holding the words as pasted.
  const after = {};
  for (const [n, html] of [[0, null], [1, `<p><b>${words.replace(/</g, '&lt;').replace(/\n/g, '</p><p>')}</b></p>`]]) {
    const how = html ? 'rich' : 'plain';
    await openBeat(n);
    await beat(n).click();
    await paste(beat(n), words, html);
    seen[`${how}_at_once`] = await chips(beat(n));
    check(`${how}_paste_marks_each_mention_at_once`, JSON.stringify(known(seen[`${how}_at_once`])) === JSON.stringify(marked), seen[`${how}_at_once`]);
    await page.keyboard.type(' ok');
    await until(`${how}: the file holds what was pasted`, () => fieldOf(n), (said) => typeof said === 'string' && said.trimEnd() === `${words} ok`).catch(() => null);
    check(`${how}_paste_is_its_text_in_the_file`, (await fieldOf(n))?.trimEnd() === `${words} ok`, await fieldOf(n));
    after[n] = await settled(beat(n));
    seen[`${how}_settled`] = after[n];
  }
  await shot('pasted');

  // Typed by hand, the same text is marked the same.
  await closeBox(page);
  await openBox(page, 'scene');
  const typedInto = here().locator('.mention-text:not([data-field^="beats."])').first();
  seen.typed_field = await typedInto.getAttribute('data-field');
  await typedInto.click();
  await page.keyboard.press('End');
  await page.keyboard.type(words.replace(/\n/g, ' '));
  seen.typed_at_once = await chips(typedInto);
  check('typing_marks_each_mention_at_once', JSON.stringify(known(seen.typed_at_once)) === JSON.stringify(marked), seen.typed_at_once);
  seen.typed_settled = await settled(typedInto);
  check('typing_marks_the_same_as_pasting', JSON.stringify(seen.typed_settled) === JSON.stringify(after[0]), { typed: seen.typed_settled, pasted: after[0] });
  await page.locator('.react-flow__pane').first().click({ position: { x: 5, y: 5 } }).catch(() => {});

  // Loaded again, each beat is marked as it was when pasted.
  await page.goto(await projectUrl(id, '/assets/scene'));
  await beat(1).waitFor();
  for (const n of [0, 1]) {
    const loaded = await settled(beat(n));
    seen[`loaded_${n}`] = loaded;
    check(`loading_beat_${n}_marks_the_same_as_pasting`, JSON.stringify(loaded) === JSON.stringify(after[n]), { loaded, pasted: after[n] });
  }
  await shot('loaded');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
