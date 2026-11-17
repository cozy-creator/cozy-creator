// The page's half of retry_elsewhere (e2e/cli/retry.sh, cl-1085, cl-1132). Paul, 11-17: "calcifer has a queue of jobs, it gives that queue
// of jobs to a machine. If that machine dies or disappears, it re-assigns that queued job to another machine." With the project open, its
// machine <stand> ends while both boxes render there: neither says a failure; each run, the same one, its second attempt, completes on this
// computer's machine on the seed it was asked with, and paged's take landing is said at the window's bottom left, then goes by itself.
// cl-1173, Paul, 11-03: "informing the user of what's going on, i.e., attempt 2/5": while paged's second attempt goes (held on local
// `at_home`), its tile's first line, its run's page and `cozy run ls` say "attempt 2 of 3: <stand> shut down".
//   node retry.js <project folder> <stand-in> <paged's run number> <cut's run number>
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $HOLDER: Calcifer's lock record; $SHOT: its pictures.
// Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFile, execFileSync } = require('child_process');
const { promisify } = require('util');
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
  const answer = await fetch(`${calcifer()}/v1/projects/${encodeURIComponent(folder)}${suffix}`, { headers: { Authorization: `Bearer ${holderToken()}` } });
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

  // Its machine ended, as Machines' End ends one. Every note the page shows until paged's take lands is kept: none says a failure. Paged's
  // take landing is said ("Version 1 of paged is ready", or one note for both, "2 renders are ready: cut, paged"), untouched (the pointer
  // away from it), and the note goes by itself: when it was first seen and when it was gone, whatever the cell looks at meanwhile.
  await page.mouse.move(1300, 120);
  execFileSync(process.env.COZY, ['--json', 'machine', 'end', stand, '--force', '-y'], { encoding: 'utf8', timeout: 60_000 });
  const said = new Set();
  let landed = 0, gone = 0;
  const collect = async () => {
    const notes = await page.locator('.notes .note').allInnerTexts().catch(() => null);
    if (!notes) return;
    for (const text of notes) said.add(text);
    if (notes.some((text) => /\bready\b/.test(text) && text.includes('paged'))) landed ||= Date.now();
    else if (landed) gone ||= Date.now();
  };
  let collecting = true;
  const collector = (async () => { while (collecting) { await collect(); await new Promise((done) => setTimeout(done, 200)); } })();
  // Its second attempt going on this computer's machine: its tile, its run's page and `cozy run ls` say which and why (cl-1173).
  const attempt = `attempt 2 of 3: ${stand} shut down`;
  const second = await until('paged on its second attempt', () => run(pagedRun), (one) => one.retry?.attempt === 2 && one.status === 'in_progress' && one.machine !== stand, 120_000).catch(() => null);
  seen.second = second && { name: second.name, machine: second.machine, status: second.status, retry: second.retry };
  check('paged_records_attempt_2_of_3', second?.retry?.attempts === 3 && second?.retry?.why === `${stand} shut down`, seen.second);
  // Its first line's words (the label drawn is cut to the tile's width).
  const tile = await until("paged's tile says its attempt", () => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.card('paged')?.rendered?.progress?.machine ?? ''), (text) => text.includes(attempt), 30_000).catch(() => '');
  seen.tile = { words: tile, drawn: await page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.card('paged')?.rendered?.labels?.machine?.text ?? null) };
  check('tile_says_attempt', tile.includes(attempt), seen.tile);
  await shot(page, '2-attempt');
  const drawer = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await signIn(drawer, await projectUrl(folder, `/runs/${pagedRun}`));
  const row = drawer.locator('.kv > div').filter({ has: drawer.locator('dt', { hasText: /^Attempt$/ }) }).locator('dd');
  seen.run_page = await row.innerText({ timeout: 30_000 }).catch(() => null);
  check('run_page_says_attempt', seen.run_page === attempt.replace(/^attempt /, ''), seen.run_page);
  await shot(drawer, '2-attempt-run');
  await drawer.close();
  // Not blocking: the notes are watched meanwhile.
  const listed = (await promisify(execFile)(process.env.COZY, ['run', 'ls'], { cwd: folder, encoding: 'utf8', timeout: 60_000 })).stdout.split('\n');
  seen.run_ls = listed.find((line) => second?.name && line.includes(second.name)) ?? listed;
  check('run_ls_says_attempt', typeof seen.run_ls === 'string' && seen.run_ls.includes(attempt), seen.run_ls);

  await until('paged landed again', () => landed, (at) => at > 0, 300_000).catch(() => 0);
  if (landed) await until("paged's note gone", () => gone, (at) => at > 0, 30_000).catch(() => 0);
  collecting = false;
  await collector;
  seen.notes = [...said];
  check('the_take_landing_is_said', landed > 0, seen.notes);
  check('no_failure_said', ![...said].some((text) => /shut down, so|didn.t render|failed/i.test(text)), seen.notes);
  const box = await page.locator('.notes').boundingBox();
  seen.notes_at = box;
  check('toasts_bottom_left', Boolean(box) && box.x <= 16 && 900 - (box.y + box.height) <= 24, box);
  await shot(page, '3-landed');
  if (landed) {
    seen.finished_gone_ms = gone ? gone - landed : null;
    check('untouched_finished_toast_goes_by_itself', gone > 0 && gone - landed >= 8_000, seen.finished_gone_ms);
  }

  // The same runs, their second attempt, done on this computer's machine on the seeds they were asked with.
  for (const [name, number, was] of [['paged', pagedRun, paged], ['cut', cutRun, cut]]) {
    const done = await until(`${name} done on local`, () => run(number), (one) => ['completed', 'failed', 'canceled'].includes(one.status), 300_000);
    seen[name] = { name: done.name, machine: done.machine, status: done.status, seed: done.request?.seed, was: was.request?.seed };
    check(`${name}_rerun_on_local`, done.machine === 'local' && done.status === 'completed', seen[name]);
    check(`${name}_same_seed`, done.request?.seed !== undefined && done.request?.seed === was.request?.seed, seen[name]);
    check(`${name}_no_failure`, !(await asset(name))?.failure, (await asset(name))?.failure);
  }
  check('paged_has_its_take', ((await asset('paged'))?.versions ?? 0) >= 1, await asset('paged'));
  seen.landed = { paged: seen.paged, cut: seen.cut };
  await shot(page, '4-after');
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);
