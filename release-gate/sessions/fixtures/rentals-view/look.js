// one-pool.sh's browser (cl-1193, cl-1224): the page as Paul reads a rental another computer ordered, on this runner's Calcifer: the tabs top
// right and that machine's activity under its tab, Machines, and the machine, each at 1440x900 and 1920x1080.
//   node look.js <the project's page, from `cozy --json open`> <the machine> <phase>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: Google Chrome; $SHOT: where its screenshots go; $SIZES: the viewports. Prints what it read as JSON; exits 1
// if a look failed.
const { chromium } = require(process.env.PLAYWRIGHT);

const [link, name, phase] = process.argv.slice(2);
const failures = [];
const seen = {};

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  // $SIZES: `WxH,WxH` (default 1440x900,1920x1080).
  for (const [width, height] of (process.env.SIZES || '1440x900,1920x1080').split(',').map((size) => size.split('x').map(Number))) {
    const size = `${width}x${height}`;
    const page = await (await browser.newContext({ viewport: { width, height } })).newPage();
    page.setDefaultTimeout(60_000);
    const shot = (what) => page.screenshot({ path: `${process.env.SHOT}-${phase}-${size}-${what}.png` });
    try {
      await page.goto(link);
      await page.waitForSelector('nav.mtabs');
      seen[`${size} tabs`] = await page.locator('nav.mtabs').innerText();
      await shot('top');
      // Its tab, or past what fits, "N more": Machines with those that have no tab first.
      const tab = page.locator(`nav.mtabs .mtab[data-machine="${name}"]`);
      if (await tab.isVisible()) {
        await tab.click();
        const activity = page.locator('.activity');
        await activity.waitFor();
        seen[`${size} activity`] = await activity.innerText();
        await shot('activity');
        await page.keyboard.press('Escape');
      } else {
        await page.locator('nav.mtabs a.mtab.more').click();
        await page.locator(`aside.drawer li.machine[data-machine="${name}"]`).waitFor();
        seen[`${size} more`] = await page.locator('aside.drawer').innerText();
        await shot('more');
      }
      const base = link.replace(/\/$/, '');
      await page.goto(`${base}/machines`);
      const row = page.locator(`li.machine[data-machine="${name}"]`);
      await row.waitFor();
      seen[`${size} machines`] = await page.locator('aside.drawer').innerText();
      seen[`${size} row`] = await row.innerText();
      await row.scrollIntoViewIfNeeded();
      await shot('machines');
      await page.goto(`${base}/machines/${encodeURIComponent(name)}`);
      await page.waitForSelector('.doing-now');
      seen[`${size} machine`] = await page.locator('aside.drawer').innerText();
      await shot('machine');
    } catch (error) {
      failures.push(`${size}: ${String(error).split('\n')[0]}`);
    }
    await page.close();
  }
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})();
