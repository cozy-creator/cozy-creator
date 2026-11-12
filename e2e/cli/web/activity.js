// What each machine is doing, top right (Paul, 11-06): `local` among the machines, a click lists what it does, an install and a download each
// once, under the machine doing them, with the runs that wait on them, and a box whose render waits on its install says so instead of a
// percentage. Where this computer runs models; the download is SDXL's 335 MB VAE from Hugging Face, kept as a local model.
// A box on its way says so on its face (cl-898): its render on this cold machine shows each phase in order, its machine, a decode's tokens
// as a count, never steps, its steps with their time left, then its take; Cancel on its face stops one waiting its turn and one rendering.
//   node activity.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { spawn } = require('child_process');
const { writeFileSync } = require('fs');
const { join } = require('path');
const { projectFromPage, projectAction, calcifer, importYaml, signIn, STEP_MS, lastLook, launch, lookAt, noStall, openBox, renderOf, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const COZY = process.env.COZY;
// One this machine doesn't hold: a held source is no download.
const VAE = 'hf://stabilityai/sdxl-vae/diffusion_pytorch_model.safetensors';
const VAE_SAID = 'Sdxl Vae';
// A picture tool of its own, new to the machine, so its environment is installed while the box waits. It takes no GPU. It goes as the
// models do, by their names: its prompt rewritten a token at a time against a cap far over what it writes, as Qwen Image 2's rewriter
// tells it (`ctx.progress(token, 2176)` under `upsampling prompt`; a package can't reach the Runtime's own token unit, which `lm.generate`
// sets); then in two parts as MiniMax H3's segments, each denoised by steps of `pace` s and decoded by frames (`decode_video`); then saved.
const TOOL = `# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The activity cell's picture tool: a weightless still of one random colour, made as a model makes one."""

import os
import time

import msgspec
from cozy_runtime.author import Adjustments, App, Context, ImageFrame, Outputs, Seed, Telemetry
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageRequest, ImageResult, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    pace: float = 0.8


@app.tool(label="Activity", named="waits", accelerator=False)
def draw(ctx: Context, payload: Drawing, adj: Adjustments, out: Outputs, tel: Telemetry) -> ImageResult:
    width, height = ImageRequest(payload.instructions.text, None, None, payload.seed).size(adj, None, SIZES, megapixels=0.25)
    with tel.stage("upsampling prompt"):
        for token in range(64):
            time.sleep(0.05)
            ctx.progress(token, 2176)
    for part in range(2):
        with tel.scope(f"Segment {part + 1} of 2"):
            with tel.stage("denoise"):
                for step in range(4):
                    time.sleep(payload.pace)
                    ctx.progress(step, 4)
            with tel.stage("decode_video"):
                for frame in range(12):
                    time.sleep(0.1)
                    ctx.progress(frame, 12)
    with tel.stage("saving image"):
        image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
`;

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1400, height: 900 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());
  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path) => (await fetch(base + path, { headers: { Authorization: `Bearer ${token}` } })).json();

  // `local` is a machine at the top right, whatever it's doing, where this computer runs models: macOS and Windows list none.
  const chip = page.locator('.mtab[data-machine="local"]');
  const usable = (await api('/v1/status')).local_usable;
  if (usable) await chip.waitFor();
  check('local_has_a_chip', (await chip.count()) === (usable ? 1 : 0), { usable, chips: await page.locator('.mtab').allInnerTexts() });
  if (!usable || process.env.ALONE) {
    seen.ran = false;
    return finish();
  }
  seen.ran = true;

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-activity-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const folder = (await api(`/v1/projects/${id}`)).path;
  seen.project = { id, folder };
  const tool = join(folder, `activity_${Date.now()}.py`);
  writeFileSync(tool, TOOL);
  // `slow` and `after`, which reads it, are what Cancel stops: one rendering, one waiting its turn.
  importYaml(folder, `version: 4\nassets:\n  waits:\n    tool: ${tool}/draw\n  slow:\n    tool: ${tool}/draw\n    pace: 6\n  after:\n    tool: ${tool}/draw\n    instructions: After <slow>.\n`);

  // Its render waits on its package's install, a second or two from uv's cache: the box says so, and the chip's list has the install once,
  // with the run that waits on it. Everything they say is kept as it shows, so a short install is seen whole.
  await page.locator('.card[data-asset="waits"]').waitFor();
  await page.evaluate(() => {
    const said = (window.said = { box: [], rows: [], marks: [], faces: {} });
    const keep = (list, text) => text && !list.includes(text) && list.push(text);
    // Each box's face as it changes: what it says, its time left, where, the detail, its phase and how far its bar is.
    const faces = (name) => {
      const face = document.querySelector(`.card[data-asset="${name}"] .run-face`);
      if (!face) return;
      const text = (selector) => face.querySelector(selector)?.textContent ?? '';
      const now = { phase: face.dataset.phase, says: text('.face-state b'), stage: text('.face-state b'), left: text('.face-left'), where: text('.fact-where dd'), facts: [...face.querySelectorAll('.face-fact dt')].map((label) => label.textContent).join(','), time: text('.fact-time dd'), detail: text('.face-detail'), bar: face.querySelector('.face-track i')?.style.transform ?? '' };
      const list = (said.faces[name] ??= []);
      if (JSON.stringify(list.at(-1)) !== JSON.stringify(now)) list.push(now);
    };
    new MutationObserver(() => {
      for (const name of ['waits', 'slow', 'after']) faces(name);
      keep(said.box, document.querySelector('.card[data-asset="waits"] .run-face .face-state b')?.textContent);
      for (const row of document.querySelectorAll('.activity [data-doing]')) keep(said.rows, `${row.dataset.doing}: ${row.innerText.replace(/\s+/g, ' ')}`);
      for (const mark of document.querySelectorAll('.mtab[data-machine="local"] .mark, .mtab[data-machine="local"] .dot')) keep(said.marks, mark.className);
    }).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
  });
  await openBox(page, 'waits');
  await (await renderOf(page, 'waits')).click();
  await page.keyboard.press('Escape');
  await chip.click();
  const list = page.locator('.activity');
  await list.waitFor();
  // A render that fails ends the wait with its own words.
  const waits = async () => (await api(`/v1/projects/${id}`)).assets.find((asset) => asset.name === 'waits');
  const first = await until('its first take', waits, (asset) => asset?.version || asset?.failure, 180_000);
  if (!first.version) throw new Error(`its render failed: ${first.failure.message}`);
  const said = await page.evaluate(() => window.said);
  seen.said = said;
  check('box_said_installing', said.box.some((text) => /^Installing activity_\d+…?$/.test(text)), said.box);
  // Its face, from Render to its take: never a word short; each phase in order, with its numbers; a decode's tokens a count, never steps;
  // its steps with their time left; where it runs; and gone once its take is here.
  const faces = said.faces.waits ?? [];
  const firstOf = (test) => faces.findIndex(test);
  const installs = firstOf((face) => /^Installing activity_\d+/.test(face.says));
  const tokens = firstOf((face) => /^Rewriting the prompt · \d+ tokens$/.test(face.stage));
  const steps = firstOf((face) => /^Segment 1 of 2 · Denoising · step \d of 4$/.test(face.stage));
  const frames = firstOf((face) => /^Segment 2 of 2 · Decoding video · frame \d+ of 12$/.test(face.stage));
  const fraction = (face) => (face.bar ? 1 + Number(face.bar.match(/-?[\d.]+/)?.[0]) / 100 : null);
  check('face_never_wordless', faces.length > 0 && faces.every((face) => face.says.trim()), faces);
  check('face_phases_in_order', installs >= 0 && tokens > installs && steps > tokens && frames > steps, { installs, tokens, steps, frames, faces });
  // Each stage says what it counts: a decode's tokens, never steps; frames; steps only where it denoises (Paul, 11-12).
  check('face_counts_tokens_not_steps', faces.filter((face) => /Rewriting the prompt/.test(face.stage)).every((face) => /^Rewriting the prompt · \d+ tokens$/.test(face.stage)), faces);
  check('face_denoise_only_denoising', faces.every((face) => !/denois/i.test(`${face.says} ${face.stage}`) || /^Segment [12] of 2 · Denoising( · step \d of 4)?$/.test(face.stage)), faces.filter((face) => /denois/i.test(`${face.says} ${face.stage}`)));
  check('face_decodes_frames', faces.some((face) => /^Segment [12] of 2 · Decoding video · frame \d+ of 12$/.test(face.stage)) && !faces.some((face) => /Decoding video.*step/.test(face.stage)), faces);
  // The bar and the time left are the whole render's: in its second part the bar is past half of it, never back to a part's start.
  const second = faces.filter((face) => /^Segment 2 of 2/.test(face.stage));
  check('face_steps_with_time_left', faces.some((face) => /^Segment [12] of 2 · Denoising · step [2-4] of 4$/.test(face.stage) && /\d+ (s|min) left$/.test(face.left)), faces);
  check('face_whole_render_bar', second.length > 0 && second.every((face) => (fraction(face) ?? 0) >= 0.4) && second.some((face) => /\d+ (s|min) left$/.test(face.left)), second);
  check('face_says_where', faces.some((face) => /^local\b/.test(face.where)), faces);
  // Its facts (Paul, 11-12): where, what's ahead and how many wait, its weights, its package, its time elapsed and left.
  check('face_lists_its_facts', faces.some((face) => /Where/.test(face.facts) && /Queue/.test(face.facts) && /Weights/.test(face.facts) && /Package/.test(face.facts) && /\d+:\d\d elapsed/.test(face.time)), faces.map((face) => [face.facts, face.time]).slice(-4));
  // Made, its file on its way here is a phase of its own with its numbers, never a take that looks done before it's here (Paul, 11-12). On
  // this runner's loopback a still lands in milliseconds, so it may pass too fast to be seen: whenever it is, it says its bytes.
  const result = faces.filter((face) => face.phase === 'result');
  seen.result_phase = result;
  check('face_result_download_says_its_bytes', result.every((face) => face.says === 'Saving' || /^Downloading the result · \d.* of \d/.test(face.says)), result);
  const gone = await until('its face gone with its take', () => page.locator('.card[data-asset="waits"] .run-face').count(), (n) => n === 0, 15_000).catch(() => 1);
  check('face_gone_with_its_take', gone === 0);
  check('install_listed_with_its_waiter', said.rows.some((row) => /^install: activity_\d+ Install .* for run \d+$/.test(row)), said.rows);
  check('a_run_waited_on_it', said.rows.some((row) => /^run: .*Waits on the install of activity_\d+$/.test(row)), said.rows);
  check('chip_marked_an_install', said.marks.includes('mark install'), said.marks);
  check('install_done_leaves_the_list', (await list.locator('[data-doing=install]').count()) === 0);

  // A download: listed once, by the bytes it fetches, with its rate and time left, and the run that waits on it.
  const name = `local/activity-vae-${Date.now()}`;
  const download = spawn(COZY, ['--json', 'model', 'install', VAE, name, '--on', 'local'], { cwd: folder, stdio: ['ignore', 'pipe', 'pipe'] });
  const evidence = seen.download_cli = { events: [], exit_code: null, signal: null, stderr: '' };
  const safe = (text) => String(text ?? '').replace(/Bearer\s+\S+/gi, 'Bearer [redacted]').replace(/cozy1-[a-z0-9_-]+/gi, '[recovery redacted]').replace(/[A-Za-z0-9_+/-]{64,}/g, '[opaque value redacted]').slice(-2048);
  let pending = '';
  download.stdout.on('data', (chunk) => {
    pending = (pending + chunk.toString()).slice(-65536);
    const lines = pending.split('\n');
    pending = lines.pop();
    for (const line of lines) {
      try {
        const event = JSON.parse(line);
        const run = event.run, progress = event.progress, error = event.error;
        evidence.events.push({ type: event.type, run: run && { number: run.number, kind: run.kind, status: run.status, machine_state: run.machine_state, reason: safe(run.reason) }, progress: progress && { stage: progress.stage, bytes_done: progress.bytes_done, bytes_total: progress.bytes_total }, error: error && { kind: error.kind, code: error.code, message: safe(error.message) } });
        if (evidence.events.length > 24) evidence.events.shift();
      } catch { /* only typed command events are retained */ }
    }
  });
  download.stderr.on('data', (chunk) => { evidence.stderr = safe(evidence.stderr + chunk.toString()); });
  const ended = new Promise((done) => {
    download.on('error', (error) => { evidence.spawn_error = safe(error.message); done(null); });
    download.on('exit', (code, signal) => { evidence.exit_code = code; evidence.signal = signal; done(code); });
  });
  const fetching = list.locator('[data-doing=download]');
  await until('the download listed under local', async () => {
    const count = await fetching.count();
    if (count === 0 && (evidence.exit_code !== null || evidence.signal !== null || evidence.spawn_error)) {
      throw new Error(`download ended before its row was observed: ${JSON.stringify(evidence)}`);
    }
    return count;
  }, (n) => n === 1, 120_000);
  const row = await until('its bytes moving', () => fetching.first().innerText(), (text) => /[1-9][\d.]* [KMG]iB \/ /.test(text), 60_000);
  seen.download = row;
  check('download_counts_its_bytes', /iB \/ \d+ MiB/.test(row), row);
  check('download_names_its_waiter', /for run \d+/.test(row), row);
  check('download_named_as_people_say_it', row.startsWith(VAE_SAID), row);
  check('its_run_waits_on_the_download', (await list.locator('[data-doing=run]').filter({ hasText: `Waits on the download of ${VAE_SAID}` }).count()) === 1, await list.innerText());
  check('chip_marks_a_download', (await chip.locator('.mark.download').count()) === 1);
  // A download and the warm run it is are no job: the chip counts function runs, its GPUs' work (cl-889).
  check('a_download_is_no_job', (await chip.locator('.load').count()) === 0, await chip.innerText());
  check('a_download_is_listed_once', (await fetching.count()) === 1);
  await shot('download');

  // Escape closes it, and so does a click elsewhere.
  await page.keyboard.press('Escape');
  check('escape_closes', (await list.count()) === 0);
  await chip.click();
  await list.waitFor();
  await page.mouse.click(700, 600);
  check('a_click_elsewhere_closes', (await list.count()) === 0);

  const code = await ended;
  check('download_ran', code === 0, code);
  // Nothing of it stays listed once it's done.
  await chip.click();
  await until('the download gone from the list', () => list.locator('[data-doing=download]').count(), (n) => n === 0, 30_000);
  check('done_leaves_the_list', true);
  await page.keyboard.press('Escape');

  // Cancel, on the face of a box waiting its turn and of one rendering (Paul, 11-12): no question asked, it says Cancelling… until its run
  // ends, it's left as it was, and Undo renders it again. `after` waits for `slow`, whose steps are slow enough to be stopped.
  const held = async (asset) => (await api(`/v1/projects/${id}`)).assets.find((one) => one.name === asset);
  const cancel = (asset) => page.locator(`.card[data-asset="${asset}"] .face-cancel`).evaluate((button) => button.click());
  const sayOf = (asset) => page.locator(`.card[data-asset="${asset}"] .run-face .face-state b`).first().textContent().catch(() => '');
  await projectAction(folder, 'render', 'slow', 'after', '--on', 'local', '--queue');
  const waited = await until('after waits its turn', () => sayOf('after'), (text) => /^Waits for <slow>/.test(text ?? ''), 60_000).catch((why) => String(why));
  const rendering = await until('slow renders its steps', () => sayOf('slow'), (text) => /^Segment 1 of 2 · Denoising/.test(text ?? ''), 120_000).catch((why) => String(why));
  seen.cancel = { waited, rendering };
  // Open (focus), its progress is between its name and its picture, at normal size, and covers none of it (Paul, 11-12).
  await openBox(page, 'slow');
  const above = page.locator('.focus-viewport[data-focused-asset="slow"] > .run-face.above');
  await above.waitFor().catch(() => {});
  const focus = await page.evaluate(() => {
    const viewport = document.querySelector('.focus-viewport[data-focused-asset="slow"]');
    const box = (selector) => viewport?.querySelector(selector)?.getBoundingClientRect();
    const [head, face, media] = [box(':scope > .focus-heading'), box(':scope > .run-face.above'), box('.focus-body .media')];
    const says = viewport?.querySelector(':scope > .run-face.above .face-state b');
    const stage = viewport?.querySelector(':scope > .run-face.above .face-state b')?.textContent ?? '';
    const bar = viewport?.querySelector(':scope > .run-face.above .face-bar .face-track')?.getBoundingClientRect().height;
    return { head: head?.bottom, top: face?.top, bottom: face?.bottom, media: media?.top, width: face && viewport && face.width / viewport.getBoundingClientRect().width, size: says && says.getBoundingClientRect().height, bar, stage, over: viewport?.querySelectorAll('.media .run-face').length };
  });
  seen.focus = focus;
  await shot('face-focus');
  check('focus_progress_between_name_and_picture', focus.top >= focus.head - 1 && focus.bottom <= focus.media + 1 && focus.width > 0.9 && focus.over === 0, focus);
  check('focus_progress_normal_size', focus.size >= 14 && focus.size <= 32, focus);
  // Its bar is the whole render's; the part it's in and that stage's count are on the line under it.
  check('focus_part_under_the_bar', /^Segment [12] of 2 · (Denoising|Decoding video)/.test(focus.stage), focus);
  await page.keyboard.press('Escape');
  await cancel('after');
  const left = await until('after left its line', async () => ({ faces: await page.locator('.card[data-asset="after"] .run-face').count(), asset: await held('after') }), (now) => now.faces === 0 && !now.asset?.making?.length, 30_000).catch((why) => ({ why: String(why) }));
  const still = (await sayOf('slow')) ?? '';
  check('cancel_while_queued', left.faces === 0 && !left.asset?.version && Boolean(still) && still !== 'Cancelling…', { left, slow: still });
  const offered = await until('its Undo', () => page.locator('.notes .note').filter({ hasText: "Canceled after's render" }).locator('button').allInnerTexts(), (labels) => labels.includes('Undo'), 5000).catch((why) => String(why));
  check('cancel_offers_undo', Array.isArray(offered), offered);
  const making = (await held('slow'))?.making ?? [];
  await cancel('slow');
  const stopped = await until('slow stopped', async () => ({ faces: await page.locator('.card[data-asset="slow"] .run-face').count(), asset: await held('slow') }), (now) => now.faces === 0 && !now.asset?.making?.length, 60_000).catch((why) => ({ why: String(why) }));
  const runs = await Promise.all(making.map((number) => api(`/v1/runs/${number}`)));
  const cancelling = (said.faces = (await page.evaluate(() => window.said.faces))).slow ?? [];
  check('cancel_says_cancelling', cancelling.some((face) => face.says === 'Cancelling…'), cancelling.slice(-6));
  check('cancel_while_rendering', stopped.faces === 0 && !stopped.asset?.version && runs.length > 0 && runs.every((run) => run.status === 'canceled'), { stopped, runs: runs.map((run) => [run.number, run.status]) });
  return finish();

  async function finish() {
    check('no_page_errors', errors.length === 0, errors);
    console.log(JSON.stringify({ failures, seen }, null, 2));
    await browser.close();
    process.exit(failures.length ? 1 : 0);
  }
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
