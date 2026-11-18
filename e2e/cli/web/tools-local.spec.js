// tools-local (coordinator, 11-17: "answer at once with what's known, `tools.changed` when a local read lands"): a project's first tool list
// after a box names local code no one has read yet answers at once, never after that code's reading (the web-3 lane's palette stood empty
// up to 15 s while the list waited on one, e2e 44103425048). The open page then hears the reading land and lists its tool, unreloaded.
const { randomBytes } = require('crypto');
const { readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, fixtureTool } = require('./fixtures');

/** At once: the list's own work, never a reading's (seconds). */
const AT_ONCE_MS = 1_000;

test('tools-local', async ({ page, project, api }) => {
  test.setTimeout(240_000);
  // The tool cells' package, its tree new to this computer: no reading of it is kept.
  const file = join(project.path, `fresh_${randomBytes(4).toString('hex')}.py`);
  writeFileSync(file, `${readFileSync(fixtureTool('tool.py'), 'utf8')}\n# ${randomBytes(8).toString('hex')}\n`);
  const tool = `${file}/still`;
  const listed = (answer) => answer.tools.some((one) => one.local && one.id === tool);
  // What the page lists, as it asks.
  const pageLists = [];
  page.on('response', async (response) => {
    if (!/\/v1\/tools\?/.test(response.url()) || response.request().method() !== 'GET') return;
    pageLists.push(await response.json().then(listed, () => false));
  });

  await project.add({ name: 'fresh', tool, kind: 'image', fields: { instructions: 'a lighthouse' }, at: { x: 0, y: 0 } });
  const began = Date.now();
  const first = await api.get(`/v1/tools?on=local&project=${encodeURIComponent(project.id)}`);
  const took = Date.now() - began;
  console.log(`TOOLS first list after the box: ${took} ms, its tool listed: ${listed(first)}`);
  expect.soft(took, `the first list answers within ${AT_ONCE_MS} ms`).toBeLessThan(AT_ONCE_MS);

  await expect.poll(() => pageLists.some(Boolean), { message: 'the open page lists the tool once its reading lands, unreloaded', timeout: 180_000 }).toBe(true);
  const again = Date.now();
  expect(listed(await api.get(`/v1/tools?on=local&project=${encodeURIComponent(project.id)}`)), 'listed from then on').toBe(true);
  expect.soft(Date.now() - again, 'a list of what is known answers at once').toBeLessThan(AT_ONCE_MS);
});
