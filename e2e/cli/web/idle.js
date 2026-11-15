// xs-189: the page as Paul keeps it, signed in, each project in its own tab, left alone. Once every canvas shows, it says `open` on
// stdout and stays until it's killed; the idle shard measures meanwhile (idle.sh).
//   node idle.js <project folder>...: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER. $PLAYWRIGHT, $CHROME
// as nodes.js.
const playwright = require(process.env.PLAYWRIGHT);
const { STEP_MS, launch, projectUrl, signIn } = require('./page');

(async () => {
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const first = await context.newPage();
  first.setDefaultTimeout(STEP_MS);
  await signIn(first);
  await first.waitForSelector('.top');
  for (const [at, folder] of process.argv.slice(2).entries()) {
    const page = at === 0 ? first : await context.newPage();
    page.setDefaultTimeout(STEP_MS);
    await page.goto(await projectUrl(folder));
    await page.waitForSelector('.react-flow__pane');
  }
  console.log('open');
  await new Promise(() => {});
})().catch((error) => {
  console.error(String(error?.stack ?? error));
  process.exit(1);
});
