// The page's sign-in: AuthKit's own form, through Calcifer to a Hub. A new email signs up. A wrong code stays in the form and says so in plain
// words, with no command to run. Asked too soon, it moves on to the code already out. Either email's code works. The account shows, a reload
// finds it, nothing of the Hub's is in the browser, and it signs out. Deleted, its code reaches AuthKit's restore screen, which says when it
// goes for good, and Restore signs in.
//   node signin.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser. $HUB_LOG: a development Hub's log to read codes from, with Calcifer already
// pointed at that Hub. Without it a stand-in Hub runs here, since a runner can't reach AuthKit; $TO_HUB (a shell command, @HUB@ for its origin)
// points Calcifer at it and $RESTART points it back.
// Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { exec } = require('child_process');
const { createPublicKey, randomBytes, randomUUID, verify } = require('crypto');
const { readFileSync } = require('fs');
const http = require('http');
const { calcifer, signIn, STEP_MS, lastLook, launch, lookAt, noStall, until } = require('./page');

const failures = [];
const seen = {};
const died = (error) => {
  seen.died = String(error);
  failures.push('died');
  void lastLook().finally(() => finish());
};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const finish = () => {
  if (heard.length) seen.hub_heard = heard;
  console.log(JSON.stringify({ ok: !failures.length, failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
};
const shell = (command) => new Promise((done, fail) => exec(command, { shell: 'bash' }, (error) => (error ? fail(error) : done())));

const b64 = (bytes) => Buffer.from(bytes).toString('base64url');
const COOLDOWN_S = 12;
/** A Hub mails first and answers after: the code can be in hand while the page still waits to hear it went. */
const ANSWERS_AFTER_MS = 1500;
const heard = [];

/** Noon, 19 days on: the restore screen counts calendar days, so midnight can't move it. */
const PURGE = (() => {
  const at = new Date();
  at.setHours(12, 0, 0, 0);
  at.setDate(at.getDate() + 19);
  return at;
})();

/** What a Hub does for a sign-in, on loopback. It holds Calcifer to its signatures and refuses as AuthKit does: a wrong code is 401, too soon is 429,
 * and a deleted account's right code is 409 with its recovery token, the enrollment still live, until that token is confirmed. */
async function standIn() {
  const enrollments = new Map(), logins = new Map(), keys = new Map(), recoveries = new Map(), deleted = new Set();
  const mailed = [];
  let name = null, nextAt = 0;
  const signed = (domain, challenge, key, signature) => {
    const publicKey = createPublicKey({ key: { kty: 'OKP', crv: 'Ed25519', x: key }, format: 'jwk' });
    return verify(null, Buffer.concat([Buffer.from(domain), Buffer.from([0]), Buffer.from(challenge, 'base64url')]), publicKey, Buffer.from(signature, 'base64url'));
  };
  const session = (id) => ({ status: 'complete', token_set: { access_token: `stand-in-${id}`, token_type: 'bearer', expires_in: 900 }, device_key: { id } });
  const account = () => ({ name, user_id: 'stand-in', orgs: [] });
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', (chunk) => (body += chunk));
    req.on('end', () => {
      const ask = body ? JSON.parse(body) : {};
      const send = (status, answer, headers = {}) => {
        heard.push(`${req.method} ${req.url.split('?')[0]} ${status}`);
        res.writeHead(status, { 'Content-Type': 'application/json', ...headers });
        res.end(answer === undefined ? undefined : JSON.stringify(answer));
      };
      const refuse = (status, code, message, headers) => send(status, { error: { code, message } }, headers);
      const path = req.url.split('?')[0];
      const route = `${req.method} ${path}`;
      if (route === 'POST /v1/auth/v1/device-keys/enroll/begin') {
        const left = Math.ceil((nextAt - Date.now()) / 1000);
        if (left > 0) return refuse(429, 'rate_limited', 'Too many attempts. Please try again later.', { 'Retry-After': String(left) });
        nextAt = Date.now() + COOLDOWN_S * 1000;
        const id = b64(randomBytes(32)), challenge = b64(randomBytes(32)), code = String(100000 + mailed.length * 7 + 11);
        enrollments.set(id, { ...ask, challenge, code });
        mailed.push(code);
        return setTimeout(() => send(200, { enrollment_id: id, challenge, expires_at: new Date(Date.now() + 600_000).toISOString() }), ANSWERS_AFTER_MS);
      }
      if (route === 'POST /v1/auth/v1/device-keys/enroll/finish') {
        const held = enrollments.get(ask.enrollment_id);
        if (!held || held.code !== ask.code || !signed('authkit.device-key-enrollment/1', held.challenge, held.public_key, ask.signature)) {
          return refuse(401, 'invalid_code', 'Invalid verification code.');
        }
        if (deleted.has(held.email)) {
          const token = b64(randomBytes(32));
          recoveries.set(token, held.email);
          const metadata = { token, expires_at: new Date(Date.now() + 600_000).toISOString(), purge_at: PURGE.toISOString() };
          return send(409, { error: { code: 'account_recovery_required', message: 'Confirm account recovery before signing in.', metadata } });
        }
        enrollments.delete(ask.enrollment_id);
        const id = randomUUID();
        keys.set(id, held.public_key);
        return send(200, session(id));
      }
      if (route === 'POST /v1/auth/v1/account/recovery/confirm') {
        const email = recoveries.get(ask.token);
        if (!email) return refuse(401, 'invalid_credentials', 'Refused.');
        recoveries.delete(ask.token);
        deleted.delete(email);
        return send(204);
      }
      if (route === 'POST /v1/auth/v1/device-keys/login/begin') {
        const id = b64(randomBytes(32)), challenge = b64(randomBytes(32));
        logins.set(id, { key: ask.device_key_id, challenge });
        return send(200, { challenge_id: id, challenge });
      }
      if (route === 'POST /v1/auth/v1/device-keys/login/finish') {
        const held = logins.get(ask.challenge_id);
        const key = held && keys.get(held.key);
        if (!key || !signed('authkit.device-key-login/1', held.challenge, key, ask.signature)) return refuse(401, 'invalid_credentials', 'Refused.');
        return send(200, session(held.key));
      }
      if (route === 'GET /v1/accounts/current') {
        return name ? send(200, account()) : refuse(409, 'account.name_required', 'choose an account name');
      }
      if (req.method === 'PUT' && path.startsWith('/v1/accounts/')) {
        name = decodeURIComponent(path.slice('/v1/accounts/'.length));
        return send(200, account());
      }
      if (route === 'GET /v1/credit') return send(200, { credit: { available_usd_micros: 5_000_000 } });
      if (req.method === 'DELETE' && path.startsWith('/v1/auth/v1/me/sign-in-keys/')) {
        keys.delete(path.split('/').pop());
        return send(204);
      }
      refuse(404, 'route.not_found', `no route ${route}`);
    });
  });
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    mailed: () => [...mailed],
    sendsIn: () => Math.max(0, nextAt - Date.now()),
    deletes: (email) => deleted.add(email),
    close: () => server.close(),
  };
}

