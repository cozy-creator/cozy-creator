// The page's half of disk_room (e2e/cli/disk.sh, cl-1175). Paul, 11-17: "if the user's disc space available is really small, we should
// probably warn them and stop them from trying to download models they cannot fit, instead telling them to free up disc-space or run on
// another machine + rent another machine". A box whose render waits because no machine of its has room on its disk for its models says
// so: its face, on the canvas and opened, "Waiting for disk space / Needs <bytes> more", and opened, its status: "This computer needs
// <bytes> more disk space for its models: free that much and it renders here, or render it on a new rental." beside Machines. Looked at
// at 1440×900 and 1920×1080, paper and dark, on the canvas and opened.
//   node room.js <project folder> <box>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $HOLDER: Calcifer's lock record; $SHOT: its pictures.
// Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { lastLook, launch, lookAt, noStall, openBox, projectUrl, settled, signIn } = require('./page');

const [folder, box] = process.argv.slice(2);
const SIZES = [[1440, 900], [1920, 1080]];
const SAID = /This computer needs \d+(\.\d+)?(MB|GB) more disk space for its models: free that much and it renders here, or render it on a new rental\./;
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
/** `name` at each size, paper and dark. */
async function shoot(page, name) {
  for (const [width, height] of SIZES) {
    await page.setViewportSize({ width, height });
    for (const theme of ['paper', 'dark']) {
      await page.evaluate((dark) => {
        if (dark) document.documentElement.dataset.theme = 'dark';
        else delete document.documentElement.dataset.theme;
        return new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      }, theme === 'dark');
      await settled(page);
      if (process.env.SHOT) await page.screenshot({ path: `${process.env.SHOT}-${name}-${width}x${height}-${theme}.png` });
    }
  }
  await page.evaluate(() => { delete document.documentElement.dataset.theme; });
  await page.setViewportSize({ width: 1440, height: 900 });
  await settled(page);
}

(async () => {
  const browser = await launch(playwright);
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  lookAt(page);
  await signIn(page, await projectUrl(folder));
  await page.locator('.stage').waitFor();
  await settled(page);
  await shoot(page, 'canvas');
  await openBox(page, box);
  const runFace = page.locator('.run-face').filter({ hasText: 'Waiting for disk space' }).first();
  const face = await runFace.waitFor({ timeout: 60_000 }).then(() => runFace.innerText(), () => '');
  check('face_says_room', /^Waiting for disk space\s*Needs\s+\d+(\.\d+)?(MB|GB) more\s*Cancel$/.test(face.trim()), face || (await page.locator('.run-face').allInnerTexts().catch(() => [])).join(' | ').slice(0, 600));
  const status = page.locator('.status.mak').filter({ hasText: SAID }).first();
  const said = await status.waitFor({ timeout: 30_000 }).then(() => status.innerText(), () => '');
  check('status_says_room', SAID.test(said) && /Machines$/.test(said.trim()), said || 'no status');
  await shoot(page, 'opened');
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
