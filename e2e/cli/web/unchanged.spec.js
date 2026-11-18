// unchanged (cl-1271; the launch walkthrough's W-S8): Render on a box whose take already holds its inputs makes nothing new, and says so.
// Paul's rulings: a box renders only itself, and a call with the very same inputs, seed included, is answered from what was made; a try
// that made nothing is a note, never silence and never standing state. project/tool.py's `plain` takes no seed: its box, rendered once
// from Focus, is rendered again. No run goes and no take is added, and within 5 s a note says why ("flat: nothing new to render: … takes no
// seed, so the same inputs always give the same result"), at 1600×1000 and again at 390×844 (kept in $OUT/web/unchanged/). `cozy render
// flat` says the same in place of a time.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, focus, fixtureTool } = require('./fixtures');

const PLAIN = `${fixtureTool('tool.py')}/plain`;
const SAID = /nothing new to render: .*takes no seed, so the same inputs always give the same result/;

test('unchanged', async ({ page, project, cli }) => {
  const shots = join(process.env.OUT, 'web', 'unchanged');
  mkdirSync(shots, { recursive: true });
  // The box's runs of `plain`, as `cozy run ls` lists them (its installs aside).
  const runs = async () => {
    const said = await cli('run', 'ls', '--limit', '0');
    return said.stdout.split('\n').filter((line) => /^\S+\s+\S*\/plain\s/.test(line)).length;
  };
  const takes = async () => (await project.asset('flat'))?.versions ?? 0;
  await project.add({ name: 'flat', tool: PLAIN, fields: { instructions: 'one colour' } });

  await test.step('rendered once from Focus: take 1', async () => {
    const region = await focus(page, 'flat');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(takes, { message: "flat's take 1", timeout: 300_000 }).toBe(1);
    await expect.poll(runs, { message: 'one run of plain', timeout: 30_000 }).toBe(1);
  });

  for (const [width, height] of [[1600, 1000], [390, 844]]) {
    await test.step(`Render again at ${width}×${height}: no run, no take, a note that says why`, async () => {
      await page.setViewportSize({ width, height });
      const region = await focus(page, 'flat');
      const notes = page.locator('.note').filter({ hasText: SAID });
      // The last one gone by itself first (a note goes after 6 s), so the one this press makes is counted alone.
      await expect(notes).toHaveCount(0, { timeout: 15_000 });
      await region.getByRole('button', { name: 'Render', exact: true }).click();
      await expect(notes, 'the note saying why nothing new was made').toHaveCount(1, { timeout: 5_000 });
      await expect(notes.last()).toContainText('flat: nothing new to render: ');
      await page.screenshot({ path: join(shots, `unchanged-${width}.png`) });
      await page.waitForTimeout(3_000);
      expect(await takes(), 'no new take').toBe(1);
      expect(await runs(), 'no run').toBe(1);
    });
  }

  await test.step('`cozy render flat` says the same in place of a time', async () => {
    const said = await cli('render', 'flat', '--on', 'local');
    expect(said.code, said.stderr).toBe(0);
    expect(said.stdout).toMatch(SAID);
    expect(said.stdout).not.toMatch(/done in/);
    expect(await takes()).toBe(1);
  });
});
