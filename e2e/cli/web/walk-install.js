// walk-install.js <link> <out dir> [engine…]: the page a fresh install opens (`cozy open --json`'s link), as a new user first sees it, in each
// engine (chrome, webkit): screenshots at 1600x1000 and 390x844, the page's errors, and how long it took to show anything. walkthrough-install.yaml.
const { chromium, webkit } = require('@playwright/test');
const { mkdirSync, writeFileSync } = require('fs');
const { join } = require('path');

(async () => {
  const [link, out, ...engines] = process.argv.slice(2);
  mkdirSync(out, { recursive: true });
  const notes = [];
  for (const engine of engines.length ? engines : ['chrome']) {
    const browser = engine === 'webkit' ? await webkit.launch() : await chromium.launch({ channel: 'chrome' });
    const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
    page.on('pageerror', (e) => notes.push(`${engine} page error: ${e.message}`));
    page.on('console', (m) => { if (m.type() === 'error') notes.push(`${engine} console: ${m.text()}`); });
    page.on('response', (r) => { if (r.status() >= 400) notes.push(`${engine} HTTP ${r.status()} ${new URL(r.url()).pathname}`); });
    const t0 = Date.now();
    try {
      await page.goto(link, { timeout: 60_000 });
      await page.waitForSelector('.stage, ul.list, main, h1', { timeout: 60_000 }).catch(() => notes.push(`${engine}: nothing shown in 60 s`));
      notes.push(`${engine}: first view in ${Date.now() - t0} ms at ${page.url()}`);
      await page.waitForTimeout(4000);
      await page.screenshot({ path: join(out, `${engine}-1600.png`) });
      writeFileSync(join(out, `${engine}-1600.aria.txt`), await page.locator('body').ariaSnapshot().catch((e) => String(e)));
      await page.setViewportSize({ width: 390, height: 844 });
      await page.waitForTimeout(1500);
      await page.screenshot({ path: join(out, `${engine}-390.png`) });
      await page.goto(new URL('/projects', link).href).catch(() => {});
      await page.waitForTimeout(2500);
      await page.screenshot({ path: join(out, `${engine}-projects-390.png`) });
    } catch (error) {
      notes.push(`${engine}: ${error.message.split('\n')[0]}`);
      await page.screenshot({ path: join(out, `${engine}-failed.png`) }).catch(() => {});
    }
    await browser.close();
  }
  writeFileSync(join(out, 'notes.txt'), `${notes.join('\n')}\n`);
  console.log(notes.join('\n'));
})();
