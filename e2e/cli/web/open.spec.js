// open (cl-973 core cell 1): `cozy open`'s link signs a tab in once, under the page's CSP, with no cookie, its stream on its own session,
// through a reload; used again, in a browser of its own, it's refused in words until that browser has a code of its own; and a new
// project from the projects page opens at its address, unencrypted.
const { randomBytes } = require('crypto');
const { test, expect } = require('./fixtures');

const redeeming = (answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/session';
const streaming = (request) => request.method() === 'GET' && new URL(request.url()).pathname === '/v1/events';
/** Signed in at `project`: its tab, the open one, over the canvas. */
const signedIn = (page, project) => page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: project.name, exact: true });
const gate = (page) => page.getByRole('heading', { name: "This page isn't signed in" });

test('open', { tag: '@engines' }, async ({ context, newContext, project, api, signIn, guards, removeAfter }) => {
  const page = await context.newPage();
  // A code as `cozy open` asks the holder for one, at the project's address.
  const { code } = await api.post('/v1/session-codes', {});
  const link = `${project.url()}#code=${encodeURIComponent(code)}`;
  let token;

  await test.step("cozy open's link signs this tab in", async () => {
    const redeemed = page.waitForResponse(redeeming);
    const streamed = page.waitForRequest(streaming);
    await page.goto(link);
    const session = await redeemed;
    expect(session.status(), 'POST /v1/session').toBe(200);
    ({ token } = await session.json());
    await expect(page, 'the code leaves the address').toHaveURL(project.url());
    await expect(signedIn(page, project)).toHaveAttribute('aria-current', 'page');
    const stream = await streamed;
    expect.soft(await stream.headerValue('authorization'), "the page's stream is on its own session").toBe(`Bearer ${token}`);
    const answer = await stream.response();
    expect.soft(answer?.status(), 'GET /v1/events').toBe(200);
    expect.soft(await answer?.headerValue('content-type')).toMatch(/^text\/event-stream/);
    expect.soft(await context.cookies(), 'cookies').toEqual([]);
  });

  await test.step('a reload stays signed in', async () => {
    const streamed = page.waitForRequest(streaming);
    await page.reload();
    await expect(signedIn(page, project)).toHaveAttribute('aria-current', 'page');
    expect.soft(await (await streamed).headerValue('authorization'), 'the same session').toBe(`Bearer ${token}`);
    await expect.soft(gate(page)).toHaveCount(0);
  });

  const other = await (await newContext()).newPage();
  await test.step('the used link, in a browser of its own, is refused in words', async () => {
    // Refused on purpose: Chrome says each in its console.
    guards.allow(/status of 400\b.*\/v1\/session$/, /status of 401\b.*\/v1\/status$/);
    const refused = other.waitForResponse(redeeming);
    await other.goto(link);
    const answer = await refused;
    expect.soft(answer.status(), 'a used code').toBe(400);
    expect.soft((await answer.json()).error?.code).toBe('session_code_invalid');
    await expect(gate(other)).toBeVisible();
    await expect.soft(other.getByText('cozy open', { exact: true })).toBeVisible();
    await expect.soft(signedIn(other, project)).toHaveCount(0);
  });

  await test.step('a code of its own signs it in', async () => {
    const session = await signIn(other, project.url());
    expect.soft(session.token, "its own session, not the first tab's").not.toBe(token);
    await expect(signedIn(other, project)).toHaveAttribute('aria-current', 'page');
    await expect.soft(gate(other)).toHaveCount(0);
  });

  await test.step('New project, on the projects page', async () => {
    const name = `web-open-made-${randomBytes(3).toString('hex')}`;
    // The open project's tab opens Projects, ready for a new one's name.
    await signedIn(page, project).click();
    const field = page.getByLabel("New project's name");
    await expect(field).toBeFocused();
    await field.fill(name);
    await page.getByLabel('Create without encryption').check();
    const made = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/projects');
    await page.getByRole('button', { name: 'Create project' }).click();
    const answer = await made;
    expect(answer.status(), 'POST /v1/projects').toBe(200);
    const opened = await answer.json();
    removeAfter(opened.path);
    await expect(page, 'it opens at its address').toHaveURL(`${api.origin}/p/${opened.code}`);
    expect.soft(opened.name, 'named as asked').toBe(name);
    await expect(signedIn(page, opened)).toHaveAttribute('aria-current', 'page');
    const listed = (await api.get('/v1/projects?limit=1000')).projects.find((row) => row.id === opened.id);
    expect.soft(listed, 'the API lists it').toMatchObject({ name, code: opened.code });
    expect.soft(await api.get(`/v1/projects/${encodeURIComponent(opened.id)}/vault`), 'unencrypted').toMatchObject({ encrypted: false });
  });
});
