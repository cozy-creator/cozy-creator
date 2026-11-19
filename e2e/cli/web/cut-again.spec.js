// cut-again (cl-1310, the Edit cl-957): an Edit cut, changed and cut again, then undone to its first timeline and rendered, lands that
// film again as its newest version, and the box shows it. Paul: "every render makes a new version, except pinned" (11-04), "without a pin, a
// node uses its newest version" (11-03). It's the first version's film by hash (no bytes copied), read with ffprobe from what the box shows
// and from its film at the project's root, and said as any cut (`cut take 3 … cut.mp4`); Redo lands v2's film the same way. A render with
// nothing changed still makes none (cl-910).
const { readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasIdle, canvasVisible } = require('./fixtures');
const { ffmpeg, fit } = require('./hands');

const JOIN_MS = 120_000;
const softly = expect.configure({ soft: true });

/** A clip of one colour under one tone, `seconds` long at 24 fps, 320×180. */
function plate(path, color, seconds, hz) {
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', `color=c=${color}:size=320x180:rate=24:duration=${seconds}`, '-f', 'lavfi', '-i', `sine=frequency=${hz}:sample_rate=32000:duration=${seconds}`,
    '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-g', '24', '-c:a', 'aac', '-shortest', path);
  return readFileSync(path);
}

/** A film's rate and frames, by ffprobe. */
function probe(path) {
  const said = ffmpeg('ffprobe')('-v', 'error', '-count_frames', '-select_streams', 'v:0', '-show_entries', 'stream=r_frame_rate,nb_read_frames', '-of', 'json', path);
  const [stream] = JSON.parse(said).streams;
  return { rate: stream.r_frame_rate, frames: Number(stream.nb_read_frames) };
}

