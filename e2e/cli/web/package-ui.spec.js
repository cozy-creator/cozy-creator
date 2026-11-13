// package-ui (cl-973 core cell 21): a package's own component (crates/calcifer/docs/PACKAGE-UI.md) from its release: e2e-boxed, published
// to the isolated Hub at seed and installed on local by setup. It draws exactly its one field, served under the page's CSP; what it sets is
// saved and read back after a reload. Served under a hash nothing holds, or throwing as it draws, it leaves the declared control and says so.
// The box's other fields stay as declared.
const { test, expect, box, focus } = require('./fixtures');
const { fit, leave } = require('./hands');

const ONE = 'e2e-boxed/one_field';
const THROWS = 'e2e-boxed/throws';
const FELL_BACK = "This tool's own box couldn't be drawn, so this is its plain one";
// Its component, the first time: Calcifer reads it out of the release's wheel from the Hub.
const FIRST_MS = 60_000;

const asked = (id) => (url) => decodeURIComponent(url.pathname) === `/v1/tools/${id}/ui`;

test('package-ui', async ({ page, project, api, cli, guards }) => {
  // What this cell brings about on purpose: a module asked for under a hash nothing holds, and one that throws as React draws it.
  guards.allow(/status of 404\b.*\/v1\/tools\/[^ ]*\/ui\/b{64}\//, /thrown on purpose by the fixture/);
  await project.add({ name: 'shaded', tool: ONE, kind: 'image', fields: { instructions: 'a pebble' }, at: { x: 0, y: 0 } });
  await project.add({ name: 'thrown', tool: THROWS, kind: 'image', fields: { instructions: 'a stone' }, at: { x: 420, y: 0 } });
  for (const name of ['shaded', 'thrown']) await expect(box(page, name)).toBeVisible();
  await fit(page);
  // Its release on this machine, as `tool_ui_not_installed`'s remedy says: its component loads only from an installed release.
  const installed = await cli('--json', 'package', 'install', 'e2e-boxed', '--on', 'local');
  expect(installed.code, `cozy package install e2e-boxed: ${installed.stderr.slice(-400)}`).toBe(0);
  for (const id of [ONE, THROWS]) {
    await expect.poll(async () => (await api.raw('GET', `/v1/tools/${encodeURIComponent(id)}/ui?project=${encodeURIComponent(project.id)}`)).status, { message: `${id}'s component served`, timeout: FIRST_MS }).toBe(200);
  }
  const region = page.getByRole('region', { name: 'Focused asset' });
  const shade = region.locator('[data-boxed=shade]');
  const declared = region.getByRole('slider', { name: 'Shade', exact: true });
  const words = region.locator('.mention-text[data-field="instructions"]');
  const fields = async (name) => (await project.asset(name))?.fields;

  await test.step('its component draws its one field, and what it sets is saved', async () => {
    await focus(page, 'shaded');
    await expect(shade).toBeVisible({ timeout: FIRST_MS });
    await expect.soft(shade).toHaveAttribute('data-asset', 'shaded');
    // Its relative import is a module too: its steps are the field's declared range.
    await expect.soft(shade).toHaveAttribute('data-steps', '0,0.25,0.5,0.75,1');
    await expect.soft(region.locator('.tool-ui'), 'one component').toHaveCount(1);
    await expect.soft(declared, 'the declared Shade is what it replaced').toHaveCount(0);
    await expect.soft(words, 'the instructions as declared').toHaveCount(1);
    await shade.getByRole('button', { name: 'Shade 0.75', exact: true }).click();
    await expect(shade).toHaveAttribute('data-saved', 'true');
    await expect.poll(() => fields('shaded'), { message: 'the API holds its shade' }).toMatchObject({ instructions: 'a pebble', shade: 0.75 });
  });

  await test.step('a reload reads it back, its module under the page\'s CSP', async () => {
    const module = page.waitForResponse((answer) => /\/v1\/tools\/[^/]+\/ui\/[0-9a-f]{64}\/ui\/shade\.js$/.test(new URL(answer.url()).pathname));
    const loaded = await page.reload();
    await expect(region).toHaveAttribute('data-focused-asset', 'shaded');
    await expect(shade.getByRole('button', { name: 'Shade 0.75', exact: true })).toHaveAttribute('aria-pressed', 'true');
    const served = await module;
    expect.soft(served.status(), 'its module').toBe(200);
    expect.soft(await served.headerValue('content-type')).toBe('application/javascript');
    expect.soft(await served.headerValue('content-security-policy'), "the page's own CSP").toBe(await loaded.headerValue('content-security-policy'));
    await expect.soft(declared).toHaveCount(0);
  });

  await test.step('served under a hash nothing holds, the declared control stands, saying so', async () => {
    const faked = asked(ONE);
    await page.route(faked, async (route) => {
      const answer = await route.fetch();
      const { module } = await answer.json();
      await route.fulfill({ response: answer, json: { module: module.replace(/\/ui\/[0-9a-f]{64}\//, `/ui/${'b'.repeat(64)}/`) } });
    });
    await page.reload();
    await expect(region).toHaveAttribute('data-focused-asset', 'shaded');
    await expect(region.locator('.tool-ui-failed')).toHaveText(FELL_BACK);
    await expect.soft(region.locator('[data-boxed]'), 'nothing of the component').toHaveCount(0);
    await expect.soft(declared, 'its declared Shade, holding what was saved').toHaveValue('0.75');
    await expect.soft(words).toHaveCount(1);
    await page.unroute(faked);
  });

  await test.step('one that throws as it draws leaves the declared box, saying so', async () => {
    await leave(page);
    await focus(page, 'thrown');
    await expect(region.locator('.tool-ui-failed')).toHaveText(FELL_BACK, { timeout: FIRST_MS });
    await expect.soft(region.locator('.tool-ui-failed'), 'why, on hover').toHaveAttribute('title', /thrown on purpose by the fixture/);
    await expect.soft(region.locator('[data-boxed]')).toHaveCount(0);
    await expect.soft(declared, 'its declared Shade at its default').toHaveValue('0.5');
    await expect.soft(words).toHaveCount(1);
  });

  await test.step('the other fields are unchanged', async () => {
    expect.soft(await fields('shaded')).toMatchObject({ instructions: 'a pebble', shade: 0.75 });
    expect.soft(await fields('thrown')).toMatchObject({ instructions: 'a stone' });
    expect.soft(await fields('thrown'), 'nothing set on the box that threw').not.toHaveProperty('shade');
  });
});
