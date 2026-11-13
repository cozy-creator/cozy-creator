// vault (cl-973 core cell 22): a project encrypted from the page, its recovery secret saved, then a picture dropped and downloaded; outside
// its mount the folder holds no plaintext copy of it. `cozy` locks it while a second tab is open: every tab is covered at once, and neither
// the DOM, Web Storage, Cache Storage nor IndexedDB keeps a name or a picture. Unlocked with the passphrase and locked from the page, then
// unlocked with the recovery secret, each time the canvas back with no reload; decrypted, its files and history are kept.
// `@vault`: on the job with the filesystem driver, sharing its Calcifer, so it touches only its own projects.
const { randomBytes, createHash } = require('crypto');
const { existsSync, readFileSync, readdirSync, realpathSync } = require('fs');
const { join, relative } = require('path');
const { test: base, expect, box, canvasIdle, middle, png } = require('./fixtures');

const test = base.extend({
  /** Encrypted projects the cell made ({id, path}): each locked again before its folder goes, as a person locks one before deleting it. */
  vaults: async ({ api, cli }, use) => {
    const made = [];
    await use(made);
    for (const { id, path } of made) {
      const at = `/v1/projects/${encodeURIComponent(id)}/vault`;
      const { encrypted, state } = await api.get(at);
      if (!encrypted || state === 'locked') continue;
      await cli('--json', 'vault', '--directory', path, 'lock');
      await expect.poll(async () => (await api.get(at)).state, { message: `${path} locked before it's removed`, timeout: 60_000 }).toBe('locked');
    }
  },
});

/** Files as a drop delivers them, at `at` on the canvas. */
async function drop(page, files, { x, y }) {
  const transfer = await page.evaluateHandle((list) => {
    const data = new DataTransfer();
    for (const file of list) data.items.add(new File([Uint8Array.from(atob(file.b64), (c) => c.charCodeAt(0))], file.name, { type: file.type }));
    return data;
  }, files.map((file) => ({ name: file.name, type: file.type, b64: file.bytes.toString('base64') })));
  for (const type of ['dragenter', 'dragover', 'drop']) await page.dispatchEvent('.stage', type, { dataTransfer: transfer, clientX: x, clientY: y });
}

/** Each regular file under `root`, links not followed and `mount` left out: [{path, name, bytes}]. A file gone while it's read is no file. */
function filesUnder(root, mount) {
  const skip = mount ? realpathSync(mount) : null;
  const found = [];
  const there = (read) => {
    try {
      return read();
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      return null;
    }
  };
  const walk = (dir) => {
    for (const entry of there(() => readdirSync(dir, { withFileTypes: true })) ?? []) {
      const path = join(dir, entry.name);
      if (entry.isDirectory() && realpathSync(path) !== skip) walk(path);
      else if (entry.isFile()) {
        const bytes = there(() => readFileSync(path));
        if (bytes) found.push({ path: relative(root, path), name: entry.name, bytes });
      }
    }
  };
  walk(root);
  return found;
}

/** A picture's file by its first bytes: PNG, JPEG, GIF, WebP, AVIF or HEIC. Encrypted storage is none of them. */
const pictureFile = (bytes) => [[0x89, 0x50, 0x4e, 0x47], [0xff, 0xd8, 0xff], [0x47, 0x49, 0x46, 0x38]].some((sign) => sign.every((byte, i) => bytes[i] === byte))
  || (bytes.subarray(0, 4).toString('latin1') === 'RIFF' && bytes.subarray(8, 12).toString('latin1') === 'WEBP')
  || ['ftypavif', 'ftypheic'].includes(bytes.subarray(4, 12).toString('latin1'));

/** What a tab keeps of the project, by the label of each of `words` ({label: text}) it holds: boxes drawn, the words in its DOM, pictures
 *  it shows from Calcifer, and what Web Storage, Cache Storage and IndexedDB hold of them. Nothing, once it's locked. */
