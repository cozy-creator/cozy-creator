// play-link (cl-1066): a plaintext project's run output plays by its link, as any did before every run became a project's (cl-1028):
// the link Calcifer mints (`GET …/runs/{id}/play`) opens a WebRTC session to the machine (web/player/cozy-webrtc.js, the hosted player's
// own client, loaded as it is), and the machine streams the output's bytes, byte for byte what landed in the project. Before, every
// project run's link was refused `project_play_unavailable`; allowed alone, the machine answered `not_found`: it looked for the run
// outside the project's workspace, where a project's run on this computer is kept.
const { readFileSync } = require('fs');
const { join } = require('path');
const { test, expect, fixtureTool } = require('./fixtures');

const DRAW = fixtureTool('draw.py/draw');
const PLAYER = join(__dirname, '..', '..', '..', 'web', 'player', 'cozy-webrtc.js');

test('play-link', async ({ page, project, api, cli }) => {
  const drawn = async () => ((await api.get(project.at('/runs?limit=100'))).runs ?? []).filter((one) => one.target?.endsWith('/draw') && one.status === 'completed');
  const ran = await cli('--json', 'run', DRAW, `instructions=play ${Date.now()}`, '--on', 'local');
  expect(ran.code, `cozy run: ${ran.stderr.slice(-400)}`).toBe(0);
  let made;
  await expect.poll(async () => (made = (await drawn())[0]), { message: 'its run, completed', timeout: 120_000 }).toBeTruthy();
  const run = await api.get(project.at(`/runs/${made.id}`));
  const item = run.output?.[0];
  expect(item, 'its output').toBeTruthy();
  const landed = Buffer.from(await (await api.raw('GET', project.at(`/runs/${run.id}/outputs/${encodeURIComponent(item.name)}`))).arrayBuffer());
  expect(landed.length, 'its landed bytes').toBeGreaterThan(0);

  const asked = await api.raw('GET', project.at(`/runs/${run.id}/play?output=${encodeURIComponent(item.name)}&expires=5m`));
  const play = await asked.json();
  expect(asked.status, `the link: ${JSON.stringify(play)}`).toBe(200);
  expect(play.url, 'the hosted player, the link in its fragment').toMatch(/^https:\/\/cozy-creator\.github\.io\/cozy-creator\/play\/#/);

  // The player's client in a page of no origin's (nothing of Calcifer's page or its policy), as the hosted player loads it.
  await page.goto('about:blank');
  await page.addScriptTag({ type: 'module', content: `${readFileSync(PLAYER, 'utf8')}\nwindow.cozyWebrtc = { parseLink, connect };` });
  await page.waitForFunction(() => Boolean(window.cozyWebrtc), null, { timeout: 10_000 });
  const streamed = await page.evaluate(async ({ hash, length }) => {
    const { parseLink, connect } = window.cozyWebrtc;
    const link = parseLink(hash);
    const session = await connect(link, { cap: link.cap });
    const bytes = new Uint8Array(length);
    let got = 0;
    await new Promise((done, failed) => {
      session.get({ run: link.run, output: link.output, index: link.index, offset: 0, length }, {
        data: (offset, payload) => { bytes.set(payload.subarray(0, Math.max(0, length - offset)), offset); got += payload.length; session.release(payload.length); },
        end: () => done(),
        error: (error) => failed(new Error(`${error.code}: ${error.message}`)),
        close: (error) => failed(new Error(`closed: ${error?.message}`)),
      });
      setTimeout(() => failed(new Error(`timed out with ${got} of ${length} bytes`)), 60_000);
    });
    session.close();
    return { got, bytes: Array.from(bytes) };
  }, { hash: new URL(play.url).hash, length: landed.length });
  expect(streamed.got, 'the machine streamed it whole').toBe(landed.length);
  expect(Buffer.from(streamed.bytes).equals(landed), 'byte for byte what landed').toBe(true);
});
