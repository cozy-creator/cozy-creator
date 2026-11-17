// tools-machines (cl-1174, cl-1126's leftover). Paul, 11-17: "if you need to display a list of packages / tools on the left, we just fetch
// them from tensorhub once ... That list is not a list of what is installed on the machine, but what is available to install". The tool
// list is one for every machine (cl-1126), so the page asks for it with no machine and keys it on none: switching the project's machines
// (`cozy machine use`, as Render here does) asks for no tools. The machines are one stand-in, another user's cozy on this runner
// (machineset.sh's stand_in), added under two names.
const { spawn } = require('child_process');
const { closeSync, mkdirSync, openSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect } = require('./fixtures');

const STAND = 'cozy-tools';
const NAMES = ['tools-a', 'tools-b'];

/** `command args…` from /, its output in files under `dir`: {code, stdout, stderr}. */
function run(dir, tag, command, args) {
  const [out, err] = [join(dir, `${tag}.out`), join(dir, `${tag}.err`)];
  const fds = [openSync(out, 'w'), openSync(err, 'w')];
  return new Promise((done) => {
    const child = spawn(command, args, { cwd: '/', stdio: ['ignore', ...fds] });
    const timer = setTimeout(() => child.kill('SIGKILL'), 240_000);
    let ended = false;
    const end = (code) => {
      if (ended) return;
      ended = true;
      clearTimeout(timer);
      fds.forEach(closeSync);
      done({ code, stdout: readFileSync(out, 'utf8'), stderr: readFileSync(err, 'utf8') });
    };
    child.on('exit', (code) => end(code ?? 1));
    child.on('error', (error) => end(`${error.message}`));
  });
}

/** The stand-in's own cozy, in its user's default home, as machineset.sh's stand_as. */
const standAs = (dir, tag, ...words) => run(dir, tag, 'sudo', ['-u', STAND, '-H', 'env',
  ...['SSL_CERT_FILE', 'COZY_HUB_URL', 'COZY_LOCAL_ORIGINS'].filter((name) => process.env[name]).map((name) => `${name}=${process.env[name]}`),
  `/opt/${STAND}/cozy`, ...words]);

test('tools-machines', { tag: '@solo' }, async ({ page, project, api, cli }) => {
  test.skip(!(process.platform === 'linux' && process.env.GITHUB_ACTIONS === 'true'), 'its stand-in machine is another user of a fresh GitHub runner');
  test.setTimeout(600_000);
  const dir = join(process.env.OUT, 'web', 'tools-machines');
  mkdirSync(dir, { recursive: true });
  const added = [];
  try {
    await test.step('a stand-in machine, added under two names', async () => {
      const user = await run(dir, 'user', 'sh', ['-c', `id ${STAND} > /dev/null 2>&1 || sudo useradd -m ${STAND}`]);
      expect(user.code, `useradd: ${user.stderr}`).toBe(0);
      const installed = await run(dir, 'install', 'sudo', ['install', '-D', '-m', '755', process.env.COZY, `/opt/${STAND}/cozy`]);
      expect(installed.code, `its cozy: ${installed.stderr}`).toBe(0);
      const its = await standAs(dir, 'local', '--json', 'machine', 'show', 'local');
      expect(its.code, `its machine show local: ${its.stdout.slice(-300)} ${its.stderr.slice(-300)}`).toBe(0);
      const fingerprint = JSON.parse(its.stdout).fingerprint;
      const key = (await cli.json('machine', 'show', 'local')).controller_key;
      const allowed = await standAs(dir, 'authorize', '--json', 'machine', 'authorize', key);
      expect(allowed.code, `authorize: ${allowed.stdout.slice(-300)} ${allowed.stderr.slice(-300)}`).toBe(0);
      const reach = JSON.parse(allowed.stdout).machine?.reach;
      expect(reach, 'where it answers').toBeTruthy();
      for (const name of NAMES) {
        await cli.json('machine', 'add', name, reach, '--fingerprint', fingerprint);
        added.push(name);
      }
      await expect.poll(async () => (await api.get('/v1/machines')).machines.filter((one) => NAMES.includes(one.name) && one.state === 'ready').length,
        { message: 'each name ready', timeout: 120_000 }).toBe(NAMES.length);
    });

    // Every read of the tool list the page makes, from its first.
    const asked = [];
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (url.pathname === '/v1/tools') asked.push(url.search);
    });
    await page.goto(project.url());
    await expect(page.locator('.stage')).toBeVisible();
    await expect(page.locator('aside.tools'), 'the tool list').toBeVisible();
    await expect.poll(() => asked.length, { message: 'the page read its tools', timeout: 60_000 }).toBeGreaterThan(0);
    // Quiet first: whatever the page read as it opened has been read.
    await page.waitForTimeout(3_000);
    const before = asked.length;

    for (const name of [...NAMES, 'local']) {
      await test.step(`the project's machines: ${name}`, async () => {
        expect((await cli.json('machine', 'use', name)).machines).toEqual([name]);
        await expect(page.locator(`.mtab.render-here[data-machine="${name}"]`), `the page has ${name} as the project's machine`).toBeVisible({ timeout: 30_000 });
        await page.waitForTimeout(2_000);
      });
    }
    console.log(JSON.stringify({ tools_read: asked, after_switches: asked.slice(before) }));
    expect(asked.slice(before), 'switching the project\'s machines read no tools').toEqual([]);
    expect(asked.filter((search) => new URLSearchParams(search).has('on')), 'no tools read names a machine').toEqual([]);
  } finally {
    await page.goto('about:blank');
    await cli('--json', 'machine', 'use', 'local');
    for (const name of added) await cli('--json', 'machine', 'end', name, '--force', '-y');
    await run(dir, 'stand-in-killed', 'sudo', ['pkill', '-KILL', '-u', STAND]);
  }
});
