// The real browser/Calcifer/securefs path. Synthetic project only; no requests are mocked.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { calcifer, signIn, launch, png, MP4, until, drop } = require('./page');
const failures = [], seen = {};
let browser, context, page, peer, scratch;
const check = (name, okay) => { seen[name] = !!okay; if (!okay) failures.push(name); };
const cli = (...args) => execFileSync(process.env.COZY, ['--json', ...args], { encoding: 'utf8', timeout: 180000 });
(async () => {
  scratch = mkdtempSync(join(tmpdir(), 'web-vault-'));
  const folder = join(scratch, 'project');
  const picture = join(scratch, 'private_vault_picture.png');
  writeFileSync(picture, png(23));
  cli('project', 'new', folder);
  cli('project', '-p', folder, 'import', picture);
  const movie = join(scratch, 'private_loop_movie.mp4');
  writeFileSync(movie, MP4);
  cli('project', '-p', folder, 'import', movie);
  const id = JSON.parse(cli('project', '-p', folder, 'status')).id;
  seen.project = { id, folder };
  browser = await launch(playwright);
  context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  page = await context.newPage();
  page.setDefaultTimeout(45000);
  const errors = [], outbound = [], fileWrites = [], secretIdempotency = [];
  page.on('pageerror', (error) => errors.push(error.message));
  context.on('request', (request) => {
    const url = new URL(request.url());
    if (request.method() === 'PUT' && /^\/v1\/(project-)?files\//.test(url.pathname)) fileWrites.push({ path: url.pathname, project: url.searchParams.get('project') });
    if (request.method() === 'POST' && /\/vault$/.test(url.pathname) && request.headers()['idempotency-key']) secretIdempotency.push(url.pathname);
    let text = `${request.url()} ${request.postData() || ''}`;
    try { text += ` ${decodeURIComponent(text)}`; } catch { /* a binary or malformed URL is still checked as sent */ }
    if (url.origin !== new URL(calcifer()).origin && /cozy1-|synthetic browser passphrase/.test(text)) outbound.push('secret-bearing external request');
  });
  await signIn(page);
  await page.goto(`${calcifer()}/#/projects/${id}/vault`);
  const body = page.locator('.drawer [data-vault-state]');
  await body.locator('input[type=password]').nth(0).fill('synthetic browser passphrase');
  await body.locator('input[type=password]').nth(1).fill('synthetic browser passphrase');
  await body.getByLabel('Remove the verified unencrypted source after conversion').check();
  await body.getByRole('button', { name: 'Enable encryption', exact: true }).click();
  await page.locator('[aria-label="Save recovery secret"] pre').waitFor();
  const recovery = await page.locator('[aria-label="Save recovery secret"] pre').innerText();
  check('recovery_is_long_and_local', recovery.startsWith('cozy1-') && recovery.length > 70);
  check('qr_is_generated_in_page', await page.locator('[aria-label="Save recovery secret"] svg').count() === 1);
  await page.getByLabel('I saved this somewhere away from this computer').check();
  await page.locator('[aria-label="Save recovery secret"]').getByRole('button', { name: 'Done', exact: true }).click();
  check('one_time_secret_dismissed', await page.locator('[aria-label="Save recovery secret"]').count() === 0);
  await page.goto(`${calcifer()}/#/projects/${id}`);
  await page.locator('.react-flow').waitFor();
  check('unlocked_canvas', await page.getByText('private_vault_picture', { exact: true }).count() > 0);
  const privateBytes = Buffer.concat([png(24), require('crypto').randomBytes(32)]);
  const imported = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/v1/projects/${id}/imports` && response.ok());
  await drop(page, [{ name: 'private_after_encryption.png', type: 'image/png', bytes: privateBytes }], 600, 400);
  await imported;
  await until('encrypted upload landed', () => page.locator('body').innerText(), (text) => text.includes('private_after_encryption'), 45000);
  check('private_upload_uses_project_route', fileWrites.length > 0 && fileWrites.every((write) => write.path.startsWith('/v1/project-files/') && write.project === id));
  const stored = await page.evaluate(() => JSON.stringify(Object.fromEntries(Object.keys(localStorage).map((key) => [key, localStorage.getItem(key)]))));
  check('no_private_browser_persistence', !stored.includes(recovery) && !stored.includes('synthetic browser passphrase') && !stored.includes('private_vault_picture') && !stored.includes('private_after_encryption'));
  // Automatic hover previews are not user activity. The real video decoder keeps
  // looping, while an idle-only deadline still locks the project.
  await page.locator('.react-flow__node').filter({ hasText: 'private_loop_movie' }).first().hover();
  await page.waitForFunction(() => [...document.querySelectorAll('video')].some((el) => el.loop && !el.paused && el.currentTime > 0.05));
  execFileSync(process.env.COZY, ['--json', 'project', '-p', folder, 'unlock', '--recovery-file', '-', '--idle', '2', '--max-duration', '0'], { input: `${recovery}\n`, encoding: 'utf8', timeout: 180000 });
  await page.locator('[data-vault-state="locked"]').waitFor();
  check('autoplay_preview_does_not_extend_idle_unlock', await page.locator('.react-flow').count() === 0);
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await page.locator('.react-flow').waitFor();

  // A second actual tab shares the real SSE connection. Locking from the CLI must
  // conceal both pages without relying on the active tab's one-second status poll.
  peer = await context.newPage();
  peer.on('pageerror', (error) => errors.push(error.message));
  await peer.goto(`${calcifer()}/#/projects/${id}`);
  await peer.locator('.react-flow').waitFor();
  cli('project', '-p', folder, 'lock');
  for (const tab of [page, peer]) {
    await until('external lock concealed a tab', () => tab.locator('body').innerText(), (text) => !text.includes('private_vault_picture') && !text.includes('private_after_encryption'), 20000);
    check(`external_lock_tab_${tab === page ? 'first' : 'second'}`, await tab.locator('.react-flow').count() === 0);
  }
  await peer.close(); peer = null;
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await page.locator('.react-flow').waitFor();

  // Chromium's actual page lifecycle suspends JavaScript while the CLI locks the
  // project. Resuming the hidden tab must verify protection before showing its canvas.
  if (!process.env.ENGINE || process.env.ENGINE === 'chromium') {
    const lifecycle = await context.newCDPSession(page);
    peer = await context.newPage();
    await peer.goto('about:blank');
    await peer.bringToFront();
    await until('private tab hidden', () => page.evaluate(() => document.visibilityState), (state) => state === 'hidden', 10000);
    await lifecycle.send('Page.setWebLifecycleState', { state: 'frozen' });
    try { cli('project', '-p', folder, 'lock'); }
    finally { await lifecycle.send('Page.setWebLifecycleState', { state: 'active' }); }
    await page.bringToFront();
    await page.locator('[data-vault-state="locked"]').waitFor();
    check('frozen_tab_resume_keeps_locked_data_hidden', await page.locator('.react-flow').count() === 0 && !(await page.locator('body').innerText()).includes('private_vault_picture'));
    await lifecycle.detach();
    await peer.close(); peer = null;
    await page.getByLabel('Unlock with').selectOption('recovery');
    await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
    await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
    await page.locator('.react-flow').waitFor();
  } else seen.frozen_tab_resume = 'Chromium lifecycle proof only; this engine still checks tabs and browser history';

  // Leaving A stops its selected-project status observer. Keep the same document
  // and query cache, lock A while B is shown, then watch every DOM mutation on return.
  const ordinary = join(scratch, 'other-project');
  cli('project', 'new', ordinary);
  const other = JSON.parse(cli('project', '-p', ordinary, 'status')).id;
  await page.evaluate(() => { window.__vaultDocument = 'same-document'; });
  await page.goto(`${calcifer()}/#/projects/${other}`);
  await page.waitForFunction(() => document.querySelector('.react-flow') && !document.body.innerText.includes('private_vault_picture'));
  cli('project', '-p', folder, 'lock');
  await page.evaluate((id) => {
    window.__privatePaints = [];
    window.__privateWatch = new MutationObserver(() => {
      if (location.hash === `#/projects/${id}` && /private_vault_picture|private_after_encryption/.test(document.body.innerText)) window.__privatePaints.push('private content');
    });
    window.__privateWatch.observe(document.body, { childList: true, subtree: true, characterData: true });
    location.hash = `#/projects/${id}`;
  }, id);
  await page.locator('[data-vault-state="locked"]').waitFor();
  check('project_reselection_verifies_before_paint', await page.evaluate(() => window.__vaultDocument === 'same-document' && window.__privatePaints.length === 0));
  await page.goBack();
  await page.waitForFunction(() => document.querySelector('.react-flow') && !document.body.innerText.includes('private_vault_picture'));
  await page.goForward();
  await page.locator('[data-vault-state="locked"]').waitFor();
  check('history_return_keeps_locked_data_hidden', await page.evaluate(() => window.__privatePaints.length === 0 && !document.querySelector('.react-flow')));
  await page.evaluate(() => window.__privateWatch.disconnect());
  await page.getByLabel('Unlock with').selectOption('recovery');
  await page.getByLabel('Recovery secret', { exact: true }).fill(recovery);
  await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
  await page.locator('.react-flow').waitFor();

  await page.goto(`${calcifer()}/#/projects/${id}/vault`);
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
})().catch((error) => { failures.push('browser vault flow'); seen.failure = error.message.replace(/cozy1(?:-[0-9a-f]{8}){9}/gi, '[REDACTED]').replaceAll('synthetic browser passphrase', '[REDACTED]'); process.exitCode = 1; }).finally(async () => {
  // Never take recovery-code screenshots or emit credentials in the fixture's artifacts.
  if (peer) await peer.close().catch(() => {});
  if (page) await page.close().catch(() => {});
  if (browser) await browser.close().catch(() => {});
  if (scratch) {
    try { cli('project', '-p', join(scratch, 'project'), 'lock'); rmSync(scratch, { recursive: true, force: true }); }
    catch { seen.cleanup_pending = 'fixture backing retained because its lock did not complete'; failures.push('fixture mount cleanup'); }
  }
  console.log(JSON.stringify({ seen, failures }, null, 2));
  if (failures.length) process.exitCode = 1;
});
