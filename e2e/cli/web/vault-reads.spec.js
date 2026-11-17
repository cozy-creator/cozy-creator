// vault-reads (cl-1111; coordinator, 11-17: "new projects are encrypted by default in 0.26.0, so every Mac user would get 1 s thumbnails and
// 9–14 s previews"): an unlocked encrypted project answers its thumbnails as fast as a plain one would, and reading them starts no process.
// On a Mac, each request into it ran `/sbin/mount` and `hdiutil info` to check its volume was still mounted, under its session's lock: ~1 s a
// thumbnail with six in flight (e2e 43964060616); here 1.8 s in the middle, and 1,335 processes for 48 reads (43998175275). Its mount is
// now read once, and checked each request with a `statfs`.
const { createHash } = require('crypto');
const { execFileSync } = require('child_process');
const { test, expect, png } = require('./fixtures');
const { runnerSpeed } = require('./speed');

const PICTURES = 24;
/** As a browser asks: six at a time, its connections to one host. */
const AT_ONCE = 6;
// On the hosted runners (speed.js scales them): a read is a file sent from an open volume, six in flight on one project's lock: Linux's
// encrypted reads take ~50 ms in the middle so (e2e 43998652989).
const BARS = { median_ms: 100, most_ms: 400 };

test.use({ projectOptions: { encrypted: true } });

test('vault-reads', async ({ project, api, cli }, testInfo) => {
  const speed = runnerSpeed();
  const bar = { median_ms: Math.round(BARS.median_ms * speed.factor), most_ms: Math.round(BARS.most_ms * speed.factor) };
  testInfo.annotations.push({ type: 'runner', description: JSON.stringify({ ...speed, bar }) });
  const at = (rest = '') => `/v1/projects/${encodeURIComponent(project.id)}${rest}`;
  expect(await api.get(at('/vault')), 'the project is encrypted and unlocked').toMatchObject({ encrypted: true, state: 'unlocked' });

  const shas = [];
  await test.step(`${PICTURES} pictures imported, each thumbnail made as it lands`, async () => {
    for (let i = 0; i < PICTURES; i++) {
      const bytes = png(i * 10, 640, 400);
      const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
      await api.call('PUT', `/v1/project-files/${digest}?project=${encodeURIComponent(project.id)}`, bytes, { type: 'application/octet-stream' });
      await api.post(at('/imports'), { source: digest, name: `pic_${i}`, exact: true });
      shas.push(digest);
    }
    await expect.poll(async () => {
      const statuses = await Promise.all(shas.map(async (sha) => { const answer = await api.raw('GET', at(`/thumbs/${encodeURIComponent(sha)}`)); await answer.arrayBuffer(); return answer.status; }));
      return statuses.filter((status) => status === 200).length;
    }, { message: 'every thumbnail made', timeout: 60_000, intervals: [500] }).toBe(PICTURES);
  });

  await test.step(`its ${PICTURES} thumbnails read ${AT_ONCE} at a time, each within ${bar.median_ms} ms in the middle and ${bar.most_ms} ms at most, and on a Mac no process started`, async () => {
    // A Mac's process ids count up: the shell's own before and after, the processes this computer started meanwhile (its daemons too: 1-2
    // here). A process each read is 48 at least; its check was ~28 a read. Linux's count its threads as well (16-50), and its check reads
    // /proc, so there it's said, not held.
    const pid = () => Number(execFileSync('/bin/sh', ['-c', 'echo $$']).toString().trim());
    const took = [];
    const before = pid();
    const began = Date.now();
    for (let round = 0; round < 2; round++) {
      const left = [...shas];
      await Promise.all(Array.from({ length: AT_ONCE }, async () => {
        for (let sha; (sha = left.shift()); ) {
          const start = performance.now();
          const answer = await api.raw('GET', at(`/thumbs/${encodeURIComponent(sha)}`));
          await answer.arrayBuffer();
          took.push(performance.now() - start);
          expect.soft(answer.status, `${sha} answered`).toBe(200);
        }
      }));
    }
    const all_ms = Date.now() - began;
    const started = pid() - before - 1;
    took.sort((a, b) => a - b);
    const seen = { reads: took.length, median_ms: Math.round(took[took.length >> 1]), most_ms: Math.round(took.at(-1)), all_ms, processes_started: started };
    await testInfo.attach('vault-reads', { body: JSON.stringify(seen, null, 2), contentType: 'application/json' });
    expect.soft(seen.median_ms, `the middle read: ${JSON.stringify(seen)}`).toBeLessThanOrEqual(bar.median_ms);
    expect.soft(seen.most_ms, `the slowest read: ${JSON.stringify(seen)}`).toBeLessThanOrEqual(bar.most_ms);
    if (process.platform === 'darwin') expect.soft(started, `processes started while ${took.length} reads were answered: ${JSON.stringify(seen)}`).toBeLessThanOrEqual(took.length / 2);
  });

  // Locked before its folder goes, as a person's would be.
  await cli('--json', 'vault', '--directory', project.path, 'lock');
  await expect.poll(async () => (await api.get(at('/vault'))).state, { message: 'locked before it is removed', timeout: 60_000 }).toBe('locked');
});
