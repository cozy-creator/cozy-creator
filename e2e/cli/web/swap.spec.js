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

const holder = () => JSON.parse(readFileSync(join(homedir(), '.cozy', 'calcifer', 'calcifer.lock.holder'), 'utf8'));

test('swap', { tag: '@solo' }, async ({ page, project, api, cli, signIn }) => {
  await signIn(page, project.url());
  await expect(page.locator('header.top'), 'the project page shows').toBeVisible({ timeout: 30_000 });
  // What the page says from here on, and whether it is still this document after: a reload loses `__swap`.
  await page.evaluate(() => {
    window.__swap = { offline: 0 };
    const look = () => {
      if ([...document.querySelectorAll('header.top .pill')].some((pill) => /offline/i.test(pill.textContent ?? ''))) window.__swap.offline++;
    };
    new MutationObserver(look).observe(document.body, { subtree: true, childList: true, characterData: true });
  });
  const was = holder();
  const restarted = await cli.json('restart');
  expect(restarted.restarted, 'cozy restart restarted Calcifer').toBe(true);
  const now = holder();
  expect(now.pid, 'another generation answers').not.toBe(was.pid);
  expect([now.address, now.token], 'at the same address, with the same bearer').toEqual([was.address, was.token]);
  // Past the page's 2 s quiet: a stream not back by then would say so.
  await page.waitForTimeout(3_000);
  expect(await page.evaluate(() => window.__swap?.offline), 'the same page, never offline').toBe(0);
  await expect(page.locator('header.top .pill').filter({ hasText: /offline|Trying again/ }), 'nothing left waiting').toHaveCount(0);
  if (!encrypted) return;
  expect(await api.get(project.at('/vault')), 'the project stayed unlocked').toMatchObject({ encrypted: true, state: 'unlocked' });
  // The view the next generation took locks as any does: its mount ends, and the project's folder can go.
  await api.post(project.at('/vault'), { action: 'lock' });
  await expect.poll(async () => (await api.get(project.at('/vault'))).state, { message: 'the carried view locked', timeout: 30_000 }).toBe('locked');
});