const keptIn = (tab, words) => tab.evaluate(async (words) => {
  const has = (text) => Object.entries(words).filter(([, word]) => String(text).includes(word)).map(([label]) => label);
  const storage = (area, kind) => Object.keys(area).flatMap((key) => has(`${key}=${area.getItem(key)}`).map((label) => `${kind} ${key}: ${label}`));
  const cached = [];
  for (const name of await caches.keys()) {
    for (const request of await (await caches.open(name)).keys()) {
      if (/\/v1\/project|[?&]project=/.test(request.url) || has(request.url).length) cached.push(`${name}: ${new URL(request.url).pathname}`);
    }
  }
  const asked = (request) => new Promise((done, failed) => {
    request.onsuccess = () => done(request.result);
    request.onerror = () => failed(request.error);
  });
  const databases = [];
  for (const { name } of await indexedDB.databases()) {
    const db = await asked(indexedDB.open(name));
    for (const store of db.objectStoreNames) {
      const values = await asked(db.transaction(store).objectStore(store).getAll());
      const binary = values.some((value) => value instanceof Blob || value instanceof ArrayBuffer || ArrayBuffer.isView(value));
      for (const label of [...has(JSON.stringify(values)), ...(binary ? ['bytes'] : [])]) databases.push(`${name}/${store}: ${label}`);
    }
    db.close();
  }
  return {
    boxes: document.querySelectorAll('.react-flow, .react-flow__node, [data-asset]').length,
    dom: has(document.documentElement.outerHTML).filter((label) => label !== 'the project id'),
    pictures: [...document.querySelectorAll('[src], [srcset], [poster]')]
      .map((el) => el.getAttribute('src') ?? el.getAttribute('srcset') ?? el.getAttribute('poster'))
      .filter((src) => /^blob:|\/v1\//.test(src)),
    webStorage: [...storage(localStorage, 'localStorage'), ...storage(sessionStorage, 'sessionStorage')],
    cacheStorage: cached,
    indexedDB: databases,
  };
}, words);
const NOTHING = { boxes: 0, dom: [], pictures: [], webStorage: [], cacheStorage: [], indexedDB: [] };

test('vault', { tag: '@vault' }, async ({ page, project, api, cli, signIn, guards, removeAfter, vaults }) => {
  const name = `web-vault-${randomBytes(3).toString('hex')}`;
  const passphrase = randomBytes(18).toString('base64url');
  const picture = `pic_${randomBytes(3).toString('hex')}`;
  // Its own bytes past the picture's end, found nowhere else: what a plaintext copy would hold.
  const tail = randomBytes(32);
  const bytes = Buffer.concat([png(23), tail]);
  const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
  const markers = [tail, Buffer.from(picture)];
  const holds = (file) => markers.some((marker) => file.bytes.includes(marker) || file.name.includes(marker.toString('latin1')));
  let made, recovery, words;
  const at = (rest = '') => `/v1/projects/${encodeURIComponent(made.id)}${rest}`;
  const vault = () => api.get(at('/vault'));
  const assetOf = async (asked) => (await api.get(at())).assets.find((row) => row.name === asked);
  /** What step 1 left on this document: gone if the page loaded again. */
  const kept = () => page.evaluate(() => window.__vaultDocument);

  await test.step('an encrypted project, made from the page', async () => {
    await page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: project.name, exact: true }).click();
    const field = page.getByLabel("New project's name");
    await expect(field).toBeFocused();
    await field.fill(name);
    await expect.soft(page.getByLabel('Create without encryption'), 'encrypted unless asked').not.toBeChecked();
    await page.getByLabel('Passphrase', { exact: true }).fill(passphrase);
    await page.getByLabel('Confirm passphrase', { exact: true }).fill(passphrase);
    const answered = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/projects', { timeout: 60_000 });
    await page.getByRole('button', { name: 'Create project', exact: true }).click();
    const answer = await answered;
    expect(answer.ok(), `POST /v1/projects: ${answer.status()}`).toBe(true);
    expect.soft(answer.headers()['cache-control'] ?? '', 'the answer with its recovery secret is never cached').toContain('no-store');
    ({ recovery_secret: recovery, ...made } = await answer.json());
    removeAfter(made.path);
    vaults.push(made);
    words = { "the picture's name": picture, 'the passphrase': passphrase, 'the recovery secret': recovery, 'the project id': made.id };
    expect(typeof recovery === 'string' && recovery.startsWith('cozy1-') && recovery.length > 70, 'a recovery secret, said once').toBe(true);

    const saving = page.getByRole('region', { name: 'Save recovery secret' });
    await expect(saving.locator('pre')).toBeVisible({ timeout: 60_000 });
    expect.soft((await saving.locator('pre').innerText()).trim() === recovery, 'the page shows the recovery secret it was given').toBe(true);
    await saving.getByLabel('I saved this somewhere away from this computer').check();
    await saving.getByRole('button', { name: 'Done', exact: true }).click();
    await expect(page, 'it opens at its address').toHaveURL(`${api.origin}/p/${made.code}`);
    await expect(saving).toHaveCount(0);
    await expect(page.locator('.stage')).toBeVisible();
    expect.soft(await vault(), 'encrypted and unlocked, locking after 30 idle minutes').toMatchObject({ encrypted: true, state: 'unlocked', idle_timeout_s: 1800 });
    expect.soft(existsSync(join(made.path, 'project.db')), 'no plaintext project.db in its folder').toBe(false);
    expect.soft((await keptIn(page, { 'the passphrase': passphrase, 'the recovery secret': recovery })).webStorage, 'Web Storage holds neither secret').toEqual([]);
    await page.evaluate(() => { window.__vaultDocument = 'kept'; });
  });

  await test.step('a picture dropped, then downloaded', async () => {
    const writes = [];
    const written = (request) => {
      const url = new URL(request.url());
      if (request.method() === 'PUT' && /^\/v1\/(project-)?files\//.test(url.pathname)) writes.push({ route: url.pathname.split('/').slice(0, 3).join('/'), project: url.searchParams.get('project') });
    };
    page.on('request', written);
    const viewport = page.viewportSize();
    const imported = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === at('/imports'), { timeout: 60_000 });
    await drop(page, [{ name: 'secret.png', type: 'image/png', bytes }], { x: Math.round(viewport.width / 2), y: Math.round(viewport.height / 2) });
    const naming = page.getByRole('textbox', { name: 'Name', exact: true });
    await expect(naming).toBeFocused();
    await naming.fill(picture);
    await naming.press('Enter');
    const landed = await imported;
    expect(landed.ok(), `POST ${at('/imports')}: ${landed.status()}`).toBe(true);
    page.off('request', written);
    await expect(box(page, picture)).toBeVisible();
    await expect.poll(async () => {
      const row = await assetOf(picture);
      return { sha256: row?.sha256, measured: row?.width > 0 };
    }, { message: 'its file held and measured', timeout: 60_000 }).toEqual({ sha256: digest, measured: true });
    expect.soft(writes.length > 0 && writes.every((write) => write.route === '/v1/project-files' && write.project === made.id), `its bytes go to the project's own store: ${JSON.stringify(writes)}`).toBe(true);

    await canvasIdle(page);
    const { x, y } = await middle(box(page, picture).locator('.media').first());
    await page.mouse.click(x, y, { button: 'right' });
    const saved = page.waitForEvent('download');
    await page.getByRole('menu', { name: `${picture}'s menu` }).getByRole('menuitem', { name: 'Download', exact: true }).click();
    const download = await saved;
    expect.soft(download.suggestedFilename(), 'saved as its box').toBe(`${picture}.png`);
    expect.soft(readFileSync(await download.path()).equals(bytes), "the download's bytes are the picture's").toBe(true);

    const { view } = await vault();
    expect(view, 'its readable view').toBeTruthy();
    expect.soft(filesUnder(view).some(holds), 'the mount holds the picture').toBe(true);
    const outside = filesUnder(made.path, view);
    expect.soft(outside.length, 'its folder holds its encrypted storage').toBeGreaterThan(0);
    expect.soft(outside.filter(holds).map((file) => file.path), 'outside the mount, no file holds the picture or its name').toEqual([]);
    expect.soft(outside.filter((file) => pictureFile(file.bytes)).map((file) => file.path), 'outside the mount, no file is a picture').toEqual([]);
  });

  // Refused on purpose: what a tab still asks of the project as it's locked or converted, which Chrome says in its console.
  const refused = `status of 409\\b.*(/v1/projects/(${made.id}|${made.code})\\b|[?&]project=${made.id}\\b|/v1/events/)`;
  guards.allow(new RegExp(refused));

  await test.step('cozy locks the project while a second tab is open', async () => {
    const second = await page.context().newPage();
    await signIn(second, `${api.origin}/p/${made.code}`);
    await expect(box(second, picture)).toBeVisible();
    const said = await cli('--json', 'vault', '--directory', made.path, 'lock');
    expect(said.code, `cozy vault lock: ${said.stdout.slice(-300)} ${said.stderr.slice(-300)}`).toBe(0);
    expect.soft(JSON.parse(said.stdout).vault?.state, 'cozy says it locked').toBe('locked');
    await Promise.all([page, second].map(async (tab, i) => {
      const which = i ? 'the second tab' : 'the first tab';
      await expect(tab.getByText('This project is locked.', { exact: true }), `${which} covered`).toBeVisible();
      expect.soft(await keptIn(tab, words), `${which} keeps nothing of it`).toEqual(NOTHING);
    }));
    expect.soft((await vault()).state).toBe('locked');
    await second.close();
  });

  await test.step('unlocked with the passphrase, then locked from the page', async () => {
    await page.getByLabel('Passphrase', { exact: true }).fill(passphrase);
    await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
    await expect(box(page, picture), 'the canvas is back').toBeVisible({ timeout: 60_000 });
    expect.soft(await kept(), 'with no reload').toBe('kept');
    expect.soft((await vault()).state).toBe('unlocked');

    await page.getByLabel('Menu', { exact: true }).click();
    await page.getByRole('link', { name: 'Project encryption', exact: true }).click();
    const drawer = page.locator('.drawer');
    await expect(drawer.locator('[data-vault-state="unlocked"]')).toBeVisible();
    const lock = drawer.getByRole('button', { name: 'Lock project', exact: true });
    await expect(lock, 'nothing waits to be saved').toBeEnabled();
    await lock.click();
    await expect(drawer.locator('[data-vault-state="locked"]')).toBeVisible({ timeout: 60_000 });
    await expect.poll(async () => (await vault()).state, { message: 'Calcifer has it locked' }).toBe('locked');
    expect.soft(await keptIn(page, words), 'the tab keeps nothing of it').toEqual(NOTHING);
  });

  await test.step('unlocked with the recovery secret, then decrypted', async () => {
    const drawer = page.locator('.drawer');
    await drawer.getByLabel('Unlock with').selectOption('recovery');
    await drawer.getByLabel('Recovery secret', { exact: true }).fill(recovery);
    await drawer.getByRole('button', { name: 'Unlock project', exact: true }).click();
    await expect(drawer.locator('[data-vault-state="unlocked"]')).toBeVisible({ timeout: 60_000 });
    await expect(box(page, picture), 'the canvas is back').toBeVisible();
    expect.soft(await kept(), 'with no reload').toBe('kept');

    const held = async () => ({
      assets: (await api.get(at())).assets.map((row) => [row.name, row.sha256, row.versions, row.version]).sort(),
      history: (await api.get(at('/history?limit=100'))).entries.map(({ id, words: said, names }) => ({ id, words: said, names })),
    });
    const before = await held();
    expect.soft(before.history.some((entry) => entry.names?.includes(picture)), `its history has the drop: ${JSON.stringify(before.history)}`).toBe(true);
    await drawer.locator('summary', { hasText: /^Decrypt project$/ }).click();
    const decrypt = drawer.getByRole('button', { name: 'Decrypt project', exact: true });
    await expect(decrypt, 'nothing waits to be saved').toBeEnabled();
    await decrypt.click();
    await expect(drawer.locator('[data-vault-state="plain"]')).toBeVisible({ timeout: 60_000 });
    await expect.poll(async () => {
      const { encrypted, state } = await vault();
      return { encrypted, state };
    }, { message: 'ordinary storage' }).toEqual({ encrypted: false, state: 'plain' });
    await expect(box(page, picture), 'the canvas is back').toBeVisible();
    const after = await held();
    expect.soft(after.assets, 'every file kept').toEqual(before.assets);
    expect.soft(after.history, 'its history kept').toEqual(expect.arrayContaining(before.history));
    expect.soft(filesUnder(made.path).some(holds), 'its folder holds the picture, now in ordinary storage').toBe(true);
    expect.soft(existsSync(join(made.path, 'project.db')), 'its project.db in its folder').toBe(true);
  });
});
