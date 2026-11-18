// walkthrough (release-gate/sessions/walkthrough.sh, 0.27.0 launch): the page as a new user meets it. Each view is kept as a screenshot and
// its accessibility tree ($WALK/page/<engine>/<stage>-NN-<name>.png, .aria.txt), each step's time and outcome in <stage>-steps.txt; a step
// that fails is said and the next goes on. Stage `tour`: projects, a new project from the page, the canvas, Tools, a picture dropped, a tool
// box from the palette, a prompt with a mention, undo, Focus, settings, Machines and a quote (nothing ordered), dark, narrow, a second
// window, an encrypted project locked and unlocked. Stage `results`: the rented tools' boxes in $WALK/garden, each in Focus, each video
// played and sought, each file downloaded, and the machines view while the rental is up.
const { mkdirSync, readFileSync, writeFileSync, existsSync } = require('fs');
const { join } = require('path');
const { randomBytes } = require('crypto');
const { test, expect, canvasIdle, focus, openPoint } = require('./fixtures');
const { dropFiles, fit, leave } = require('./hands');
const { carry } = require('./geometry');

const W = process.env.WALK;
const STAGE = process.env.WALK_STAGE || 'tour';
const ENGINE = process.env.WALK_ENGINE || 'chrome';
const DIR = join(W, 'page', ENGINE);
mkdirSync(DIR, { recursive: true });
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40);
const log = [];
const said = [];
let n = 0;

async function snap(page, name) {
  const base = join(DIR, `${STAGE}-${String(++n).padStart(2, '0')}-${slug(name)}`);
  await page.screenshot({ path: `${base}.png` }).catch(() => {});
  const aria = await page.locator('body').ariaSnapshot({ timeout: 10_000 }).catch((error) => `(no aria: ${error.message})`);
  writeFileSync(`${base}.aria.txt`, `${page.url()}\n${aria}`);
}

async function step(page, name, fn) {
  const t0 = Date.now();
  try {
    await fn();
    log.push(`STEP-OK ${name} ${Date.now() - t0} ms`);
  } catch (error) {
    log.push(`STEP-FAIL ${name} ${Date.now() - t0} ms: ${String(error?.message ?? error).split('\n').slice(0, 4).join(' | ').slice(0, 600)}`);
    await snap(page, `${name}-FAILED`);
  }
  writeFileSync(join(DIR, `${STAGE}-steps.txt`), `${log.join('\n')}\n\nconsole and page errors:\n${said.join('\n')}\n`);
}

const listen = (page, label) => {
  page.on('pageerror', (error) => said.push(`${label} page error: ${error.message}`.slice(0, 400)));
  page.on('console', (message) => { if (['error', 'warning'].includes(message.type())) said.push(`${label} console ${message.type()}: ${message.text()}`.slice(0, 400)); });
  page.on('response', (r) => { if (r.status() >= 400) said.push(`${label} HTTP ${r.status()} ${r.request().method()} ${new URL(r.url()).pathname}`); });
};

