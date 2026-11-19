// timeline-fields (xs-215, hack audit web.md row 12): the Video Timeline's frame rate, shape and size are its tool's fields, as every tool's
// settings are (Paul, 11-17: "do not change the design language between tools for no reason"), not a right-click menu's, and Calcifer keeps
// each clip's time when its rate changes, whoever changes it. An Edit of two plates and a still, cut at 24 fps with points on halves at
// 30 fps: `cozy diff` says each point that moves, and `cozy apply` of 30 fps moves each to Math.round(point × 30 / 24), half up. 24 again
// brings each back exactly; 60 → 25 → 60 moves some by a frame, as rounding twice does. In Focus the timeline's bar holds Frame rate (24),
// Aspect ratio and Resolution (Same as the first clip), and the timeline's keys stay its own. Frame rate 25 there moves the points by the
// same rule, and each field set is in the API and in the film joined after it (ffprobe: its rate, frames and size). Its menu has no settings.
const { readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasIdle, canvasPoint, canvasVisible, focus, png, themed } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

const FPS = 24;
// A film joined a moment after it stops changing, the first fetching what joins it.
const JOIN_MS = 120_000;
const CLIPS = [{ clip: '<red>', in: 6, out: 42 }, { clip: '<blue>', in: 10, out: 62 }, { clip: '<title>', frames: 38 }];
const FIRST_CLIP = 'Same as the first clip';
const softly = expect.configure({ soft: true });

/** A clip of one colour under one tone, `seconds` long at 24 fps, 320×180. */
function plate(path, color, seconds, hz) {
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `color=c=${color}:size=320x180:rate=${FPS}:duration=${seconds}`, '-f', 'lavfi', '-i', `sine=frequency=${hz}:sample_rate=32000:duration=${seconds}`,
    '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-g', String(FPS), '-c:a', 'aac', '-shortest', path);
  return readFileSync(path);
}

/** Each clip's points at `to` fps from `from`, as the page's `retimed` moved them: Math.round. */
const rescaled = (points, from, to) => points.map((clip) => Object.fromEntries(Object.entries(clip).map(([key, frame]) => [key, Math.round((frame * to) / from)])));

/** A film's rate, size and frames, by ffprobe. */
function probe(path) {
  const said = ffmpeg('ffprobe')('-v', 'error', '-count_frames', '-select_streams', 'v:0', '-show_entries', 'stream=width,height,r_frame_rate,nb_read_frames', '-of', 'json', path);
  const [stream] = JSON.parse(said).streams;
  return { rate: stream.r_frame_rate, width: stream.width, height: stream.height, frames: Number(stream.nb_read_frames) };
}

