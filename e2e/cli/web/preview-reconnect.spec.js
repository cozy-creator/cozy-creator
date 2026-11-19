// preview-reconnect (xs-215, hack audit web.md row 6, review of #3980; solo): a video's hover preview asked for while Calcifer is down
// plays once Calcifer is back, under the same resting pointer. The ask's fetch throws, as nothing answers: the page took that as nothing at
// all and never asked again, where a 202 and a broken file are asked again as a thumbnail is. Its clip is never hovered before, so no copy
// of its preview is in the page or the browser's cache.
const { test, expect, box, canvasCard, canvasVisible, hoverBox, newCanvas, previewState } = require('./fixtures');
const { ffmpeg, fit, rest } = require('./hands');

test('preview-reconnect', { tag: '@solo' }, async ({ page, project, api, guards, calciferControl }) => {
  // What this cell does on purpose, from the stop until its preview plays: past that, the same is a failure.
  const origin = api.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const refused = new RegExp(`^console error: Failed to load resource: net::ERR_[A-Z_]+ ${origin}/`);
  let away = false;
  guards.allow({ test: (line) => away && (refused.test(line) || /status of 503\b.*\/v1\/events\//.test(line)) });
  const clip = test.info().outputPath('clip.mp4');
  ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=30:duration=6', '-c:v', 'libx264', '-preset', 'superfast', '-pix_fmt', 'yuv420p', clip);
  await project.import(require('fs').readFileSync(clip), { name: 'clip', at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'clip'), { timeout: 60_000 }).toBe(true);
  await fit(page);
  const asks = [];
  page.on('requestfailed', (request) => { if (new URL(request.url()).searchParams.has('preview')) asks.push({ failed: request.failure()?.errorText }); });
  page.on('response', (response) => { if (new URL(response.url()).searchParams.has('preview')) asks.push({ status: response.status() }); });

  await test.step('Calcifer stopped, the pointer rests on the clip: its preview is asked for and nothing answers', async () => {
    away = true;
    await calciferControl.stop();
    await hoverBox(page, 'clip');
    await rest(page, (await newCanvas(page)) ? (await canvasCard(page, 'clip')).body : box(page, 'clip').locator('.media'));
    await expect.poll(() => asks.some((ask) => ask.failed), { message: 'its preview asked for while Calcifer was down', timeout: 30_000 }).toBe(true);
    expect((await previewState(page, 'clip')).playing, 'no preview plays with Calcifer down').toBe(false);
  });

  await test.step('Calcifer back, the pointer still: its preview plays', async () => {
    await calciferControl.start();
    await expect.poll(async () => (await previewState(page, 'clip')).playing, { message: `its preview plays once Calcifer is back (asks ${JSON.stringify(asks)})`, timeout: 90_000 }).toBe(true);
    away = false;
  });
});
