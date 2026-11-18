// restart (cl-1067, solo): Cozy's own Chrome is gone (Paul, 11-16: "why does a restart close chrome tabs? don't do that"; of the private
// Chrome, "I don't want this anymore"). `cozy open` starts a private window of the user's browser (entrypoints), a tab like any. What's typed in
// an encrypted project as `cozy vault lock` runs is sent before it locks: the page hears `project.saving` and answers once it has, as
// Cozy's own browser was made to. A Calcifer restart leaves the tabs open: a plain project's reconnects and saves what's typed next, an
// encrypted one's shows the locked page with Unlock, and neither loads again.
const { randomBytes } = require('crypto');
/** How long a tab is busy once it has answered the stop's ask: past the next Calcifer's start. */
const BUSY_MS = 2500;
const { dirname, join } = require('path');
const { test, expect, canvasIdle, canvasVisible, focus } = require('./fixtures');

test.use({ projectOptions: { starter: false, encrypted: true } });

const WAS = 'What it said before the lock.';
const FIRST = 'Typed as it locks: one two three';
const MORE = ' and more';
const PLAIN = 'Typed after the restart.';

/** The top bar's word on Calcifer: none once the page is back with it. */
const bar = (page) => page.locator('header.top .pill');

/** Every box in view. */
async function fit(page) {
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
}

/** `name`'s words in Focus, all selected, to be typed over. */
async function typeInto(page, name) {
  const region = await focus(page, name);
  const text = region.getByRole('textbox', { name: 'Text' });
  await text.click();
  await page.keyboard.press('ControlOrMeta+a');
  return region;
}

test('restart', { tag: '@solo' }, async ({ context, page, project, api, cli, signIn, guards, calciferControl, removeAfter }) => {
  const plain = await api.post('/v1/projects', { path: join(dirname(project.path), `web-restart-plain-${randomBytes(4).toString('hex')}`), unencrypted: true });
  removeAfter(plain.path);
  await project.add({ name: 'words', kind: 'text', fields: { text: WAS }, at: { x: 0, y: 0 } });
  await api.post(`/v1/projects/${encodeURIComponent(plain.id)}/assets`, { tool: '', exact: true, name: 'note', kind: 'text', fields: { text: 'Plain before.' }, at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'words'), { timeout: 60_000 }).toBe(true);
  await fit(page);
  const second = await context.newPage();
  await signIn(second, `${api.origin}/p/${plain.code}`);
  await expect.poll(() => canvasVisible(second, 'note'), { timeout: 60_000 }).toBe(true);
  await fit(second);
  for (const tab of [page, second]) await tab.evaluate(() => { window.unreloaded = true; });

  // Refused on purpose: what the encrypted tab still asks of its project as it locks, and everything while Calcifer is away.
  guards.allow(new RegExp(`status of (409|423)\\b.*(/v1/projects/(${project.id}|${project.code})\\b|[?&]project=${project.id}\\b|/v1/events/)`));
  const away = new RegExp(`^console error: Failed to load resource: net::ERR_[A-Z_]+ ${api.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}/`);
  let stopped = false;
  guards.allow({ test: (line) => stopped && away.test(line) });
  // A stopping Calcifer admits no write: the tab's word of what it shows, as its project locks for the stop.
  guards.allow({ test: (line) => stopped && /status of 503\b.*\/v1\/events\//.test(line) });

  try {
    await test.step('what is typed as cozy locks the project is saved first', async () => {
      await typeInto(page, 'words');
      // Never a 300 ms pause, so nothing is written by itself: the lock begins mid-word, and the typing goes on until the tab covers.
      await page.keyboard.type(FIRST, { delay: 60 });
      const locking = cli('--json', 'vault', 'lock');
      const covered = page.getByText('This project is locked.', { exact: true });
      let typed = FIRST;
      for (const until = Date.now() + 60_000; Date.now() < until && !(await covered.isVisible());) {
        const next = MORE[(typed.length - FIRST.length) % MORE.length];
        await page.keyboard.type(next, { delay: 40 });
        typed += next;
      }
      const said = await locking;
      expect(said.code, `cozy vault lock: ${said.stdout.slice(-300)} ${said.stderr.slice(-300)}`).toBe(0);
      await expect(covered, 'the tab covered').toBeVisible({ timeout: 60_000 });
      // Unlocked as `cozy vault unlock` does, and what it kept read before the tab is asked anything.
      await api.post(project.at('/vault'), { action: 'unlock', passphrase: project.passphrase });
      const kept = (await project.asset('words'))?.fields?.text ?? '';
      expect(kept !== WAS && typed.startsWith(kept) && kept.length >= FIRST.length,
        `what was typed before the lock began is saved (${FIRST.length} characters at least): ${JSON.stringify(kept)}`).toBe(true);
      await expect.poll(() => canvasVisible(page, 'words'), { message: 'the tab shows its canvas again', timeout: 60_000 }).toBe(true);
      expect.soft(await page.evaluate(() => window.unreloaded), 'with no reload').toBe(true);
    });

    await test.step('a Calcifer restart leaves both tabs open, each where it was', async () => {
      stopped = true;
      // Each tab busy once it has answered the stop's ask for what was typed, as a page on a slow computer is: what Calcifer said last
      // (its project locked, its stream's end) is read once the next Calcifer is up. Telling that one what it shows on the stream that
      // ended, a tab was answered 404, which the browser logs (e2e 44073749095).
      await Promise.all([page, second].map((tab) => tab.evaluate((ms) => {
        const real = window.fetch.bind(window);
        window.fetch = async (input, init) => {
          const answer = await real(input, init);
          if (/^\/v1\/events\/[^/]+\/saved$/.test(new URL(input instanceof Request ? input.url : String(input), location.href).pathname)) {
            const until = performance.now() + ms;
            while (performance.now() < until);
          }
          return answer;
        };
      }, BUSY_MS)));
      await calciferControl.stop();
      await calciferControl.start();
      await expect(bar(second), "the plain project's tab is back with Calcifer").toHaveCount(0, { timeout: 60_000 });
      await expect(page.getByText('This project is locked.', { exact: true }), 'the encrypted tab shows the locked page').toBeVisible({ timeout: 60_000 });
      await expect(page.getByRole('button', { name: 'Unlock project', exact: true }), 'with Unlock').toBeVisible();
      // The stop's window closes once both tabs are past their busy spell: a busy tab shows no pill, so the bar's count alone passed
      // before either had heard of the stop, and the encrypted tab's 503 from the stopping Calcifer was said after (e2e 44217308449).
      for (const tab of [page, second]) await tab.evaluate(() => new Promise((done) => setTimeout(done, 0)));
      stopped = false;
      expect((await api.get(project.at('/vault'))).state, 'Calcifer locked it as it stopped').toBe('locked');
      for (const [tab, which] of [[page, 'the encrypted tab'], [second, 'the plain tab']]) {
        expect.soft(await tab.evaluate(() => window.unreloaded), `${which} didn't load again`).toBe(true);
      }
      await typeInto(second, 'note');
      await second.keyboard.type(PLAIN);
      await second.keyboard.press('Escape');
      await canvasIdle(second);
      await expect.poll(async () => (await api.get(`/v1/projects/${encodeURIComponent(plain.id)}`)).assets.find((row) => row.name === 'note')?.fields.text,
        { message: "the plain tab's edit lands on the new Calcifer", timeout: 30_000 }).toBe(PLAIN);
    });
  } finally {
    guards.over();
    if ((await api.get(project.at('/vault'))).state !== 'locked') await cli('--json', 'vault', 'lock');
  }
});
