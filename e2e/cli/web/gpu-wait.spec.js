// gpu-wait (cl-1197, cl-1247): a run waiting for a GPU says so on its box's face, on its machine's tab and in its machine's queue,
// and while its machine can't time the wait, nothing counts down to an end ("17s of 17s" on the box, "0s left" on the tab). Paul read
// PRO6-61's 96 s wait for 2 GPUs as stuck: its face and machine tab said its stage, "Preparing, time left unknown". Its box's line says
// what it waits for, whole and uncut (`Waiting for a GPU`: a box has room for no more); its tab's row and its queue's row say all of it,
// as its drawer does (`Waiting for a GPU, held by run KEY-1, about 2 min`). How long moves as it waits, so each place is read against
// what Calcifer says of the run at that moment. On a rental, from release-gate/sessions/gpu-wait.sh, which stages the wait (its `holding`
// box holds the one GPU, its `waiting` box waits for it) and names it here: WAIT_URL (its project's page), WAIT_BOX, WAIT_RUN (the
// waiting run's number), WAIT_MACHINE, WAIT_SAID (the run's `waiting` as the session first read it; none on a machine with no GPU, its
// $0 pre-flight, where nothing waits and the page is only read) and WAIT_PAGE (which page this is, for its pictures). The face, the tab
// and the queue in paper and dark: $OUT/web/gpu-wait/<page>-*.png.
const { spawn } = require('child_process');
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
/** `1st`, `2nd`, `11th`, as format.ts `ordinal` says it. */
const ordinal = (n) => `${n}${n % 100 >= 11 && n % 100 <= 13 ? 'th' : ['th', 'st', 'nd', 'rd'][n % 10] ?? 'th'}`;
/** What a row says of a waiting run, as words.ts does: in its machine's line its place, then its reason (`lineWords`); else its reason
 *  (`waitWords`); nothing while it waits for nothing. */
const rowWords = (one) =>
  one.queue_position != null ? `Queued, ${ordinal(one.queue_position)} in line${one.waiting ? `. ${line(one.waiting)}` : ''}` : one.waiting ? line(one.waiting) : null;