test('cut-again', async ({ page, project, cli }) => {
  test.setTimeout(600_000);
  await project.import(plate(test.info().outputPath('red.mp4'), 'red', 2, 440), { name: 'red', at: { x: 0, y: 0 } });
  await project.import(plate(test.info().outputPath('blue.mp4'), 'blue', 3, 880), { name: 'blue', at: { x: 0, y: 260 } });
  const BOTH = [{ clip: '<red>' }, { clip: '<blue>' }];
  await project.add({ name: 'cut', tool: 'cozy/edit', kind: 'video', fields: { fps: 24, timeline: { tracks: [{ clips: BOTH }] } }, at: { x: 420, y: 0 } });
  for (const name of ['red', 'blue', 'cut']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  const asset = () => project.asset('cut');
  const numbers = async () => ((await asset())?.prefixes ?? []).map((version) => version.number);
  const clips = async () => ((await asset())?.fields?.timeline?.tracks?.[0]?.clips ?? []).map((clip) => clip.clip);
  /** What the box shows: the version that is its state, its film's hash, rate and frames. */
  const shown = async () => {
    const now = await asset();
    const version = (now?.prefixes ?? []).find((held) => held.number === now.version);
    if (!version?.main?.path || version.main.absent) return null;
    try {
      return { number: version.number, sha256: version.main.sha256, ...probe(join(project.path, version.main.path)) };
    } catch {
      return null;
    }
  };
  /** Its film at the project's root, which follows what it shows. */
  const root = () => {
    try {
      return probe(join(project.path, 'cut.mp4'));
    } catch (error) {
      return String(error).slice(0, 200);
    }
  };
  /** `cozy render cut`, as an agent runs it. */
  const render = async (what) => {
    const said = await cli('render', 'cut', '--on', 'local');
    test.info().annotations.push({ type: `cozy render (${what})`, description: `${said.code}: ${said.stdout.trim().slice(-300)}` });
    softly(said.code, `cozy render (${what}): ${said.stderr.slice(-300)}`).toBe(0);
    return said;
  };

  let first, second;
  await test.step('v1: red and blue cut, 120 frames at 24 fps', async () => {
    await render('v1');
    await expect.poll(numbers, { message: 'its first version', timeout: JOIN_MS }).toEqual([1]);
    await expect.poll(async () => (first = await shown())?.frames, { message: 'v1 shown', timeout: JOIN_MS }).toBe(120);
    expect(first).toMatchObject({ number: 1, rate: '24/1', frames: 120 });
  });

  await test.step('rendered again with nothing changed: no new version (cl-910)', async () => {
    const said = await render('nothing changed');
    softly(said.stdout, 'said current').toContain('version 1 already cuts what it reads now');
    await page.waitForTimeout(2_000);
    expect(await numbers(), 'no new version').toEqual([1]);
  });

  await test.step('cozy apply takes blue out; cut again, v2 is red alone, 48 frames', async () => {
    const rev = (await cli.json('get', 'cut')).rev;
    const file = test.info().outputPath('cut.json');
    writeFileSync(file, JSON.stringify({ cut: { rev, 'cozy/edit': { timeline: { tracks: [{ clips: [{ clip: '<red>' }] }] } } } }));
    const applied = await cli.json('apply', '-f', file);
    expect(applied.applied, `cozy apply: ${JSON.stringify(applied.errors ?? applied.conflicts)}`).toBe(true);
    await render('v2');
    await expect.poll(async () => (await shown())?.number, { message: 'v2 shown', timeout: JOIN_MS }).toBe(2);
    second = await shown();
    expect(second).toMatchObject({ number: 2, rate: '24/1', frames: 48 });
  });

  await test.step("Undo on the page: the timeline red and blue again", async () => {
    await canvasIdle(page);
    const undo = page.locator('.top .history').getByRole('button', { name: 'Undo', exact: true });
    await expect(undo).toBeEnabled();
    await undo.click();
    await expect.poll(clips, { message: 'blue back' }).toEqual(['<red>', '<blue>']);
  });

  await test.step("rendered: v1's film lands again as v3, by hash, said as any cut, and the box and its root film show it", async () => {
    const said = await render('after Undo');
    softly(said.stdout, 'said as any cut: its take and its film at the root').toMatch(/\bcut take 3\b[^\n]*\/cut\.mp4/);
    await softly.poll(numbers, { message: 'a new version, v3', timeout: JOIN_MS }).toEqual([1, 2, 3]);
    await softly.poll(shown, { message: "the box shows v3, v1's film", timeout: JOIN_MS }).toEqual({ number: 3, sha256: first.sha256, rate: '24/1', frames: 120 });
    await softly.poll(root, { message: 'its film at the root, as it shows', timeout: JOIN_MS }).toEqual({ rate: '24/1', frames: 120 });
    test.info().annotations.push({ type: 'shown after Undo', description: JSON.stringify(await shown()) });
  });

  await test.step('rendered again with nothing changed: no new version (cl-910)', async () => {
    const before = await numbers();
    const said = await render('nothing changed, again');
    softly(said.stdout, 'said current').toMatch(/version \d+ already cuts what it reads now/);
    await page.waitForTimeout(2_000);
    expect(await numbers(), 'no new version').toEqual(before);
  });

  await test.step("Redo on the page, rendered: v2's film lands again as v4, by hash, said as any cut", async () => {
    const redo = page.locator('.top .history').getByRole('button', { name: 'Redo', exact: true });
    await expect(redo).toBeEnabled();
    await redo.click();
    await expect.poll(clips, { message: 'blue out again' }).toEqual(['<red>']);
    const said = await render('after Redo');
    softly(said.stdout, 'said as any cut').toMatch(/\bcut take 4\b[^\n]*\/cut\.mp4/);
    await softly.poll(numbers, { message: 'a new version, v4', timeout: JOIN_MS }).toEqual([1, 2, 3, 4]);
    await softly.poll(shown, { message: "the box shows v4, v2's film", timeout: JOIN_MS }).toEqual({ number: 4, sha256: second.sha256, rate: '24/1', frames: 48 });
    await softly.poll(root, { message: 'its film at the root, as it shows', timeout: JOIN_MS }).toEqual({ rate: '24/1', frames: 48 });
  });
});
