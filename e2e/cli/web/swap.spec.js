// swap (cl-1113, solo): Paul, 11-16: "it should be overlapping (v1 → v2) so I never have downtime." `cozy restart` while a page shows
// an encrypted project, as every update restarts Calcifer: the page never says Cozy is offline, never reloads, and its project stays
// unlocked and takes an edit after.
const { readFileSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { test, expect } = require('./fixtures');

// Encrypted but on Windows, whose web lane has no WinFsp (as every native web cell's project is plain there): swap.py carries an
// encrypted view across a swap on Windows too.
const encrypted = process.platform !== 'win32';
test.use({ projectOptions: { encrypted } });

const SWAPS = 5;
const holder = () => JSON.parse(readFileSync(join(homedir(), '.cozy', 'calcifer', 'calcifer.lock.holder'), 'utf8'));

test('swap', { tag: '@solo' }, async ({ page, project, api, cli, signIn, guards }) => {
  // Every refused /v1 answer the page met, with its body: what a guard's console line can't say.
  const refused = [];
  page.on('response', async (response) => {
    if (response.status() < 400 || !new URL(response.url()).pathname.startsWith('/v1/')) return;
    const body = await response.text().catch(() => '');
    refused.push(`${response.status()} ${response.request().method()} ${new URL(response.url()).pathname} ${body.slice(0, 400)}`);
  });
  await signIn(page, project.url());
  await expect(page.locator('header.top'), 'the project page shows').toBeVisible({ timeout: 30_000 });
  // The control: the page's first reads, the burst a swap's reconnect makes, three times against one generation.
  for (let reload = 1; reload <= SWAPS; reload++) {
    await page.waitForTimeout(3_000);
    expect(refused, `reload ${reload - 1}, no swap: no answer the page met was refused`).toEqual([]);
    await page.reload();
    await expect(page.locator('header.top'), `reload ${reload}: the project page shows`).toBeVisible({ timeout: 30_000 });
  }
  await page.waitForTimeout(3_000);
  expect(refused, 'before any swap: no answer the page met was refused').toEqual([]);
  // What the page says from here on, and whether it is still this document after: a reload loses `__swap`.
  await page.evaluate(() => {
    window.__swap = { offline: 0 };
    const look = () => {
      if ([...document.querySelectorAll('header.top .pill')].some((pill) => /offline/i.test(pill.textContent ?? ''))) window.__swap.offline++;
    };
    new MutationObserver(look).observe(document.body, { subtree: true, childList: true, characterData: true });
  });
  // Three in a row, each a fresh chance at the page's first reads meeting the next generation.
  for (let swap = 1; swap <= SWAPS; swap++) {
    const was = holder();
    const restarted = await cli.json('restart');
    expect(restarted.restarted, `swap ${swap}: cozy restart restarted Calcifer`).toBe(true);
    const now = holder();
    expect(now.pid, `swap ${swap}: another generation answers`).not.toBe(was.pid);
    expect([now.address, now.token], `swap ${swap}: at the same address, with the same bearer`).toEqual([was.address, was.token]);
    // Past the page's 2 s quiet: a stream not back by then would say so.
    await page.waitForTimeout(3_000);
    expect(await page.evaluate(() => window.__swap?.offline), `swap ${swap}: the same page, never offline`).toBe(0);
    await expect(page.locator('header.top .pill').filter({ hasText: /offline|Trying again/ }), `swap ${swap}: nothing left waiting`).toHaveCount(0);
    expect(refused, `swap ${swap}: no answer the page met was refused`).toEqual([]);
  }
  if (!encrypted) return;
  expect(await api.get(project.at('/vault')), 'the project stayed unlocked').toMatchObject({ encrypted: true, state: 'unlocked' });
  // The view the next generation took locks as any does: its mount ends, and the project's folder can go. Locked while the page shows it,
  // its reads then are refused (as in restart.spec), and only then: the swap above allowed nothing.
  let locking = false;
  const refusedOnLock = new RegExp(`status of (409|423)\\b.*(/v1/projects/(${project.id}|${project.code})\\b|[?&]project=${project.id}\\b)`);
  guards.allow({ test: (line) => locking && refusedOnLock.test(line) });
  locking = true;
  await api.post(project.at('/vault'), { action: 'lock' });
  await expect.poll(async () => (await api.get(project.at('/vault'))).state, { message: 'the carried view locked', timeout: 30_000 }).toBe('locked');
});
