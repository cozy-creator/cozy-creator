// project-settings (cl-1097). Paul, 11-16: "in the top right panel, the hamburger menu, there shoudl be some sort of per-project specific
// settings modal you can open up ... that will allow you to turn on or off encryption, rename the project, and also delete the project,
// from one location", and "I think the theme + settings can be global, and then have project overrides". Project settings is a drawer from
// the menu. Everywhere's appearance (Settings) changes a project with none of its own; one with its own keeps it; back to "Same as
// everywhere", it follows again. A rename renames its folder and shows in every open page. Encrypted then decrypted, its file is as it was.
// Deleted, its folder is in the trash, it leaves the index, and an open page says it's gone; a locked one deletes with no passphrase.
// Each drawer at 1440×900 and 1920×1080, paper and dark, kept to look at. `@solo`: it changes this Calcifer's appearance, every cell's.
const { existsSync, mkdirSync, readFileSync, rmSync } = require('fs');
const { homedir } = require('os');
const { basename, dirname, join } = require('path');
const { randomBytes, createHash } = require('crypto');
const { test, expect, themed, png } = require('./fixtures');

test.use({ projectOptions: { encrypted: true } });

const LOOKS = [
  { width: 1440, height: 900, theme: 'paper' },
  { width: 1440, height: 900, theme: 'dark' },
  { width: 1920, height: 1080, theme: 'paper' },
  { width: 1920, height: 1080, theme: 'dark' },
];
const TRASH = join(process.env.XDG_DATA_HOME || join(homedir(), '.local', 'share'), 'Trash');

