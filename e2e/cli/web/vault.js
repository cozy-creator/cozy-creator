// The real browser/Calcifer/securefs path. Synthetic project only; no requests are mocked.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { calcifer, signIn, launch, png, until, drop } = require('./page');
const failures = [], seen = {};
let browser, page, scratch;
const check = (name, okay) => { seen[name] = !!okay; if (!okay) failures.push(name); };
const cli = (...args) => execFileSync(process.env.COZY, ['--json', ...args], { encoding: 'utf8', timeout: 180000 });
(async () => {
  scratch = mkdtempSync(join(tmpdir(), 'web-vault-'));
  const folder = join(scratch, 'project');
  const picture = join(scratch, 'private_vault_picture.png');
  writeFileSync(picture, png(23));
  cli('project', 'new', folder);
  cli('project', '-p', folder, 'import', picture);
  const id = JSON.parse(cli('project', '-p', folder, 'status')).id;
  seen.project = { id, folder };
  browser = await launch(playwright);
  page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
  page.setDefaultTimeout(45000);
  const errors = [], outbound = [], fileWrites = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('request', (request) => { const url = new URL(request.url()); if (request.method() === 'PUT' && /^\/v1\/(project-)?files\//.test(url.pathname)) fileWrites.push({ path: url.pathname, project: url.searchParams.get('project') }); });
  page.on('request', (request) => { if (new URL(request.url()).origin !== new URL(calcifer()).origin && /cozy1-|synthetic browser passphrase/.test(request.postData() || request.url())) outbound.push('secret-bearing external request'); });
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
  await drop(page, [{ name: 'private_after_encryption.png', type: 'image/png', bytes: privateBytes }], 600, 400);
  await until('encrypted upload landed', () => page.locator('body').innerText(), (text) => text.includes('private_after_encryption') && !text.includes('Sending'), 45000);
  check('private_upload_uses_project_route', fileWrites.length > 0 && fileWrites.every((write) => write.path.startsWith('/v1/project-files/') && write.project === id));
  const stored = await page.evaluate(() => JSON.stringify(Object.fromEntries(Object.keys(localStorage).map((key) => [key, localStorage.getItem(key)]))));
  check('no_private_browser_persistence', !stored.includes(recovery) && !stored.includes('synthetic browser passphrase') && !stored.includes('private_vault_picture') && !stored.includes('private_after_encryption'));
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
  check('no_page_errors', errors.length === 0);
  seen.page_errors = errors;
})().catch((error) => { failures.push('browser vault flow'); seen.failure = error.message.replace(/cozy1(?:-[0-9a-f]{8}){9}/gi, '[REDACTED]').replaceAll('synthetic browser passphrase', '[REDACTED]'); process.exitCode = 1; }).finally(async () => {
  // Never take recovery-code screenshots or emit credentials in the fixture's artifacts.
  if (page) await page.close().catch(() => {});
  if (browser) await browser.close().catch(() => {});
  if (scratch) {
    try { cli('project', '-p', join(scratch, 'project'), 'lock'); rmSync(scratch, { recursive: true, force: true }); }
    catch { seen.cleanup_pending = 'fixture backing retained because its lock did not complete'; failures.push('fixture mount cleanup'); }
  }
  console.log(JSON.stringify({ seen, failures }, null, 2));
  if (failures.length) process.exitCode = 1;
});
