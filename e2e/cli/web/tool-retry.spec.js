// tool-retry (cl-1110, vault): a lock in one project left another tab's top bar saying "Trying again, attempt 5/5" for ever (found by
// web-restart, cl-1067, run 43970433251). The lock retired its project in every tab, and that cancelled every tool read, whichever
// project it was for; one cancelled while it waited to try again never cleared its pill. Here a plain project's tab reads its tools
// while Calcifer can't be reached for them, and another, encrypted project locks during the read's last wait: the read is the plain
// project's, so the lock leaves it alone, and the pill clears once it lands.
const { randomBytes } = require('crypto');
const { dirname, join } = require('path');
const { test, expect } = require('./fixtures');

/** The tab's read of its tools, which is Calcifer's tool list. */
const tools = (url) => url.pathname === '/v1/tools';

test('tool-retry', { tag: '@vault' }, async ({ context, project, api, signIn, guards, removeAfter }) => {
  const sealed = await api.post('/v1/projects', { path: join(dirname(project.path), `web-tool-retry-${randomBytes(4).toString('hex')}`), passphrase: randomBytes(18).toString('base64url') });
  removeAfter(sealed.path);
  // Refused on purpose: the tool reads this cell keeps from reaching Calcifer.
  guards.allow(/net::ERR_CONNECTION_REFUSED .*\/v1\/tools\?/);

  const page = await context.newPage();
  await page.route(tools, (route) => route.abort('connectionrefused'));
  await signIn(page, project.url());
  const pill = page.locator('header.top .pill');
  // Its fifth try waits 4 s: the lock lands inside that wait, and the tools answer once it ends.
  await expect(pill.filter({ hasText: 'Trying again, attempt 5/5' }), 'the tool read waits to try a fifth time').toBeVisible({ timeout: 30_000 });
  await page.unroute(tools);
  await api.post(`/v1/projects/${encodeURIComponent(sealed.id)}/vault`, { action: 'lock' });
  await expect.poll(async () => (await api.get(`/v1/projects/${encodeURIComponent(sealed.id)}/vault`)).state, { message: 'the other project locked' }).toBe('locked');
  await expect(pill, 'no "Trying again" is left once the read lands').toHaveCount(0, { timeout: 15_000 });
});
