// orbit (cl-1131). Paul, 11-17: "videos can be marked as 'orbit' videos (a property of the video). In that case, they get a special player; you
// can grab the image and spin it around, 360-style, as if it were a 3d model. Currently instead they are treated just as regular videos, where you
// push play and watch it in time." A video box whose tool declares its file an orbit (project/orbit.py's `turn`: `with_metadata`, every frame a
// keyframe, as the `orbit` package does) lands that metadata on its version. At rest on the board it carries the orbit mark, which a plain video
// doesn't; in Focus it has no play button or timeline, and a drag turns it, there and in full screen. Pictures to look at, at rest and in Focus,
// at 1440×900 and 1920×1080, paper and dark: $OUT/web/orbit/<browser>.
const { mkdirSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, focus, canvasVisible, canvasIdle, fixtureTool } = require('./fixtures');
const { ffmpeg, fit, leave } = require('./hands');

const TURN = fixtureTool('orbit.py/turn');
const ORBIT = { degrees: 360, direction: 'cw', loops: true };
const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: web.sh's web_suite sets it`); })();

/** `locator`'s box once full screen has laid it out: grown past `was` and the same three reads running. */
async function grown(locator, was) {
  let box = null, same = 0;
  await expect.poll(async () => {
    const now = await locator.boundingBox();
    same = now && box && now.width > was.width + 1 && Math.abs(now.width - box.width) < 1 && Math.abs(now.x - box.x) < 1 ? same + 1 : 0;
    box = now;
    return same >= 2;
  }, { message: 'laid out full screen', intervals: [150] }).toBe(true);
  return box;
}

/** What the board drew for `name` at rest: its orbit mark, or null. */
const markOf = (page, name) => page.evaluate((name) => document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered?.orbit ?? null, name);

test('orbit', async ({ page, project, api, cli }) => {
  await cli.json('machine', 'use', 'local');
  const shots = join(need('OUT'), 'web', 'orbit', test.info().project.name);
  mkdirSync(shots, { recursive: true });
  const plain = test.info().outputPath('plain.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=24:duration=2', '-c:v', 'libx264', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', plain);
  await project.import(readFileSync(plain), { name: 'plain', at: { x: 520, y: 0 } });
  await project.add({ name: 'turned', tool: TURN, kind: 'video', fields: { frames: 48 }, at: { x: 0, y: 0 } });
  const region = page.getByRole('region', { name: 'Focused asset' });

  await test.step("its tool's orbit lands on its version", async () => {
    const rendered = await cli('--json', 'render', 'turned', '--on', 'local');
    expect(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
    await expect.poll(async () => Boolean((await project.asset('turned'))?.prefixes?.at(-1)?.main), { message: "turned's take", timeout: 120_000 }).toBe(true);
    expect((await project.asset('turned')).prefixes.at(-1).main.metadata?.orbit, "its version's file says it's an orbit").toEqual(ORBIT);
  });

  await test.step('at rest it carries the orbit mark, and a plain video does not', async () => {
    for (const name of ['turned', 'plain']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
    await fit(page);
    await expect.poll(() => markOf(page, 'turned'), { message: 'the orbit mark on turned at rest' }).not.toBeNull();
    const mark = await markOf(page, 'turned');
    expect.soft(mark.width, 'the mark as big as on screen as a control').toBeGreaterThanOrEqual(14);
    expect(await markOf(page, 'plain'), 'no mark on a plain video').toBeNull();
  });

  await test.step('in Focus: no play button or timeline, and a drag turns it', async () => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await focus(page, 'turned');
    const orbit = region.getByRole('slider', { name: 'Turn', exact: true });
    await expect(region.locator('.orbit.ready')).toBeVisible();
    await expect(region.locator('.tl-transport, .tl-take'), 'no timeline').toHaveCount(0);
    await expect(region.getByRole('button', { name: /^(Play|Pause)$/ }), 'no play button').toHaveCount(0);
    const turn = await orbit.boundingBox();
    const y = turn.y + turn.height / 2;
    await page.mouse.move(turn.x + turn.width * 0.25, y);
    await page.mouse.down();
    await page.mouse.move(turn.x + turn.width * 0.5, y, { steps: 8 });
    await page.mouse.up();
    await expect.poll(async () => Number(await orbit.getAttribute('data-angle')), { message: 'a quarter of its width turns it a quarter round' }).toBeCloseTo(90, -1);
    expect.soft(Number(await orbit.getAttribute('data-frame')), 'the frame at 90°: 48 × 90/360').toBe(12);
  });

  await test.step('full screen: a drag turns it there too', async () => {
    const orbit = region.getByRole('slider', { name: 'Turn', exact: true });
    await region.locator('.orbit-bar').getByRole('button', { name: 'Front', exact: true }).click();
    await expect.poll(async () => Number(await orbit.getAttribute('data-angle'))).toBe(0);
    const was = await orbit.boundingBox();
    await region.locator('.orbit-bar').getByRole('button', { name: 'Full screen', exact: true }).click();
    await expect.poll(() => page.evaluate(() => Boolean(document.fullscreenElement?.matches('.focus-viewer[data-full]'))), { message: "the orbit's Full screen" }).toBe(true);
    const turn = await grown(orbit, was);
    const y = turn.y + turn.height / 2;
    await page.mouse.move(turn.x + turn.width * 0.25, y);
    await page.mouse.down();
    await page.mouse.move(turn.x + turn.width * 0.75, y, { steps: 12 });
    await page.mouse.up();
    await expect.poll(async () => Number(await orbit.getAttribute('data-angle')), { message: 'half its width turns it half round' }).toBeCloseTo(180, -1);
    await page.screenshot({ path: join(shots, 'orbit-full.png') });
    await page.keyboard.press('Escape');
    await expect.poll(() => page.evaluate(() => document.fullscreenElement === null)).toBe(true);
    await expect(region).toHaveAttribute('data-focused-asset', 'turned');
    await page.emulateMedia({ reducedMotion: null });
  });

  await test.step('to look at: at rest and in Focus at 1440×900 and 1920×1080, paper and dark', async () => {
    for (const theme of ['paper', 'dark']) {
      if (await region.count()) await leave(page);
      await project.look(theme);
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await themed(page, theme);
      for (const [width, height] of [[1440, 900], [1920, 1080]]) {
        if (await region.count()) await leave(page);
        await page.setViewportSize({ width, height });
        await canvasIdle(page);
        await fit(page);
        await expect.poll(() => markOf(page, 'turned')).not.toBeNull();
        await page.screenshot({ path: join(shots, `rest-${width}x${height}-${theme}.png`) });
        await focus(page, 'turned');
        await expect(region.locator('.orbit.ready')).toBeVisible();
        await page.screenshot({ path: join(shots, `focus-${width}x${height}-${theme}.png`) });
      }
    }
  });
});
