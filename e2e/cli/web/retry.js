// The page's half of retry_elsewhere (e2e/cli/retry.sh, cl-1085). Paul, 11-16: "the toast notification tells you to retry it. you push the
// retry button and we'll try to place that run on a different machine"; "toast notification should be in the far-left ... and also they
// should disappear after some period of time"; "get rid of the 'cozy run watch 47856 copy' nonsense". With the project open, its machine
// <stand> ends while both boxes render there: each says so in a toast at the window's bottom left, with Retry, a copy icon and no command.
// <cut>'s toast, untouched, goes by itself; <paged>'s stays while the pointer is on it, and its Retry renders <paged> again on this
// computer's machine, on the seed it failed with.
//   node retry.js <project folder> <stand-in> <paged's failed run number> <cut's failed run number>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $HOLDER: Calcifer's lock record; $SHOT: its pictures.
// Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { calcifer, holderToken, lastLook, launch, lookAt, noStall, projectUrl, settled, signIn, until } = require('./page');

const [folder, stand, pagedRun, cutRun] = process.argv.slice(2);
const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}
const shot = (page, name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : null);
const api = async (suffix) => {
  const answer = await fetch(`${calcifer()}/v1/projects/${encodeURIComponent(folder)}${suffix}`, { headers: { Authorization: `Bearer ${holderToken()}`, 'cozy-project-storage': '3' } });
  if (!answer.ok) throw new Error(`GET ${suffix}: ${answer.status} ${await answer.text()}`);
  return answer.json();
};
const run = (number) => api(`/runs/${number}?detail=full`);
const asset = async (name) => (await api('')).assets?.find((row) => row.name === name);

(async () => {
  const browser = await launch(playwright);
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  lookAt(page);
  await signIn(page, await projectUrl(folder));
  await page.locator('.stage').waitFor();
  await settled(page);
  const [paged, cut] = [await run(pagedRun), await run(cutRun)];
  check('both_rendering_on_the_stand_in', [paged, cut].every((one) => one.status === 'in_progress' && one.machine === stand), [paged, cut].map((one) => `${one.name} ${one.status} on ${one.machine}`));
  await shot(page, '1-rendering');

  // Its machine ended, as Machines' End ends one.
  execFileSync(process.env.COZY, ['--json', 'machine', 'end', stand, '--force', '-y'], { encoding: 'utf8', timeout: 60_000 });
  const note = (name, runName) => page.locator('.notes .note').filter({ hasText: `${name}: ${stand} shut down, so ${runName} stopped` });
  const [pagedNote, cutNote] = [note('paged', paged.name), note('cut', cut.name)];
  await pagedNote.waitFor({ timeout: 60_000 });
  const said = Date.now();
  await cutNote.waitFor({ timeout: 15_000 }).catch(() => {});
  const cutSaid = Date.now();
  const lost = await until('paged failed as its machine shut down', () => asset('paged'), (row) => Boolean(row?.failure));
  check('failure_machine_lost', lost.failure.code === 'machine_lost', lost.failure);
  const retry = pagedNote.getByRole('button', { name: 'Retry', exact: true });
  check('toast_offers_retry', (await retry.count()) === 1, await pagedNote.innerText());
  const copy = pagedNote.getByRole('button', { name: 'Copy', exact: true });
  check('toast_copy_is_an_icon', (await copy.count()) === 1 && (await copy.innerText()).trim() === '' && (await copy.locator('svg.icon').count()) === 1, await pagedNote.innerHTML());
  const text = await page.locator('.notes').innerText();
  check('toast_says_no_command', !/cozy /.test(text) && (await page.locator('.notes code').count()) === 0, text);
  const box = await page.locator('.notes').boundingBox();
  seen.notes_at = box;
  check('toasts_bottom_left', Boolean(box) && box.x <= 16 && 900 - (box.y + box.height) <= 24, box);
  await shot(page, '2-toast-retry');

  // Held by the pointer, paged's toast stays past its 10 s; cut's, untouched, goes by itself.
  await pagedNote.locator('.line').hover();
  await expect0(cutNote, 'an untouched toast goes by itself');
  seen.untouched_gone_ms = Date.now() - cutSaid;
  check('untouched_toast_waited', seen.untouched_gone_ms >= 8_000, seen.untouched_gone_ms);
  await page.waitForTimeout(Math.max(0, 12_000 - (Date.now() - said)));
  check('toast_held_while_pointed_at', (await pagedNote.count()) === 1);

  // Retry: rendered again where it finishes soonest now, the stand-in gone: this computer's machine, on the seed it failed with.
  await retry.click();
  await expect0(pagedNote, 'the pressed toast goes');
  // The only run since both failed: cut is retried after this, from the CLI.
  const again = await until('paged rendered again', async () => {
    const runs = (await api('/runs?limit=20')).runs ?? [];
    return runs.find((one) => /\/held(@|$)/.test(one.target) && one.number > Math.max(Number(pagedRun), Number(cutRun))) ?? null;
  }, (one) => Boolean(one), 120_000);
  await shot(page, '3-retried');
  const done = await until('paged done on local', () => run(again.number), (one) => ['completed', 'failed', 'canceled'].includes(one.status), 300_000);
  seen.again = { name: done.name, machine: done.machine, status: done.status, seed: done.request?.seed, was: paged.request?.seed };
  check('retried_on_local', done.machine === 'local' && done.status === 'completed', seen.again);
  check('retried_same_seed', done.request?.seed !== undefined && done.request?.seed === paged.request?.seed, seen.again);
  check('paged_has_its_take', ((await asset('paged'))?.versions ?? 0) >= 1, await asset('paged'));

  // The finished take's toast goes by itself, untouched.
  const finished = page.locator('.notes .note').filter({ hasText: 'paged finished' });
  if (await finished.count()) {
    const since = Date.now();
    await shot(page, '4-finished');
    await expect0(finished, 'the finished toast goes by itself');
    seen.finished_gone_ms = Date.now() - since;
  }
  await shot(page, '5-after');
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);

  /** `locator` gone within 15 s, else `what` failed. */
  async function expect0(locator, what) {
    const gone = await locator.waitFor({ state: 'detached', timeout: 15_000 }).then(() => true, () => false);
    check(what.replaceAll(/\W+/g, '_'), gone, await locator.allInnerTexts().catch(() => []));
  }
})().catch(died);