test('project-settings', { tag: '@solo' }, async ({ page, project, api, cli, removeAfter, newContext, signIn, guards }) => {
  const shots = join(process.env.OUT, 'web', 'project-settings');
  mkdirSync(shots, { recursive: true });
  const drawer = page.locator('aside.drawer');
  const menu = page.locator('details.main-menu');
  const pick = async (name) => {
    if ((await menu.getAttribute('open')) === null) await menu.locator('summary').click();
    await menu.locator('nav').getByRole('link', { name, exact: true }).click();
  };
  const looks = (theme) => api.call('PUT', '/v1/appearance', { theme });
  const index = async () => (await api.get('/v1/projects?limit=1000')).projects;
  const make = async (word) => {
    const folder = join(process.env.OUT, 'web', 'projects', `web-settings-${word}-${randomBytes(3).toString('hex')}`);
    removeAfter(folder);
    return api.post('/v1/projects', { path: folder, unencrypted: true });
  };
  const at = (one, rest = '') => `/v1/projects/${encodeURIComponent(one.id)}${rest}`;
  const open = async (one, where = page) => {
    await where.goto(`${api.origin}/p/${one.code}`);
    await expect(where.locator('.stage')).toBeVisible();
  };
  const fileOf = async (one, name) => (await api.get(at(one))).assets.find((asset) => asset.name === name);
  const bytesOf = async (one, path) => Buffer.from(await (await api.raw('GET', at(one, `/files/${basename(path)}`))).arrayBuffer());
  const trashed = [];

  try {
    const plain = await make('look');
    const other = await make('other');
    // What a page still asks of a project as it's renamed, converted, locked or trashed is refused, as the browser says in its console.
    const theirs = [plain, other, project].flatMap((one) => [one.id, one.code]).join('|');
    guards.allow(new RegExp(`status of (404|409|423|503)\\b.*(/v1/projects/(${theirs})\\b|[?&]project=(${theirs})\\b|/v1/events/)`));

    await test.step('appearance: everywhere, a project of its own, and back', async () => {
      await looks('paper');
      await open(plain);
      await themed(page, 'paper');
      await pick('Settings');
      await expect(drawer.locator('.drawer-head b', { hasText: 'Settings' })).toBeVisible();
      await drawer.getByRole('radio', { name: 'Dark', exact: true }).click();
      await themed(page, 'dark');
      expect.soft(await api.get('/v1/appearance'), 'everywhere is dark').toMatchObject({ theme: 'dark' });

      await pick('Project settings');
      await expect(drawer.locator('.drawer-head b', { hasText: 'Project settings' })).toBeVisible();
      await drawer.getByRole('radio', { name: 'Its own', exact: true }).click();
      await drawer.getByRole('group', { name: "This project's own appearance" }).getByRole('radio', { name: 'Paper', exact: true }).click();
      await themed(page, 'paper');
      await expect.poll(async () => (await api.get(at(plain))).appearance, { message: 'kept in the project' }).toMatchObject({ theme: 'paper' });
      await drawer.getByRole('radiogroup', { name: 'Landscape' }).getByRole('radio', { name: 'Hidden', exact: true }).click();
      await expect.poll(async () => (await api.get(at(plain))).appearance?.options?.landscape).toBe('hidden');

      // Everywhere changes: one with its own keeps it, one without follows.
      await looks('dark');
      await page.waitForTimeout(500);
      await themed(page, 'paper');
      await open(other);
      await themed(page, 'dark');
      await open(plain);
      await themed(page, 'paper');

      await pick('Project settings');
      await drawer.getByRole('radio', { name: 'Same as everywhere', exact: true }).click();
      await themed(page, 'dark');
      await expect.poll(async () => (await api.get(at(plain))).appearance ?? null, { message: 'its own gone' }).toBeNull();
      await looks('paper');
      await themed(page, 'paper');
      await page.keyboard.press('Escape');
    });

    await test.step('rename: its folder is renamed, and every open page shows it', async () => {
      const second = await (await newContext()).newPage();
      await signIn(second, `${api.origin}/p/${plain.code}`);
      await expect(second.locator('.stage')).toBeVisible();
      const name = `web-settings-renamed-${randomBytes(3).toString('hex')}`;
      const next = join(dirname(plain.path), name);
      removeAfter(next);
      await open(plain);
      await pick('Project settings');
      const field = drawer.getByRole('textbox', { name: 'Project name' });
      await expect(field).toHaveValue(basename(plain.path));
      await field.fill(name);
      await drawer.getByRole('button', { name: 'Rename', exact: true }).click();
      // Renamed, or what refused it, said in the drawer.
      const refused = drawer.locator('.failure');
      await expect.poll(async () => ((await refused.count()) ? await refused.innerText() : existsSync(next) && !existsSync(plain.path) ? 'renamed' : 'renaming'),
        { message: 'its folder renamed', timeout: 60_000 }).toBe('renamed');
      await expect.poll(async () => (await index()).find((row) => row.id === plain.id), { message: 'the index' }).toMatchObject({ name, path: next });
      for (const [which, tab] of [['this page', page], ['another page', second]]) {
        await expect.soft(tab.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name, exact: true }), `${which}: its tab`).toBeVisible();
      }
      await expect.soft(second.locator('.unread, .gate'), 'the other page still has it open').toHaveCount(0);
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      expect.soft((await api.get(at(plain))).path, 'it opens where it is now').toBe(next);
      plain.path = next;
      await second.context().close();
    });

    await test.step('encrypted, then decrypted: its file is as it was', async () => {
      const bytes = Buffer.concat([png(97), randomBytes(16)]);
      await api.post(at(other, '/imports'), { source: await upload(api, other, bytes), name: 'kept', exact: true });
      const before = await fileOf(other, 'kept');
      await open(other);
      await pick('Project settings');
      const passphrase = randomBytes(18).toString('base64url');
      await drawer.getByLabel('Passphrase', { exact: true }).fill(passphrase);
      await drawer.getByLabel('Confirm passphrase', { exact: true }).fill(passphrase);
      await drawer.getByRole('button', { name: 'Encrypt project', exact: true }).click();
      const saving = drawer.getByRole('region', { name: 'Save recovery secret' });
      await expect(saving).toBeVisible({ timeout: 120_000 });
      await saving.getByLabel('I saved this somewhere away from this computer').check();
      await saving.getByRole('button', { name: 'Done', exact: true }).click();
      await expect.poll(async () => (await api.get(at(other, '/vault'))).state, { timeout: 60_000 }).toBe('unlocked');
      expect.soft(existsSync(join(other.path, 'project.db')), 'no plain project.db').toBe(false);
      expect.soft((await fileOf(other, 'kept'))?.sha256, 'its file, encrypted').toBe(before.sha256);
      await drawer.locator('summary', { hasText: /^Decrypt project$/ }).click();
      await drawer.getByRole('button', { name: 'Decrypt project', exact: true }).click();
      await expect.poll(async () => (await api.get(at(other, '/vault'))).state, { timeout: 120_000 }).toBe('plain');
      const after = await fileOf(other, 'kept');
      expect.soft(after?.sha256, 'its file, decrypted').toBe(before.sha256);
      expect.soft(createHash('sha256').update(await bytesOf(other, after.path)).digest('hex'), 'its bytes').toBe(createHash('sha256').update(bytes).digest('hex'));
      await page.keyboard.press('Escape');
    });

    await test.step('screenshots: Settings and Project settings, plain and encrypted, paper and dark', async () => {
      for (const { width, height, theme } of LOOKS) {
        const tag = `${width}x${height}-${theme}`;
        await looks(theme);
        await page.setViewportSize({ width, height });
        await page.goto(`${api.origin}/p/${plain.code}/settings`);
        await expect(drawer.locator('.drawer-head b', { hasText: 'Settings' })).toBeVisible();
        await themed(page, theme);
        await page.screenshot({ path: join(shots, `settings-${tag}.png`) });
        await page.goto(`${api.origin}/p/${plain.code}/project-settings`);
        await expect(drawer.getByRole('button', { name: 'Encrypt project', exact: true })).toBeVisible();
        await page.screenshot({ path: join(shots, `project-plain-${tag}.png`) });
        await drawer.getByRole('button', { name: 'Delete project', exact: true }).click();
        await page.screenshot({ path: join(shots, `project-delete-${tag}.png`) });
        await page.goto(`${project.url()}/project-settings`);
        await expect(drawer.getByRole('button', { name: 'Lock project', exact: true })).toBeVisible();
        await page.screenshot({ path: join(shots, `project-encrypted-${tag}.png`) });
      }
      await looks('paper');
      await page.setViewportSize({ width: 1600, height: 1000 });
    });

    await test.step('delete: in the trash, out of the index, and an open page says it went', async () => {
      const second = await (await newContext()).newPage();
      await signIn(second, `${api.origin}/p/${other.code}`);
      await expect(second.locator('.stage')).toBeVisible();
      await open(other);
      await pick('Project settings');
      await drawer.getByRole('button', { name: 'Delete project', exact: true }).click();
      await expect.soft(drawer.locator('.confirm .says'), 'it names the folder').toHaveText(`Delete this project? Its folder ${other.path} moves to the Trash.`);
      await drawer.locator('.confirm').getByRole('button', { name: 'Delete project', exact: true }).click();
      await expect.poll(async () => ((await drawer.locator('.failure').count()) ? await drawer.locator('.failure').innerText() : new URL(page.url()).pathname),
        { message: 'deleted, and the page goes home', timeout: 60_000 }).toBe('/');
      await expect.soft(page.locator('.notes .note', { hasText: `Moved ${basename(other.path)} to the Trash.` })).toBeVisible();
      const landed = join(TRASH, 'files', basename(other.path));
      trashed.push(landed, join(TRASH, 'info', `${basename(other.path)}.trashinfo`));
      expect.soft(existsSync(other.path), 'its folder moved').toBe(false);
      expect.soft(existsSync(join(landed, 'project.db')), 'into the trash, whole').toBe(true);
      expect.soft(readFileSync(join(TRASH, 'info', `${basename(other.path)}.trashinfo`), 'utf8'), 'with where it was').toContain(`Path=${other.path}`);
      expect.soft((await index()).map((row) => row.id), 'it leaves the index').not.toContain(other.id);
      await expect.soft(second.locator('.unread h2', { hasText: `This project's folder is gone: ${other.path}` }), 'the open page says so').toBeVisible();
      await second.context().close();
    });

    await test.step('a locked project deletes with no passphrase', async () => {
      const said = await cli('--json', 'vault', '--directory', project.path, 'lock');
      expect(said.code, `cozy vault lock: ${said.stderr.slice(-300)}`).toBe(0);
      await page.goto(`${project.url()}/project-settings`);
      await expect(drawer.getByRole('button', { name: 'Unlock project', exact: true }), 'the unlock right there').toBeVisible();
      await expect.soft(drawer.getByText('Unlock the project to change how it looks.')).toBeVisible();
      await drawer.getByRole('button', { name: 'Delete project', exact: true }).click();
      await page.screenshot({ path: join(shots, 'project-locked-delete-1600x1000-paper.png') });
      await drawer.locator('.confirm').getByRole('button', { name: 'Delete project', exact: true }).click();
      await expect.poll(async () => ((await drawer.locator('.failure').count()) ? await drawer.locator('.failure').innerText() : new URL(page.url()).pathname),
        { message: 'deleted, and the page goes home', timeout: 60_000 }).toBe('/');
      trashed.push(join(TRASH, 'files', basename(project.path)), join(TRASH, 'info', `${basename(project.path)}.trashinfo`));
      expect.soft(existsSync(project.path), 'its folder moved').toBe(false);
      expect.soft(existsSync(join(TRASH, 'files', basename(project.path))), 'into the trash').toBe(true);
      expect.soft((await index()).some((row) => row.path === project.path), 'it leaves the index').toBe(false);
    });
  } finally {
    await looks('paper').catch(() => {});
    if (existsSync(project.path) && (await api.get(`/v1/projects/${encodeURIComponent(project.id)}/vault`).catch(() => ({ state: 'locked' }))).state !== 'locked') {
      await cli('--json', 'vault', '--directory', project.path, 'lock');
    }
    for (const path of trashed) rmSync(path, { recursive: true, force: true });
  }
  guards.over();
});

/** Bytes held in `one`'s own content store: their `sha256:`. */
async function upload(api, one, bytes) {
  const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
  await api.call('PUT', `/v1/project-files/${digest}?project=${encodeURIComponent(one.id)}`, bytes, { type: 'application/octet-stream' });
  return digest;
}
