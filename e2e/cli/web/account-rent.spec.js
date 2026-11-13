// account-rent (cl-973 core cell 24, solo): the account and renting, as a person does them in the page. Signed out by Account's inline
// confirm, Rent says to sign in; a wrong code stays in the form in plain words, the code the isolated Hub logs signs in, and nothing of
// the Hub's is in the browser. On localfit's GPU stand-in (a proxy in front of the isolated Hub that sells an rtx-4090 and places runs on
// it), a box this computer would render poorly asks on itself, Rent a GPU first: Run here anyway renders here and is kept, and Rent opens
// Machines with the card chosen, rendering nothing. Then the docker provider's cpu, rented from Machines at its quoted price, listed as the
// CLI lists it, made the project's machine and ended: nothing of it left running. Never paid: that provider starts a container here that
// never gets ready (e2e/cli/rental.sh).
// $E2E_HUB_EMAIL and $E2E_HUB_LOG (e2e/isolated.sh): the suite's account, and the isolated Hub's log of the codes it would mail.
const { execFile, spawn } = require('child_process');
const { appendFileSync, readFileSync, readdirSync } = require('fs');
const http = require('http');
const { dirname, join } = require('path');
const { promisify } = require('util');
const { test, expect, box, canvasIdle, focus } = require('./fixtures');

const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: web.sh's web_suite runs where e2e/isolated.sh sets it`); })();
const run = promisify(execFile);
const escaped = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
/** A tool that composes a GPU function loading BiRefNet (heavy.py): local code, so the Hub places no card for it. */
const HEAVY = `${join(__dirname, 'heavy.py')}/heavy`;
/** What the isolated Hub's docker provider runs, here on this runner. */
const CONTAINERS = ['ps', '-aq', '--filter', 'label=com.cozy.tensorhub.provider=docker'];

// The isolated Hub sells no GPU (its docker provider offers CPU machines alone) and so places no GPU tool. The stand-in sells one card as a
// provider would: listed in its catalog, and the pick of a placement that found none. Everything else is the Hub's own answer, passed through.
const CARD = {
  name: 'rtx-4090', accelerator_model: 'NVIDIA GeForce RTX 4090', base_worker_profile: '', compute_capability: '8.9', vram_gb: 24,
  python_provisionable_minors: ['3.12'], python_interpreters: [],
  widths: [{ accelerator_count: 1, price_usd_micros_per_hour: 690_000, container_disk_gb: 100,
    expected_price: { hours: 1, download_gb: 0, gpu_usd_micros: 690_000, disk_usd_micros: 0, download_usd_micros: 0, total_usd_micros: 690_000 } }],
};

/** localfit's stand-in in front of `upstream`, on a port of its own: `{origin, asked, close}`. */
function standIn(upstream) {
  const to = new URL(upstream);
  const asked = [];
  const server = http.createServer((request, response) => {
    const { pathname } = new URL(request.url, 'http://hub');
    const sold = request.method === 'GET' && pathname === '/v1/rental-skus';
    const placing = request.method === 'POST' && pathname === '/v1/rental-placements';
    const ask = http.request({ host: to.hostname, port: to.port, method: request.method, path: request.url, headers: { ...request.headers, host: to.host } }, (answer) => {
      if (!sold && !placing) {
        response.writeHead(answer.statusCode, answer.headers);
        answer.pipe(response);
        return;
      }
      const chunks = [];
      answer.on('data', (chunk) => chunks.push(chunk));
      answer.on('end', () => {
        const text = Buffer.concat(chunks).toString();
        let body = {};
        try { body = JSON.parse(text || '{}'); } catch { body = {}; }
        asked.push(`${request.method} ${pathname} ${answer.statusCode}`);
        // The Hub lists its catalog as a bare array (api::hub::Client::list_rental_skus).
        if (sold) body = [...(Array.isArray(body) ? body : []).filter((sku) => sku.name !== CARD.name), CARD];
        if (placing && !(body.machines ?? []).length) {
          const width = CARD.widths[0];
          body = { weight_bytes: 0, accelerator: true, weights: [], excluded: [], warnings: [], ...(answer.statusCode < 300 ? body : {}),
            machines: [{ name: CARD.name, accelerator_model: CARD.accelerator_model, vram_gb: CARD.vram_gb, accelerator_count: width.accelerator_count,
              price_usd_micros_per_hour: width.price_usd_micros_per_hour, container_disk_gb: width.container_disk_gb }] };
        }
        const said = JSON.stringify(body);
        response.writeHead(200, { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(said) });
        response.end(said);
      });
    });
    ask.on('error', (error) => {
      response.writeHead(502);
      response.end(String(error));
    });
    request.pipe(ask);
  });
  return new Promise((done) => server.listen(0, '127.0.0.1', () => done({
    origin: `http://127.0.0.1:${server.address().port}`,
    asked,
    close: () => new Promise((closed) => {
      server.closeAllConnections();
      server.close(() => closed());
    }),
  })));
}

