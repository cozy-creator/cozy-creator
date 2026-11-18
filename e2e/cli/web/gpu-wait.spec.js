// gpu-wait (cl-1197): a run waiting for a GPU says so on its box's face, on its machine's tab and in its machine's queue. Paul read
// PRO6-61's 96 s wait for 2 GPUs as stuck: its face and machine tab said its stage, "Preparing, time left unknown". Its box's line says
// what it waits for, whole and uncut (`Waiting for a GPU`: a box has room for no more); its tab's row and its queue's row say all of it,
// as its drawer does (`Waiting for a GPU, held by run KEY-1, about 2 min`). How long moves as it waits, so each place is read against
// what Calcifer says of the run at that moment. On a rental, from release-gate/sessions/gpu-wait.sh, which stages the wait (its `holding`
// box holds the one GPU, its `waiting` box waits for it) and names it here: WAIT_URL (its project's page), WAIT_BOX, WAIT_RUN (the
// waiting run's number), WAIT_MACHINE, WAIT_SAID (the run's `waiting` as the session first read it; none on a machine with no GPU, its
// $0 pre-flight, where nothing waits and the page is only read) and WAIT_PAGE (which page this is, for its pictures). The face, the tab
// and the queue in paper and dark: $OUT/web/gpu-wait/<page>-*.png.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasCard, canvasVisible, themed } = require('./fixtures');
const { fit } = require('./hands');

/** A wait as a line, as words.ts `waitWords` says it: its machine state's prefix dropped, its first letter a capital. */
const line = (said) => {
  const reason = said.replace(/^(queued|preparing|running): /, '');
  return reason.charAt(0).toUpperCase() + reason.slice(1);
};
/** What it waits for alone, a box's line, as words.ts `waitFor` says it: `line` up to its first comma. */
const short = (said) => line(said).split(', ')[0];

