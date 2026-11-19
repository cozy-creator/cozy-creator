// preview-pending (xs-215, hack audit web.md row 6): a video's hover preview asked for before Calcifer has made it plays once it's made,
// while the pointer rests. Calcifer answers that ask `202 thumb_pending` and says `thumb` when it's made (cl-982). The page took the 202 as a
// broken video: it asked again with `&try=1..3` at 2, 6 and 14 s, then not for 30 s, so a preview that took longer than that to make never
// showed under a resting pointer. Here Calcifer's draw is slow for real: a noisy video with one keyframe, flat at the 64 × 36 Calcifer
// judges a poster by, so each of the poster's five tries decodes from the first frame. At 20 s it took 12-15 s on CI runners; at 60 s,
// three times that. Then the same preview on its take in Focus, the take row's own player.
const { test, expect, box, canvasCard, canvasVisible, focus, hoverBox, newCanvas, previewState, themed } = require('./fixtures');
const { ffmpeg, fit, rest } = require('./hands');

/** The page's last timed ask on rc/0.27.0 (`&try=3`): a preview still pending past it is the case. */
const LAST_TIMED_ASK_S = 14;

/** 1080p grey under fine noise with a small test pattern, `seconds` long, one keyframe: costly to decode, flat when drawn small. */
function slowClip(path, seconds) {
  ffmpeg('ffmpeg')('-v', 'error', '-y',
    '-f', 'lavfi', '-i', `color=c=0x808080:s=1920x1080:r=30:d=${seconds}`,
    '-f', 'lavfi', '-i', `testsrc2=size=160x90:rate=30:duration=${seconds}`,
    '-filter_complex', '[0:v]noise=alls=40:allf=t+u[n];[n][1:v]overlay=24:24',
    '-c:v', 'libx264', '-preset', 'superfast', '-crf', '30', '-x264-params', 'keyint=infinite:scenecut=0', '-pix_fmt', 'yuv420p', path);
  return require('fs').readFileSync(path);
}

test('preview-pending', async ({ page, project }) => {
  const asks = [];
  const started = Date.now();
  page.on('response', (response) => {
    const url = new URL(response.url());
    if (/\/thumbs\//.test(url.pathname) && url.searchParams.has('preview')) asks.push({ s: (Date.now() - started) / 1000, status: response.status(), url: url.pathname + url.search });
  });
  await project.import(slowClip(test.info().outputPath('slow.mp4'), 60), { name: 'slow', at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, 'slow'), { timeout: 60_000 }).toBe(true);
  await fit(page);

  await test.step('hovered as it lands, its preview plays once Calcifer has made it', async () => {
    await hoverBox(page, 'slow');
    await rest(page, (await newCanvas(page)) ? (await canvasCard(page, 'slow')).body : box(page, 'slow').locator('.media'));
    const hovered = Date.now();
    const played = await expect.poll(async () => (await previewState(page, 'slow')).playing, { timeout: 150_000 }).toBe(true).then(() => true, () => false);
    test.info().annotations.push({ type: 'asks', description: JSON.stringify(asks) });
    const firstAsk = asks[0]?.s ?? (hovered - started) / 1000;
    const pageMade = asks.find((ask) => ask.status === 200);
    // The case: still being made past the last timed ask. A draw this runner made sooner proves nothing either way, so it's red too.
    const late = asks.some((ask) => ask.status === 202 && ask.s - firstAsk >= LAST_TIMED_ASK_S - 1) || Boolean(pageMade && pageMade.s - firstAsk > LAST_TIMED_ASK_S + 1);
    expect.soft(late, `the case: Calcifer still making the preview ${LAST_TIMED_ASK_S} s after it was first asked (asks ${JSON.stringify(asks)})`).toBe(true);
    // Whether Calcifer had made it by now, asked once from the page as its `<video>` would if the page didn't: a red is the page's only if it had.
    const made = pageMade ?? (asks[0] ? { s: (Date.now() - started) / 1000, url: 'asked by the cell', status: await page.evaluate((url) => fetch(url, { cache: 'no-store' })
      .then((answer) => { void answer.body?.cancel(); return answer.status; }), asks[0].url) } : null);
    expect(made?.status, `Calcifer made the preview (asks ${JSON.stringify(asks)})`).toBe(200);
    expect(played, `its preview played while the pointer rested, once Calcifer made it at ${made?.s?.toFixed(1)} s (asks ${JSON.stringify(asks)})`).toBe(true);
    expect.soft(asks.filter((ask) => /[?&]try=/.test(ask.url)), 'no ask guessed by a timer (`&try=`)').toEqual([]);
    expect.soft(asks.length, `asks bounded by its thumb events and waits, not a loop (${JSON.stringify(asks)})`).toBeLessThanOrEqual(12);
  });

  await test.step('screenshots of it playing, paper and dark', async () => {
    for (const theme of ['paper', 'dark']) {
      await project.look(theme);
      await themed(page, theme);
      await expect.poll(async () => (await previewState(page, 'slow')).playing, { timeout: 30_000 }).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`preview-pending-${theme}.png`) });
    }
    await project.look(null);
  });

  await test.step("in Focus, its take on the take row plays its preview while the pointer rests on it (TakeThumb's Preview)", async () => {
    await focus(page, 'slow');
    const take = page.getByRole('region', { name: 'Focused asset' }).locator('.take-thumb').first();
    await expect(take, 'its take on the row').toBeVisible();
    await rest(page, take);
    const playing = () => take.evaluate((el) => {
      const video = el.querySelector('video.preview');
      return Boolean(video && video.classList.contains('on') && !video.paused && video.currentTime > 0.3);
    });
    await expect.poll(playing, { message: "the take's preview plays", timeout: 30_000 }).toBe(true);
    for (const theme of ['paper', 'dark']) {
      await project.look(theme);
      await themed(page, theme);
      await expect.poll(playing, { timeout: 30_000 }).toBe(true);
      await page.screenshot({ path: test.info().outputPath(`preview-pending-take-${theme}.png`) });
    }
    await project.look(null);
  });
});