/** The codes a development Hub logged for `email`, oldest first. */
const logged = (email) => () => readFileSync(process.env.HUB_LOG, 'utf8').split('\n').flatMap((line) => {
  try {
    const said = JSON.parse(line);
    return said.msg === 'development device enrollment code' && said.email === email ? [said.code] : [];
  } catch {
    return [];
  }
});

(async () => {
  const base = calcifer();
  const email = `web-signin-${Date.now()}@scratch.test`;
  const hub = process.env.HUB_LOG ? null : await standIn();
  const mailed = hub ? hub.mailed : logged(email);

  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1400, height: 800 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const [violations, errors, elsewhere] = [[], [], []];
  await page.addInitScript(() => {
    window.__violations = [];
    addEventListener('securitypolicyviolation', (e) => window.__violations.push(`${e.violatedDirective} blocked ${e.blockedURI}`));
  });
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => /Content Security Policy/.test(message.text()) && violations.push(message.text()));
  page.on('request', (request) => {
    if (new URL(request.url()).origin !== base && !request.url().startsWith('data:') && !request.url().startsWith('blob:')) elsewhere.push(request.url());
  });
  await signIn(page);
  await page.waitForSelector('.top');
  // After the link: a restart loses a code not yet redeemed, and keeps a session.
  if (hub) await shell(process.env.TO_HUB.replaceAll('@HUB@', hub.origin));
  await page.goto(`${base}/#/account`);

  const form = page.locator('.authui');
  const said = () => form.innerText();
  const address = page.locator('input[name="identifier"]');
  const code = page.locator('input[data-input-otp]');
  const go = page.getByRole('button', { name: 'Continue', exact: true });
  const count = () => mailed().length;

  await address.waitFor();
  check('authkit_form', /Sign in or sign up/.test(await said()), await said());

  await address.fill(email);
  await go.click();
  await code.waitFor();
  const first = await until('the first code', () => mailed()[0], Boolean);
  check('a_code_went_out', count() === 1, mailed());

  // Paul's dead end: the Hub answers a wrong code 401, and the page took that for losing its own session.
  await code.pressSequentially(first === '000000' ? '000001' : '000000');
  await page.getByText("That code isn't right").waitFor();
  check('wrong_code_stays_in_the_form', (await page.locator('.gate').count()) === 0 && (await code.count()) === 1);
  // Shutting the field for an answer takes the cursor with it: it comes back, so no one has to click in to try again.
  const inTheField = () => until('the cursor in the field', () => page.evaluate(() => document.activeElement?.matches('input[data-input-otp]')), Boolean, 3000).then(() => true, () => false);
  check('wrong_code_leaves_the_cursor_in_the_field', await inTheField());
  check('wrong_code_names_no_command', !/cozy /.test(await said()), await said());

  // Too soon: the code already out is the answer, and no second mail goes.
  await page.getByRole('button', { name: 'Use a different email' }).click();
  await address.fill(email);
  await go.click();
  await code.waitFor();
  check('too_soon_mails_nothing', count() === 1, mailed());
  const wait = () => said().then((text) => Number(/ask for another code in (\d+) s/.exec(text)?.[1]));
  const stated = await until('a stated wait', wait, (left) => left > 0);
  seen.stated_wait = stated;
  check('stated_wait_says_check_your_email', /Check your email/.test(await said()), await said());
  await until('the wait to fall', wait, (left) => left < stated);

  // Past it, a second code goes, and the first still signs in: each email has its own.
  const resend = page.getByRole('button', { name: /^Resend/ });
  await until('resend to open', () => resend.isEnabled(), Boolean);
  await resend.click();
  const second = await until('a second code', () => mailed()[1], Boolean);
  check('two_emails_two_codes', first !== second, mailed());
  // The mail can land before the page hears it went, and the field is shut until it does: what's typed then goes nowhere.
  await page.getByText('A new code is on its way').waitFor();
  await until('the field to open', () => code.isEnabled(), Boolean);
  check('resend_leaves_the_cursor_in_the_field', await inTheField());
  // The keyboard alone, as a person with the code in hand: nothing here clicks or focuses the field.
  await page.keyboard.type(first);

  // A new account's name, then the account in the page.
  const name = `signin${Date.now() % 100000}`;
  await page.getByLabel('Name').fill(name);
  await page.getByRole('button', { name: 'Name it' }).click();
  const record = (text) => page.locator('dd', { hasText: text });
  await record(email).waitFor();
  check('account_shows', (await record(name).count()) === 1);
  // The wrong code, then the older code on the newest's refusal: past the second mail, a Hub must hear both or this proved nothing.
  if (hub) {
    const proofs = heard.filter((line) => line.includes('/enroll/finish')).map((line) => line.split(' ').pop());
    check('older_code_found_past_the_newest', proofs.join() === '401,401,200', proofs);
  }

  await page.reload();
  await record(email).waitFor();
  check('reload_finds_the_account', (await record(name).count()) === 1);

  // The key is Calcifer's, and nothing a Hub issued reached the page.
  const stored = await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage) }));
  check('no_authkit_in_storage', ![...stored.local, ...stored.session].some((key) => /authkit/i.test(key)), stored);
  check('no_cookie', (await context.cookies()).length === 0, await context.cookies());
  check('only_this_origin', elsewhere.length === 0, elsewhere);

  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await page.locator('.confirm').getByRole('button', { name: 'Sign out', exact: true }).click();
  await address.waitFor();
  check('signs_out', (await page.locator('.gate').count()) === 0);

  // The account deletes itself, and its owner signs in again: AuthKit's restore screen, the days left, and Restore signs in.
  if (hub) {
    hub.deletes(email);
    await new Promise((done) => setTimeout(done, hub.sendsIn()));
    await address.fill(email);
    await go.click();
    await code.waitFor();
    const third = await until('a code to the deleted account', () => mailed()[2], Boolean);
    await code.pressSequentially(third);
    await form.getByText('Restore your account?').waitFor();
    const when = `in 19 days, on ${new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric' }).format(PURGE)}`;
    check('restore_says_when_it_goes', (await said()).includes(`It will be deleted for good ${when}.`), await said());
    await page.getByRole('button', { name: 'Restore account', exact: true }).click();
    await record(email).waitFor();
    check('restore_signs_in', (await record(name).count()) === 1);
    const steps = heard.filter((line) => /\/enroll\/finish|\/recovery\/confirm/.test(line)).slice(-3).map((line) => line.split(' ').slice(1).join(' '));
    check('restore_is_authkits_step', steps.join() === '/v1/auth/v1/device-keys/enroll/finish 409,/v1/auth/v1/account/recovery/confirm 204,/v1/auth/v1/device-keys/enroll/finish 200', steps);
  }

  check('no_csp_violation', [...violations, ...(await page.evaluate(() => window.__violations))].length === 0, violations);
  check('no_page_error', errors.length === 0, errors);
  await browser.close();
  if (hub) {
    await shell(process.env.RESTART);
    hub.close();
  }
  finish();
})().catch(died);
