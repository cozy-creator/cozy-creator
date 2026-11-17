// Every page string cl-1066 made longer by naming the product (Paul, 11-17: "Cozy Creator" for users), as pictures to be looked at: at
// 1280×800, 1920×1080 and 390×844, on paper and dark. The top bar offline and with changes waiting, and a drawer's failure while it's
// away; the update toast; the Video Timeline box's tool line on the canvas and in Focus; the machine's drawer and Machines, Account's sign
// out, Install's machine picker, Project settings; the gates; an encrypted project locked and unlocked. It judges nothing: a view it
// couldn't reach is listed under `missed`, with what no runner shows.
//   node names.js: as look.js ($PLAYWRIGHT, $CHROME, $HOLDER, $SHOT), with $COZY to stop, start and lock.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { appendFileSync, mkdtempSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, BARS, DARK_OPEN, MP4, STEP_MS, calcifer, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, openBox, openCanvas, png, projectFromPage, settled, signIn, until, upload, canvasBox } = require('./page');

const SIZES = [[1280, 800], [1920, 1080], [390, 844]];
const NEWER = '0.99.0';

const seen = {
  looks: 0, shots: [], lines: {}, missed: {
    // What no runner shows: each needs a machine, a run or a release this job doesn't have.
    'run-stalled': "a stalled run's line (\"… won't cancel it on its own\")",
    'machine-broken': "a broken machine's words",
    'gpu-use-old': 'GPU use on an older machine',
    'rental-end': "a rental's End confirm",
    'price-unconfirmed': 'a price the Hub did not confirm',
    'checkpoint-refused': 'a model dropped on a box whose model is unknown',
  },
};
let during = 'the start';
noStall(seen, died);
async function died(why) {
  seen.missed[during] = String(why?.stack ?? why).slice(0, 800);
  await lastLook();
  console.log(JSON.stringify({ failures: seen.shots.length ? [] : ['took nothing'], seen }, null, 2));
  process.exit(seen.shots.length ? 0 : 1);
}

const cozy = (...words) => {
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  return execFileSync(process.env.COZY, words, { encoding: 'utf8', timeout: 5 * STEP_MS });
};
const alive = (pid) => { try { process.kill(pid, 0); return true; } catch { return false; } };
// One matcher each, so `unroute` finds the route it took.
const EVENTS = (url) => url.pathname === '/v1/events';
const STATUS = (url) => url.pathname === '/v1/status';

