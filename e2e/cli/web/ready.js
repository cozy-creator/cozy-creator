// The page's half of prefetch_ready (e2e/cli/prefetch.sh, cl-1104). Paul, 11-16: "when a user attaches a tool, like the qwen image 2.1
// tool, we could figure out what machine, if we got a request, that request would land on. On that machine, we can pre-emptively install
// packages + models, so that when we do get a request, we'll have those already ready." With the project open and its box <box> open,
// whose machine Calcifer made ready with no render asked, the box says so beside its Render: "Ready on <where>", a note in the box's own
// row of actions, left of the button and on its line. Looked at at 1440×900 and 1920×1080, and on the canvas under the pointer. Then
// its Render is pressed.
//   node ready.js <project folder> <box> <where, as the page names the machine>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $HOLDER: Calcifer's lock record; $SHOT: its pictures.
// Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { hoverBox, lastLook, launch, lookAt, noStall, openBox, projectUrl, renderOf, settled, signIn } = require('./page');

const [folder, box, where] = process.argv.slice(2);
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
const shot = (page, name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : null);

(async () => {
  const browser = await launch(playwright);
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  lookAt(page);
  await signIn(page, await projectUrl(folder));
  await page.locator('.stage').waitFor();
  await settled(page);
  // On the canvas, under the pointer: looked at, and whatever it says of its machine stays inside its card.
  const card = await hoverBox(page, box);
  await page.waitForTimeout(1500);
  const [onCard, cardAt] = [await card.locator('.ready-on').boundingBox().catch(() => null), await card.boundingBox()];
  seen.card_note_at = onCard;
  check('card_note_inside_its_card', !onCard || (Boolean(cardAt) && onCard.x >= cardAt.x - 1 && onCard.x + onCard.width <= cardAt.x + cardAt.width + 1), { onCard, cardAt });
  await shot(page, 'card');
  await openBox(page, box);
  const render = await renderOf(page, box);
  const note = page.locator(`.card.here[data-asset="${box}"] .ready-on`);
  const said = await note.waitFor({ timeout: 60_000 }).then(() => note.innerText(), () => '');
  check('box_says_ready', said.trim() === `Ready on ${where}`, said || 'no note beside Render');
  const [at, button] = [await note.boundingBox().catch(() => null), await render.boundingBox()];
  seen.note_at = at;
  seen.render_at = button;
  const beside = Boolean(at && button) && at.x >= 0 && at.x + at.width <= button.x + 1 && Math.abs(at.y + at.height / 2 - (button.y + button.height / 2)) <= button.height / 2;
  check('note_beside_render', beside, { at, button });
  await shot(page, '1440');
  await page.setViewportSize({ width: 1920, height: 1080 });
  await settled(page);
  check('says_ready_at_1920', (await note.innerText().catch(() => '')).trim() === `Ready on ${where}`, await note.count());
  await shot(page, '1920');
  await page.setViewportSize({ width: 1440, height: 900 });
  await settled(page);
  await (await renderOf(page, box)).click();
  seen.render_pressed = true;
  await page.waitForTimeout(2000);
  await shot(page, 'rendering');
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