test('timeline-fields', async ({ page, project, cli, api }) => {
  test.setTimeout(600_000);
  await project.import(plate(test.info().outputPath('red.mp4'), 'red', 2, 440), { name: 'red', at: { x: 0, y: 0 } });
  await project.import(plate(test.info().outputPath('blue.mp4'), 'blue', 3, 880), { name: 'blue', at: { x: 0, y: 260 } });
  await project.import(png(0, 320, 180, false, () => [0, 255, 0]), { name: 'title', at: { x: 0, y: 520 } });
  await project.add({ name: 'cut', tool: 'cozy/edit', kind: 'video', fields: { fps: FPS, timeline: { tracks: [{ clips: CLIPS }] } }, at: { x: 420, y: 0 } });
  for (const name of ['red', 'blue', 'title', 'cut']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  const AT_24 = CLIPS.map(({ clip, ...points }) => points);
  const fieldsOf = async () => (await project.asset('cut'))?.fields ?? {};
  /** Each clip's points as the project holds them. */
  const points = async () => ((await fieldsOf()).timeline?.tracks?.[0]?.clips ?? []).map(({ clip, sha256, ...held }) => held);
  /** The film of the version that is the cut's state, once it's `want`: its rate, size and frames, and which version. Each is a cut not
   *  made before: one made again is the version that holds it, which Calcifer doesn't make its state (reported, not this cell's). */
  const film = async (what, want) => {
    let seen = null;
    await softly.poll(async () => {
      const asset = await project.asset('cut');
      const version = (asset?.prefixes ?? []).find((held) => held.number === asset.version && held.main?.path && !held.main.absent);
      if (!version) return null;
      // A film still being written can't be read yet: read again on the next look.
      try {
        seen = { ...probe(join(project.path, version.main.path)), number: version.number };
      } catch {
        return null;
      }
      const { number, ...shown } = seen;
      return shown;
    }, { message: `${what}: the cut's version, its film ${JSON.stringify(want)}`, timeout: JOIN_MS }).toEqual(want);
    test.info().annotations.push({ type: 'film', description: `${what}: ${JSON.stringify(seen)}` });
  };
  /** How many frames clips with these points add up to. */
  const frames = (held) => held.reduce((sum, clip) => sum + (clip.frames ?? clip.out - clip.in), 0);

  const file = test.info().outputPath('cut.json');
  /** `cozy apply -f` of the cut at `fps`, from its rev: what `cozy diff` said of it first, its paths. */
  const applied = async (fps) => {
    const rev = (await cli.json('get', 'cut')).rev;
    writeFileSync(file, JSON.stringify({ cut: { rev, 'cozy/edit': { fps } } }));
    const diff = await cli('--json', 'diff', '-f', file);
    const changes = JSON.parse(diff.stdout || '{}').changes ?? [];
    const answer = await cli.json('apply', '-f', file);
    expect(answer.applied, `cozy apply fps ${fps}: ${JSON.stringify(answer.errors ?? answer.conflicts)}`).toBe(true);
    return changes.map((change) => change.path).sort();
  };
  const said = (type, description) => test.info().annotations.push({ type, description });

  let expected = AT_24;
  await test.step('cozy apply at 30 fps: each point the frame at the same time, said by cozy diff first; the film 30 fps', async () => {
    const paths = await applied(30);
    softly(paths, 'cozy diff: the rate and each point that moves').toEqual(['cut.fps', ...['0.in', '0.out', '1.in', '1.out', '2.frames'].map((at) => `cut.timeline.tracks.0.clips.${at}`)].sort());
    expected = rescaled(expected, 24, 30);
    softly(await points(), 'each point at 30 fps: Math.round(point × 30 / 24)').toEqual(expected);
    said('24 → 30', JSON.stringify(await points()));
    const cut = await cli('render', 'cut', '--on', 'local');
    softly(cut.code, `cozy render cut: ${cut.stderr.slice(-300)}`).toBe(0);
    await film('cozy apply at 30 fps', { rate: '30/1', width: 320, height: 180, frames: frames(expected) });
  });

  await test.step('cozy apply at 24 again: each point back exactly; the film 24 fps', async () => {
    await applied(24);
    expected = rescaled(expected, 30, 24);
    softly(expected, 'the rule brings 24 → 30 → 24 back').toEqual(AT_24);
    softly(await points(), '24 → 30 → 24: each point where it was').toEqual(AT_24);
    said('24 → 30 → 24', JSON.stringify(await points()));
    const cut = await cli('render', 'cut', '--on', 'local');
    softly(cut.code, `cozy render cut: ${cut.stderr.slice(-300)}`).toBe(0);
    await film('cozy apply at 24 fps', { rate: '24/1', width: 320, height: 180, frames: frames(AT_24) });
  });

  await test.step('rounding twice: 24 → 60 → 25 → 60 → 24, each as the rule says, its drift said', async () => {
    let from = 24;
    const sixty = [];
    for (const to of [60, 25, 60, 24]) {
      await applied(to);
      expected = rescaled(expected, from, to);
      softly(await points(), `${from} → ${to}: each point by the rule`).toEqual(expected);
      if (to === 60) sixty.push(await points());
      from = to;
    }
    said('60 → 25 → 60', `first ${JSON.stringify(sixty[0])}, then ${JSON.stringify(sixty[1])}`);
    softly(await points(), 'and back at 24, where they began').toEqual(AT_24);
  });

  const region = page.getByRole('region', { name: 'Focused asset' });
  const bar = region.getByRole('toolbar', { name: 'Timeline tools', exact: true });
  const rate = bar.getByRole('combobox', { name: 'Frame rate', exact: true });
  const shape = bar.getByRole('combobox', { name: 'Aspect ratio', exact: true });
  const size = bar.getByRole('combobox', { name: 'Resolution', exact: true });
  const chosen = (select) => select.evaluate((el) => el.selectedOptions[0]?.textContent?.trim() ?? null);

  await test.step("in Focus: Frame rate, Aspect ratio and Resolution in the timeline's bar, at the menu's defaults", async () => {
    await focus(page, 'cut');
    await expect(region.locator('.tl-clip')).toHaveCount(3);
    expect.soft(await chosen(rate), 'Frame rate: the cut\'s, 24').toBe('24');
    expect.soft(await chosen(shape), `Aspect ratio: ${FIRST_CLIP}`).toBe(FIRST_CLIP);
    expect.soft(await chosen(size), `Resolution: ${FIRST_CLIP}`).toBe(FIRST_CLIP);
    expect.soft(await rate.evaluate((el) => [...el.options].filter((option) => !option.hidden).map((option) => option.textContent.trim())), 'its rates, NTSC\'s among them').toEqual(['23.976', '24', '25', '29.97', '30', '48', '50', '59.94', '60']);
    expect.soft(await size.evaluate((el) => [...el.options].map((option) => option.textContent.trim())), 'its sizes').toEqual([FIRST_CLIP, '720p', '1080p', '1440p', '2160p']);
  });

  await test.step("a key in a field is the field's: S there splits nothing", async () => {
    const rail = await region.locator('.tl-rail').boundingBox();
    await page.mouse.click(rail.x + rail.width * 0.4, rail.y + rail.height / 2);
    await expect(region.locator('.tl-playhead')).not.toHaveAttribute('data-frame', '0');
    await rate.focus();
    await page.keyboard.press('s');
    await page.waitForTimeout(500);
    await expect.soft(region.locator('.tl-clip'), 'still three clips').toHaveCount(3);
    softly(await points(), 'the timeline as it was').toEqual(AT_24);
  });

  await test.step('Frame rate 25 in Focus: in the API, each point moved as the rule moves it, the film joined at 25 fps', async () => {
    await rate.selectOption({ label: '25' });
    await expect.poll(async () => (await fieldsOf()).fps, { message: 'the rate, in the API' }).toBe(25);
    const at25 = rescaled(AT_24, 24, 25);
    await softly.poll(points, { message: 'each point at 25 fps: Math.round(point × 25 / 24)' }).toEqual(at25);
    said('Focus 24 → 25', JSON.stringify(await points()));
    await film('Frame rate 25', { rate: '25/1', width: 320, height: 180, frames: frames(at25) });
  });

  const at25 = frames(rescaled(AT_24, 24, 25));
  await test.step("Aspect ratio 1:1: the film square, at the first clip's short edge", async () => {
    await shape.selectOption({ label: '1:1' });
    await expect.poll(async () => (await fieldsOf()).aspect_ratio, { message: 'the shape, in the API' }).toBe('1:1');
    await film('Aspect ratio 1:1', { rate: '25/1', width: 180, height: 180, frames: at25 });
  });

  await test.step('Resolution 720p: the film 720×720', async () => {
    await size.selectOption({ label: '720p' });
    await expect.poll(async () => (await fieldsOf()).resolution, { message: 'the size, in the API' }).toBe('720p');
    await film('Resolution 720p', { rate: '25/1', width: 720, height: 720, frames: at25 });
  });

  await test.step(`Aspect ratio back to ${FIRST_CLIP}: the film 1280×720, the first clip's 16:9`, async () => {
    await shape.selectOption({ label: FIRST_CLIP });
    await expect.poll(async () => 'aspect_ratio' in (await fieldsOf()), { message: 'the shape unset, in the API' }).toBe(false);
    expect.soft(await chosen(shape)).toBe(FIRST_CLIP);
    await film(`Aspect ratio ${FIRST_CLIP}`, { rate: '25/1', width: 1280, height: 720, frames: at25 });
  });

  /** A PATCH of the cut's `fields`, as the page writes one: its status, what it said, and its warnings. */
  const patched = async (fields) => {
    const answer = await api.raw('PATCH', project.at('/assets/cut'), { body: { fields } });
    const body = await answer.json().catch(() => ({}));
    return { status: answer.status, said: body.error?.message ?? body.message ?? null, warnings: body.warnings ?? [] };
  };

  // #4042's review: one rule for every write, the one `cozy apply` checks. Only what the cut can't use is refused, in one line.
  await test.step('PATCH: fps 0 and a non-number refused, each in one line; the rate stays', async () => {
    for (const [fps, line] of [[0, "fps 0 isn't a frame rate: a number above 0, at most 240"], ['fast', "fps fast isn't a frame rate: a number above 0, at most 240"]]) {
      const answer = await patched({ fps });
      test.info().annotations.push({ type: `PATCH fps ${JSON.stringify(fps)}`, description: JSON.stringify(answer) });
      softly(answer.status, `fps ${JSON.stringify(fps)}: refused`).toBeGreaterThanOrEqual(400);
      softly(answer.said, `fps ${JSON.stringify(fps)}: in one line`).toBe(line);
    }
    softly((await fieldsOf()).fps, 'the rate as it was').toBe(25);
  });

  await test.step('PATCH: 15 fps and 2.35:1, unlisted but usable: kept, each said in one line; the selects show them (pictures)', async () => {
    const rates = '23.976, 24, 25, 29.97, 30, 48, 50, 59.94 or 60';
    const slow = await patched({ fps: 15 });
    test.info().annotations.push({ type: 'PATCH fps 15', description: JSON.stringify(slow) });
    softly(slow.status, 'fps 15: kept').toBe(200);
    softly(slow.warnings, 'fps 15: said').toEqual([`fps 15 isn't one of Frame rate's presets (${rates}), so it's used as it is`]);
    const scope = await patched({ aspect_ratio: '2.35:1' });
    test.info().annotations.push({ type: 'PATCH aspect_ratio 2.35:1', description: JSON.stringify(scope) });
    softly(scope.status, '2.35:1: kept').toBe(200);
    softly(scope.warnings.length === 1 && scope.warnings[0].startsWith("aspect_ratio 2.35:1 isn't one of Aspect ratio's presets (16:9, "), `2.35:1: said: ${JSON.stringify(scope.warnings)}`).toBe(true);
    await softly.poll(async () => [await chosen(rate), await chosen(shape)], { message: 'the selects show what they hold' }).toEqual(['15', '2.35:1']);
    for (const theme of ['paper', 'dark']) {
      await project.look(theme);
      await themed(page, theme);
      await page.mouse.move(2, 2);
      await page.waitForTimeout(500);
      await page.screenshot({ path: test.info().outputPath(`unlisted-${theme}.png`) });
    }
    await project.look(null);
  });

  await test.step('Frame rate 29.97 in Focus: NTSC kept exactly, 30000/1001, and the film joined at 30000/1001', async () => {
    await rate.selectOption({ label: '29.97' });
    await expect.poll(async () => (await fieldsOf()).fps, { message: 'the rate, in the API: 30000/1001' }).toBe(30000 / 1001);
    const at = await points();
    await film('Frame rate 29.97', { rate: '30000/1001', width: Math.round((720 * 2.35) / 2) * 2, height: 720, frames: frames(at) });
  });

  await test.step('its menu: no settings, what any box has', async () => {
    await leave(page);
    const { x, y } = await canvasPoint(page, 'cut');
    await page.mouse.click(x, y, { button: 'right' });
    const menu = page.getByRole('menu', { name: "cut's menu" });
    await expect(menu).toBeVisible();
    await expect.soft(menu.getByRole('combobox'), 'no selects').toHaveCount(0);
    expect.soft(await menu.getByRole('menuitem').allTextContents(), 'its items').toEqual(['Download', 'Star', 'Duplicate', 'Rename', 'Delete']);
    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);
    await canvasIdle(page);
  });
});