(async () => {
  const base = calcifer();
  const browser = await launch(playwright, BARS);
  const pages = {};
  for (const theme of ['paper', 'dark']) {
    const context = await browser.newContext({ viewport: { width: SIZES[0][0], height: SIZES[0][1] } });
    const page = await context.newPage();
    page.setDefaultTimeout(STEP_MS);
    page.on('pageerror', (error) => (seen.page_errors ??= []).push(`${theme}: ${error.message}`));
    await signIn(page);
    await page.waitForSelector('.top');
    pages[theme] = page;
  }
  lookAt(pages.paper);
  const looks = async (page, theme) => {
    await page.evaluate((theme) => fetch('/v1/appearance', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ theme }) }), theme);
    await page.waitForFunction((dark) => (document.documentElement.dataset.theme === 'dark') === dark, theme === 'dark');
  };
  const both = async (what) => { for (const [theme, page] of Object.entries(pages)) { await looks(page, theme); await what(page, theme); } };
  const out = (name) => `${process.env.SHOT}-${name}.png`;
  // Settled where there's a canvas; a gate has none.
  const calm = async (page) => {
    if (await page.evaluate(() => Boolean(document.querySelector('.canvas-root')?.canvasInspection))) await settled(page);
    else await page.waitForTimeout(300);
  };
  const pill = (page) => page.locator('header.top .pill');

  /** `name` at each size: `open` once a page, `prepare` after each resize. `only` one theme; `away`, Calcifer is, so the theme stays. */
  async function shoot(name, { open, prepare, only, away = false } = {}) {
    during = name;
    for (const [theme, page] of Object.entries(pages)) {
      if (only && only !== theme) continue;
      try {
        if (open) await open(page, theme);
        if (!away) await looks(page, theme);
      } catch (why) {
        seen.missed[`${name} ${theme}`] = String(why.message ?? why).slice(0, 400);
        continue;
      }
      for (const [width, height] of SIZES) {
        const label = `${name}-${width}x${height}-${theme}`;
        try {
          await page.setViewportSize({ width, height });
          await page.waitForTimeout(250);
          if (prepare) await prepare(page, theme);
          await calm(page);
          await page.waitForTimeout(150);
          if (process.env.SHOT) await page.screenshot({ path: out(label) });
          const said = await pill(page).allInnerTexts();
          if (said.length) seen.lines[label] = said.join(' | ');
          seen.shots.push(label);
          seen.looks++;
        } catch (why) {
          seen.missed[label] = String(why.message ?? why).slice(0, 400);
        }
      }
      await page.setViewportSize({ width: SIZES[0][0], height: SIZES[0][1] }).catch(() => {});
    }
  }
  /** A close look at what `locate` finds on each page, at the first size. */
  async function crop(name, locate, { pad = 12, open } = {}) {
    during = name;
    for (const [theme, page] of Object.entries(pages)) {
      try {
        await page.setViewportSize({ width: SIZES[0][0], height: SIZES[0][1] });
        if (open) await open(page, theme);
        await looks(page, theme);
        await calm(page);
        const target = await locate(page);
        if (target === null) throw new Error('not on the canvas');
        const box = target?.bounds ?? await target.boundingBox();
        if (!box) throw new Error('not shown');
        const size = page.viewportSize();
        const x = Math.max(0, box.x - pad), y = Math.max(0, box.y - pad);
        const clip = { x, y, width: Math.min(size.width, box.x + box.width + pad) - x, height: Math.min(size.height, box.y + box.height + pad) - y };
        if (clip.width < 4 || clip.height < 4) throw new Error(`off screen: ${JSON.stringify(clip)}`);
        if (process.env.SHOT) await page.screenshot({ path: out(`crop-${name}-${theme}`), clip });
        seen.shots.push(`crop-${name}-${theme}`);
        seen.looks++;
      } catch (why) {
        seen.missed[`crop-${name}-${theme}`] = String(why.message ?? why).slice(0, 400);
      }
    }
  }
  const tryStep = async (name, what) => {
    during = name;
    try {
      return await what();
    } catch (why) {
      seen.missed[name] = String(why.stack ?? why).slice(0, 600);
      return undefined;
    }
  };
  const goto = async (page, path) => {
    await page.goto(base + path);
    await page.waitForSelector('.top');
    await page.waitForTimeout(500);
  };
  const leave = async (page) => {
    for (let i = 0; i < 4 && (await page.locator('.card.here, [role=menu], .activity, .main-menu[open], [role=dialog]').count()); i++) {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);
    }
    await closeBox(page).catch(() => {});
  };

  const paper = pages.paper;
  const api = (path, init = {}) => paper.evaluate(async ({ path, init }) => {
    const answer = await fetch(path, { ...init, signal: AbortSignal.timeout(240_000), headers: { 'Content-Type': 'application/json', ...(init.method === 'POST' ? { 'Idempotency-Key': `names-${Date.now()}-${Math.random()}` } : {}) } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init });
  const post = async (path, body) => {
    const answer = await api(path, { method: 'POST', body: JSON.stringify(body) });
    if (answer.status >= 300) throw new Error(`${path}: ${JSON.stringify(answer)}`);
    return answer.body;
  };

  // A project of two pictures, two clips and an Edit of them, whose tool line reads "<Package> · Video Timeline".
  during = 'a project';
  await goto(paper, '/');
  await paper.getByRole('button', { name: 'New project', exact: true }).click();
  await paper.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-names-${Date.now()}`));
  await paper.getByLabel('Create without encryption', { exact: true }).check();
  await paper.click('form button[type=submit]');
  await paper.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const projectPath = new URL(paper.url()).pathname;
  const { id, path: project } = await projectFromPage(paper);
  seen.project = { id, folder: project };
  const at = `/v1/projects/${id}`;
  const token = holderToken();
  const folder = mkdtempSync(join(tmpdir(), 'web-names-'));
  writeFileSync(join(folder, 'amy.png'), png(30, 640, 400));
  writeFileSync(join(folder, 'boat.png'), png(190, 640, 400));
  writeFileSync(join(folder, 'clip_a.mp4'), MP4);
  writeFileSync(join(folder, 'clip_b.mp4'), DARK_OPEN);
  const imports = [['amy', 'amy.png', 0, 0], ['boat', 'boat.png', 440, 0], ['clip_a', 'clip_a.mp4', 0, 420], ['clip_b', 'clip_b.mp4', 440, 420]];
  for (const [name, file, x, y] of imports) await post(`${at}/imports`, { source: await upload(token, join(folder, file), id), name, exact: true, at: { x, y } });
  await post(`${at}/assets`, { tool: 'cozy/edit', kind: 'video', name: 'edit', exact: true, fields: { timeline: { fps: 24, tracks: [{ clips: [{ clip: '<clip_a>' }, { clip: '<clip_b>' }] }] } }, at: { x: 880, y: 0 } });
  const boxes = ['amy', 'boat', 'clip_a', 'clip_b', 'edit'];
  await both(async (page) => {
    if (new URL(page.url()).pathname !== projectPath) await goto(page, projectPath);
    await until('every box drawn', async () => {
      await fitCanvas(page);
      return page.evaluate((names) => names.filter((name) => !document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered), boxes);
    }, (missing) => missing.length === 0, 30_000);
  });

  // The canvas fitted, the Edit's tile at 100% (its tool line), and the Edit in Focus.
  await shoot('canvas', { open: (page) => leave(page), prepare: (page) => fitCanvas(page).catch(() => {}) });
  const tileOf = (page, name) => page.evaluate((name) => {
    const card = document.querySelector('.canvas-root')?.canvasInspection?.card(name);
    if (!card) return null;
    const drawn = card.rendered;
    const rects = [card.bounds, ...(drawn ? [...Object.values(drawn.labels), ...drawn.controls, drawn.stateDot, drawn.meter].filter(Boolean) : [])]
      .filter((r) => r.width > 0 && r.height > 0);
    const left = Math.min(...rects.map((r) => r.x)), top = Math.min(...rects.map((r) => r.y));
    const right = Math.max(...rects.map((r) => r.x + r.width)), bottom = Math.max(...rects.map((r) => r.y + r.height));
    return { bounds: { x: left, y: top, width: right - left, height: bottom - top } };
  }, name);
  await crop('tile-edit', (page) => tileOf(page, 'edit'), {
    pad: 36,
    open: async (page) => {
      await leave(page);
      await fitCanvas(page);
      const { x, y, width, height } = (await canvasBox(page, 'edit')).body;
      await page.mouse.click(x + width / 2, y + height / 2);
      await page.keyboard.press('Shift+2');
      await settled(page);
      await page.keyboard.press('Shift+0');
      await settled(page);
      const away = await openCanvas(page);
      await page.mouse.click(away.x, away.y);
      await page.mouse.move(away.x, away.y);
      await page.waitForTimeout(300);
    },
  });
  await shoot('focus-edit', { open: async (page) => { await leave(page); await fitCanvas(page); await openBox(page, 'edit'); } });
  await crop('focus-edit-tool', (page) => page.locator('.image-tool').first(), { pad: 16 });
  await both(leave);

  // Any note already saying the product's name (ffmpeg fetched as the project opened, say).
  await tryStep('notes so far', async () => {
    const said = await paper.locator('.notes .note').allInnerTexts();
    seen.notes = said;
    if (said.some((line) => line.includes('Cozy Creator')) && process.env.SHOT) await paper.screenshot({ path: out('notes-so-far') });
  });

  // The update toast: a hello from a newer Calcifer, its version only, on a reload at each size.
  during = 'toast-updated';
  for (const [theme, page] of Object.entries(pages)) {
    for (const [width, height] of SIZES) {
      const label = `toast-updated-${width}x${height}-${theme}`;
      try {
        await looks(page, theme);
        await page.setViewportSize({ width, height });
        await page.route(EVENTS, (route) => route.fulfill({ status: 200, headers: { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' }, body: `data: ${JSON.stringify({ type: 'hello', version: NEWER, stream: 'names' })}\n\n` }), { times: 1 });
        await page.reload();
        await page.waitForSelector('.top');
        const note = page.locator('.notes .note', { hasText: NEWER }).first();
        await note.waitFor({ timeout: 30_000 });
        await page.waitForTimeout(400);
        if (process.env.SHOT) await page.screenshot({ path: out(label) });
        const box = await page.locator('.notes').boundingBox();
        if (box && process.env.SHOT) await page.screenshot({ path: out(`crop-${label}`), clip: { x: Math.max(0, box.x - 12), y: Math.max(0, box.y - 12), width: Math.min(width - Math.max(0, box.x - 12), box.width + 24), height: Math.min(height - Math.max(0, box.y - 12), box.height + 24) } });
        seen.lines[label] = await note.innerText();
        seen.shots.push(label);
        seen.looks++;
      } catch (why) {
        seen.missed[label] = String(why.message ?? why).slice(0, 400);
      }
    }
    await page.unroute(EVENTS).catch(() => {});
    await page.setViewportSize({ width: SIZES[0][0], height: SIZES[0][1] });
  }
  await both((page) => goto(page, projectPath));

  // Calcifer away (SIGTERM, which its service leaves down), once a theme: the bar, then with changes waiting, then a drawer it can't fill.
  const partAt = async (page, name, part) => {
    const away = await openCanvas(page);
    await page.mouse.move(away.x, away.y);
    await page.waitForTimeout(200);
    return until(`${name}'s ${part}`, () => page.evaluate(({ name, part }) => {
      const card = document.querySelector('.canvas-root')?.canvasInspection?.card(name), shown = card?.rendered;
      const rect = shown?.parts?.[part] ?? (part === 'title' ? shown?.labels?.title ?? card?.title : shown?.controls?.find((one) => one.part === part));
      return rect?.width && rect?.height ? { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 } : null;
    }, { name, part }), Boolean, 15_000);
  };
  for (const theme of ['paper', 'dark']) {
    const page = pages[theme];
    const ok = await tryStep(`offline ${theme}`, async () => {
      await looks(page, theme);
      await goto(page, projectPath);
      await fitCanvas(page);
      const { pid } = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
      process.kill(pid, 'SIGTERM');
      await until(`Calcifer ${pid} stopped`, async () => alive(pid), (up) => !up, STEP_MS);
      await pill(page).filter({ hasText: /offline/i }).waitFor({ timeout: 30_000 });
      return true;
    });
    if (!ok) continue;
    await shoot('offline', { only: theme, away: true });
    await tryStep(`changes ${theme}`, async () => {
      await fitCanvas(page);
      const title = await partAt(page, 'amy', 'title');
      await page.mouse.move(title.x, title.y);
      await page.mouse.down();
      await page.mouse.move(title.x + 8, title.y + 2);
      await page.mouse.move(title.x + 90, title.y + 30, { steps: 8 });
      await page.mouse.up();
      await settled(page);
      const trash = await partAt(page, 'boat', 'delete');
      await page.mouse.click(trash.x, trash.y);
      const clear = await openCanvas(page);
      await page.mouse.click(clear.x, clear.y);
      await page.keyboard.press('ControlOrMeta+z');
      await pill(page).filter({ hasText: /waiting/ }).waitFor({ timeout: 15_000 });
    });
    // Five tries in, it says to restart it.
    await tryStep(`restart words ${theme}`, () => pill(page).filter({ hasText: /restart/i }).waitFor({ timeout: 60_000 }));
    await shoot('offline-waiting', { only: theme, away: true, prepare: (page) => fitCanvas(page).catch(() => {}) });
    await shoot('offline-drawer', {
      only: theme,
      away: true,
      open: async (page) => {
        await page.click('summary[aria-label="Menu"]');
        await page.getByRole('link', { name: 'Installations', exact: true }).click();
        await page.locator('.drawer [role=alert]').first().waitFor({ timeout: 45_000 });
      },
    });
    await tryStep(`back ${theme}`, async () => {
      cozy('machine', 'ls');
      await until('the bar clear', () => pill(page).count(), (count) => count === 0, 120_000);
    });
  }
  await both((page) => goto(page, projectPath));

  // Drawers.
  await shoot('drawer-machines', { open: (page) => goto(page, `${projectPath}/machines`) });
  await shoot('drawer-machine', {
    open: async (page) => {
      await goto(page, `${projectPath}/machines/local`);
      await page.locator('.drawer button.more', { hasText: 'Details' }).click();
      const update = page.locator('.drawer details.sect summary', { hasText: 'Update' });
      if (await update.count()) {
        await update.click();
        await page.locator('.drawer details.sect[open] input.field').last().fill('0.26.0');
      }
    },
    prepare: async (page) => { await page.locator('.drawer dl.kv').first().scrollIntoViewIfNeeded().catch(() => {}); },
  });
  await shoot('drawer-machine-update', { prepare: async (page) => { await page.locator('.drawer details.sect[open] form').first().scrollIntoViewIfNeeded(); } });
  await shoot('drawer-account', {
    open: async (page) => {
      await goto(page, `${projectPath}/account`);
      const out = page.locator('.drawer button[aria-expanded="false"]', { hasText: 'Sign out' });
      if (await out.count()) await out.click();
    },
  });
  await shoot('drawer-models-upload', {
    open: async (page) => {
      await goto(page, `${projectPath}/models`);
      await page.locator('.drawer details.sect summary', { hasText: 'Upload a model file' }).click({ timeout: 15_000 });
    },
    prepare: async (page) => { await page.getByRole('combobox', { name: 'Machine' }).first().scrollIntoViewIfNeeded(); },
  });
  const pkg = await tryStep('a package', async () => ((await api(`/v1/tools?project=${id}`)).body?.tools ?? []).find((tool) => tool.package && tool.package !== 'cozy' && !tool.local)?.package);
  seen.package = pkg ?? null;
  if (pkg) {
    await shoot('drawer-package', {
      open: (page) => goto(page, `${projectPath}/packages/${encodeURIComponent(pkg)}`),
      prepare: async (page) => { await page.getByRole('combobox', { name: 'Machine' }).first().scrollIntoViewIfNeeded(); },
    });
  }
  await shoot('drawer-project-settings', { open: (page) => goto(page, `${projectPath}/project-settings`) });
  await both((page) => goto(page, projectPath));

  // The gates: Calcifer not answering as the page opens, and the page not Calcifer's own. Each page keeps its theme as it was shown.
  await shoot('gate-unreachable', {
    open: async (page, theme) => {
      await looks(page, theme);
      await page.route(STATUS, (route) => route.abort());
      await page.reload();
      await page.locator('main.gate h1').waitFor({ timeout: STEP_MS });
    },
  });
  await both(async (page) => { await page.unroute(STATUS); await goto(page, projectPath); });
  await shoot('gate-not-own', {
    open: async (page, theme) => {
      await looks(page, theme);
      await page.route(STATUS, (route) => route.fulfill({ status: 403, contentType: 'application/json', body: JSON.stringify({ error: { kind: 'forbidden', code: 'not_own_page', message: 'Not this page' } }) }));
      await page.reload();
      await page.locator('main.gate h1').waitFor({ timeout: STEP_MS });
    },
  });
  await both(async (page) => { await page.unroute(STATUS); await goto(page, projectPath); });

  // An encrypted project, locked (its page and its settings), then unlocked (its settings, Change the encryption key open).
  const passphrase = `names-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const vault = await tryStep('an encrypted project', async () => {
    const made = await post('/v1/projects', { path: projectFolder(`web-names-vault-${Date.now()}`), passphrase });
    seen.vault = { id: made.id, folder: made.path };
    cozy('--json', 'vault', '--directory', made.path, 'lock');
    await until('locked', async () => (await api(`/v1/projects/${encodeURIComponent(made.id)}/vault`)).body?.state, (state) => state === 'locked', STEP_MS);
    // Locked, it's listed under a handle of its own: what the list opens.
    made.locked = (await api('/v1/projects?limit=1000')).body.projects.find((row) => row.path === made.path).id;
    return made;
  });
  if (vault) {
    await shoot('vault-locked', {
      open: async (page) => {
        await page.goto(`${base}/projects/${vault.locked}`);
        await page.getByText('This project is locked.', { exact: true }).first().waitFor({ timeout: STEP_MS });
      },
    });
    await shoot('vault-locked-settings', {
      open: async (page) => {
        await page.goto(`${base}/projects/${vault.locked}/project-settings`);
        await page.locator('.drawer [data-vault-state="locked"]').waitFor({ timeout: STEP_MS });
      },
    });
    const opened = await tryStep('unlocked', async () => {
      await looks(paper, 'paper');
      await paper.goto(`${base}/projects/${vault.locked}`);
      await paper.getByLabel('Passphrase', { exact: true }).fill(passphrase);
      await paper.getByRole('button', { name: 'Unlock project', exact: true }).click();
      await until('unlocked', async () => (await api(`/v1/projects/${encodeURIComponent(vault.id)}/vault`)).body?.state, (state) => state === 'unlocked', STEP_MS);
      return true;
    });
    if (opened) {
      await shoot('vault-unlocked-settings', {
        open: async (page) => {
          await goto(page, `/p/${vault.code}/project-settings`);
          await page.locator('.drawer [data-vault-state="unlocked"]').waitFor({ timeout: STEP_MS });
          await page.locator('.drawer details.sect summary', { hasText: 'Change the encryption key' }).click();
        },
        prepare: async (page) => { await page.locator('.drawer details.sect[open] p').last().scrollIntoViewIfNeeded(); },
      });
    }
    await tryStep('locked again', () => cozy('--json', 'vault', '--directory', vault.path, 'lock'));
  }

  await browser.close();
  console.log(JSON.stringify({ failures: seen.shots.length ? [] : ['took nothing'], seen }, null, 2));
  process.exit(seen.shots.length ? 0 : 1);
})().catch(died);
