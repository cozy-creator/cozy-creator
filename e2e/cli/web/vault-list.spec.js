// vault-list (cl-1090, xs-203's user flow; Paul 11-16: "open webui, decrypt project"): an encrypted project, locked, opened from the
// Projects list under its `locked-…` handle and unlocked there, opens at its own address: no "can't be opened", and nothing more is asked
// under the handle (its tool reads and dropped files answered 404). `@vault`: on the job with the filesystem driver.
const { execFileSync } = require('child_process');
const { dirname, join } = require('path');
const { test, expect } = require('./fixtures');

/** Calcifer's index as it is now, read-only: its handles (`project_handles`, cl-1090) and where each project is (an id's first 12). */
const index = () => JSON.parse(execFileSync('python3', ['-I', '-c', [
  'import json, sqlite3, sys',
  'db = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)',
  'print(json.dumps({"handles": db.execute("SELECT handle, path FROM project_handles").fetchall(), "projects": db.execute("SELECT substr(id, 1, 12), path FROM projects").fetchall()}))',
].join('\n'), join(dirname(process.env.HOLDER), 'state.db')]).toString());
const handlesOf = (snapshot, folder) => snapshot.handles.filter(([, path]) => path === folder).map(([handle]) => handle);

test.use({ projectOptions: { starter: false, encrypted: true } });

test('vault-list', { tag: '@vault' }, async ({ page, project, api, cli, guards }) => {
  const vault = () => api.get(project.at('/vault'));
  const locked = await cli('--json', 'vault', '--directory', project.path, 'lock');
  expect(locked.code, `cozy vault lock: ${locked.stdout.slice(-300)} ${locked.stderr.slice(-300)}`).toBe(0);
  await expect.poll(async () => (await vault()).state, { message: 'locked', timeout: 60_000 }).toBe('locked');
  const listed = (await api.get('/v1/projects?limit=1000')).projects.find((row) => row.path === project.path);
  expect(listed?.id, 'listed under its locked handle').toMatch(/^locked-/);
  // Refused on purpose: what the page asks of it before it has read that it's locked.
  guards.allow(new RegExp(`status of 409\\b.*/v1/projects/${listed.id}\\b`));
  guards.allow(new RegExp(`status of 409\\b.*/v1/projects/(${project.id}|${project.code})\\b`));

  let opened = null; // once it's at its own address
  const underHandle = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (opened !== null && `${url.pathname}${url.search}`.includes(listed.id)) underHandle.push(`${request.method()} ${url.pathname}${url.search}`);
  });
  await page.goto(`${api.origin}/projects/${listed.id}`);
  await expect(page.getByText('This project is locked.', { exact: true }), 'it says it is locked').toBeVisible({ timeout: 30_000 });
  await page.getByLabel('Passphrase', { exact: true }).fill(project.passphrase);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await expect.poll(async () => (await vault()).state, { message: 'Calcifer has it unlocked', timeout: 60_000 }).toBe('unlocked');
  await expect(page, 'it goes on to its own address').toHaveURL(new RegExp(`/p/${project.code}(?:[/?#]|$)`), { timeout: 30_000 });
  opened = Date.now();
  await expect(page.locator('.stage'), 'its canvas shows').toBeVisible({ timeout: 30_000 });
  await expect(page.getByText('This project is locked.', { exact: true }), 'no longer covered').toHaveCount(0);
  await expect(page.getByText(/couldn't open|can't be opened/i), 'never "can\'t be opened"').toHaveCount(0);
  await page.waitForTimeout(3000);
  expect(underHandle, 'nothing is asked under the locked handle once it opened').toEqual([]);
  const open = index();
  expect.soft(handlesOf(open, project.path), `its handle names it while it is unlocked: ${project.path} in ${JSON.stringify(open)}`).toEqual([listed.id]);

  const again = await cli('--json', 'vault', '--directory', project.path, 'lock');
  expect.soft(again.code, `locked again before its folder goes: ${again.stderr.slice(-300)}`).toBe(0);
  await expect.poll(async () => (await vault()).state, { message: 'locked again', timeout: 60_000 }).toBe('locked');
  // cl-984: after a lock, nothing in the index pairs a handle it had with its folder.
  const shut = index();
  expect(handlesOf(shut, project.path), `no handle row for its folder once locked: ${JSON.stringify(shut)}`).toEqual([]);
});
