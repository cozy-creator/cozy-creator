// svg (Paul, 11-14: "we can use SVGs, but also block scripts"; cl-985): a tool's picture that is an SVG holding a <script> and an onload
// (project/svg.py, as a package may make one) is drawn in its run's drawer as its picture; opened by itself it's sandboxed, with
// `script-src 'none'` and nosniff; and neither its script nor its onload asks for anything. (The canvas takes no SVG: an import of one is
// refused, and a tool whose output is a file is no canvas tool.) web/access.js keeps the same by hand; this is the rule's cell on the RC.
const { test, expect, fixtureTool, screenshotPixels } = require('./fixtures');

const SVG = fixtureTool('svg.py/svg');
const RED = [204, 51, 51];

test('svg', async ({ page, context, project, cli, guards }) => {
  // The browser saying it blocked the SVG's script, opened by itself in its sandbox, is the rule at work.
  guards.allow(/Blocked script execution in '[^']*\/outputs\/picture' because the document's frame is sandboxed/);
  const asked = [];
  context.on('request', (request) => { if (request.url().includes('svg=')) asked.push(request.url()); });
  let made;
  await test.step('made by `cozy run`', async () => {
    const ran = await cli('--json', 'run', SVG, '--on', 'local');
    expect(ran.code, `cozy run: ${ran.stderr.slice(-400)}`).toBe(0);
    made = (await project.api.get(project.at('/runs?limit=10'))).runs.find((run) => run.target?.endsWith('/svg') && run.status === 'completed');
    expect(made, 'its run, completed').toBeTruthy();
  });

  await test.step("drawn in its run's drawer", async () => {
    await page.goto(project.url(`runs/${made.number}`));
    const picture = page.locator('img[alt="picture"]').first();
    await expect.poll(() => picture.evaluate((img) => img.complete && img.naturalWidth > 0).catch(() => false), { message: 'its picture drawn' }).toBe(true);
    const r = await picture.boundingBox();
    const read = screenshotPixels(await page.screenshot());
    const middle = read(r.x + r.width / 2, r.y + r.height / 2);
    expect.soft(middle.every((value, k) => Math.abs(value - RED[k]) <= 40), `its red, not ${middle}`).toBe(true);
  });

  await test.step('opened by itself: sandboxed, no script', async () => {
    const answered = page.waitForResponse((response) => response.url().includes('/outputs/picture') && response.request().resourceType() === 'document');
    // Followed from the page, as a person opens it (same origin), not typed in.
    await page.evaluate((src) => { location.href = src; }, `/v1/projects/${project.id}/runs/${encodeURIComponent(made.id)}/outputs/picture`);
    const answer = await answered;
    const headers = answer.headers(), policy = headers['content-security-policy'] ?? '';
    expect.soft(answer.status(), 'served').toBe(200);
    expect.soft(headers['content-type'] ?? '', 'as an SVG').toMatch(/^image\/svg/);
    expect.soft(policy, 'sandboxed').toMatch(/(^|;)\s*sandbox(\s|;|$)/);
    expect.soft(policy, 'its script blocked').toContain("script-src 'none'");
    expect.soft(headers['x-content-type-options'], 'not sniffed').toBe('nosniff');
  });

  await test.step('its script and onload asked for nothing', async () => {
    // Its onload and script would ask as it loads; a beat for a late one.
    await page.waitForTimeout(1000);
    expect(asked).toEqual([]);
  });
});