/** Each code the isolated Hub logged for `email`, oldest first: what it would have mailed (tensorhub internal/authn/email.go). */
const codesFor = (email) => readFileSync(need('E2E_HUB_LOG'), 'utf8').split('\n')
  .filter((line) => line.includes('development device enrollment code') && line.includes(`"email":${JSON.stringify(email)}`))
  .map((line) => /"code":"([^"]+)"/.exec(line)?.[1])
  .filter(Boolean);

/** `cozy --json login <email>` in `cwd`, answered with the code the Hub logs after it asks, as rental.sh's signin does. Refused as too soon,
 *  asked again after the wait the Hub states. What it said. */
async function cliSignIn(email, cwd) {
  for (let tries = 1; ; tries += 1) {
    const before = codesFor(email).length;
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `--json\x1flogin\x1f${email}\x1f\x1e`);
    const child = spawn(need('COZY'), ['--json', 'login', email], { cwd, stdio: ['pipe', 'pipe', 'pipe'] });
    const said = { out: '', err: '', exit: null };
    child.stdout.on('data', (chunk) => { said.out += chunk; });
    child.stderr.on('data', (chunk) => { said.err += chunk; });
    // Its stdin closes with it: a write that finds it gone is told by its exit.
    child.stdin.on('error', (error) => { said.err += `stdin: ${error.message}`; });
    const exited = new Promise((done) => child.on('close', (code) => done((said.exit = code ?? 1))));
    await expect.poll(() => said.exit !== null || codesFor(email).length > before, { message: `cozy login ${email}: a code logged, or its exit`, timeout: 90_000 }).toBe(true);
    const code = codesFor(email).slice(before).at(-1);
    if (said.exit === null) child.stdin.end(code ? `${code}\n` : '');
    const exit = await exited;
    if (exit === 0) return JSON.parse(said.out);
    const wait = /"retry_after_s":\s*(\d+)/.exec(said.out)?.[1];
    if (!wait || tries === 3) throw new Error(`cozy --json login ${email}: exit ${exit}: ${said.out.slice(-400)} ${said.err.slice(-300)}`);
    // The Hub spaces codes to one email, and says how long: that long, and once more.
    await new Promise((done) => setTimeout(done, (Number(wait) + 1) * 1000));
  }
}

/** The Hub bearers Calcifer keeps beside its Hub keys (`~/.cozy/keys/*.bearer.json`, api::hub::device::Bearer): what must never reach the
 *  page. Never said in a failure: only where one was found is. */
function hubTokens() {
  const keys = join(dirname(dirname(need('HOLDER'))), 'keys');
  return readdirSync(keys)
    .filter((file) => file.endsWith('.bearer.json'))
    .map((file) => JSON.parse(readFileSync(join(keys, file), 'utf8')).token)
    .filter(Boolean);
}

/** This project's runs of function `fn`, as `GET …/runs` lists them. */
const runsOf = async (project, fn) => ((await project.api.get(project.at('/runs?limit=100'))).runs ?? []).filter((one) => new RegExp(`/${fn}(@\\S+)?$`).test(one.target ?? ''));

