// cut-1254 (cl-1254, cozy-creator #3950): the cut check's page leg, run by release-gate/sessions/cut-1254.sh with `local` on the Latest's
// pair, below this cozy's Runtime minor. A box of draw.py (a weightless still) on the cell's project, Render pressed in Focus: the box says
// "Updating local first" while local moves to this cozy's Runtime, then its picture lands. What it saw goes to $CUT_OUT/page.json.
const { writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, focus, canvasText } = require('./fixtures');
const { fit } = require('./hands');

const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: release-gate/sessions/cut-1254.sh sets it`); })();

test('a page render on local below this cozy\'s Runtime minor updates local first, then lands', async ({ page, project, cli }) => {
  const out = need('CUT_OUT');
  const name = 'from-page';
  const seen = { said: null, at_ms: null, take: null, failure: null, use: null, faces: [] };
  const record = () => writeFileSync(join(out, 'page.json'), JSON.stringify(seen, null, 2));
  await project.add({ name, tool: need('DRAW'), kind: 'image', fields: { instructions: `A still from the page ${Date.now()}` }, at: { x: 0, y: 0 } });
  // The project's machines: local, as a person picks it (`cozy machine use`); else Render goes where it finishes soonest, any rental of the
  // account's included.
  const use = await cli('machine', 'use', 'local');
  seen.use = { code: use.code, said: `${use.stdout}${use.stderr}`.trim().slice(-200) };
  expect(use.code, `cozy machine use local: ${seen.use.said}`).toBe(0);
  await fit(page);
  const region = await focus(page, name);
  const pressed = Date.now();
  await region.getByRole('button', { name: 'Render', exact: true }).click();
  const shown = async () => [
    await region.textContent().catch(() => ''),
    await page.evaluate(() => document.body.innerText).catch(() => ''),
    await canvasText(page, name).catch(() => ''),
  ].join('\n');
  const landed = async () => {
    const row = await project.asset(name);
    if (row?.failure) seen.failure = row.failure;
    return Boolean(row?.prefixes?.length) || Boolean(row?.failure);
  };
  await expect.poll(async () => {
    const text = await shown();
    // What the box and the page said as it went, each distinct line once (the record of what a person saw).
    for (const line of text.split('\n').map((l) => l.trim()).filter((l) => /updat|install|wait|prepar|queued|start/i.test(l))) {
      if (!seen.faces.includes(line) && seen.faces.length < 40) seen.faces.push(line);
    }
    const said = text.match(/updating local first[^\n]*|updating local's runtime[^\n]*/i);
    if (said && !seen.said) {
      Object.assign(seen, { said: said[0].slice(0, 200), at_ms: Date.now() - pressed });
      await page.screenshot({ path: join(out, 'page-updating.png') });
    }
    return Boolean(seen.said) || (await landed());
  }, { message: 'the box says "Updating local first", or its take or a failure comes first', timeout: 600_000, intervals: [250] }).toBe(true);
  await expect.poll(landed, { message: 'its take', timeout: 900_000, intervals: [1_000] }).toBe(true);
  seen.take = (await project.asset(name))?.prefixes?.at(-1) ?? null;
  await page.screenshot({ path: join(out, 'page-after.png') });
  record();
  expect(seen.failure, 'the render did not fail').toBeNull();
  expect(seen.said, 'the page said it updates local first').not.toBeNull();
  expect(seen.take?.main?.machine, 'it rendered on local').toBe('local');
  expect(seen.take?.main?.width ?? 0, 'its picture landed').toBeGreaterThan(0);
});
