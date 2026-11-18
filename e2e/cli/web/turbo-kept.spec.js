// turbo-kept (cl-1200, xs-215 row 1): the page writes nothing the person didn't set, and draws H3's fields as it declares them, like any
// tool's (Paul, 11-17: "build general solutions, not hacks"; "do not change the design language between tools for no reason"). An H3 box
// given `turbo: false` by `cozy apply` keeps it through the page's Render: the box still holds it, and the render asks for it. In Focus
// Turbo shows off, with Quality and Steps beside it; kept on paper and dark.
const { mkdirSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, canvasIdle, canvasVisible, focus } = require('./fixtures');
const { fit } = require('./hands');

const H3 = 'minimax-h3/generate';
const RUN_MS = 120_000;

test('turbo-kept', async ({ page, project, api, cli }, testInfo) => {
  const shots = join(process.env.OUT, 'web', 'turbo-kept');
  mkdirSync(shots, { recursive: true });
  mkdirSync(testInfo.outputDir, { recursive: true });
  const h3 = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.find((tool) => tool.id === H3);
  expect(h3, "the Hub lists H3's Shot").toBeTruthy();
  const declared = Object.fromEntries(h3.fields.map((field) => [field.field, field]));
  expect.soft(declared.turbo, 'H3 declares Turbo, on unless set').toMatchObject({ type: 'toggle', default: true });
  expect.soft(declared.steps?.advanced, 'and Steps Advanced()').toBe(true);
  const fieldsOf = async () => (await project.asset('shot'))?.fields ?? {};

  await project.add({ name: 'shot', tool: H3, kind: 'video', fields: { summary: 'A harbour at dusk.', beats: [{ description: 'Boats come in.', duration: 5 }] }, at: { x: 0, y: 0 } });
  await test.step('`cozy apply` sets its turbo false', async () => {
    const got = await cli.json('get', 'shot');
    const block = Object.keys(got.nodes.shot).find((key) => key.includes('/'));
    expect(block, `its tool block: ${JSON.stringify(got.nodes.shot)}`).toBeTruthy();
    // The node as read, Turbo off: what the file changes is all that's written.
    const file = testInfo.outputPath('shot.json');
    writeFileSync(file, JSON.stringify({ shot: { rev: got.rev, ...got.nodes.shot, [block]: { ...got.nodes.shot[block], turbo: false } } }));
    const applied = await cli.json('apply', '-f', file);
    expect(applied.applied, `cozy apply: ${JSON.stringify(applied.errors ?? applied.conflicts)}`).toBe(true);
    await expect.poll(async () => (await fieldsOf()).turbo, { message: 'the box holds turbo false' }).toBe(false);
  });

  await expect.poll(() => canvasVisible(page, 'shot')).toBe(true);
  await fit(page);
  let region = await focus(page, 'shot');
  const turbo = () => region.getByRole('switch', { name: 'Turbo', exact: true });

  await test.step('Focus draws Turbo, Quality and Steps as H3 declares them, Turbo off; kept on paper and dark', async () => {
    await expect.soft(turbo(), 'Turbo, a switch').toBeVisible();
    await expect.soft(turbo(), 'off, as the CLI set it').not.toBeChecked();
    await expect.soft(region.getByRole('radiogroup', { name: 'Quality', exact: true }), 'Quality').toBeVisible();
    await expect.soft(region.getByRole('combobox', { name: 'Steps', exact: true }).or(region.getByRole('radiogroup', { name: 'Steps', exact: true })), 'Steps').toBeVisible();
    for (const theme of ['paper', 'dark']) {
      await project.look(theme);
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, theme);
      region = page.getByRole('region', { name: 'Focused asset' });
      await expect(region).toHaveAttribute('data-focused-asset', 'shot');
      // Its settings, below its prompts: at the top, then scrolled to.
      const settings = region.locator('.edit-parameters').first();
      await expect(settings).toBeVisible();
      await canvasIdle(page);
      await page.screenshot({ path: join(shots, `focus-${theme}.png`) });
      await settings.scrollIntoViewIfNeeded();
      await page.screenshot({ path: join(shots, `focus-settings-${theme}.png`) });
      await settings.screenshot({ path: join(shots, `settings-${theme}.png`) });
    }
    await project.look(null);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    region = page.getByRole('region', { name: 'Focused asset' });
    await expect(region).toHaveAttribute('data-focused-asset', 'shot');
  });

  await test.step("the page's Render keeps turbo false, and the render asks for it", async () => {
    const runs = async () => ((await api.get(project.at('/runs?limit=50'))).runs ?? []).filter((run) => String(run.target ?? '').includes(H3));
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    let run;
    await expect.poll(async () => (run = (await runs())[0])?.number ?? null, { message: 'its render', timeout: RUN_MS }).not.toBeNull();
    try {
      expect.soft((await fieldsOf()).turbo, 'the box still holds turbo false').toBe(false);
      // Its record as it stands, whatever it has come to (`--once` exits 1 on a failed run).
      const watched = await cli('--json', '--full', 'run', 'watch', String(run.number), '--once');
      const input = (() => { try { return JSON.parse(watched.stdout).request?.input; } catch { return undefined; } })();
      expect.soft(input?.turbo, `the render's request: ${watched.stdout.slice(0, 400)}${watched.stderr.slice(-300)}`).toBe(false);
      await expect.soft(turbo(), 'Focus still shows Turbo off').not.toBeChecked();
    } finally {
      // It needs a GPU this computer hasn't got, and would install H3 to run on the CPU: ended as soon as it's looked at.
      await cli('run', 'cancel', String(run.number));
      await expect.poll(async () => (await runs()).find((one) => one.number === run.number)?.status, { message: 'its render ended', timeout: RUN_MS }).toMatch(/^(canceled|failed|completed)$/);
    }
    await page.waitForTimeout(1000);
    expect.soft((await fieldsOf()).turbo, 'and holds it after').toBe(false);
  });
});
