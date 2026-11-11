// The real browser/Calcifer/securefs path. Synthetic project only; no requests are mocked.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { dirname, join } = require('path');
const { calcifer, signIn, launch, png, MP4, until, drop, openCanvas, importFile, projectJson, projectUrl, settled, fitCanvas, pointAt } = require('./page');
const failures = [], seen = {};
let browser, context, page, peer, scratch;
let stage = 'starting browser';
let reportedStage;
const progress = process.env.COZY_TEST_PROGRESS ? setInterval(() => {
  if (stage !== reportedStage) { reportedStage = stage; console.error(`[local-vault] ${stage}`); }
}, 250) : undefined;
progress?.unref();
const errors = [];
const projectReads = new Set();
const responseStatuses = new WeakMap();
const check = (name, okay) => { seen[name] = !!okay; if (!okay) failures.push(name); };
const cli = (...args) => execFileSync(process.env.COZY, ['--json', ...args], { encoding: 'utf8', timeout: 180000 });
const vault = (directory, ...args) => execFileSync(process.env.COZY, ['--json', 'vault', ...args], { cwd: directory, encoding: 'utf8', timeout: 180000 });
const requestLock = (directory) => {
  let answer;
  try {
    answer = JSON.parse(vault(directory, 'lock'));
  } catch (error) {
    let refusal;
    try { refusal = JSON.parse(String(error.stdout ?? '')).error; } catch { /* retain only typed error metadata */ }
    if (refusal?.code !== 'project_busy') throw new Error(`external vault lock refused: ${refusal?.code ?? 'unreadable_cli_error'}`);
    const state = JSON.parse(vault(directory, 'status')).state;
    if (state !== 'locking' && state !== 'locked') throw new Error('busy external lock did not revoke the view');
    (seen.external_lock_replies ??= []).push({ code: refusal.code, state });
    return;
  }
  if (answer.vault?.state !== 'locked') throw new Error('external lock did not acknowledge locked');
  (seen.external_lock_replies ??= []).push({ code: 'success', state: 'locked' });
};
const drainedLock = (directory) => until('external lock drained', () => JSON.parse(vault(directory, 'status')).state, (state) => state === 'locked', 45000);
const sanitized = (text) => String(text ?? '').replace(/cozy1-[a-z0-9_-]+/gi, '[REDACTED]').replaceAll('synthetic browser passphrase', '[REDACTED]');
const newPage = async () => {
  const tab = await context.newPage();
  if (!process.env.ENGINE || process.env.ENGINE === 'chromium') {
    const version = require(join(dirname(require.resolve(process.env.PLAYWRIGHT)), 'package.json')).version;
    // playwright-core@1.55.0/lib/server/chromium/crPage.js forces every tab visible.
    // Undo that on its owning session; a new CDP session cannot clear another's
    // emulation. Chrome itself then emits the real visibilitychange events below.
    const owner = tab._connection?.toImpl?.(tab)?.delegate?._mainFrameSession?._client;
    if (version !== '1.55.0' || !owner?.send) throw new Error('native visibility proof requires the pinned owning Chromium driver session');
    await owner.send('Emulation.setFocusEmulationEnabled', { enabled: false });
  }
  return tab;
};
(async () => {
  scratch = mkdtempSync(join(tmpdir(), 'web-vault-'));
  const folder = join(scratch, 'project');
  const picture = join(scratch, 'private_vault_picture.png');
  writeFileSync(picture, png(23));
  cli('new', folder);
  importFile(folder, picture);
  const movie = join(scratch, 'private_loop_movie.mp4');
  writeFileSync(movie, MP4);
  importFile(folder, movie);
  const project = projectJson(folder);
  const { id } = project;
  const href = await projectUrl(project);
  seen.project = { id, folder };
  browser = await launch(playwright);
  context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  page = await newPage();
  page.setDefaultTimeout(45000);
  const outbound = [], fileWrites = [], secretIdempotency = [];
  page.on('pageerror', (error) => errors.push(error.message));
  context.on('requestfinished', (request) => projectReads.delete(request));
  context.on('requestfailed', (request) => projectReads.delete(request));
  context.on('request', (request) => {
    const url = new URL(request.url());
    if ((request.method() === 'GET' || request.method() === 'HEAD') &&
        (url.pathname.startsWith(`/v1/projects/${id}`) || url.searchParams.get('project') === id)) {
      projectReads.add(request);
    }
    if (request.method() === 'PUT' && /^\/v1\/(project-)?files\//.test(url.pathname)) fileWrites.push({ path: url.pathname, project: url.searchParams.get('project') });
    if (request.method() === 'POST' && /\/vault$/.test(url.pathname) && request.headers()['idempotency-key']) secretIdempotency.push(url.pathname);
    let text = `${request.url()} ${request.postData() || ''}`;
    try { text += ` ${decodeURIComponent(text)}`; } catch { /* a binary or malformed URL is still checked as sent */ }
    if (url.origin !== new URL(calcifer()).origin && /cozy1-|synthetic browser passphrase/.test(text)) outbound.push('secret-bearing external request');
  });
  context.on('response', async (response) => {
    responseStatuses.set(response.request(), response.status());
    const path = new URL(response.url()).pathname;
    const method = response.request().method();
    if (method === 'GET' && /^\/v1\/projects\/[^/]+\/(address|vault)$/.test(path)) {
      const route = path.endsWith('/address') ? 'address' : 'vault';
      (seen.vault_read_statuses ??= {})[route] = response.status();
    }
    if (method === 'GET' && path === `/v1/projects/${id}` && !new URL(response.url()).searchParams.has('assets')) {
      const read = { status: response.status() };
      if (response.ok()) {
        const state = await response.json().catch(() => null);
        if (Array.isArray(state?.assets)) read.assets = state.assets.length;
      }
      seen.private_project_read = read;
    }
    if (method === 'PUT' && /^\/v1\/events\/[^/]+$/.test(path)) seen.project_interest_status = response.status();
    if (method !== 'POST' || !/\/vault$/.test(path)) return;
    try {
      const action = JSON.parse(response.request().postData() || '{}').action;
      const answer = await response.json();
      (seen.vault_replies ??= []).push({ action, status: response.status(), encrypted: answer.vault?.encrypted, state: answer.vault?.state, has_recovery_secret: typeof answer.recovery_secret === 'string', error_code: answer.error?.code, error_kind: answer.error?.kind, error_message: answer.error ? sanitized(answer.error.message) : undefined });
    } catch { /* Failed/offline replies carry no readable evidence. Never retain secret bodies. */ }
  });
  await signIn(page);
  // Code redemption finishes after the document loads. Wait for its signed-in
  // shell before opening the compact link in another document.
  await page.locator('.top').waitFor();
  await page.goto(`${href}/vault`);
  const body = page.locator('.drawer [data-vault-state]');
  await page.locator('.drawer [data-vault-state="plain"]').waitFor();
  // Import returns before first-use ffprobe preparation and background measurement.
  // Wait for its actual committed result before asking to replace that plain storage.
  await until('plain seed media measured before encryption', () => {
    const state = projectJson(folder);
    const movie = state.assets.find((asset) => asset.name === 'private_loop_movie');
    const picture = state.assets.find((asset) => asset.name === 'private_vault_picture');
    return { idle: !state.busy && !state.moving, movie: { width: movie?.width, height: movie?.height, duration_ms: movie?.duration_ms }, picture: { width: picture?.width, height: picture?.height } };
  }, (state) => state.idle && state.movie.width > 0 && state.movie.height > 0 && state.movie.duration_ms > 0 && state.picture.width > 0 && state.picture.height > 0);
  check('plain_seed_media_measurement_finished', true);
  await until('initial project reads finished before conversion', () => projectReads.size, (count) => count === 0, 45000);
  check('initial_project_reads_drained', true);
  await body.locator('input[type=password]').nth(0).fill('synthetic browser passphrase');
  await body.locator('input[type=password]').nth(1).fill('synthetic browser passphrase');
  await body.getByLabel('Remove the verified unencrypted source after conversion').check();
  const encryptionReply = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/v1/projects/${id}/vault` && JSON.parse(response.request().postData() || '{}').action === 'encrypt');
  await body.getByRole('button', { name: 'Enable encryption', exact: true }).click();
  const encrypted = await encryptionReply;
  if (!encrypted.ok()) {
    const answer = await encrypted.json();
    const alert = body.getByRole('alert');
    await alert.waitFor({ timeout: 5000 }).catch(() => {});
    seen.encryption_refusal = { status: encrypted.status(), code: answer.error?.code, kind: answer.error?.kind, message: sanitized(answer.error?.message), visible_alert: (await alert.allTextContents()).map(sanitized) };
    throw new Error(`initial encryption refused: ${encrypted.status()} ${sanitized(answer.error?.code)}`);
  }
  await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
  const recovery = await page.locator('[aria-label="Save recovery secret"] pre').innerText();
  check('recovery_is_long_and_local', recovery.startsWith('cozy1-') && recovery.length > 70);
  check('qr_is_generated_in_page', await page.locator('[aria-label="Save recovery secret"] svg').count() === 1);
  await page.getByLabel('I saved this somewhere away from this computer').check();
  await page.locator('[aria-label="Save recovery secret"]').getByRole('button', { name: 'Done', exact: true }).click();
  check('one_time_secret_dismissed', await page.locator('[aria-label="Save recovery secret"]').count() === 0);
  stage = 'encrypted canvas after conversion';
  const readyCanvas = async () => {
    await page.getByText('private_vault_picture', { exact: true }).first().waitFor();
    await settled(page);
  };
  await page.goto(href);
  await readyCanvas();
  check('unlocked_canvas', await page.getByText('private_vault_picture', { exact: true }).count() > 0);
  stage = 'initial private upload';
  const privateBytes = Buffer.concat([png(24), require('crypto').randomBytes(32)]);
  const imported = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/v1/projects/${id}/imports` && response.ok());
  const importAt = await openCanvas(page);
  check('private_import_starts_on_empty_canvas', true);
  await drop(page, [{ name: 'private_after_encryption.png', type: 'image/png', bytes: privateBytes }], importAt.x, importAt.y);
  await page.locator('.nm-field').waitFor();
  await page.keyboard.press('Enter');
  await imported;
  await until('encrypted upload landed', () => page.locator('body').innerText(), (text) => text.includes('private_after_encryption'), 45000);
  check('private_upload_uses_project_route', fileWrites.length > 0 && fileWrites.every((write) => write.path.startsWith('/v1/project-files/') && write.project === id));
  const stored = await page.evaluate(() => JSON.stringify(Object.fromEntries(Object.keys(localStorage).map((key) => [key, localStorage.getItem(key)]))));
  check('no_private_browser_persistence', !stored.includes(recovery) && !stored.includes('synthetic browser passphrase') && !stored.includes('private_vault_picture') && !stored.includes('private_after_encryption'));

  // Real browser network loss must cover private content without destroying its
  // in-memory pending edit. Hiding and showing the same tab adds no lock authority.
  projectJson(folder, 'POST', '/assets', { tool: '', name: 'private_pending_words', exact: true, fields: { text: 'before interruption' } });
  await page.goto(`${href}/assets/private_pending_words`);
  const pendingWords = page.locator('.card.here[data-asset="private_pending_words"] .mention-text[data-field="text"]');
  await pendingWords.waitFor();
  await pendingWords.click();
  await page.evaluate(() => {
    window.__pendingCanvas = document.querySelector('.react-flow');
    window.__coverPaints = [];
    window.__coverWatch = new MutationObserver(() => {
      if ((document.visibilityState === 'hidden' || document.querySelector('.gate')) && /private_vault_picture|private_after_encryption|private_pending_words/.test(document.body.innerText)) window.__coverPaints.push('private content');
    });
    window.__coverWatch.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true });
  });
  await context.setOffline(true);
  await page.keyboard.press('End');
  await page.keyboard.type(' RETAINED-THROUGH-PRIVATE-COVER');
  await page.keyboard.press('Tab');
  await until('encrypted edit waits for network', () => page.locator('.top').innerText(), (text) => /1 change/.test(text), 15000);
  // Offline queries pause; the real visibility transition triggers the privacy cover.
  peer = await newPage();
  await peer.goto('about:blank');
  await peer.bringToFront();
  await until('pending private tab hidden', () => page.evaluate(() => document.visibilityState), (state) => state === 'hidden', 10000);
  await page.locator('.gate').waitFor();
  check('offline_cover_keeps_private_canvas_owner', await page.evaluate(() => document.querySelector('.react-flow') === window.__pendingCanvas));
  check('offline_cover_conceals_private_content', !(await page.locator('body').innerText()).includes('private_pending_words'));
  check('temporary_cover_does_not_lock_project', JSON.parse(vault(folder, 'status')).state === 'unlocked');
  await page.bringToFront();
  await page.locator('.gate').waitFor();
  check('hide_show_keeps_pending_edit', /1 change/.test(await page.locator('.top').innerText()));
  await context.setOffline(false);
  await until('pending encrypted edit commits after reconnect', () => projectJson(folder).assets.find((asset) => asset.name === 'private_pending_words')?.fields?.text?.includes('RETAINED-THROUGH-PRIVATE-COVER') ?? false, Boolean, 45000);
  await until('same private canvas revealed', () => page.evaluate(() => document.querySelector('.react-flow') === window.__pendingCanvas && !document.querySelector('.react-flow')?.closest('[hidden]')), Boolean, 20000);
  check('pending_edit_commits_after_cover', true);
  check('no_private_paint_during_cover', await page.evaluate(() => window.__coverPaints.length === 0));
  await page.evaluate(() => window.__coverWatch.disconnect());
  await peer.close(); peer = null;
  stage = 'private canvas before navigation upload';
  await page.goto(href);
  await readyCanvas();

  // Route changes also retire only the drawn canvas, never an unlocked project's
  // pending upload. Exercise the real private upload/retry path across A -> B -> A.
  const ordinary = join(scratch, 'other-project');
  cli('new', ordinary);
  const other = await projectUrl(projectJson(ordinary));
  const navigate = (to) => page.evaluate((url) => {
    history.pushState(null, '', url);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, to);
  const navigationAt = await openCanvas(page);
  check('navigation_import_starts_on_empty_canvas', true);
  const { createHash, randomBytes } = require('crypto');
  const navigationBytes = Buffer.concat([png(25), randomBytes(32)]);
  const navigationHash = createHash('sha256').update(navigationBytes).digest('hex');
  const navigationFile = (url) => decodeURIComponent(url.pathname).replace('/v1/project-files/sha256:', '/v1/project-files/') === `/v1/project-files/${navigationHash}`
    && url.searchParams.get('project') === id;
  let blockedPut, interrupt;
  const held = new Promise((done) => { interrupt = done; });
  const pausePut = async (route) => {
    if (route.request().method() !== 'PUT') return route.fallback();
    blockedPut ??= route.request();
    await held;
    await route.abort('internetdisconnected').catch(() => {});
  };
  await page.route(navigationFile, pausePut);
  stage = 'private navigation upload';
  try {
    // Start a real edit while the private canvas is readable. Disconnect only
    // after the actual protected PUT has begun, before it can reach Calcifer.
    await drop(page, [{ name: 'private_navigation_import.png', type: 'image/png', bytes: navigationBytes }], navigationAt.x, navigationAt.y);
    await page.locator('.nm-field').waitFor();
    await page.keyboard.press('Enter');
    await until('private navigation PUT held', () => blockedPut, Boolean, 45000);
    const failed = page.waitForEvent('requestfailed', { predicate: (request) => request === blockedPut });
    await context.setOffline(true);
    interrupt();
    await failed;
  } finally {
    interrupt();
    await page.unroute(navigationFile, pausePut);
  }
  if (projectJson(folder).assets.some((asset) => asset.name === 'private_navigation_import')) throw new Error('private navigation import committed before the interrupted upload could be retried');
  await until('private upload waits for network', () => page.locator('.top').innerText(), (text) => /1 change/.test(text), 15000);
  stage = 'offline private route changes';
  await navigate(other);
  await page.locator('.gate').waitFor();
  await navigate(href);
  await page.locator('.gate').waitFor();
  check('navigation_retains_pending_private_import', /1 change/.test(await page.locator('.top').innerText()));
  check('offline_navigation_covers_private_content', !(await page.locator('body').innerText()).includes('private_navigation_import'));
  await context.setOffline(false);
  await until('private import commits after project navigation', () => projectJson(folder).assets.some((asset) => asset.name === 'private_navigation_import' && asset.sha256), Boolean, 45000);
  await page.locator('.react-flow').waitFor();
  check('private_import_survives_project_navigation', true);
  // Automatic hover previews are not user activity. The real video decoder keeps
  // looping, while an idle-only deadline still locks the project.
  stage = 'autoplay preview on the exposed canvas';
  await fitCanvas(page);
  const preview = page.locator('.card[data-asset="private_loop_movie"] .media').first();
  await preview.waitFor();
  await until('private preview exposed', async () => {
    const rect = await preview.boundingBox();
    return rect && page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('.card')?.dataset.asset === 'private_loop_movie',
      { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 });
  }, Boolean, 45000);
  await pointAt(page, preview);
  await until('automatic private preview playing', () => page.evaluate(() => [...document.querySelectorAll('video')].some((el) => el.loop && !el.paused && el.currentTime > 0.05)), Boolean, 45000);
  execFileSync(process.env.COZY, ['--json', 'vault', 'unlock', '--recovery-file', '-', '--idle', '2', '--max-duration', '0'], { cwd: folder, input: `${recovery}\n`, encoding: 'utf8', timeout: 180000 });
  await page.locator('[data-vault-state="locked"]').waitFor();
  check('autoplay_preview_does_not_extend_idle_unlock', await page.locator('.react-flow').count() === 0);
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await page.locator('.react-flow').waitFor();

  // A second actual tab shares the real SSE connection. Locking from the CLI must
  // conceal both pages without relying on the active tab's one-second status poll.
  peer = await newPage();
  peer.on('pageerror', (error) => errors.push(error.message));
  await peer.goto(href);
  await peer.locator('.react-flow').waitFor();
  requestLock(folder);
  for (const tab of [page, peer]) {
    await until('external lock concealed a tab', () => tab.locator('body').innerText(), (text) => !text.includes('private_vault_picture') && !text.includes('private_after_encryption'), 20000);
    check(`external_lock_tab_${tab === page ? 'first' : 'second'}`, await tab.locator('.react-flow').count() === 0);
  }
  await drainedLock(folder);
  // A fresh document has no remembered address. The compact link must still
  // open the unlock UI without exposing or opening the private project.
  await peer.goto(href);
  await peer.locator('[data-vault-state="locked"]').waitFor();
  check('locked_compact_link_opens_unlock', await peer.locator('.react-flow').count() === 0 && !(await peer.locator('body').innerText()).includes('private_vault_picture'));
  await peer.close(); peer = null;
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await page.locator('.react-flow').waitFor();

  // Chromium's actual page lifecycle suspends JavaScript while the CLI locks the
  // project. Resuming the hidden tab must verify protection before showing its canvas.
  if (!process.env.ENGINE || process.env.ENGINE === 'chromium') {
    const lifecycle = await context.newCDPSession(page);
    peer = await newPage();
    await peer.goto('about:blank');
    await peer.bringToFront();
    await until('private tab hidden', () => page.evaluate(() => document.visibilityState), (state) => state === 'hidden', 10000);
    await lifecycle.send('Page.setWebLifecycleState', { state: 'frozen' });
    try { requestLock(folder); }
    finally { await lifecycle.send('Page.setWebLifecycleState', { state: 'active' }); }
    await page.bringToFront();
    await page.locator('[data-vault-state="locked"]').waitFor();
    check('frozen_tab_resume_keeps_locked_data_hidden', await page.locator('.react-flow').count() === 0 && !(await page.locator('body').innerText()).includes('private_vault_picture'));
    await drainedLock(folder);
    await lifecycle.detach();
    await peer.close(); peer = null;
    await page.getByLabel('Unlock with').selectOption('recovery');
    await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
    await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
    await page.locator('.react-flow').waitFor();
  } else seen.frozen_tab_resume = 'Chromium lifecycle proof only; this engine still checks tabs and browser history';

  // Leaving A stops its selected-project status observer. Keep the same document
  // and query cache, lock A while B is shown, then watch every DOM mutation on return.
  await page.evaluate(() => { window.__vaultDocument = 'same-document'; });
  await navigate(other);
  await until('ordinary project canvas visible', () => page.evaluate(() => Boolean(document.querySelector('.react-flow')) && !document.body.innerText.includes('private_vault_picture')), Boolean, 45000);
  // Real browser offline mode prevents both status replies and the lock SSE event;
  // the CLI remains online. Cached unlocked data must still stay hidden on selection.
  await context.setOffline(true);
  requestLock(folder);
  await page.evaluate((pathname) => {
    window.__privatePaints = [];
    window.__privateWatch = new MutationObserver(() => {
      if (location.pathname === pathname && /private_vault_picture|private_after_encryption/.test(document.body.innerText)) window.__privatePaints.push('private content');
    });
    window.__privateWatch.observe(document.body, { childList: true, subtree: true, characterData: true });
  }, new URL(href).pathname);
  await navigate(href);
  await until('private project covered', () => page.evaluate(() => Boolean(document.querySelector('.gate')) && !document.querySelector('.react-flow')), Boolean, 45000);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  check('offline_reselection_hides_cached_private_data', await page.evaluate(() => window.__privatePaints.length === 0));
  await context.setOffline(false);
  await page.locator('[data-vault-state="locked"]').waitFor();
  check('project_reselection_verifies_before_paint', await page.evaluate(() => window.__vaultDocument === 'same-document' && window.__privatePaints.length === 0));
  await drainedLock(folder);
  await page.goBack();
  await until('ordinary project canvas visible', () => page.evaluate(() => Boolean(document.querySelector('.react-flow')) && !document.body.innerText.includes('private_vault_picture')), Boolean, 45000);
  await page.goForward();
  await page.locator('[data-vault-state="locked"]').waitFor();
  check('history_return_keeps_locked_data_hidden', await page.evaluate(() => window.__privatePaints.length === 0 && !document.querySelector('.react-flow')));
  await page.evaluate(() => window.__privateWatch.disconnect());
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await page.locator('.react-flow').waitFor();

  await page.goto(`${href}/vault`);
  await page.getByRole('button', { name: 'Lock project', exact: true }).click();
  await until('vault locked', () => page.locator('.drawer [data-vault-state]').getAttribute('data-vault-state'), (state) => state === 'locked', 20000);
  check('locked_canvas_is_removed', await page.locator('.react-flow').count() === 0);
  check('locked_private_name_is_removed', !(await page.locator('body').innerText()).includes('private_vault_picture') && !(await page.locator('body').innerText()).includes('private_after_encryption'));
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await until('recovery unlock', () => page.locator('.drawer [data-vault-state]').getAttribute('data-vault-state'), (state) => state === 'unlocked', 45000);
  check('recovery_unlock', true);
  await page.getByRole('button', { name: 'Lock project', exact: true }).click();
  await until('locked again', () => page.locator('.drawer [data-vault-state]').getAttribute('data-vault-state'), (state) => state === 'locked', 20000);
  await page.getByLabel('Unlock with').selectOption('passphrase');
  await page.getByLabel('Passphrase', { exact: true }).fill('synthetic browser passphrase');
  await page.getByText('Lock timing', { exact: true }).click();
  await page.getByLabel('Lock after inactivity (seconds; 0 disables)').fill('0');
  await page.getByLabel('Maximum unlock duration (seconds; 0 disables)').fill('2');
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await until('timed unlock', () => page.locator('.drawer [data-vault-state]').getAttribute('data-vault-state'), (state) => state === 'unlocked', 45000);
  await until('absolute lock', () => page.locator('.drawer [data-vault-state]').getAttribute('data-vault-state'), (state) => state === 'locked', 20000);
  check('polling_does_not_extend_unlock', true);
  check('no_external_secret_delivery', outbound.length === 0);
  check('secret_actions_skip_idempotency_cache', secretIdempotency.length === 0);
  check('no_page_errors', errors.length === 0);
  seen.page_errors = errors;
})().catch(async (error) => {
  failures.push('browser vault flow');
  seen.failure = sanitized(error.message);
  seen.failure_stage = stage;
  seen.page_errors = errors.map(sanitized);
  seen.pending_project_reads = [...projectReads].map((request) => ({
    method: request.method(), path: new URL(request.url()).pathname,
    resource_type: request.resourceType(), response_status: responseStatuses.get(request) ?? null,
  }));
  if (page) {
    seen.canvas_at_failure = await page.evaluate(() => ({
      covered: Boolean(document.querySelector('.gate')),
      nodes: document.querySelectorAll('.react-flow__node').length,
      naming_fields: document.querySelectorAll('.nm-field').length,
    })).catch(() => null);
  }
  process.exitCode = 1;
}).finally(async () => {
  clearInterval(progress);
  // Never take recovery-code screenshots or emit credentials in the fixture's artifacts.
  if (peer) await peer.close().catch(() => {});
  if (page) await page.close().catch(() => {});
  if (browser) await browser.close().catch(() => {});
  if (scratch) {
    try {
      const folder = join(scratch, 'project');
      if (JSON.parse(vault(folder, 'status')).encrypted) {
        requestLock(folder);
        await drainedLock(folder);
      }
      rmSync(scratch, { recursive: true, force: true });
    } catch {
      seen.cleanup_pending = 'fixture backing retained because its lock did not complete';
      failures.push('fixture mount cleanup');
    }
  }
  console.log(JSON.stringify({ seen, failures }, null, 2));
  if (failures.length) process.exitCode = 1;
});
