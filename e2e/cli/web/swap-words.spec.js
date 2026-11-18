// swap-words (cl-1200, xs-215 row 2): a box's words go with it when its tool changes, from where its tool holds them to where the next one
// reads them, each tool's main text, whatever either calls the field (Paul, 11-17: "build general solutions, not hacks"). LTX-2.5's words
// are its `prompt`, MiniMax H3's its `summary`: H3 let go on an LTX-2.5 box from Tools › Video reads them as its summary, and LTX-2.5 back
// as its prompt. The tool line's X leaves them on the box, and H3 then put on it reads them again.
const { test, expect, canvasPoint, canvasVisible, focus } = require('./fixtures');
const { carry } = require('./geometry');
const { fit, leave, toolRow } = require('./hands');

const LTX = 'ltx-2.5/generate';
const H3 = 'minimax-h3/generate';
const WORDS = 'A lighthouse at dusk, waves breaking below it, gulls calling over the wind.';
const softly = expect.configure({ soft: true });

test('swap-words', async ({ page, project, api }) => {
  const tools = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools;
  const [ltx, h3] = [LTX, H3].map((id) => tools.find((tool) => tool.id === id));
  expect(ltx, 'the Hub lists LTX-2.5').toBeTruthy();
  expect(h3, "and MiniMax H3's Shot").toBeTruthy();
  // As each declares them: LTX-2.5's words a plain `prompt: str`, H3's a `summary` prompt.
  expect.soft(ltx.fields.find((field) => field.type === 'prompt' || field.type === 'words')?.field, "LTX-2.5's words").toBe('prompt');
  expect.soft(h3.fields.find((field) => field.type === 'prompt')?.field, "H3's words").toBe('summary');
  const fieldsOf = async () => (await project.asset('clip'))?.fields ?? {};
  const tool = async () => (await project.asset('clip'))?.tool || null;
  /** The box's words in Focus, in the field its tool reads them from. */
  const shown = async (field) => {
    const region = await focus(page, 'clip');
    await expect.soft(region.locator(`.mention-text[data-field="${field}"]`).first(), `Focus shows them as its ${field}`).toHaveText(WORDS);
    await leave(page);
  };
  /** `id` from Tools › Video let go on the box. */
  const put = async (id) => carry(page, await toolRow(page, 'Video', id), await canvasPoint(page, 'clip'));

  await project.add({ name: 'clip', tool: LTX, kind: 'video', fields: { prompt: WORDS }, at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'clip')).toBe(true);
  await fit(page);

  await test.step('H3 on the LTX-2.5 box reads its words as its summary', async () => {
    await put(H3);
    await expect.poll(tool, { message: 'H3 in its place' }).toBe(H3);
    await softly.poll(async () => (await fieldsOf()).summary ?? null, { message: 'its words, its summary' }).toBe(WORDS);
    expect.soft(await fieldsOf(), "and no LTX-2.5 prompt left over").not.toHaveProperty('prompt');
    await shown('summary');
  });

  await test.step('and LTX-2.5 back reads them as its prompt', async () => {
    await put(LTX);
    await expect.poll(tool, { message: 'LTX-2.5 back' }).toBe(LTX);
    await softly.poll(async () => (await fieldsOf()).prompt ?? null, { message: 'its words, its prompt' }).toBe(WORDS);
    expect.soft(await fieldsOf(), 'and no H3 summary left over').not.toHaveProperty('summary');
    await shown('prompt');
  });

  await test.step("the tool line's X leaves its words on the box, and H3 put on then reads them", async () => {
    const region = await focus(page, 'clip');
    await region.getByRole('button', { name: 'Remove video tool', exact: true }).click();
    await expect.poll(tool, { message: 'the tool is off' }).toBeNull();
    await softly.poll(async () => (await fieldsOf()).prompt ?? null, { message: 'its words stay' }).toBe(WORDS);
    await leave(page);
    await put(H3);
    await expect.poll(tool, { message: 'H3 on it' }).toBe(H3);
    await softly.poll(async () => (await fieldsOf()).summary ?? null, { message: 'its words, its summary' }).toBe(WORDS);
    await shown('summary');
  });
});
