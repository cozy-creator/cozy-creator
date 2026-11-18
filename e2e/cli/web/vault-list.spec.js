// vault-list (cl-1090, xs-203's user flow; Paul 11-16: "open webui, decrypt project"): an encrypted project, locked, is listed by its id
// and code (cl-1171), opened from the Projects list at its address, `/p/<code>`, and unlocked there: no "can't be opened", and it stays
// where it was. Locked again, the index keeps nothing of it but its folder (cl-984). `@vault`: on the job with the filesystem driver.
const { execFileSync } = require('child_process');
const { dirname, join } = require('path');
const { test, expect } = require('./fixtures');

/** Calcifer's index as it is now, read-only: each project's id (its first 12), code and folder. */
const index = () => JSON.parse(execFileSync('python3', ['-I', '-c', [
  'import json, sqlite3, sys',
  'db = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)',
  'print(json.dumps(db.execute("SELECT substr(id, 1, 12), code, path FROM projects").fetchall()))',
].join('\n'), join(dirname(process.env.HOLDER), 'state.db')]).toString());

test.use({ projectOptions: { starter: false, encrypted: true } });

test('vault-list', { tag: '@vault' }, async ({ page, project, api, cli, guards }) => {
  const vault = () => api.get(project.at('/vault'));
  const locked = await cli('--json', 'vault', '--directory', project.path, 'lock');
  expect(locked.code, `cozy vault lock: ${locked.stdout.slice(-300)} ${locked.stderr.slice(-300)}`).toBe(0);
  await expect.poll(async () => (await vault()).state, { message: 'locked', timeout: 60_000 }).toBe('locked');
  const listed = (await api.get('/v1/projects?limit=1000')).projects.find((row) => row.path === project.path);
  expect(listed, 'listed by its id and code').toMatchObject({ id: project.id, code: project.code });
  // Refused on purpose: what the page asks of it before it has read that it's locked.
  guards.allow(new RegExp(`status of 409\\b.*/v1/projects/(${project.id}|${project.code})\\b`));

  await page.goto(`${api.origin}/projects`);
  await page.locator('ul.list').locator(`a[title=${JSON.stringify(project.path)}]`).click();
  await expect(page, 'the list opens it at its address').toHaveURL(`${api.origin}/p/${project.code}`);
  await expect(page.getByText('This project is locked.', { exact: true }), 'it says it is locked').toBeVisible({ timeout: 30_000 });
  await page.getByLabel('Passphrase', { exact: true }).fill(project.passphrase);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await expect.poll(async () => (await vault()).state, { message: 'Calcifer has it unlocked', timeout: 60_000 }).toBe('unlocked');
  await expect(page.locator('.stage'), 'its canvas shows').toBeVisible({ timeout: 30_000 });
  await expect(page, 'it stays where it was').toHaveURL(`${api.origin}/p/${project.code}`);
  await expect(page.getByText('This project is locked.', { exact: true }), 'no longer covered').toHaveCount(0);
  await expect(page.getByText(/couldn't open|can't be opened/i), 'never "can\'t be opened"').toHaveCount(0);

  const again = await cli('--json', 'vault', '--directory', project.path, 'lock');
  expect.soft(again.code, `locked again before its folder goes: ${again.stderr.slice(-300)}`).toBe(0);
  await expect.poll(async () => (await vault()).state, { message: 'locked again', timeout: 60_000 }).toBe('locked');
  // cl-984: once locked, the index keeps its folder under a handle, and neither its id nor its code.
  const shut = index().filter(([, , path]) => path === project.path);
  expect(shut.map(([id, code]) => [id.startsWith('locked-'), code]), `its folder's row: ${JSON.stringify(shut)}`).toEqual([[true, '']]);
});
