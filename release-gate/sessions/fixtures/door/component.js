// door.sh's look at a package's component through the door: the page on a machine's project, in a real Chrome. Prints what its box drew and
// where the page asked for a component, and door.sh judges it.
//   node component.js <sign-in link from `cozy web --no-open`> <machine> <its project's id>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: Google Chrome. Every step is bounded.
const { chromium } = require(process.env.PLAYWRIGHT);
const [link, machine, id] = process.argv.slice(2);
const STEP_MS = 60000;

(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await context.newPage();
  page.setDefaultTimeout(STEP_MS);
  const toolUi = [];
  page.on('request', (asked) => {
    const { pathname } = new URL(asked.url());
    if (/\/v1\/tools\/.*\/ui(\/|$)/.test(pathname)) toolUi.push(pathname);
  });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  await page.goto(link);
  await page.waitForSelector('.top');
  await page.evaluate((name) => sessionStorage.setItem('cozy.calcifer', name), machine);
  await page.goto(`${new URL(link).origin}/#/projects/${encodeURIComponent(id)}`);
  await page.reload();
  const box = page.locator('.react-flow__node').filter({ has: page.locator('.nm:text-is("shaded")') });
  await box.waitFor();
  await box.locator('.nm').hover();
  await box.locator('button[aria-label=Render]').waitFor();
  // The tools list is the machine's: it says the tool has a `ui`, and the page asks this computer's Calcifer.
  for (let waited = 0; waited < 300 && !toolUi.length; waited++) await page.waitForTimeout(100);
  await page.waitForTimeout(1000);
  const out = {
    drawn: await box.locator('[data-boxed]').count(),
    declared: await box.locator('input[type=range]').count(),
    tool_ui: toolUi,
    violations: await page.evaluate(() => window.violations),
  };
  console.log(JSON.stringify(out));
  await browser.close();
})().catch((error) => { console.error(error); process.exit(1); });
