// tools-told (xs-215, hack audit web.md row 7): the page's tool list is told, not guessed. A project's list holds every tool its boxes
// name, so boxes of tools taken off your list are known at once, with no search per box (the page asked `?q=` for each). And local code
// edited between renders is told (`tools.changed`) and its new function listed, while a render of the same code reads the list no more
// (the page read it again after every run it ended). @solo: it takes tools off this Calcifer's list.
const { randomUUID } = require('crypto');
const { appendFileSync, copyFileSync, mkdirSync } = require('fs');
const { dirname, join } = require('path');
const { test, expect, canvasVisible, fixtureTool } = require('./fixtures');
const { fit } = require('./hands');

/** Long enough for a read the page would make on what just happened to have gone out. */
const QUIET_MS = 5_000;

test('tools-told', async ({ page, project, api }) => {
  const reads = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname === '/v1/tools') reads.push({ at: Date.now(), q: url.searchParams.get('q') ?? '' });
  });
  const listFor = async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id);
  const published = (await listFor()).filter((id) => !id.startsWith('cozy/') && !id.startsWith('/') && !id.includes('.py'));
  const off = published.slice(0, 3);
  expect(off.length, `three published tools listed (${published.join(', ')})`).toBe(3);

  try {
    await test.step('boxes of three tools taken off your list: in the project\'s list, with no search per box', async () => {
      await api.post('/v1/tools/list', { remove: off });
      for (const [i, tool] of off.entries()) await project.add({ name: `off_${i + 1}`, tool, at: { x: i * 420, y: 0 } });
      expect.soft(await listFor(), 'the project\'s list holds every tool its boxes name, taken off or not').toEqual(expect.arrayContaining(off));
      await page.reload();
      for (const i of [1, 2, 3]) await expect.poll(() => canvasVisible(page, `off_${i}`), { timeout: 60_000 }).toBe(true);
      await fit(page);
      await page.waitForTimeout(QUIET_MS);
      expect.soft(reads.filter((read) => read.q), `no search per box (the page's reads: ${JSON.stringify(reads)})`).toEqual([]);
    });

    await test.step('local code edited between renders: the change told and listed; a render of the same code reads the list no more', async () => {
      // A copy of the fixture, as code a person is writing: the cells' own file stays as it is.
      const source = join(test.info().outputPath('local'), 'tool.py');
      mkdirSync(dirname(source), { recursive: true });
      copyFileSync(fixtureTool('tool.py'), source);
      await project.add({ name: 'still', tool: `${source}/still`, kind: 'image', fields: { instructions: 'A still.' }, at: { x: 0, y: 420 } });
      const versions = async () => ((await project.asset('still'))?.prefixes ?? []).length;
      // The list reads from the render's ask until a while after its take lands: an install's `tools.changed` comes before its call.
      const render = async (n) => {
        const asked = Date.now();
        await api.call('POST', project.at('/runs'), { names: ['still'], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
        await expect.poll(versions, { message: `render ${n} landed`, timeout: 240_000 }).toBe(n);
        await page.waitForTimeout(QUIET_MS);
        return reads.filter((read) => !read.q && read.at >= asked).length;
      };
      await render(1);
      // A function more in its code, as a person adds one: the render after it keeps a new reading, which is told.
      appendFileSync(source, '\n\n@app.tool(label="Another still", named="another")\ndef another(payload: StillBox) -> Picture:\n    """`still` again, added by the cell."""\n    return still(payload)\n');
      const edited = await render(2);
      expect.soft(edited, 'its code changed: the list read again (told by tools.changed)').toBeGreaterThan(0);
      expect.soft(await listFor(), 'and its new function listed').toEqual(expect.arrayContaining([`${source}/another`]));
      const same = await render(3);
      expect.soft(same, `the same code rendered again: the list not read again (the page's reads: ${JSON.stringify(reads)})`).toBe(0);
    });
  } finally {
    // All of it again (`POST /v1/tools/list` with nothing named), as this Calcifer's other cells find it.
    await api.post('/v1/tools/list', {});
  }
});
