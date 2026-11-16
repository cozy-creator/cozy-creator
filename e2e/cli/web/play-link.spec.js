// play-link (cl-1066): a plaintext project's run output plays by its link, as any did before every run became a project's (cl-1028):
// the link Calcifer mints (`GET …/runs/{id}/play`) opens a WebRTC session to the machine (web/player/cozy-webrtc.js, the hosted player's
// own client, loaded as it is), and the machine streams the output's bytes, byte for byte what landed in the project. Before, every
// project run's link was refused `project_play_unavailable`; allowed alone, the machine answered `not_found`: it looked for the run
// outside the project's workspace, where a project's run on this computer is kept.
// `@vault` (web-engines, with the filesystem driver): a link made while the project is plaintext plays no more once it is encrypted,
// though it is unlocked (#3569 review): the machine reads only an unencrypted workspace at the generation the link names. A new link
// is refused `project_play_encrypted`.
const { mkdtempSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, fixtureTool } = require('./fixtures');

const DRAW = fixtureTool('draw.py/draw');
const PLAYER = join(__dirname, '..', '..', '..', 'web', 'player', 'cozy-webrtc.js');

/** A drawn picture in `project`, as it landed there: its run and output. */
async function drawn(project, api, cli) {
  const completed = async () => ((await api.get(project.at('/runs?limit=100'))).runs ?? []).filter((one) => one.target?.endsWith('/draw') && one.status === 'completed');
  const ran = await cli('--json', 'run', DRAW, `instructions=play ${Date.now()}`, '--on', 'local');
  expect(ran.code, `cozy run: ${ran.stderr.slice(-400)}`).toBe(0);
  let made;
  await expect.poll(async () => (made = (await completed())[0]), { message: 'its run, completed', timeout: 120_000 }).toBeTruthy();
  const run = await api.get(project.at(`/runs/${made.id}`));
  const item = run.output?.[0];
  expect(item, 'its output').toBeTruthy();
  const landed = Buffer.from(await (await api.raw('GET', project.at(`/runs/${run.id}/outputs/${encodeURIComponent(item.name)}`))).arrayBuffer());
  expect(landed.length, 'its landed bytes').toBeGreaterThan(0);
  return { run, item, landed };
}

/** Its link, as Calcifer answers for it. */
const mint = async (project, api, run, item) => {
  const asked = await api.raw('GET', project.at(`/runs/${run.id}/play?output=${encodeURIComponent(item.name)}&expires=5m`));
  return { status: asked.status, play: await asked.json() };
};

/** The link played by the hosted player's client in a page of no origin's (nothing of Calcifer's page or its policy): its bytes, or why not. */
async function stream(page, url, length) {
  await page.goto('about:blank');
  await page.addScriptTag({ type: 'module', content: `${readFileSync(PLAYER, 'utf8')}\nwindow.cozyWebrtc = { parseLink, connect };` });
  await page.waitForFunction(() => Boolean(window.cozyWebrtc), null, { timeout: 10_000 });
  return page.evaluate(async ({ hash, length }) => {
    const { parseLink, connect } = window.cozyWebrtc;
    const link = parseLink(hash);
    const session = await connect(link, { cap: link.cap });
    const bytes = new Uint8Array(length);
    let got = 0;
    const failed = await new Promise((done) => {
      session.get({ run: link.run, output: link.output, index: link.index, offset: 0, length }, {
        data: (offset, payload) => { bytes.set(payload.subarray(0, Math.max(0, length - offset)), offset); got += payload.length; session.release(payload.length); },
        end: () => done(null),
        error: (error) => done(`${error.code}: ${error.message}`),
        close: (error) => done(`closed: ${error?.message}`),
      });
      setTimeout(() => done(`timed out with ${got} of ${length} bytes`), 60_000);
    });
    session.close();
    return { got, bytes: Array.from(bytes), failed };
  }, { hash: new URL(url).hash, length });
}

test('play-link', async ({ page, project, api, cli }) => {
  const { run, item, landed } = await drawn(project, api, cli);
  const { status, play } = await mint(project, api, run, item);
  expect(status, `the link: ${JSON.stringify(play)}`).toBe(200);
  expect(play.url, 'the hosted player, the link in its fragment').toMatch(/^https:\/\/cozy-creator\.github\.io\/cozy-creator\/play\/#/);
  const streamed = await stream(page, play.url, landed.length);
  expect(streamed.failed, 'the machine streamed it').toBeNull();
  expect(streamed.got, 'whole').toBe(landed.length);
  expect(Buffer.from(streamed.bytes).equals(landed), 'byte for byte what landed').toBe(true);
});

test('play-link, encrypted since', { tag: '@vault' }, async ({ page, project, api, cli }) => {
  const { run, item, landed } = await drawn(project, api, cli);
  const { status, play } = await mint(project, api, run, item);
  expect(status, `the link while plaintext: ${JSON.stringify(play)}`).toBe(200);
  const before = await stream(page, play.url, landed.length);
  expect(before.failed, 'it plays while the project is plaintext').toBeNull();

  const secrets = mkdtempSync(join(process.env.OUT, 'web-play-secret-'));
  writeFileSync(join(secrets, 'passphrase'), 'play-link-encrypted-since\n', { mode: 0o600 });
  const encrypted = await cli('--json', 'vault', 'encrypt', '--passphrase-file', join(secrets, 'passphrase'), '--recovery-out', join(secrets, 'recovery'), '--idle', '0');
  expect(encrypted.code, `cozy vault encrypt: ${encrypted.stderr.slice(-400)}`).toBe(0);
  expect(await api.get(project.at('/vault')), 'encrypted and unlocked').toMatchObject({ encrypted: true, state: 'unlocked' });

  const after = await stream(page, play.url, landed.length);
  expect(after.failed, 'the link made while plaintext is refused by the machine').toMatch(/^unavailable: .*as it was when the link was made/);
  expect(after.got, 'and nothing of the output came').toBe(0);
  const again = await mint(project, api, run, item);
  expect(again.status, `a new link: ${JSON.stringify(again.play)}`).toBe(403);
  expect(again.play.error?.code).toBe('project_play_encrypted');
});
