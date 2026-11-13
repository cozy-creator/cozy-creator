// The real browser/Calcifer/securefs path. Synthetic project only; no requests are mocked.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { dirname, join } = require('path');
const { holderToken, projectFolder, calcifer, signIn, launch, png, MP4, until, drop, openCanvas, importFile, projectJson, projectUrl, settled, fitCanvas, pointAt } = require('./page');
const failures = [], seen = {};
let browser, context, page, peer, scratch;
const createdFolders = [];
let stage = 'starting browser';
let reportedStage;
const progress = process.env.COZY_TEST_PROGRESS ? setInterval(() => {
  if (stage !== reportedStage) { reportedStage = stage; console.error(`[local-vault] ${stage}`); }
}, 250) : undefined;
progress?.unref();
const errors = [];
const projectReads = new Set();
const responseStatuses = new WeakMap();
const responseCachePolicies = new WeakMap();
const readIds = new WeakMap();
const readStarts = new WeakMap();
const readEndings = [];
const retiredReads = [];
let nextReadId = 0;
let cleaningUp = false;
const readMetadata = (request) => {
  const timing = request.timing();
  const failure = request.failure();
  const source = readStarts.get(request);
  return {
    request_id: readIds.get(request) ?? null, method: request.method(), path: new URL(request.url()).pathname,
    resource_type: request.resourceType(), response_status: responseStatuses.get(request) ?? null,
    response_immutable: responseCachePolicies.get(request)?.immutable ?? null,
    response_no_store: responseCachePolicies.get(request)?.noStore ?? null,
    age_ms: timing.startTime > 0 ? Math.round(Date.now() - timing.startTime) : null,
    observed_age_ms: source ? Date.now() - source.at : null,
    creation_phase: source?.phase ?? null, owner: source?.owner ?? null,
    owner_closed: source?.page?.isClosed() ?? null, frame_detached: source?.frame?.isDetached() ?? null,
    response_start_ms: timing.responseStart, response_end_ms: timing.responseEnd,
    failure: failure ? failure.errorText.match(/^(?:net::)?[A-Z_0-9]+$/)?.[0] ?? 'failed' : null,
  };
};
const retireRead = (request, reason) => {
  if (!projectReads.delete(request)) return;
  retiredReads.push({ reason, during_cleanup: cleaningUp, retired_phase: stage, ...readMetadata(request) });
  if (retiredReads.length > 32) retiredReads.shift();
  seen.retired_project_reads = retiredReads;
};
const goto = async (tab, url) => {
  const old = [...projectReads].filter((request) => readStarts.get(request)?.page === tab);
  const response = await tab.goto(url);
  // Only requests owned by the successfully replaced document are retired.
  // Requests created by the new document during navigation remain tracked.
  for (const request of old) retireRead(request, 'document_replaced');
  return response;
};
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
  tab.on('close', () => {
    for (const request of projectReads) {
      if (readStarts.get(request)?.page === tab) retireRead(request, 'owner_page_closed');
    }
  });
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
  const sourceFolders = () => readdirSync(scratch).filter((name) => name.startsWith('.cozy-project-source-')).sort();
  const picture = join(scratch, 'private_vault_picture.png');
  writeFileSync(picture, png(23));
  cli('new', '--unencrypted', folder);
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
  const isVaultWrite = (request) => {
    const path = new URL(request.url()).pathname;
    return (request.method() === 'POST' && /\/vault$/.test(path)) ||
      (request.method() === 'PUT' && /\/vault\/operations\/[^/]+$/.test(path));
  };
  const isProjectRead = (request) => {
    const url = new URL(request.url());
    return (request.method() === 'GET' || request.method() === 'HEAD') &&
      (url.pathname.startsWith(`/v1/projects/${id}`) || url.searchParams.get('project') === id);
  };
  const readEnded = (event, request) => {
    const tracked = projectReads.delete(request);
    if (readIds.has(request) || isProjectRead(request)) {
      readEndings.push({ event, tracked, during_cleanup: cleaningUp, ...readMetadata(request) });
      if (readEndings.length > 64) readEndings.shift();
    }
  };
  // Lifecycle polling continues while the controls are open. Drain content
  // requests while retaining every request in the diagnostics.
  const contentReads = () => [...projectReads].filter((request) => {
    const path = new URL(request.url()).pathname;
    return path !== `/v1/projects/${id}/vault` && path !== `/v1/projects/${id}/address`;
  });
  const plainCacheBody = (request) => {
    const url = new URL(request.url()), policy = responseCachePolicies.get(request);
    return request.method() === 'GET' && url.origin === new URL(calcifer()).origin &&
      url.pathname.startsWith(`/v1/projects/${id}/thumbs/`) && responseStatuses.get(request) === 200 &&
      policy?.immutable === true && policy.noStore === false;
  };
  const plainStorageReady = async (label) => {
    const status = JSON.parse(vault(folder, 'status'));
    if (status.encrypted !== false || status.state !== 'plain') throw new Error('plain-storage readiness requires an actually plain project');
    // A plain immutable thumbnail has released Project/ACCESS before its headers.
    // Its remaining global-cache body is not part of the project storage cutover.
    // Private no-store thumbnails and every other content request still drain.
    await until(label, () => contentReads().filter((request) => !plainCacheBody(request)).length, (count) => count === 0, 45000);
    (seen.plain_storage_readiness ??= []).push({ label, global_cache_bodies: contentReads().filter(plainCacheBody).map(readMetadata) });
  };
  const pageError = (error) => errors.push(error.message);
  page.on('pageerror', pageError);
  context.on('requestfinished', (request) => readEnded('requestfinished', request));
  context.on('requestfailed', (request) => readEnded('requestfailed', request));
  context.on('request', (request) => {
    const url = new URL(request.url());
    if (isProjectRead(request)) {
      if (!readIds.has(request)) {
        readIds.set(request, ++nextReadId);
        let frame, owner;
        try { frame = request.frame(); owner = frame.page(); } catch { /* retain unknown owners */ }
        readStarts.set(request, { at: Date.now(), phase: stage, page: owner, frame, owner: owner === page ? 'main' : owner === peer ? 'peer' : 'unknown' });
      }
      projectReads.add(request);
    }
    if (request.method() === 'PUT' && /^\/v1\/(project-)?files\//.test(url.pathname)) fileWrites.push({ path: url.pathname, project: url.searchParams.get('project') });
    const secretCreation = url.pathname === '/v1/projects' && !JSON.parse(request.postData() || '{}').unencrypted;
    if ((isVaultWrite(request) || (request.method() === 'POST' && secretCreation)) && request.headers()['idempotency-key']) secretIdempotency.push(url.pathname);
    if (isVaultWrite(request) && url.pathname.startsWith(`/v1/projects/${id}/vault`) && JSON.parse(request.postData() || '{}').action === 'encrypt') {
      (seen.reads_at_encrypt_submit ??= []).push([...projectReads].map(readMetadata));
    }
    let text = `${request.url()} ${request.postData() || ''}`;
    try { text += ` ${decodeURIComponent(text)}`; } catch { /* a binary or malformed URL is still checked as sent */ }
    if (url.origin !== new URL(calcifer()).origin && /cozy1-|synthetic browser passphrase/.test(text)) outbound.push('secret-bearing external request');
  });
  context.on('response', async (response) => {
    responseStatuses.set(response.request(), response.status());
    const cache = (response.headers()['cache-control'] ?? '').toLowerCase().split(',').map((part) => part.trim());
    responseCachePolicies.set(response.request(), { immutable: cache.includes('immutable'), noStore: cache.includes('no-store') });
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
    if (!isVaultWrite(response.request())) return;
    try {
      const action = JSON.parse(response.request().postData() || '{}').action;
      if (action === 'activity') {
        const answer = await response.json().catch(() => ({}));
        (seen.activity_replies ??= []).push({ status: response.status(), code: answer.error?.code });
        return;
      }
      if ((response.headers()['content-type'] ?? '').startsWith('text/event-stream')) {
        // The page consumes and aborts this stream after its terminal event. Do
        // not clone or retain a second copy of its recovery-bearing body.
        (seen.vault_replies ??= []).push({ action, status: response.status(), stream: true, no_store: cache.includes('no-store') });
        return;
      }
      const answer = await response.json();
      (seen.vault_replies ??= []).push({ action, status: response.status(), encrypted: answer.vault?.encrypted, state: answer.vault?.state, has_recovery_secret: typeof answer.recovery_secret === 'string', error_code: answer.error?.code, error_kind: answer.error?.kind, error_message: answer.error ? sanitized(answer.error.message) : undefined });
    } catch { /* Failed/offline replies carry no readable evidence. Never retain secret bodies. */ }
  });
  await signIn(page);
  // Code redemption finishes after the document loads, so close that document
  // explicitly before tracking the vault page's current content requests.
  await page.locator('.top').waitFor();
  const authenticated = page;
  page = await newPage();
  page.setDefaultTimeout(45000);
  authenticated.off('pageerror', pageError);
  page.on('pageerror', pageError);
  await authenticated.close();
  seen.initial_document_isolation = { auth_page_closed: authenticated.isClosed() };
  // This tab opens the page on its own.
  await signIn(page, `${href}/vault`);
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
  await plainStorageReady('initial project storage reads finished before conversion');
  check('initial_project_reads_drained', true);
  await body.locator('input[type=password]').nth(0).fill('synthetic browser passphrase');
  await body.locator('input[type=password]').nth(1).fill('synthetic browser passphrase');
  await body.getByText('Lock timing', { exact: true }).click();
  check('ui_default_idle_is_thirty_minutes', await body.getByLabel('Lock after inactivity (seconds; 0 disables)').inputValue() === '1800');
  check('ui_encryption_offers_no_plaintext_copy', await body.getByLabel('Keep an unencrypted copy after encryption').count() === 0);
  const sourcesBeforeEncryption = sourceFolders();
  const encryptionReply = page.waitForResponse((response) => isVaultWrite(response.request()) && new URL(response.url()).pathname.startsWith(`/v1/projects/${id}/vault`) && JSON.parse(response.request().postData() || '{}').action === 'encrypt');
  await body.getByRole('button', { name: 'Enable encryption', exact: true }).click();
  const encrypted = await encryptionReply;
  if (!encrypted.ok()) {
    const answer = await encrypted.json();
    const alert = body.getByRole('alert');
    await alert.waitFor({ timeout: 5000 }).catch(() => {});
    seen.encryption_refusal = { status: encrypted.status(), code: answer.error?.code, kind: answer.error?.kind, message: sanitized(answer.error?.message), visible_alert: (await alert.allTextContents()).map(sanitized) };
    throw new Error(`initial encryption refused: ${encrypted.status()} ${sanitized(answer.error?.code)}`);
  }
  check('vault_receipt_is_streamed', (encrypted.headers()['content-type'] ?? '').startsWith('text/event-stream') && (encrypted.headers()['cache-control'] ?? '').includes('no-store'));
  await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
  const recovery = await page.locator('[aria-label="Save recovery secret"] pre').innerText();
  check('ui_encryption_removes_its_source', JSON.stringify(sourceFolders()) === JSON.stringify(sourcesBeforeEncryption));
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
  await goto(page, href);
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
  // Unlocked, a right click's Download is the person's own export: the file's bytes, named after its box (cl-891).
  stage = 'private download';
  await page.locator('.react-flow__node:has(.card[data-asset="private_after_encryption"]) .nm').first().click({ button: 'right' });
  const [saved] = await Promise.all([page.waitForEvent('download'), page.locator('.asset-menu').getByRole('menuitem', { name: 'Download', exact: true }).click()]);
  const savedBytes = readFileSync(await saved.path());
  check('private_download_is_its_file', saved.suggestedFilename() === 'private_after_encryption.png' && savedBytes.equals(privateBytes), { name: saved.suggestedFilename(), bytes: savedBytes.length, wanted: privateBytes.length });

  // Real browser network loss must cover private content without destroying its
  // in-memory pending edit. Hiding and showing the same tab adds no lock authority.
  projectJson(folder, 'POST', '/assets', { tool: '', name: 'private_pending_words', exact: true, fields: { text: 'before interruption' } });
  await goto(page, `${href}/assets/private_pending_words`);
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
  await goto(peer, 'about:blank');
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
  await goto(page, href);
  await readyCanvas();

  // Route changes also retire only the drawn canvas, never an unlocked project's
  // pending upload. Exercise the real private upload/retry path across A -> B -> A.
  const ordinary = join(scratch, 'other-project');
  cli('new', '--unencrypted', ordinary);
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
  await signIn(peer, href);
  await peer.locator('.react-flow').waitFor();
  requestLock(folder);
  for (const tab of [page, peer]) {
    await until('external lock concealed a tab', () => tab.locator('body').innerText(), (text) => !text.includes('private_vault_picture') && !text.includes('private_after_encryption'), 20000);
    check(`external_lock_tab_${tab === page ? 'first' : 'second'}`, await tab.locator('.react-flow').count() === 0);
  }
  await drainedLock(folder);
  // A fresh document has no remembered address. The compact link must still
  // open the unlock UI without exposing or opening the private project.
  await goto(peer, href);
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
    await goto(peer, 'about:blank');
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

  await goto(page, `${href}/vault`);
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

  stage = 'decrypt waits for fresh plain status';
  const openSection = async (name) => {
    const summary = page.locator('summary').filter({ hasText: new RegExp(`^${name}$`) });
    if (!await summary.evaluate((element) => element.parentElement.open)) await summary.click();
  };
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await openSection('Lock timing');
  await page.getByLabel('Lock after inactivity (seconds; 0 disables)').fill('0');
  await page.getByLabel('Maximum unlock duration (seconds; 0 disables)').fill('0');
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await until('unlocked before decryption', () => page.locator('.drawer [data-vault-state]').getAttribute('data-vault-state'), (state) => state === 'unlocked', 45000);
  await page.locator('.react-flow').waitFor();
  const beforeDecrypt = projectJson(folder);
  const assetDigests = (project) => project.assets.map((asset) => [asset.name, asset.sha256]).sort(([left], [right]) => left.localeCompare(right));
  const oldCanvas = await page.locator('.react-flow').elementHandle();
  // Delay transport of real responses only. No status body or production handler is replaced.
  const statusPath = (url) => url.pathname === `/v1/projects/${id}/vault`;
  const heldStatuses = [];
  let holdStatus = true, captureOldStatus = true, oldStatus, statusTransportError;
  const statusRoute = async (route) => {
    if (route.request().method() !== 'GET' || !holdStatus) return route.continue();
    if (captureOldStatus) {
      captureOldStatus = false;
      try {
        const response = await route.fetch({ headers: { ...route.request().headers(), authorization: `Bearer ${holderToken()}` } });
        oldStatus = { route, response, state: (await response.json()).state };
      } catch (error) { statusTransportError = error; }
    } else heldStatuses.push(route);
  };
  const liveStatuses = () => heldStatuses.filter((route) => !route.request().failure());
  const deliver = async (route, response) => {
    try {
      if (response) await route.fulfill({ response });
      else await route.continue();
    } catch (error) {
      if (!route.request().failure()) throw error;
    }
  };
  const resumeStatus = async () => {
    holdStatus = false;
    for (const route of heldStatuses.splice(0)) await deliver(route);
  };
  const covered = () => page.evaluate(() => Boolean(document.querySelector('.gate')) && !document.querySelector('.react-flow') && !/private_vault_picture|private_after_encryption/.test(document.body.innerText));
  await page.route(statusPath, statusRoute);
  // Nothing polls the status (xs-189): the drawer opened again reads it, as a person coming back to it does, and that read is the old one.
  await navigate(href);
  await navigate(`${href}/vault`);
  await until('actual old unlocked status held', () => {
    if (statusTransportError) throw statusTransportError;
    return oldStatus?.state;
  }, (state) => state === 'unlocked', 45000);
  await openSection('Decrypt project');
  await page.getByRole('button', { name: 'Decrypt project', exact: true }).click();
  await until('same project converted to plain', () => JSON.parse(vault(folder, 'status')), (status) => status.encrypted === false && status.state === 'plain', 45000);
  await until('plain event covers old private view', covered, Boolean, 45000);
  check('decrypt_purges_private_canvas_before_status', oldCanvas !== null && !await oldCanvas.evaluate((element) => element.isConnected));
  check('decrypt_keeps_browser_persistence_private_until_status', await page.evaluate((project) => !['cozy.view:', 'cozy.selection:', 'cozy.more:'].some((prefix) => localStorage.getItem(prefix + project) !== null), id));
  await deliver(oldStatus.route, oldStatus.response);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  check('pre_decrypt_status_cannot_reopen_private_view', await covered());
  await until('fresh status request held', () => liveStatuses().length, (count) => count > 0, 45000);
  const failedStatus = liveStatuses()[0];
  heldStatuses.splice(heldStatuses.indexOf(failedStatus), 1);
  await failedStatus.abort('failed');
  await until('fresh status network error observed', () => failedStatus.request().failure(), Boolean, 45000);
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  check('failed_plain_status_recheck_keeps_view_covered', await covered());
  await resumeStatus();
  await until('fresh plain status reveals project', () => page.locator('.drawer [data-vault-state]').getAttribute('data-vault-state'), (state) => state === 'plain', 45000);
  await page.locator('.react-flow').waitFor();
  const decrypted = projectJson(folder);
  check('decrypt_preserves_project_identity_and_assets', decrypted.id === id && JSON.stringify(assetDigests(decrypted)) === JSON.stringify(assetDigests(beforeDecrypt)));
  await oldCanvas.dispose();
  await page.unroute(statusPath, statusRoute);
  await goto(page, href);
  await readyCanvas();
  const plainAt = await openCanvas(page);
  // An ordinary project's viewport is kept in its project.db, not the browser.
  const viewBefore = JSON.stringify(projectJson(folder, 'GET', '/view').viewport);
  await page.mouse.move(plainAt.x, plainAt.y);
  await page.mouse.down();
  await page.mouse.move(plainAt.x + 24, plainAt.y + 16, { steps: 4 });
  await page.mouse.up();
  await until('ordinary viewport persists after fresh plain status', () => projectJson(folder, 'GET', '/view').viewport, (viewport) => viewport !== null && JSON.stringify(viewport) !== viewBefore, 45000);
  check('fresh_plain_status_allows_view_persistence', true);
  const plainImported = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/v1/projects/${id}/imports` && response.ok());
  const plainImportAt = await openCanvas(page);
  await drop(page, [{ name: 'after_decrypt_plain.png', type: 'image/png', bytes: png(97) }], plainImportAt.x, plainImportAt.y);
  await page.locator('.nm-field').waitFor();
  await page.keyboard.press('Enter');
  await plainImported;
  await until('ordinary edit lands after decryption', () => projectJson(folder).assets.some((asset) => asset.name === 'after_decrypt_plain' && asset.sha256), Boolean, 45000);
  check('fresh_plain_status_allows_editing', true);

  stage = 'UI encryption after in-place decryption';
  await goto(page, `${href}/vault`);
  await page.locator('.drawer [data-vault-state="plain"]').waitFor();
  await until('ordinary imported media ready for encryption', () => {
    const state = projectJson(folder);
    return !state.busy && !state.moving && state.assets.find((asset) => asset.name === 'after_decrypt_plain')?.width > 0;
  }, Boolean, 45000);
  await plainStorageReady('ordinary project storage reads drained before re-encryption');
  await page.locator('.drawer input[type=password]').nth(0).fill('synthetic browser passphrase');
  await page.locator('.drawer input[type=password]').nth(1).fill('synthetic browser passphrase');
  const sourcesBeforeReencryption = sourceFolders();
  await page.getByRole('button', { name: 'Enable encryption', exact: true }).click();
  await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
  const replacementRecovery = await page.locator('[aria-label="Save recovery secret"] pre').innerText();
  check('ui_reencryption_removes_its_source', JSON.stringify(sourceFolders()) === JSON.stringify(sourcesBeforeReencryption));
  check('ui_reencryption_says_source_removed', await page.getByText('The unencrypted project was removed.', { exact: true }).isVisible());
  check('decrypted_project_can_be_encrypted_again', projectJson(folder).id === id && JSON.parse(vault(folder, 'status')).encrypted === true);

  stage = 'plain recheck observes a later locked generation';
  holdStatus = true; captureOldStatus = false;
  await page.route(statusPath, statusRoute);
  await openSection('Decrypt project');
  await page.getByRole('button', { name: 'Decrypt project', exact: true }).click();
  await until('second in-place decryption complete', () => JSON.parse(vault(folder, 'status')), (status) => status.encrypted === false && status.state === 'plain', 45000);
  await until('second plain transition awaits status', covered, Boolean, 45000);
  check('decrypt_removes_one_time_recovery_disclosure', await page.locator('[aria-label="Save recovery secret"]').count() === 0 && !(await page.locator('body').innerText()).includes(replacementRecovery));
  const competing = JSON.parse(execFileSync(process.env.COZY, ['--json', 'vault', '--directory', folder, 'encrypt', '--passphrase-file', '-', '--idle', '0'], { cwd: scratch, input: 'synthetic browser passphrase\n', encoding: 'utf8', timeout: 180000 }));
  if (!competing.vault.encrypted || competing.vault.state !== 'unlocked') throw new Error('competing encryption did not finish unlocked');
  delete competing.recovery_secret;
  requestLock(folder);
  await drainedLock(folder);
  await until('new encrypted generation concealed', covered, Boolean, 45000);
  await resumeStatus();
  await until('fresh status confirms current generation locked', () => page.locator('[data-vault-state="locked"]').count(), (count) => count > 0, 45000);
  check('plain_recheck_respects_current_locked_generation', await covered() && await page.locator('[aria-label="Save recovery secret"]').count() === 0);
  await page.unroute(statusPath, statusRoute);
  stage = 'default encrypted project creation';
  const origin = new URL(href).origin;
  const saveCreationRecovery = async (suffix = '') => {
    await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
    await page.getByLabel('I saved this somewhere away from this computer').check();
    await page.getByRole('button', { name: 'Done', exact: true }).click();
    await page.waitForURL(new RegExp(`/p/[0-9A-HJKMNP-TV-Z]{9}${suffix}$`));
  };
  await goto(page, `${origin}/projects`);
  check('new_form_defaults_to_encryption', !(await page.getByLabel('Create without encryption', { exact: true }).isChecked()));
  await page.getByLabel("New project's folder", { exact: true }).fill(projectFolder(`web-vault-new-${Date.now()}`));
  await page.getByLabel('Passphrase', { exact: true }).fill('synthetic new project passphrase');
  await page.getByLabel('Confirm passphrase', { exact: true }).fill('synthetic new project passphrase');
  const creationReply = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/projects' && answer.ok());
  await page.getByRole('button', { name: 'Create project', exact: true }).click();
  const createdResponse = await creationReply;
  const newlyCreated = await createdResponse.json();
  const newRecovery = newlyCreated.recovery_secret;
  delete newlyCreated.recovery_secret;
  createdFolders.push(newlyCreated.path);
  check('new_creation_response_is_private', /no-store/.test(createdResponse.headers()['cache-control'] || '') && !createdResponse.request().headers()['idempotency-key']);
  await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
  check('new_creation_discloses_recovery_locally', Boolean(newRecovery) && await page.locator('[aria-label="Save recovery secret"] pre').innerText() === newRecovery);
  await saveCreationRecovery();
  check('new_creation_has_no_plaintext_database', !existsSync(join(newlyCreated.path, 'project.db')) && JSON.parse(vault(newlyCreated.path, 'status')).encrypted);
  check('new_creation_default_idle_is_thirty_minutes', JSON.parse(vault(newlyCreated.path, 'status')).idle_timeout_s === 1800);
  const persisted = await page.evaluate(() => JSON.stringify(localStorage));
  check('new_creation_credentials_are_not_browser_storage', !persisted.includes(newRecovery) && !persisted.includes('synthetic new project passphrase'));

  stage = 'implicit canvas creation waits for secure setup';
  // A global route keeps an unowned canvas behind its drawer, so this actually
  // exercises first-drop setup.
  await goto(page, `${origin}/runs`);
  await page.locator('.empty-canvas').waitFor();
  const uploadsBeforeSetup = fileWrites.length;
  const emptyAt = await openCanvas(page);
  await drop(page, [{ name: 'new_private_drop.png', type: 'image/png', bytes: png(109) }], emptyAt.x, emptyAt.y);
  const setup = page.getByRole('region', { name: 'New project', exact: true });
  await setup.waitFor();
  check('implicit_creation_waits_before_upload', fileWrites.length === uploadsBeforeSetup);
  await setup.getByLabel('Passphrase', { exact: true }).fill('synthetic implicit project passphrase');
  await setup.getByLabel('Confirm passphrase', { exact: true }).fill('synthetic implicit project passphrase');
  const implicitReply = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/projects' && answer.ok());
  await setup.getByRole('button', { name: 'Create project', exact: true }).click();
  const implicit = await (await implicitReply).json();
  delete implicit.recovery_secret;
  createdFolders.push(implicit.path);
  await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
  check('implicit_creation_waits_for_saved_recovery', fileWrites.length === uploadsBeforeSetup);
  // Recovery acknowledgement still verifies the real status on the global route.
  // Hold only the selected project's subsequent refresh, after adoption, without
  // fabricating any status reply. The pending draft must remain private in RAM.
  const adoptionStatus = `**/v1/projects/${implicit.id}/vault`;
  let releaseAdoption;
  const adoptionGate = new Promise((resolve) => { releaseAdoption = resolve; });
  let adoptionReads = 0;
  const holdAdoption = async (route) => {
    if (route.request().method() !== 'GET' || !new URL(page.url()).pathname.startsWith(`/p/${implicit.code}/`)) return route.continue();
    adoptionReads++;
    await adoptionGate;
    await route.continue();
  };
  await page.route(adoptionStatus, holdAdoption);
  try {
    await saveCreationRecovery('/runs');
    await until('adopted project awaits its fresh protection reply', () => adoptionReads, (count) => count > 0, 45000);
    check('implicit_adoption_covers_the_retained_private_draft', await page.locator('.gate').isVisible() && await page.locator('.nm-field').count() === 1 && !(await page.locator('.nm-field').isVisible()));
    const adoptionStored = await page.evaluate((project) => ['cozy.view:', 'cozy.selection:', 'cozy.more:'].some((prefix) => localStorage.getItem(prefix + project) !== null), implicit.id);
    check('implicit_adoption_cannot_persist_as_plain_before_refresh', !adoptionStored);
  } finally {
    releaseAdoption();
    // This is the page's only remaining route. Drain its released callbacks before
    // removing interception, which otherwise auto-continues their paused requests.
    await page.unrouteAll({ behavior: 'wait' });
  }
  const importedName = page.getByRole('textbox', { name: 'Name', exact: true });
  await importedName.waitFor();
  await importedName.fill('new_private_drop');
  await importedName.press('Enter');
  await until('implicit encrypted import lands', () => projectJson(implicit.path).assets.some((asset) => asset.name === 'new_private_drop' && asset.sha256), Boolean, 45000);
  const privatePan = await openCanvas(page);
  await page.mouse.move(privatePan.x, privatePan.y);
  await page.mouse.down();
  await page.mouse.move(privatePan.x + 24, privatePan.y + 16, { steps: 4 });
  await page.mouse.up();
  await settled(page);
  check('implicit_project_stays_private_after_fresh_refresh', !(await page.evaluate((project) => ['cozy.view:', 'cozy.selection:', 'cozy.more:'].some((prefix) => localStorage.getItem(prefix + project) !== null), implicit.id)));
  check('implicit_creation_uses_private_uploads', fileWrites.slice(uploadsBeforeSetup).length > 0 && fileWrites.slice(uploadsBeforeSetup).every((write) => write.path.startsWith('/v1/project-files/') && write.project === implicit.id));
  check('implicit_creation_is_encrypted', JSON.parse(vault(implicit.path, 'status')).encrypted && !existsSync(join(implicit.path, 'project.db')));

  const prepareCreation = async (suffix) => {
    await goto(page, `${origin}/projects`);
    await page.getByLabel("New project's folder", { exact: true }).fill(projectFolder(`web-vault-${suffix}-${Date.now()}`));
    await page.getByLabel('Passphrase', { exact: true }).fill('synthetic pending project passphrase');
    await page.getByLabel('Confirm passphrase', { exact: true }).fill('synthetic pending project passphrase');
  };
  stage = 'new recovery panel observes external lock';
  await prepareCreation('lock');
  const visibleReply = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/projects' && answer.ok());
  await page.getByRole('button', { name: 'Create project', exact: true }).click();
  const visible = await (await visibleReply).json();
  delete visible.recovery_secret;
  createdFolders.push(visible.path);
  await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
  requestLock(visible.path);
  await drainedLock(visible.path);
  await until('new recovery disappears after lock', () => page.locator('[aria-label="Save recovery secret"]').count(), (count) => count === 0, 45000);
  check('new_recovery_is_purged_on_lock', true);

  stage = 'late new-project response cannot disclose after lock';
  await prepareCreation('late');
  let releaseCreation;
  const delivery = new Promise((resolve) => { releaseCreation = resolve; });
  let received;
  const heldCreate = async (route) => {
    if (route.request().method() !== 'POST') return route.continue();
    const answer = await route.fetch({ headers: { ...route.request().headers(), authorization: `Bearer ${holderToken()}` } });
    const created = await answer.json();
    delete created.recovery_secret;
    if (created.path) createdFolders.push(created.path);
    received = created;
    await delivery;
    await route.fulfill({ response: answer });
  };
  await page.route('**/v1/projects', heldCreate);
  try {
    await page.getByRole('button', { name: 'Create project', exact: true }).click();
    await until('real creation response is held', () => received, Boolean, 45000);
    const late = received;
    if (!late.path) throw new Error('encrypted creation failed before the held-response check');
    requestLock(late.path);
    await drainedLock(late.path);
    const delivered = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/projects' && answer.ok());
    releaseCreation();
    await (await delivered).finished();
    await until('late creation handler settles', () => page.getByRole('button', { name: 'Creating…', exact: true }).count(), (count) => count === 0, 45000);
    check('late_creation_does_not_disclose_recovery_after_lock', await page.locator('[aria-label="Save recovery secret"]').count() === 0);
  } finally {
    releaseCreation();
    await page.unroute('**/v1/projects', heldCreate);
  }
  await goto(page, `${origin}/projects`);
  for (const directory of createdFolders) {
    requestLock(directory);
    await drainedLock(directory);
    rmSync(directory, { recursive: true, force: true });
  }

  check('no_external_secret_delivery', outbound.length === 0);
  check('secret_actions_skip_idempotency_cache', secretIdempotency.length === 0);
  const activityReplies = seen.activity_replies ?? [];
  check('activity_updates_are_supported', activityReplies.some((reply) => reply.status === 200) && activityReplies.every((reply) => reply.status === 200 || (reply.status === 409 && reply.code === 'project_busy')));
  check('no_page_errors', errors.length === 0);
  seen.page_errors = errors;
})().catch(async (error) => {
  failures.push('browser vault flow');
  seen.failure = sanitized(error.message);
  seen.failure_stage = stage;
  seen.page_errors = errors.map(sanitized);
  seen.pending_project_reads = [...projectReads].map(readMetadata);
  seen.project_read_endings = readEndings;
  if (page) {
    seen.canvas_at_failure = await page.evaluate(() => ({
      covered: Boolean(document.querySelector('.gate')),
      nodes: document.querySelectorAll('.react-flow__node').length,
      naming_fields: document.querySelectorAll('.nm-field').length,
    })).catch(() => null);
  }
  process.exitCode = 1;
}).finally(async () => {
  cleaningUp = true;
  clearInterval(progress);
  // Never take recovery-code screenshots or emit credentials in the fixture's artifacts.
  if (peer) await peer.close().catch(() => {});
  if (page) await page.close().catch(() => {});
  if (browser) await browser.close().catch(() => {});
  for (const directory of createdFolders) {
    if (!existsSync(directory)) continue;
    try {
      requestLock(directory);
      await drainedLock(directory);
      rmSync(directory, { recursive: true, force: true });
    } catch {
      seen.creation_cleanup_pending = 'new fixture backing retained because its lock did not complete';
      failures.push('new fixture mount cleanup');
    }
  }
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
