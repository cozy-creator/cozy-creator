// stop (solo): Calcifer stops when told, as a service manager, `cozy down` or an update tells it, however busy (e2e 44098633751: SIGTERM 5 s
// after systemd started it again from a kill -9, with a page typing, it said `stopping` and was still running 5 min later, so no restart or
// update could follow). Each round: words typed in Focus, Calcifer killed with -9 under them and started again by its service manager, then
// SIGTERM a moment later as typing goes on: `stopped` in its health log within STOP_S, and the process gone.
const { readFileSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { test, expect, canvasVisible, focus } = require('./fixtures');

const ROUNDS = 6;
/** Calcifer's own deadline (serve.rs STOP_WITHIN, 15 s) and a margin. */
const STOP_S = 20;
const HEALTH = join(homedir(), '.cozy', 'calcifer', 'health.log');
const alive = (pid) => { try { process.kill(pid, 0); return true; } catch (error) { return error.code === 'EPERM'; } };
/** What Calcifer `pid` said of its stop: `stopped`, and the step a late one was on. */
const said = (pid) => {
  const lines = readFileSync(HEALTH, 'utf8').split('\n').filter((line) => line.includes(` pid=${pid} `));
  return { stopped: lines.some((line) => line.includes('event="stopped"')), late: lines.filter((line) => line.includes('event="stop_late"')).map((line) => line.match(/step="([^"]*)"/)?.[1]) };
};

test('stop', { tag: '@solo' }, async ({ context, project, api, signIn, guards, calciferControl }) => {
  test.setTimeout(ROUNDS * 120_000);
  await project.add({ name: 'words', fields: { text: 'Typed through stops.' }, at: { x: 0, y: 0 } });
  const page = await context.newPage();
  await signIn(page, project.url());
  await expect(page.locator('.stage')).toBeVisible();
  await expect.poll(() => canvasVisible(page, 'words')).toBe(true);
  // Calcifer away: what the page asks of it fails as the browser says in its console, on purpose, until the page is back with it.
  const origin = api.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const refused = new RegExp(`^console error: Failed to load resource: net::ERR_[A-Z_]+ ${origin}/`);
  let away = false;
  guards.allow({ test: (line) => away && refused.test(line) });
  const region = await focus(page, 'words');
  await region.getByRole('textbox', { name: 'Text' }).click();

  for (let round = 1; round <= ROUNDS; round++) {
    await test.step(`round ${round}: killed with -9 while typing, started again, then SIGTERM as typing goes on`, async () => {
      away = true;
      const typing = page.keyboard.type(` round ${round} one two three four five six seven eight nine ten`, { delay: 90 });
      await calciferControl.kill();
      // A moment into the new one, its start's work and the page's reconnect under way.
      await page.waitForTimeout(1_000 + round * 800);
      const { pid } = calciferControl.holder;
      const began = Date.now();
      process.kill(pid, 'SIGTERM');
      await expect.poll(() => said(pid).stopped && !alive(pid), { message: `Calcifer ${pid} stopped within ${STOP_S} s`, timeout: STOP_S * 1_000 }).toBe(true);
      const { late } = said(pid);
      console.log(`STOP round ${round} pid ${pid}: ${Date.now() - began} ms${late.length ? `, late on ${late.join(', ')}` : ''}`);
      expect.soft(late, `Calcifer ${pid}: no step of its stop past its deadline`).toEqual([]);
      await calciferControl.start();
      await typing;
      await expect(page.locator('header.top .pill'), 'the page back with it').toHaveCount(0, { timeout: 60_000 });
      away = false;
    });
  }
});