test('gpu-wait: a run waiting for the GPU says so on its face, its machine tab and its queue', { tag: '@gpu' }, async ({ page, api }) => {
  const { WAIT_URL: url, WAIT_BOX: box, WAIT_RUN: run, WAIT_MACHINE: machine, WAIT_SAID: said, WAIT_PAGE: which = 'own' } = process.env;
  test.skip(!url || !box || !run || !machine, 'release-gate/sessions/gpu-wait.sh sets WAIT_URL, WAIT_BOX, WAIT_RUN and WAIT_MACHINE');
  test.setTimeout(600_000);
  const shots = join(process.env.OUT, 'web', 'gpu-wait');
  mkdirSync(shots, { recursive: true });
  test.info().annotations.push({ type: 'wait', description: JSON.stringify({ page: which, run, said }) });
  // The session's project, not this cell's.
  const code = new URL(url).pathname.split('/')[2];
  const id = (await api.get('/v1/projects?limit=1000')).projects.find((one) => one.code === code)?.id;
  expect(id, `the session's project, ${code}, listed`).toBeTruthy();
  /** The run's `waiting` as Calcifer says it now. */
  const waiting = async () => (await api.get(`/v1/projects/${encodeURIComponent(id)}/runs?limit=100`)).runs.find((one) => String(one.number) === run)?.waiting ?? null;
  /** `read` says `words` of the run's wait as it is then, within 30 s: its time left moves ("about 2 min", then "under a minute"). */
  const agrees = (message, read, words) => expect.configure({ soft: true }).poll(async () => {
    const [now, shown] = [await waiting(), await read()];
    return now && shown === words(now) ? 'agrees' : `shows ${JSON.stringify(shown)}; Calcifer says ${JSON.stringify(now)}`;
  }, { message, timeout: 30_000 }).toBe('agrees');
  /** The page's own files still coming (its scripts, styles, fonts: not /v1): a navigation cancels them, which a candidate page's
   *  delivery counts as its errors (pre-flight 44274095291: `Invalid InterceptionId`). Each `away` waits for none before it goes. */
  const coming = new Set();
  const own = (request) => { const at = new URL(request.url()); return at.origin === new URL(url).origin && !at.pathname.startsWith('/v1') && request.method() === 'GET'; };
  page.on('request', (request) => { if (own(request)) coming.add(request); });
  for (const done of ['requestfinished', 'requestfailed']) page.on(done, (request) => coming.delete(request));
  const away = async (to) => {
    await expect.poll(() => coming.size, { message: 'the page\'s own files arrived before it navigates', timeout: 30_000 }).toBe(0);
    await page.goto(to);
  };
  /** What each place says of the run now: its box's face, its machine's tab, its machine's queue. */
  const seen = {};
  const face = async () => (await canvasCard(page, box))?.rendered?.progress?.state ?? null;
  /** Its line as the board drew it, cut to its box's room. */
  const drawn = async () => (await canvasCard(page, box))?.rendered?.labels?.progress?.text ?? null;

  await test.step(`${which}: its box's face`, async () => {
    await away(url);
    await expect(page.locator('.stage')).toBeVisible();
    await expect.poll(() => canvasVisible(page, box), { timeout: 60_000 }).toBe(true);
    await fit(page);
    await expect.poll(face, { message: `${box}'s face drawn`, timeout: 60_000 }).not.toBeNull();
    if (said) {
      await agrees(`${box}'s face says what it waits for`, face, short);
      await agrees(`${box}'s line is drawn whole, nothing cut off`, drawn, short);
    }
    seen.face = await face();
    seen.drawn = await drawn();
  });

  await test.step(`${which}: its machine's tab`, async () => {
    await page.getByRole('navigation', { name: 'Machines', exact: true }).locator(`.mtab[data-machine="${machine}"]`).click();
    const doing = page.getByRole('dialog', { name: /^What .+ is doing$/ });
    await expect(doing).toBeVisible();
    const row = doing.locator(`li.doing[data-run="${run}"] .said`);
    if (said) await agrees('the tab says its wait', async () => ((await row.count()) ? row.textContent() : null), line);
    seen.tab = (await row.count()) ? await row.textContent() : null;
    await page.keyboard.press('Escape');
  });

  await test.step(`${which}: its machine's queue`, async () => {
    await away(`${url}/machines/${encodeURIComponent(machine)}`);
    const row = page.locator(`li.qrow[data-run="${run}"] .said`);
    await expect(row).toBeVisible({ timeout: 60_000 });
    if (said) await agrees('the queue says its wait', () => row.textContent(), line);
    seen.queue = await row.textContent();
  });
  test.info().annotations.push({ type: 'seen', description: JSON.stringify(seen) });

  await test.step(`${which}: its looks, paper and dark, while it waits`, async () => {
    // Its own appearance, as the page's switch sets it.
    for (const theme of ['paper', 'dark']) {
      await api.call('PUT', `/v1/projects/${encodeURIComponent(id)}/appearance`, { appearance: { theme } });
      // The open page takes it at once (its `appearance` event) and fetches that theme's pictures: they arrive before it navigates
      // (rental 44277530156: navigating as they began canceled 4 of them, each a candidate delivery error).
      await themed(page, theme);
      await away(url);
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, theme);
      await expect.poll(() => canvasVisible(page, box), { timeout: 60_000 }).toBe(true);
      await fit(page);
      await page.screenshot({ path: join(shots, `${which}-face-${theme}.png`) });
      await page.getByRole('navigation', { name: 'Machines', exact: true }).locator(`.mtab[data-machine="${machine}"]`).click();
      await expect(page.getByRole('dialog', { name: /^What .+ is doing$/ })).toBeVisible();
      await page.screenshot({ path: join(shots, `${which}-tab-${theme}.png`) });
      await page.keyboard.press('Escape');
      await away(`${url}/machines/${encodeURIComponent(machine)}`);
      await expect(page.locator(`li.qrow[data-run="${run}"]`)).toBeVisible({ timeout: 60_000 });
      await page.screenshot({ path: join(shots, `${which}-queue-${theme}.png`) });
    }
  });
});
