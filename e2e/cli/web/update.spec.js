// update (Paul, 11-17: "remove this notification ... the webpage doesn't need to be updated to use it, does it? ... I guess we could
// auto-refresh the webui to fetch the new version from calcifer"): a tab whose Calcifer comes back as another build of Cozy reloads itself
// with no notice as soon as nothing would be lost, where it was: the canvas where it stood. Typing in a box, it waits until the field is
// left and the words are saved; a passphrase typed in a form holds it until the form is put away. Calcifer's other build is its stream's
// hello saying another version, as a tab open across an update hears it once Calcifer is back.
const { test, expect, canvasIdle, canvasPoint, canvasVisible, focus, png } = require('./fixtures');
const { camera, sameCamera } = require('./geometry');

const NEWER = '9.9.9-newer';
const BOXES = ['first', 'second', 'words'];
/** How long an idle tab may take to reload itself. */
const RELOADS_MS = 10_000;
/** How long a tab that mustn't reload is watched not doing it. */
const WATCHED_MS = 3_000;

/** In each page: its stream's hello says `window.__calciferVersion` for the version once that's set, and `window.__dropStream()` ends
 *  the stream, as Calcifer stopping for an update does, so the page connects again and hears it. */
function newBuildOnCue() {
  const real = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const response = await real(input, init);
    const url = new URL(typeof input === 'string' ? input : input.url, location.href);
    if (url.pathname !== '/v1/events' || !response.body) return response;
    const reader = response.body.getReader();
    const [decoder, encoder] = [new TextDecoder(), new TextEncoder()];
    const body = new ReadableStream({
      start(stream) { window.__dropStream = () => { try { stream.error(new Error('Calcifer stopped')); } catch { /* ended */ } reader.cancel().catch(() => {}); }; },
      async pull(stream) {
        const { done, value } = await reader.read();
        if (done) return stream.close();
        const text = decoder.decode(value, { stream: true });
        stream.enqueue(encoder.encode(window.__calciferVersion ? text.replace(/("type":"hello"[^\n]*?"version":")[^"]*"/, `$1${window.__calciferVersion}"`) : text));
      },
      cancel(reason) { return reader.cancel(reason); },
    });
    return new Response(body, { status: response.status, statusText: response.statusText, headers: response.headers });
  };
}

/** Calcifer back as another build: the page hears it on its stream. */
const updated = (page) => page.evaluate((version) => { window.__calciferVersion = version; window.__dropStream?.(); }, NEWER);

/** Marked now: gone once the tab has loaded again. */
const mark = (page) => page.evaluate(() => { window.__notReloaded = true; });
const reloaded = (page) => page.evaluate(() => !window.__notReloaded).catch(() => false);
const boxesUp = async (page) => {
  for (const name of BOXES) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await canvasIdle(page);
};

test('update', async ({ context, project, api, signIn }) => {
  await context.addInitScript(newBuildOnCue);
  for (const [i, name] of ['first', 'second'].entries()) await project.import(png(50 + i * 70), { name, at: { x: i * 400, y: 0 } });
  await project.add({ name: 'words', kind: 'text', fields: { text: 'Before the update.' }, at: { x: 0, y: 420 } });

  await test.step(`idle: it reloads itself within ${RELOADS_MS / 1000} s, says nothing, and the canvas stands where it was`, async () => {
    const page = await context.newPage();
    await signIn(page, project.url());
    await boxesUp(page);
    // A view of its own, saved: moved over `second`.
    const at = await canvasPoint(page, 'second');
    await page.mouse.move(at.x, at.y);
    await page.mouse.wheel(0, 300);
    await canvasIdle(page);
    await expect.poll(async () => {
      const [{ viewport }, now] = await Promise.all([api.get(project.at('/view')), camera(page)]);
      return Boolean(viewport) && sameCamera({ x: viewport.x, y: viewport.y, zoom: viewport.zoom }, now);
    }, { message: "the page's view saved" }).toBe(true);
    const before = await camera(page);
    await page.mouse.move(1, 1);
    await mark(page);
    await updated(page);
    await expect.poll(() => reloaded(page), { message: 'it reloaded itself', timeout: RELOADS_MS }).toBe(true);
    await boxesUp(page);
    expect.soft(sameCamera(await camera(page), before), 'the canvas where it stood').toBe(true);
    await expect.soft(page.getByText(/Cozy updated|Reload the page/), 'and no notice').toHaveCount(0);
    await page.close();
  });

  await test.step('typing in a box: it waits until the field is left and the words are saved, then reloads itself', async () => {
    const page = await context.newPage();
    let release;
    const writes = new Promise((done) => { release = done; });
    await page.route('**/v1/projects/*/assets/words', async (route) => {
      if (route.request().method() === 'PATCH') await writes;
      await route.continue();
    });
    await signIn(page, project.url());
    await boxesUp(page);
    const region = await focus(page, 'words');
    await region.getByRole('textbox', { name: 'Text' }).click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('After the update.');
    await mark(page);
    await updated(page);
    await page.waitForTimeout(WATCHED_MS);
    expect.soft(await reloaded(page), 'not while the field has focus').toBe(false);
    await region.getByRole('button', { name: 'Back to canvas', exact: true }).click();
    await page.waitForTimeout(WATCHED_MS);
    expect.soft(await reloaded(page), 'nor while its words are on their way').toBe(false);
    release();
    await expect.poll(() => reloaded(page), { message: 'it reloaded itself once they were saved', timeout: RELOADS_MS }).toBe(true);
    expect((await project.asset('words'))?.fields?.text, 'with the words saved').toBe('After the update.');
    await boxesUp(page);
    await expect.soft(page.getByText(/Cozy updated|Reload the page/), 'and no notice').toHaveCount(0);
    await page.close();
  });

  await test.step('a passphrase typed for a new project: it waits until the form is put away, then reloads itself', async () => {
    const page = await context.newPage();
    await signIn(page, project.url());
    await boxesUp(page);
    await page.getByRole('button', { name: 'New project', exact: true }).click();
    const panel = page.getByRole('region', { name: 'New project', exact: true });
    await panel.getByLabel('Passphrase', { exact: true }).fill('a passphrase not used yet');
    // Typed and left, as a person looks away: no field has focus, the form still holds it.
    await page.evaluate(() => (document.activeElement instanceof HTMLElement ? document.activeElement.blur() : undefined));
    await mark(page);
    await updated(page);
    await page.waitForTimeout(WATCHED_MS);
    expect.soft(await reloaded(page), 'not while the form holds a passphrase').toBe(false);
    await panel.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect.poll(() => reloaded(page), { message: 'it reloaded itself once the form was put away', timeout: RELOADS_MS }).toBe(true);
  });
});