test('gpu-wait: a run waiting for the GPU says so on its face, its machine tab and its queue', { tag: '@gpu' }, async ({ page, api }) => {
  const { WAIT_URL: url, WAIT_BOX: box, WAIT_MACHINE: machine, WAIT_SAID: said, WAIT_PAGE: which = 'own', WAIT_RENDER: rendered, WAIT_DIR: dir } = process.env;
  /** The run read: the session's (WAIT_RUN), or the one this cell's own render of WAIT_RENDER makes once its page has loaded (cl-1263). */
  let run = process.env.WAIT_RUN;
  test.skip(!url || !box || !(run || (rendered && dir)) || !machine, 'release-gate/sessions/gpu-wait.sh sets WAIT_URL, WAIT_BOX, WAIT_MACHINE and WAIT_RUN or WAIT_RENDER');
  test.setTimeout(600_000);
  const shots = join(process.env.OUT, 'web', 'gpu-wait');
  mkdirSync(shots, { recursive: true });
  test.info().annotations.push({ type: 'wait', description: JSON.stringify({ page: which, run, said }) });
  // The session's project, not this cell's.
  const code = new URL(url).pathname.split('/')[2];
  const id = (await api.get('/v1/projects?limit=1000')).projects.find((one) => one.code === code)?.id;
  expect(id, `the session's project, ${code}, listed`).toBeTruthy();
  /** The project's runs as Calcifer says them now. */
  const runs = async () => (await api.get(`/v1/projects/${encodeURIComponent(id)}/runs?limit=100`)).runs;
  /** The run's `waiting` as Calcifer says it now. */
  const waiting = async () => (await runs()).find((one) => String(one.number) === run)?.waiting ?? null;
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
  /** Its machine's tab, and what it opens. */
  const tab = () => page.getByRole('navigation', { name: 'Machines', exact: true }).locator(`.mtab[data-machine="${machine}"]`);
  const doing = page.getByRole('dialog', { name: /^What .+ is doing$/ });
  /** Its machine's queue, opened in the page as a person does, by its tab's Details, and its canvas again by the drawer's Close: never
   *  page.goto, which cancels the page's own files under way, each a candidate delivery error (cl-1262: rentals 44277530156 and
   *  44293715839 still met it after `away`, as their pictures loaded). */
  const queue = async () => {
    await tab().click();
    await expect(doing).toBeVisible();
    await doing.getByRole('link', { name: 'Details', exact: true }).click();
  };
  const canvas = async () => {
    await page.locator('aside.drawer').getByRole('link', { name: 'Close', exact: true }).click();
    await expect.poll(() => new URL(page.url()).pathname, { message: 'back at its canvas' }).toBe(new URL(url).pathname);
  };
  // PROOF ONLY (cl-1264, never merged): the page cancels its own candidate requests in flight, as a live theme change does.
  await page.evaluate(async () => {
    const own = [...new Set(performance.getEntriesByType('resource').map((e) => e.name))]
      .filter((u) => { const at = new URL(u); return at.origin === location.origin && !at.pathname.startsWith('/v1'); });
    own.slice(0, 16).forEach((u, i) => {
      const stop = new AbortController();
      fetch(u, { cache: 'reload', signal: stop.signal }).catch(() => {});
      setTimeout(() => stop.abort(), i % 4);
    });
    await new Promise((done) => setTimeout(done, 4000));
  });
  /** What each place says of the run now: its box's face, its machine's tab, its machine's queue. */
  const seen = {};
  const face = async () => (await canvasCard(page, box))?.rendered?.progress?.state ?? null;
  /** Its line as the board drew it, cut to its box's room. */
  const drawn = async () => (await canvasCard(page, box))?.rendered?.labels?.progress?.text ?? null;
  /** Its time as its face says it, `17s of 40s`, or none. */
  const timed = async () => (await canvasCard(page, box))?.rendered?.progress?.time ?? '';
  /** What its box says, whole: its words, then its count when it has one (`Queued,` `1st in line`). */
  const boxed = async () => {
    const progress = (await canvasCard(page, box))?.rendered?.progress;
    return progress ? [progress.state, progress.count].filter(Boolean).join(' ') : null;
  };
  /** A row says what its box says, then more (cl-1263): one run, one story, as a GPU wait's rows begin with `Waiting for a GPU` and a
   *  run in its machine's line's with `Queued, 1st in line`. Read at one moment, within 30 s, as its place and reason move. (A download
   *  wait's box and rows are #3842's pair, `Waiting for download` and `Waiting for Sdxl to download`: web-shared-download holds them.) */
  const begins = (message, read) => expect.configure({ soft: true }).poll(async () => {
    const [boxSays, rowSays] = [await boxed(), await read()];
    return boxSays && rowSays?.startsWith(boxSays) ? 'begins' : `box ${JSON.stringify(boxSays)}; row ${JSON.stringify(rowSays)}`;
  }, { message, timeout: 30_000 }).toBe('begins');
  /** A row says what Calcifer says of the run now, as words.ts words it, within 30 s as it changes (cl-1263): a row read from the
   *  machine list's copy of a run stands still between machine changes while its box moves on. */
  const follows = (message, read) => expect.configure({ soft: true }).poll(async () => {
    const now = (await runs()).find((one) => String(one.number) === run);
    const [want, rowSays] = [now ? rowWords(now) : null, await read()];
    return !want || rowSays === want ? 'follows' : `row ${JSON.stringify(rowSays)}; the run ${JSON.stringify(want)}`;
  }, { message, timeout: 30_000 }).toBe('follows');
  /** A wait its machine can't time (cl-1247): the run's time left isn't known, so nothing counts down to an end ("17s of 17s", "0s
   *  left" read as stuck at its end). */
  const untimed = Boolean(said && /isn't known/.test(said));

  await test.step(`${which}: its box's face`, async () => {
    await away(url);
    await expect(page.locator('.stage')).toBeVisible();
    await expect.poll(() => canvasVisible(page, box), { timeout: 60_000 }).toBe(true);
    if (rendered) {
      // Its render, once the page has loaded, as a person runs it from the project's folder: what it waits for reaches the page after it
      // opened (cl-1263). Its follower runs on and never cancels it.
      const before = new Set((await runs()).map((one) => one.number));
      const render = spawn(process.env.COZY || 'cozy', ['--json', 'render', rendered, '--on', machine, '--new', '-y'], { cwd: dir, stdio: 'ignore', detached: true });
      render.unref();
      await expect.poll(async () => {
        run = (await runs()).filter((one) => !before.has(one.number)).map((one) => String(one.number))[0];
        return run ?? null;
      }, { message: `${rendered}'s render made a run`, timeout: 120_000 }).not.toBeNull();
      test.info().annotations.push({ type: 'rendered', description: JSON.stringify({ box: rendered, run }) });
    }
    await fit(page);
    await expect.poll(face, { message: `${box}'s face drawn`, timeout: 60_000 }).not.toBeNull();
    if (said) {
      await agrees(`${box}'s face says what it waits for`, face, short);
      await agrees(`${box}'s line is drawn whole, nothing cut off`, drawn, short);
    }
    if (untimed) await expect.configure({ soft: true }).poll(timed, { message: `${box} counts no time down while its wait isn't timed`, timeout: 30_000 }).not.toMatch(/ of /);
    seen.face = await face();
    seen.drawn = await drawn();
    seen.time = await timed();
  });

  await test.step(`${which}: its machine's tab`, async () => {
    await tab().click();
    await expect(doing).toBeVisible();
    const row = doing.locator(`li.doing[data-run="${run}"] .said`);
    if (said) await agrees('the tab says its wait', async () => ((await row.count()) ? row.textContent() : null), line);
    await begins('the tab begins with what its box says', async () => ((await row.count()) ? row.textContent() : null));
    await follows("the tab says what the run says now", async () => ((await row.count()) ? row.textContent() : null));
    seen.tab = (await row.count()) ? await row.textContent() : null;
    if (untimed) expect.soft(seen.tab ?? '', 'the tab counts no time down while its wait isn\'t timed').not.toMatch(/\bleft\b/);
    await page.keyboard.press('Escape');
  });

  await test.step(`${which}: its machine's queue`, async () => {
    await queue();
    const row = page.locator(`li.qrow[data-run="${run}"] .said`);
    await expect(row).toBeVisible({ timeout: 60_000 });
    if (said) await agrees('the queue says its wait', () => row.textContent(), line);
    await begins('the queue begins with what its box says', () => row.textContent());
    await follows("the queue says what the run says now", () => row.textContent());
    seen.queue = await row.textContent();
    seen.box = await boxed();
  });
  test.info().annotations.push({ type: 'seen', description: JSON.stringify(seen) });

  await test.step(`${which}: its looks, paper and dark, while it waits`, async () => {
    // Its own appearance, as the page's switch sets it: the open page takes it at once (its `appearance` event), with no reload.
    for (const theme of ['paper', 'dark']) {
      await api.call('PUT', `/v1/projects/${encodeURIComponent(id)}/appearance`, { appearance: { theme } });
      await themed(page, theme);
      await canvas();
      await expect.poll(() => canvasVisible(page, box), { timeout: 60_000 }).toBe(true);
      await fit(page);
      await page.screenshot({ path: join(shots, `${which}-face-${theme}.png`), animations: 'disabled' });
      await tab().click();
      await expect(doing).toBeVisible();
      await page.screenshot({ path: join(shots, `${which}-tab-${theme}.png`), animations: 'disabled' });
      await page.keyboard.press('Escape');
      await queue();
      await expect(page.locator(`li.qrow[data-run="${run}"]`)).toBeVisible({ timeout: 60_000 });
      await page.screenshot({ path: join(shots, `${which}-queue-${theme}.png`), animations: 'disabled' });
    }
  });
});
