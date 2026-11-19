// picture-pending (xs-215, hack audit web.md row 8 part B, review of #4031): every picture is asked for as one policy says (`asking`).
// A picture asked for before Calcifer has drawn its thumbnail (`202 thumb_pending`, cl-982) is drawn once it's made, on the board and in
// the Assets drawer's row, with no timed retry. And a picture whose thumbnail can't be made (`thumb_failed`): once one viewer has given up
// on it, the next says so at once, where it waited, blank, for the file's next event.
const { createHash, randomBytes } = require('crypto');
const { readFileSync } = require('fs');
const { test, expect, canvasCard, focus, png } = require('./fixtures');
const { ffmpeg, fit } = require('./hands');

/** A PNG whose header says 400 × 400 and whose picture is noise: it imports as a picture, and ffmpeg can't draw it. */
function brokenPng() {
  const good = png(70, 400, 400);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(4096);
  const iend = Buffer.from('0000000049454e44ae426082', 'hex');
  return Buffer.concat([good.subarray(0, 33), length, Buffer.from('IDAT'), randomBytes(4096), Buffer.alloc(4), iend]);
}

test('picture-pending', async ({ page, project, guards }) => {
  // The broken picture's thumbnail, refused (`thumb_failed`) as the board asks for it: this cell's own doing, for that file alone.
  const broken = brokenPng();
  const refused = new RegExp(`status of 4\\d\\d\\b.*/thumbs/${createHash('sha256').update(broken).digest('hex')}\\b`);
  guards.allow(refused);
  const asks = [];
  const started = Date.now();
  page.on('response', (response) => {
    const url = new URL(response.url());
    if (/\/thumbs\//.test(url.pathname) && !url.searchParams.has('preview')) asks.push({ s: (Date.now() - started) / 1000, status: response.status(), url: url.pathname + url.search });
  });
  const menu = page.locator('details.main-menu');
  const drawer = page.locator('aside.drawer');

  await test.step('a 144 MP picture imported with the Assets drawer open: asked for before its thumbnail is made, drawn once it is', async () => {
    if ((await menu.getAttribute('open')) === null) await menu.locator('summary').click();
    await menu.locator('nav').getByRole('link', { name: 'Assets', exact: true }).click();
    const path = test.info().outputPath('big.jpg');
    ffmpeg('ffmpeg')('-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=12000x12000:rate=1', '-frames:v', '1', '-q:v', '3', path);
    await project.import(readFileSync(path), { name: 'big', at: { x: 0, y: 0 } });
    const row = drawer.locator('.assetrow[data-asset="big"] img');
    await expect.poll(() => row.evaluate((img) => img.complete && img.naturalWidth > 0).catch(() => false), { message: "its row in the Assets drawer draws its thumbnail", timeout: 120_000 }).toBe(true);
    await expect.poll(async () => (await canvasCard(page, 'big'))?.rendered?.media?.ready ?? false, { message: 'its card on the board draws its thumbnail', timeout: 120_000 }).toBe(true);
    test.info().annotations.push({ type: 'asks', description: JSON.stringify(asks) });
    // The case: Calcifer still drawing it when it was first asked for. A runner that drew it sooner proves nothing either way.
    expect.soft(asks.some((ask) => ask.status === 202), `the case: its thumbnail asked for before it was made (asks ${JSON.stringify(asks)})`).toBe(true);
    expect.soft(asks.filter((ask) => /[?&]try=/.test(ask.url)), 'no ask guessed by a timer (`&try=`)').toEqual([]);
  });

  await test.step("a picture whose thumbnail can't be made: once the board has given up on it, its take in Focus says so at once", async () => {
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await project.import(broken, { name: 'broken', at: { x: 0, y: 900 } });
    await fit(page);
    // The board's six tries, each after its wait (2, 4, 8, 16, 32 s), each answered `thumb_failed`.
    await expect.poll(async () => (await canvasCard(page, 'broken'))?.rendered?.media?.failed ?? false, { message: 'the board gives up on its thumbnail', timeout: 150_000 }).toBe(true);
    const failedAsks = asks.filter((ask) => ask.status >= 400).length;
    const region = await focus(page, 'broken');
    await expect(region.locator('.take-thumb .media-note'), 'its take, a second viewer, says it has no preview at once').toHaveText('No preview', { timeout: 10_000 });
    expect.soft(asks.filter((ask) => ask.status >= 400).length, 'and asks nothing more of it').toBe(failedAsks);
  });
});
