// palette-tools (coordinator, 11-17: "a palette whose local tools flash to 'none' every few seconds is a 0.26 regression a user sees at
// once"): once a project's local tool is read, the palette offers it soon after every load and never says there's none again, sampled
// every 100 ms. Loaded again and again, as a theme or a size changes; and while renders end one after another and the list takes seconds
// to answer, as on a busy computer: each ending asked the list again and aborted the read going, so a slow list never landed and the
// palette said "No LLM tool available" for as long (web-llm, e2e 44088809798).
const { randomUUID } = require('crypto');
const { test, expect, fixtureTool } = require('./fixtures');

const LLM = fixtureTool('llm.py/ask');
const STILL = `${fixtureTool('tool.py')}/still`;
const LOADS = [['paper', 1440, 900], ['dark', 1920, 1080], ['paper', 1920, 1080]];
const EVERY_MS = 100;
/** How long the list takes to answer while renders end. */
const SLOW_MS = 4000;

/** What the palette offers for an LLM, every EVERY_MS for `watch` ms: the tool (`there`), a group with none (`none`), or nothing yet. */
const sample = (page, watch) => page.evaluate(async ({ watch, every }) => {
  const said = [];
  for (const until = performance.now() + watch; performance.now() < until;) {
    const tools = document.querySelector('aside[aria-label="Tools"]');
    const group = tools?.querySelector('button[data-operation="llm"]');
    const row = tools?.querySelector('li[data-implementation][data-tool-operation="llm"]');
    said.push(row ? 'there' : group?.disabled ? 'none' : group ? 'there' : 'nothing');
    await new Promise((done) => setTimeout(done, every));
  }
  return said;
}, { watch, every: EVERY_MS });
const judged = (samples) => {
  const first = samples.indexOf('there');
  return { shown_ms: first < 0 ? null : first * EVERY_MS, flashes: first < 0 ? 0 : samples.slice(first).filter((one) => one !== 'there').length };
};

test('palette-tools', async ({ page, project, api }) => {
  await project.add({ name: 'asker', tool: LLM, kind: 'text', fields: { instructions: 'Say hello.' }, at: { x: 0, y: 0 } });
  await project.add({ name: 'still', tool: STILL, kind: 'image', fields: { instructions: 'a lighthouse' }, at: { x: 400, y: 0 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === LLM),
    { message: 'the project lists its local tool', timeout: 180_000 }).toBe(true);
  const render = async (i) => {
    await api.call('PATCH', project.at('/assets/still'), { fields: { instructions: `a lighthouse ${i}` } });
    await api.call('POST', project.at('/runs'), { names: ['still'], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
  };
  await render(0);
  await expect.poll(async () => (await project.asset('still'))?.versions ?? 0, { message: 'a render of still ends', timeout: 120_000 }).toBeGreaterThan(0);

  await test.step('loaded again and again: offered within 3 s of each load, and never none again', async () => {
    const seen = [];
    for (const [theme, width, height] of LOADS) {
      await project.look(theme);
      await page.setViewportSize({ width, height });
      await page.reload();
      seen.push({ theme, width, ...judged(await sample(page, 6000)) });
    }
    expect.soft(seen.every((load) => load.shown_ms !== null && load.shown_ms <= 3000 && load.flashes === 0), JSON.stringify(seen)).toBe(true);
  });

  await test.step(`while renders end and the list takes ${SLOW_MS / 1000} s: offered as soon as it answers, and never none again`, async () => {
    await page.route('**/v1/tools?*', async (route) => {
      await new Promise((done) => setTimeout(done, SLOW_MS));
      await route.continue().catch(() => {});
    });
    let rendering = true;
    const renders = (async () => { for (let i = 1; rendering; i++) { await render(i).catch(() => {}); await new Promise((done) => setTimeout(done, 700)); } })();
    await page.reload();
    const got = judged(await sample(page, SLOW_MS * 3));
    rendering = false;
    await renders;
    await page.unroute('**/v1/tools?*');
    expect.soft(got.shown_ms !== null && got.shown_ms <= SLOW_MS + 2000 && got.flashes === 0, JSON.stringify(got)).toBe(true);
  });
});
