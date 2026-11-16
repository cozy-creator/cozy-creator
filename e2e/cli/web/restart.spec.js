// restart (cl-1067, solo): Cozy's own Chrome is gone (Paul, 11-16: "why does a restart close chrome tabs? don't do that"; of the private
// Chrome, "I don't want this anymore"). `cozy open` starts the default browser (entrypoints), and its tab is a tab like any. What's typed in
// an encrypted project as `cozy vault lock` runs is sent before it locks: the page hears `project.vault saving` and answers once it has, as
// Cozy's own browser was made to. A Calcifer restart leaves the tabs open: a plain project's reconnects and saves what's typed next, an
// encrypted one's shows the locked page with Unlock, and neither loads again.
const { randomBytes } = require('crypto');
const { dirname, join } = require('path');
const { test, expect, canvasIdle, canvasVisible, focus } = require('./fixtures');

test.use({ projectOptions: { starter: false, encrypted: true } });

const WAS = 'What it said before the lock.';
const FIRST = 'Typed as it locks: one two three';
const MORE = ' and more';
const PLAIN = 'Typed after the restart.';

/** The top bar's word on Calcifer: none once the page is back with it. */
const bar = (page) => page.locator('header.top .pill');

/** `words` typed over `name`'s words in Focus, the textbox left open. */
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
  await project.add({ name: 'words', fields: { text: WAS }, at: { x: 0, y: 0 } });
  await api.post(`/v1/projects/${encodeURIComponent(plain.id)}/assets`, { tool: '', exact: true, name: 'note', fields: { text: 'Plain before.' }, at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'words'), { timeout: 60_000 }).toBe(true);
  const second = await context.newPage();
  await signIn(second, `${api.origin}/p/${plain.code}`);
  await expect.poll(() => canvasVisible(second, 'note'), { timeout: 60_000 }).toBe(true);
  for (const tab of [page, second]) await tab.evaluate(() => { window.unreloaded = true; });

  // Refused on purpose: what the encrypted tab still asks of its project as it locks, and everything while Calcifer is away.
  guards.allow(new RegExp(`status of (409|423)\\b.*(/v1/projects/(${project.id}|${project.code})\\b|[?&]project=${project.id}\\b|/v1/events/)`));
  const away = new RegExp(`^console error: Failed to load resource: net::ERR_[A-Z_]+ ${api.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}/`);
  let stopped = false;
  guards.allow({ test: (line) => stopped && away.test(line) });

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
      await page.getByLabel('Passphrase', { exact: true }).fill(project.passphrase);
      await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
      await expect.poll(() => canvasVisible(page, 'words'), { message: 'the canvas is back', timeout: 60_000 }).toBe(true);
      const kept = (await project.asset('words'))?.fields?.text ?? '';
      expect(kept !== WAS && typed.startsWith(kept) && kept.length >= FIRST.length,
        `what was typed before the lock began is saved (${FIRST.length} characters at least): ${JSON.stringify(kept)}`).toBe(true);
      expect.soft(await page.evaluate(() => window.unreloaded), 'with no reload').toBe(true);
    });

    await test.step('a Calcifer restart leaves both tabs open, each where it was', async () => {
      stopped = true;
      await calciferControl.stop();
      await calciferControl.start();
      await expect(bar(second), "the plain project's tab is back with Calcifer").toHaveCount(0, { timeout: 60_000 });
      stopped = false;
      await expect(page.getByText('This project is locked.', { exact: true }), 'the encrypted tab shows the locked page').toBeVisible({ timeout: 60_000 });
      await expect(page.getByRole('button', { name: 'Unlock project', exact: true }), 'with Unlock').toBeVisible();
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
