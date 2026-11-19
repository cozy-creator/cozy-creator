// tool-words (xs-215, hack audit web.md row 2, Calcifer's half): a box's tool changed through Calcifer (its PATCH, as `cozy apply` writes
// one) without its words carries them by each tool's main text, as the page does: H3's `summary`, LTX-2.5's `prompt`. Calcifer knew a box's
// words only as `text`, `summary` and `instructions`, so an H3 box given LTX-2.5 kept its words where LTX-2.5 doesn't read them.
const { test, expect } = require('./fixtures');

const H3 = 'minimax-h3/generate';
const LTX = 'ltx-2.5/generate';
const WORDS = 'A street at noon, people crossing.';

test('tool-words', async ({ project, api }) => {
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'H3 and LTX-2.5 listed', timeout: 120_000 })
    .toEqual(expect.arrayContaining([H3, LTX]));
  await project.add({ name: 'clip', tool: H3, kind: 'video', fields: { summary: WORDS }, at: { x: 0, y: 0 } });
  const fields = async () => (await project.asset('clip'))?.fields ?? {};

  await api.patch(project.at('/assets/clip'), { tool: LTX });
  const asLtx = await fields();
  expect.soft(asLtx.prompt, `H3 → LTX-2.5: its words are LTX-2.5's prompt (fields: ${JSON.stringify(asLtx)})`).toBe(WORDS);
  expect.soft(asLtx.summary ?? null, 'and no longer H3\'s summary').toBeNull();

  await api.patch(project.at('/assets/clip'), { tool: H3 });
  const asH3 = await fields();
  expect.soft(asH3.summary, `LTX-2.5 → H3: its words are H3's summary again (fields: ${JSON.stringify(asH3)})`).toBe(WORDS);

  await api.patch(project.at('/assets/clip'), { tool: '' });
  const off = await fields();
  expect.soft(off.summary, `its tool taken off a video box: its words stay (fields: ${JSON.stringify(off)})`).toBe(WORDS);
});