test('walkthrough', async ({ context, page, project, api, cli, signIn, newContext, guards }) => {
  test.setTimeout(3 * 3600_000);
  guards.allow(/./);
  listen(page, 'main');
  const origin = api.origin;
  const canvas = async (p = page) => { await expect(p.locator('.stage')).toBeVisible({ timeout: 60_000 }); await canvasIdle(p).catch(() => {}); };

  if (STAGE === 'tour') {
    await step(page, 'the project page opens', async () => { await canvas(); await snap(page, 'empty-canvas'); });

    await step(page, 'projects list', async () => { await page.goto(`${origin}/projects`); await page.waitForTimeout(1500); await snap(page, 'projects'); });

    await step(page, 'a new project from the page', async () => {
      await page.goto(project.url());
      await canvas();
      await page.getByRole('button', { name: 'New project', exact: true }).first().click();
      await page.waitForTimeout(800);
      await snap(page, 'new-project-form');
      const name = `walk-${randomBytes(3).toString('hex')}`;
      await page.getByLabel("New project's name", { exact: true }).fill(name);
      await snap(page, 'new-project-named');
      writeFileSync(join(DIR, 'new-project-form.txt'), await page.locator('form, [role=dialog], .new-project, aside').first().innerText().catch(() => '(none)'));
      await page.getByRole('button', { name: 'Create project', exact: true }).click();
      await page.waitForTimeout(3000);
      await snap(page, 'new-project-after-create');
      const pass = page.getByLabel(/passphrase/i);
      if (await pass.count()) {
        const words = randomBytes(12).toString('base64url');
        for (let i = 0; i < await pass.count(); i++) await pass.nth(i).fill(words).catch(() => {});
        await snap(page, 'new-project-passphrase');
        const go = page.getByRole('button', { name: /create|encrypt|continue/i }).first();
        if (await go.count()) await go.click();
        await page.waitForTimeout(5000);
        await snap(page, 'new-project-secret');
        const saved = page.getByRole('button', { name: /saved|continue|done|open/i }).first();
        if (await saved.count()) { await saved.click(); await page.waitForTimeout(3000); }
      }
      await snap(page, 'new-project-opened');
      writeFileSync(join(DIR, 'new-project-url.txt'), page.url());
    });

    await step(page, 'the Tools palette, every group open', async () => {
      await page.goto(project.url());
      await canvas();
      const tools = page.getByRole('complementary', { name: 'Tools' });
      await expect(tools).toBeVisible();
      await snap(page, 'tools-closed');
      const buttons = tools.locator('button[aria-expanded="false"]');
      for (let i = 0; i < 20 && await buttons.count(); i++) await buttons.first().click();
      await page.waitForTimeout(500);
      await snap(page, 'tools-open');
      writeFileSync(join(DIR, 'tools.txt'), await tools.innerText());
      const rows = await tools.locator('li[data-tool]').evaluateAll((els) => els.map((el) => `${el.dataset.tool}\t${el.innerText.replace(/\s+/g, ' ')}`));
      writeFileSync(join(DIR, 'tool-rows.tsv'), rows.join('\n'));
    });

    await step(page, 'a picture dropped on the canvas', async () => {
      await page.goto(project.url());
      await canvas();
      await dropFiles(page, [{ name: 'photo.jpg', type: 'image/jpeg', bytes: readFileSync(join(process.env.REF || join(__dirname, '../../../release-gate/ref'), 'seed.jpg')) }], await openPoint(page));
      const naming = page.getByRole('textbox', { name: 'Name', exact: true });
      await expect(naming).toBeFocused({ timeout: 30_000 });
      await snap(page, 'picture-naming');
      await naming.fill('photo');
      await naming.press('Enter');
      await expect.poll(async () => (await project.asset('photo'))?.sha256 ?? null, { timeout: 120_000 }).not.toBeNull();
      await fit(page);
      await snap(page, 'picture-dropped');
    });

    let made = null;
    await step(page, 'Qwen Image 2 from Tools onto open canvas', async () => {
      const tools = page.getByRole('complementary', { name: 'Tools' });
      const row = tools.locator('li[data-implementation][data-tool$="qwen-image-2/image"]').first();
      if (!(await row.isVisible())) { const g = tools.getByRole('button', { name: 'Image', exact: true }); if (await g.count() && (await g.getAttribute('aria-expanded')) !== 'true') await g.click(); }
      await expect(row).toBeVisible({ timeout: 60_000 });
      const before = new Set((await project.assets()).map((a) => a.name));
      await carry(page, row, await openPoint(page));
      await expect.poll(async () => { made = (await project.assets()).find((a) => !before.has(a.name))?.name ?? null; return made; }, { timeout: 30_000 }).not.toBeNull();
      await canvasIdle(page);
      await snap(page, 'qwen-box-made');
    });

    await step(page, 'a prompt typed with a mention of the picture', async () => {
      const r = await focus(page, made);
      await snap(page, 'qwen-focus');
      const field = r.locator('.mention-text[data-field="prompt"], .mention-text').first();
      await field.click();
      await page.keyboard.type('a watercolor of <pho');
      await page.waitForTimeout(1200);
      await snap(page, 'mention-offered');
      await page.keyboard.press('Enter');
      await page.keyboard.type(' on a hill at dawn');
      await page.waitForTimeout(1500);
      await snap(page, 'mention-chip');
      writeFileSync(join(DIR, 'qwen-focus.txt'), await r.innerText());
      const box = await project.asset(made);
      writeFileSync(join(DIR, 'qwen-box.json'), JSON.stringify(box, null, 2));
    });

    await step(page, 'Undo and Redo', async () => {
      await leave(page).catch(() => {});
      const before = JSON.stringify((await project.asset(made))?.fields ?? null);
      await page.keyboard.press('Control+z');
      await page.waitForTimeout(1500);
      await snap(page, 'after-undo');
      const after = JSON.stringify((await project.asset(made))?.fields ?? null);
      await page.keyboard.press('Control+Shift+z');
      await page.waitForTimeout(1500);
      await snap(page, 'after-redo');
      writeFileSync(join(DIR, 'undo.txt'), `before ${before}\nafter undo ${after}\nafter redo ${JSON.stringify((await project.asset(made))?.fields ?? null)}\n`);
    });

    await step(page, 'the picture in Focus', async () => { await focus(page, 'photo'); await snap(page, 'photo-focus'); await leave(page); });

    await step(page, 'the menu and project settings', async () => {
      const menu = page.getByRole('button', { name: /^(Menu|Open menu|More|Settings)$/i }).first();
      await menu.click();
      await page.waitForTimeout(600);
      await snap(page, 'menu-open');
      const item = page.getByRole('menuitem', { name: /project settings/i }).or(page.getByRole('button', { name: /project settings/i })).first();
      await item.click();
      await page.waitForTimeout(800);
      await snap(page, 'project-settings');
      await page.keyboard.press('Escape');
    });

    await step(page, 'Machines and a quote, nothing ordered', async () => {
      await page.goto(`${project.url()}/machines`);
      await page.waitForTimeout(3000);
      await snap(page, 'machines');
      const rows = page.locator('li.sku');
      await expect(rows.first()).toBeVisible({ timeout: 120_000 });
      writeFileSync(join(DIR, 'skus.txt'), (await rows.allInnerTexts()).map((t) => t.replace(/\s+/g, ' ')).join('\n'));
      const row = rows.filter({ has: page.locator('b:text-is("rtx-4090")') }).first();
      const target = (await row.count()) ? row : rows.first();
      await target.getByRole('button', { name: 'Rent', exact: true }).click();
      const primary = page.locator('.order .primary');
      await expect(primary).toBeEnabled({ timeout: 120_000 });
      await snap(page, 'quote');
      writeFileSync(join(DIR, 'quote.txt'), await page.locator('.order').innerText());
      await page.locator('.order').getByRole('button', { name: 'Not now', exact: true }).click();
      await snap(page, 'quote-dismissed');
    });

    await step(page, 'dark', async () => {
      await project.look('dark');
      await page.goto(project.url());
      await canvas();
      await snap(page, 'dark-canvas');
      await focus(page, made).catch(() => {});
      await snap(page, 'dark-focus');
      await page.goto(`${project.url()}/machines`);
      await page.waitForTimeout(2000);
      await snap(page, 'dark-machines');
      await project.look(null);
    });

    await step(page, 'narrow and tablet', async () => {
      for (const [w, h] of [[390, 844], [820, 1180]]) {
        await page.setViewportSize({ width: w, height: h });
        await page.goto(project.url());
        await canvas();
        await snap(page, `canvas-${w}`);
        await focus(page, made).catch(() => {});
        await snap(page, `focus-${w}`);
        await page.goto(`${origin}/projects`);
        await page.waitForTimeout(1500);
        await snap(page, `projects-${w}`);
        await page.goto(`${project.url()}/machines`);
        await page.waitForTimeout(2000);
        await snap(page, `machines-${w}`);
        const wide = await page.evaluate(() => ({ doc: document.documentElement.scrollWidth, win: innerWidth }));
        writeFileSync(join(DIR, `overflow-${w}.json`), JSON.stringify(wide));
      }
      await page.setViewportSize({ width: 1600, height: 1000 });
    });

    await step(page, 'a second window on the same project', async () => {
      const other = await newContext({ viewport: { width: 1200, height: 900 } });
      const two = await other.newPage();
      listen(two, 'second');
      await signIn(two, project.url());
      await canvas(two);
      await project.add({ name: 'note', kind: 'text', fields: { text: 'Written from the API while two windows are open.' } });
      await page.goto(project.url());
      await canvas();
      await page.waitForTimeout(3000);
      await snap(two, 'second-window');
      await snap(page, 'first-window');
      await other.close();
    });

    await step(page, 'an encrypted project locked, then unlocked from the page', async () => {
      const folder = join(process.env.OUT, 'web', 'projects', `walk-vault-${randomBytes(3).toString('hex')}`);
      const passphrase = randomBytes(18).toString('base64url');
      const t0 = Date.now();
      const opened = await api.post('/v1/projects', { path: folder, passphrase });
      writeFileSync(join(DIR, 'vault-create-ms.txt'), String(Date.now() - t0));
      const lock = await cli('vault', '--directory', folder, 'lock');
      writeFileSync(join(DIR, 'vault-lock.txt'), `${lock.code}\n${lock.stdout}\n${lock.stderr}`);
      await page.goto(`${origin}/projects`);
      await page.waitForTimeout(1500);
      await snap(page, 'projects-with-locked');
      await page.locator('ul.list').locator(`a[title=${JSON.stringify(folder)}]`).click();
      await expect(page.getByText('This project is locked.', { exact: true })).toBeVisible({ timeout: 60_000 });
      await snap(page, 'locked');
      await page.getByLabel('Passphrase', { exact: true }).fill('not the passphrase');
      await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
      await page.waitForTimeout(3000);
      await snap(page, 'wrong-passphrase');
      await page.getByLabel('Passphrase', { exact: true }).fill(passphrase);
      const t1 = Date.now();
      await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
      await expect(page.locator('.stage')).toBeVisible({ timeout: 120_000 });
      writeFileSync(join(DIR, 'vault-unlock-ms.txt'), String(Date.now() - t1));
      await snap(page, 'unlocked');
      writeFileSync(join(DIR, 'vault-url.txt'), `${page.url()}\nexpected /p/${opened.code}`);
    });

    await step(page, 'settings and account', async () => {
      for (const where of ['settings', 'account', 'machines']) {
        await page.goto(`${origin}/${where}`);
        await page.waitForTimeout(2000);
        await snap(page, `route-${where}`);
      }
    });
  }

  if (STAGE === 'results') {
    const garden = process.env.WALK_PROJECT || join(W, 'garden');
    let code = null;
    await step(page, 'the walkthrough project opened', async () => {
      const opened = await api.post('/v1/projects', { path: garden }).catch(async () => null);
      code = opened?.code;
      if (!code) {
        const listed = await api.get('/v1/projects');
        code = (listed.projects ?? listed).find((p) => p.path === garden)?.code;
      }
      await page.goto(`${origin}/p/${code}`);
      await canvas();
      await fit(page).catch(() => {});
      await snap(page, 'garden-canvas');
    });
    const id = encodeURIComponent(garden);
    const assets = (await api.get(`/v1/projects/${id}`).catch(() => ({ assets: [] }))).assets ?? [];
    writeFileSync(join(DIR, 'garden-assets.json'), JSON.stringify(assets, null, 2));
    mkdirSync(join(W, 'out', 'downloads', ENGINE), { recursive: true });
    for (const asset of assets) {
      await step(page, `${asset.name} in Focus`, async () => {
        const r = await focus(page, asset.name);
        await page.waitForTimeout(1500);
        await snap(page, `${asset.name}-focus`);
        writeFileSync(join(DIR, `${slug(asset.name)}-focus.txt`), await r.innerText());
        const video = r.locator('video').first();
        if (await video.count()) {
          const play = r.getByRole('button', { name: /^Play/ }).first();
          if (await play.count()) await play.click(); else await video.click();
          await page.waitForTimeout(3000);
          const a = await video.evaluate((v) => ({ t: v.currentTime, paused: v.paused, d: v.duration, w: v.videoWidth, h: v.videoHeight, err: v.error?.code ?? null, ready: v.readyState }));
          await snap(page, `${asset.name}-playing`);
          await video.evaluate((v) => { v.currentTime = Math.max(0, (v.duration || 2) * 0.6); });
          await page.waitForTimeout(1500);
          const b = await video.evaluate((v) => ({ t: v.currentTime, paused: v.paused, ready: v.readyState }));
          await snap(page, `${asset.name}-sought`);
          writeFileSync(join(DIR, `${slug(asset.name)}-video.json`), JSON.stringify({ a, b }));
        }
        const download = r.getByRole('button', { name: /download/i }).or(r.getByRole('link', { name: /download/i })).first();
        if (await download.count()) {
          const [file] = await Promise.all([page.waitForEvent('download', { timeout: 60_000 }), download.click()]);
          const to = join(W, 'out', 'downloads', ENGINE, file.suggestedFilename());
          await file.saveAs(to);
          writeFileSync(join(DIR, `${slug(asset.name)}-download.txt`), to);
        } else writeFileSync(join(DIR, `${slug(asset.name)}-download.txt`), 'no Download in Focus');
        await leave(page);
      });
    }
    await step(page, 'the machines view with the rental', async () => {
      await page.goto(`${origin}/p/${code}/machines`);
      await page.waitForTimeout(3000);
      await snap(page, 'machines-rented');
      const name = process.env.WALK_MACHINE;
      if (name) {
        await page.goto(`${origin}/p/${code}/machines/${encodeURIComponent(name)}`);
        await page.waitForTimeout(3000);
        await snap(page, 'machine-drawer');
        writeFileSync(join(DIR, 'machine-drawer.txt'), await page.locator('main, body').first().innerText());
      }
    });
  }
});