async function zoomToFit(page) {
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
}

test('account-rent', { tag: '@solo' }, async ({ page, project, api, cli, calciferControl, guards }, testInfo) => {
  const email = need('E2E_HUB_EMAIL');
  const seeded = JSON.parse(readFileSync(join(need('OUT'), 'web', 'fixtures.json'), 'utf8')).hub;
  // Qwen Image 2 from the Hub, which the Hub places on a card: by the stand-in, its rtx-4090.
  const kite = (seeded['qwen-image-2'] ?? []).find((id) => /\/image$/.test(id));
  expect(kite, `the seed's Qwen Image 2 tool among ${JSON.stringify(seeded['qwen-image-2'])}`).toBeDefined();
  const drawer = page.locator('aside.drawer');
  const rent = drawer.locator('section').filter({ has: page.getByRole('heading', { name: 'Rent a machine', exact: true }) });
  const account = (text) => drawer.getByRole('definition').filter({ hasText: new RegExp(`^${escaped(text)}$`) });
  const menu = async (item) => {
    await page.getByLabel('Menu', { exact: true }).click();
    await page.locator('details.main-menu nav').getByRole('link', { name: item, exact: true }).click();
  };
  // Every render the page asks for: its boxes and machines.
  const asked = [];
  const renders = `/v1/projects/${encodeURIComponent(project.id)}/runs`;
  page.on('request', (request) => {
    if (request.method() === 'POST' && new URL(request.url()).pathname === renders) asked.push(request.postDataJSON());
  });
  const renderOf = (name) => (request) => request.method() === 'POST' && new URL(request.url()).pathname === renders && request.postDataJSON()?.names?.includes(name);
  let stand = null;
  let rental = null;
  let failure = null;
  // Two boxes this computer would render poorly, on the open canvas.
  await project.add({ name: 'heavy', tool: HEAVY, kind: 'image', fields: { instructions: 'a lantern in fog', seconds: 8 }, at: { x: 0, y: 0 } });
  await project.add({ name: 'kite', tool: kite, kind: 'image', fields: { instructions: 'a red kite over dunes' }, at: { x: 420, y: 0 } });
  await expect(box(page, 'heavy')).toBeVisible();
  await expect(box(page, 'kite')).toBeVisible();

  try {
    await test.step('Account › Sign out, an inline confirm; signed in again, a wrong code first, then the code the Hub logs', async () => {
      // The suite starts signed out (e2e/isolated.sh): signed in first, as this computer's terminal signs in.
      await cliSignIn(email, project.path);
      const who = await cli.json('whoami');
      expect(who, 'cozy login signed this computer in').toMatchObject({ logged_in: true, email });
      // On purpose, and Chrome says each: Calcifer refusing a second code to the email inside the Hub's 30 s spacing (answered 503
      // `rate_limited`, run 43401337115, though it's the client's to wait: a 429), and the wrong code (400 `code_wrong`).
      guards.allow(/status of (429 \(Too Many Requests\)|503 \(Service Unavailable\)) http:\/\/\S+\/v1\/account\/codes$/, /status of 400 \(Bad Request\) http:\/\/\S+\/v1\/account\/codes\/code-[0-9a-f]+$/);

      await menu('Account');
      await expect(account(email), 'the account, signed in').toBeVisible();
      await drawer.getByRole('button', { name: 'Sign out', exact: true }).click();
      const confirm = drawer.locator('.confirm');
      await expect(confirm, 'Sign out asks in the drawer').toBeVisible();
      await expect.soft(confirm.locator('.says')).toHaveText(`Sign this computer out of ${email}? Its key is revoked at the Hub and erased here. Rentals go on billing until they end, and it can't rent or end them until it signs in again.`);
      await confirm.getByRole('button', { name: 'Sign out', exact: true }).click();
      const address = drawer.getByLabel('Email', { exact: true });
      await expect(address, "AuthKit's form, signed out").toBeVisible();
      await expect.poll(async () => (await cli.json('whoami')).logged_in, { message: 'cozy whoami, signed out' }).toBe(false);

      await menu('Machines');
      await expect.soft(rent.getByText('Sign in to rent.', { exact: true }), 'signed out, Rent says so').toBeVisible();
      for (const button of await rent.getByRole('button', { name: 'Rent', exact: true }).all()) await expect.soft(button).toBeDisabled();
      await rent.getByRole('link', { name: 'Sign in', exact: true }).click();
      await address.fill(email);
      const asked = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/account/codes');
      await drawer.getByRole('button', { name: 'Continue', exact: true }).click();
      // Too soon is the client's to wait out: a 429 with how long, never a 503 (run 43401337115 answered 503 `rate_limited`).
      expect.soft((await asked).status(), 'a code asked for too soon').toBe(429);
      const field = drawer.getByLabel('Verification code', { exact: true });
      await expect(field).toBeVisible();
      // cozy login's code went out moments ago, and the Hub spaces codes to one email: the page says how long, and Resend opens once it
      // may ask again.
      await expect.soft(drawer.getByText(/^Check your email\. You can ask for another code in \d+ s\.$/), 'the wait, said in words').toBeVisible();
      const resend = drawer.getByRole('button', { name: 'Resend', exact: true });
      await expect(resend).toBeEnabled({ timeout: 75_000 });
      const before = codesFor(email).length;
      await resend.click();
      await expect.poll(() => codesFor(email).length, { message: `a code the Hub logged for ${email}`, timeout: 30_000 }).toBeGreaterThan(before);
      const code = codesFor(email).at(-1);

      await expect(field).toBeEnabled();
      await field.pressSequentially(code === '000000' ? '000001' : '000000');
      await expect.soft(drawer.getByText("That code isn't right. Use the code from your latest email, or ask for another.", { exact: true }), 'a wrong code, said in plain words').toBeVisible();
      await expect.soft(field, 'it stays in the form').toBeVisible();
      await expect.soft(address, 'not back at the email').toHaveCount(0);
      await expect.soft(page.getByRole('heading', { name: "This page isn't signed in" }), "the page's own session holds").toHaveCount(0);

      await expect(field).toBeEnabled();
      await field.pressSequentially(code);
      await expect(account(email), 'the logged code signs in: the account shows').toBeVisible({ timeout: 30_000 });
      await expect.soft(account(who.account ?? '(unnamed)'), "the account's name").toBeVisible();
      await expect.poll(async () => (await cli.json('whoami')).logged_in, { message: 'cozy whoami, signed in from the page' }).toBe(true);
      const tokens = hubTokens();
      expect.soft(tokens.length, 'a Hub bearer Calcifer keeps for this sign-in').toBeGreaterThan(0);
      const stored = await page.evaluate(() => [localStorage, sessionStorage].flatMap((store) => Object.keys(store).map((key) => [key, store.getItem(key) ?? ''])));
      expect.soft(stored.filter(([key, value]) => tokens.some((token) => key.includes(token) || value.includes(token))).map(([key]) => key), 'Web Storage holding a Hub token').toEqual([]);
      expect.soft(stored.map(([key]) => key).filter((key) => /authkit/i.test(key)), "AuthKit's keys in Web Storage").toEqual([]);
      expect.soft(await page.context().cookies(), 'cookies').toEqual([]);
    });

    await test.step("Calcifer restarted on localfit's GPU stand-in", async () => {
      stand = await standIn(need('COZY_HUB_URL'));
      // Away while it restarts: the page's own reconnect is the offline cell's.
      await page.goto('about:blank');
      await calciferControl.hub(stand.origin);
      expect.soft((await api.get('/v1/rental-skus')).rental_skus.map((sku) => sku.name), 'the stand-in sells a GPU').toContain(CARD.name);
      // A machine that hasn't said what it has isn't judged: this one has said it has no GPU.
      await expect.poll(async () => (await api.get('/v1/machines/local')).status?.gpus, { message: "local's GPUs", timeout: 60_000 }).toEqual([]);
    });

    await test.step('A heavy box asks first; Run here anyway renders here, then Cancel; Render again, and again after a reload', async () => {
      await page.goto(project.url());
      await expect(box(page, 'heavy')).toBeVisible();
      await expect(box(page, 'kite')).toBeVisible();
      await zoomToFit(page);
      let region = await focus(page, 'heavy');
      const asking = region.getByRole('group', { name: 'Render here?' });
      await region.getByRole('button', { name: 'Render', exact: true }).click();
      await expect(asking, 'it asks on the box itself').toBeVisible({ timeout: 60_000 });
      await expect.soft(asking.locator('p').first()).toHaveText(/^This computer has no GPU, so .+ would run on its CPU, slowly\./);
      expect.soft(await asking.getByRole('button').allInnerTexts(), 'Rent a GPU first, Run here anyway second').toEqual(['Rent a GPU', 'Run here anyway']);
      expect.soft(asked.filter((one) => one.names?.includes('heavy')), 'no render sent before the choice').toEqual([]);

      const first = page.waitForRequest(renderOf('heavy'));
      await asking.getByRole('button', { name: 'Run here anyway', exact: true }).click();
      const sent = await first;
      expect.soft(sent.postDataJSON().on, 'Run here anyway renders here').toEqual(['local']);
      expect((await sent.response())?.ok(), 'its render asked').toBe(true);
      // Cancel as a person presses it, as soon as its face offers it. Run 43401337115: pressed 80 ms after its 202, Calcifer answered
      // {"dropped":["heavy"]}, and the run went on to complete with its take. Since #3259 a job dropped as it's sent is never sent (no
      // run), and a run it was making is canceled at its machine: settled once nothing is asked, queued or running.
      await region.getByRole('status', { name: "heavy's render" }).getByRole('button', { name: "Cancel heavy's render", exact: true }).click();
      const ended = ['completed', 'failed', 'canceled'];
      await expect.poll(async () => {
        const { busy, queue } = await project.state();
        return !busy && !queue?.length && (await runsOf(project, 'heavy')).every((one) => ended.includes(one.status));
      }, { message: "heavy's render settled", timeout: 60_000 }).toBe(true);
      const canceled = await runsOf(project, 'heavy');
      const takes = (await project.asset('heavy')).versions ?? 0;
      expect.soft({ runs: canceled.filter((one) => one.status !== 'canceled').map((one) => `#${one.number} ${one.status}`), takes }, 'Cancel stops its render: no run of it but canceled, no take').toEqual({ runs: [], takes: 0 });
      expect.soft((await api.post(project.at('/fit'), { names: ['heavy'] })).tools, 'Run here anyway, kept by Calcifer').toMatchObject([{ here: true }]);

      /** Render, asking nothing: sent to this computer, and one more take lands. */
      const again = async (what) => {
        const had = (await project.asset('heavy')).versions ?? 0;
        const next = page.waitForRequest(renderOf('heavy'));
        await region.getByRole('button', { name: 'Render', exact: true }).click();
        expect.soft((await next).postDataJSON().on, `${what} renders here, asking nothing`).toEqual(['local']);
        await expect.soft(asking).toHaveCount(0);
        await expect.poll(async () => (await project.asset('heavy')).versions ?? 0, { message: `${what}: heavy's take ${had + 1}`, timeout: 120_000 }).toBe(had + 1);
      };
      await again('Render again');
      await page.reload();
      region = page.getByRole('region', { name: 'Focused asset' });
      await expect(region).toHaveAttribute('data-focused-asset', 'heavy');
      await canvasIdle(page);
      await again('Render after a reload');
      const later = (await runsOf(project, 'heavy')).filter((one) => !canceled.some((was) => was.number === one.number));
      expect.soft(later.map((one) => `${one.status} on ${one.machine}`), 'the later renders, each on this computer').toEqual(['completed on local', 'completed on local']);
    });

    await test.step("Rent a GPU on another heavy box: Machines opens with the stand-in's rtx-4090 chosen, and nothing renders", async () => {
      // heavy.py is local code, whose card no Hub places, and Run here anyway is kept for its tool: the other box is the Hub's Qwen Image 2.
      await page.getByRole('region', { name: 'Focused asset' }).getByRole('button', { name: 'Back to canvas' }).click();
      await expect(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
      await canvasIdle(page);
      // Asked first: were it not said to render poorly here, its Render would run Qwen Image 2 on this runner's CPU.
      expect(await api.post(project.at('/fit'), { names: ['kite'] }), 'the Hub, by the stand-in, places it on the rtx-4090').toMatchObject({ tools: [{ names: ['kite'] }], rent: { sku: CARD.name, card: 'RTX 4090' } });
      const region = await focus(page, 'kite');
      const asking = region.getByRole('group', { name: 'Render here?' });
      await region.getByRole('button', { name: 'Render', exact: true }).click();
      await expect(asking, 'it asks on the box itself').toBeVisible({ timeout: 60_000 });
      expect.soft(await asking.getByRole('button').allInnerTexts(), 'Rent a GPU first, Run here anyway second').toEqual(['Rent an RTX 4090', 'Run here anyway']);
      await asking.getByRole('button', { name: 'Rent an RTX 4090', exact: true }).click();
      await expect(page, 'Machines, the card asked for').toHaveURL(`${project.url('machines')}?rent=${CARD.name}&gpus=1`);
      await expect.soft(rent.getByRole('listitem').filter({ has: page.getByText(CARD.name, { exact: true }) }), 'the rtx-4090 chosen').toHaveAttribute('aria-current', 'true');
      expect.soft(asked.filter((one) => one.names?.includes('kite')), 'Rent renders nothing').toEqual([]);
      expect.soft(await runsOf(project, 'image'), 'no run of it').toEqual([]);
      expect.soft((await project.asset('kite')).versions ?? 0, 'no take of it').toBe(0);
      testInfo.annotations.push({ type: 'stand-in', description: stand.asked.join(', ') });
    });

    await test.step('Machines › Rent a machine › cpu › Rent, then Rent 1× CPU at its price', async () => {
      // The Hub restored now, not after: Calcifer keeps a key per Hub origin, and the account signed in is the isolated Hub's own.
      await page.goto('about:blank');
      await calciferControl.hub();
      await page.goto(project.url('machines'));
      const cpu = (await api.get('/v1/rental-skus')).rental_skus.find((sku) => sku.name === 'cpu');
      expect(cpu, "the docker provider's cpu, listed").toBeDefined();
      const [width] = cpu.widths;
      const row = rent.getByRole('listitem').filter({ has: page.getByText('cpu', { exact: true }) });
      // Its vCPUs as its width lists them, "?" for none. The isolated Hub lists none: tensorhub's docker provider sizes its container by
      // the CPU worker image's shape but leaves the shape off the offer it lists (internal/provider/docker.go, Inventory).
      await expect.soft(row.locator('small'), 'its vCPUs, as the Hub lists them').toHaveText(`${width.vcpu_count ?? '?'} vCPU, no GPU`);
      await expect.soft(row.locator('.price'), 'its price').toHaveText(`$${(width.price_usd_micros_per_hour / 1e6).toFixed(2)}/h`);
      await row.getByRole('button', { name: 'Rent', exact: true }).click();
      await expect.soft(rent.getByText('It bills until you end it, or after 15 minutes idle.', { exact: true }), 'what the quote bills').toBeVisible();
      const order = rent.getByRole('button', { name: /^Rent 1× CPU at \$\d+\.\d{2}\/h$/ });
      await expect(order, 'one click on its stated price').toBeEnabled();
      const placed = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/machines');
      await order.click();
      const answer = await placed;
      expect(answer.ok(), `POST /v1/machines: ${answer.status()}`).toBe(true);
      const rented = await answer.json();
      rental = rented.machine.rental_id;
      expect(rental, 'a rental placed').toBeTruthy();
      expect.soft(rented.replay, 'a new order').toBe(false);
      expect.soft(rented.machine.placeholder, 'named by what it rents').toBe('Renting a CPU…');
      await expect(page, 'its own drawer').toHaveURL(project.url(`machines/${encodeURIComponent(rental)}`));
      const live = (await cli.json('machine', 'ls')).machines.filter((one) => one.kind === 'rental' && one.state !== 'ended');
      expect.soft(live.map((one) => one.rental_id), 'one rental, the same in cozy machine ls').toEqual([rental]);
      const title = drawer.locator('.drawer-head b');
      await expect(async () => {
        const listed = (await cli.json('machine', 'ls')).machines.find((one) => one.rental_id === rental);
        expect(await title.textContent()).toBe(listed.placeholder ?? listed.name);
      }, 'called as cozy machine ls calls it: what it rents until its host is known').toPass({ timeout: 30_000 });
      await expect.soft(title, 'never its rent- handle').not.toHaveText(/^rent-/);
    });

    await test.step('Render here checked, then End › End it', async () => {
      const here = drawer.getByRole('checkbox', { name: /^Render on / });
      await expect(here).toBeEnabled();
      // The press shows with the click itself (run 43405964751: its box sprang back until the project's cache told its readers).
      await here.check();
      await expect.soft(here).toBeChecked();
      await expect.poll(async () => {
        const machine = (await api.get('/v1/machines')).machines.find((one) => one.rental_id === rental);
        const chosen = (await project.state()).machines ?? [];
        return chosen.length === 1 && [machine?.name, machine?.alias, machine?.rental_id].includes(chosen[0]);
      }, { message: "the project's machines: the rental alone" }).toBe(true);

      await drawer.getByRole('button', { name: 'End', exact: true }).click();
      const confirm = drawer.locator('.confirm');
      await expect.soft(confirm.locator('.says'), 'End asks in the drawer').toHaveText(/^End (this rental|\S+)\? Outputs of its runs that haven't landed here land first, then it shuts down\. Its files on the pod go\./);
      await confirm.getByRole('button', { name: 'End it', exact: true }).click();
      await expect.soft(drawer.getByText(/^Shutting down\./), 'ending').toBeVisible();
      await expect(drawer.getByText('Ended. Nothing of it bills now, and what landed here stays.', { exact: true }), 'ended').toBeVisible({ timeout: 90_000 });
      const record = (await cli.json('machine', 'ls', '--history')).rentals.find((one) => one.rental_id === rental);
      expect.soft(record?.ended_at_ms, "ended once its Hub says it's gone").toBeGreaterThan(0);
      await expect.poll(async () => (await run('docker', CONTAINERS)).stdout.trim(), { message: "the docker provider's containers", timeout: 90_000 }).toBe('');
      expect.soft((await cli.json('machine', 'ls')).machines.filter((one) => one.rental_id === rental && one.state !== 'ended'), 'machine ls: no live rental').toEqual([]);
    });
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    // Whatever happened, nothing it rendered, rented or ran is left (calciferControl puts the Hub back).
    const left = [];
    const attempt = async (what, act) => {
      try {
        await act();
      } catch (error) {
        left.push(`${what}: ${error.message}`);
      }
    };
    if (failure) await attempt('its renders canceled', () => api.post(project.at('/cancel'), { names: ['heavy', 'kite'] }));
    if (rental) {
      await attempt(`${rental} ended`, async () => {
        if (!(await cli.json('machine', 'ls')).machines.some((one) => one.rental_id === rental && one.state !== 'ended')) return;
        const said = await cli('machine', 'end', rental, '--force', '-y');
        if (said.code !== 0) throw new Error(`exit ${said.code}: ${`${said.stdout}${said.stderr}`.slice(-300)}`);
      });
    }
    await attempt("the docker provider's containers removed", async () => {
      const ids = (await run('docker', CONTAINERS)).stdout.split(/\s+/).filter(Boolean);
      if (ids.length) await run('docker', ['rm', '-f', ...ids]);
    });
    if (stand) await attempt('the stand-in closed', () => stand.close());
    if (left.length) testInfo.annotations.push({ type: 'cleanup', description: left.join('; ') });
    if (left.length && !failure) throw new Error(`account-rent left something behind: ${left.join('; ')}`);
  }
});
