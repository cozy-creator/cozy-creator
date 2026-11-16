// machine-tabs (cl-1099): Paul, 11-16: "we should have a + button next to the right of machines in the header. That'll open the new-machine
// panel, allowing you to open up more machines", and "when there are too many machines in that panel (it runs out of space) it should also
// have an expandable section like ... so you can see a bigger list of all the machines you have running (there could be quite a few)".
// With more machines than fit, the tabs stay one row in the bar, the rest behind "N more", which opens Machines listing every one, those
// with no tab first. The + sits right after the tabs, as tall as they are, and opens New machine on the rent form; Esc closes it, and so
// does a press outside, never one that began inside. Nothing is rented. The machines are one stand-in, another user's cozy on this runner
// (machineset.sh's stand_in), added under twelve names. Each view at 1440×900 and 1920×1080, paper and dark, kept for a person to look at.
const { spawn } = require('child_process');
const { closeSync, mkdirSync, openSync, readFileSync } = require('fs');
const { join } = require('path');
const { test, expect } = require('./fixtures');

const LOOKS = [
  { width: 1440, height: 900, theme: 'paper' },
  { width: 1440, height: 900, theme: 'dark' },
  { width: 1920, height: 1080, theme: 'paper' },
  { width: 1920, height: 1080, theme: 'dark' },
];
const STAND = 'cozy-mtabs';
const NAMES = ['studio-4090', 'garage-3090', 'attic-a6000', 'office-4080', 'lab-l40s', 'spare-3060', 'den-4070', 'loft-a5000', 'basement-3080',
  'kitchen-2080', 'porch-4060', 'shed-a4000'];

/** `command args…` from /, its output in files under `dir` (a Calcifer it starts keeps no pipe of ours open): {code, stdout, stderr}. */
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

/** Where the bar, the strip, its tabs and the + are. */
const geometry = (page) => page.evaluate(() => {
  const box = (el) => el && (({ top, bottom, left, right, width, height }) => ({ top, bottom, left, right, width, height }))(el.getBoundingClientRect());
  const nav = document.querySelector('nav.mtabs');
  return {
    bar: box(document.querySelector('header.top')),
    nav: box(nav),
    kids: [...(nav?.children ?? [])].map(box),
    plus: box(document.querySelector('a[aria-label="New machine"]')),
    ptab: box(document.querySelector('nav.ptabs .ptab')),
    projectPlus: box(document.querySelector('button[aria-label="New project"]')),
  };
});

/** Every machine the tabs show, as Calcifer lists them. */
const machinesNow = async (api) => (await api.get('/v1/machines')).machines
  .filter((row) => row.state !== 'ended' && (row.kind !== 'rental' || row.ordered !== false)).map((row) => row.name);

test('machine-tabs', { tag: '@solo' }, async ({ page, project, api, cli }) => {
  test.skip(!(process.platform === 'linux' && process.env.GITHUB_ACTIONS === 'true'), 'its stand-in machine is another user of a fresh GitHub runner');
  test.setTimeout(900_000);
  const shots = join(process.env.OUT, 'web', 'machine-tabs');
  mkdirSync(shots, { recursive: true });
  const drawer = page.locator('aside.drawer');
  const title = drawer.locator('.drawer-head b');
  const nav = page.getByRole('navigation', { name: 'Machines', exact: true });
  const plus = page.getByRole('link', { name: 'New machine', exact: true });
  const more = nav.getByRole('link', { name: /^\d+ more$/ });
  const look = async ({ width, height, theme }) => {
    await page.setViewportSize({ width, height });
    await page.evaluate((one) => localStorage.setItem('cozy.theme', one), theme);
    await page.goto(project.url());
    await expect(page.locator('.stage')).toBeVisible();
    await expect(nav.locator('.mtab[data-machine="local"]')).toBeVisible();
    return `${width}x${height}-${theme}`;
  };
  // The + where it belongs: right after the tabs, as tall as they are and centred on them, a control at least 28 px tall.
  const plusPlaced = async (tag) => {
    const at = await geometry(page);
    expect.soft(at.plus, `${tag}: a + beside the machine tabs`).toBeTruthy();
    if (!at.plus || !at.nav) return;
    expect.soft(at.plus.left - at.nav.right, `${tag}: the + just right of the tabs`).toBeGreaterThanOrEqual(0);
    expect.soft(at.plus.left - at.nav.right, `${tag}: the + just right of the tabs`).toBeLessThanOrEqual(20);
    expect.soft(Math.abs((at.plus.top + at.plus.bottom) / 2 - (at.nav.top + at.nav.bottom) / 2), `${tag}: centred on the tabs`).toBeLessThanOrEqual(1.5);
    expect.soft(at.plus.height, `${tag}: the + at least 28 px tall`).toBeGreaterThanOrEqual(28);
    if (at.projectPlus) expect.soft(Math.round(at.plus.width), `${tag}: as wide as the project tabs' +`).toBe(Math.round(at.projectPlus.width));
  };

  await test.step('the + right of the machine tabs, at each size', async () => {
    for (const one of LOOKS) {
      const tag = await look(one);
      await page.screenshot({ path: join(shots, `plus-${tag}.png`) });
      await plusPlaced(tag);
    }
  });

  const added = [];
  try {
    await test.step('a stand-in machine, added under twelve names', async () => {
      const dir = join(shots, 'stand-in');
      mkdirSync(dir, { recursive: true });
      const user = await run(dir, 'user', 'sh', ['-c', `id ${STAND} > /dev/null 2>&1 || sudo useradd -m ${STAND}`]);
      expect(user.code, `useradd: ${user.stderr}`).toBe(0);
      const installed = await run(dir, 'install', 'sudo', ['install', '-D', '-m', '755', process.env.COZY, `/opt/${STAND}/cozy`]);
      expect(installed.code, `its cozy: ${installed.stderr}`).toBe(0);
      const its = await standAs(dir, 'local', '--json', 'machine', 'show', 'local');
      expect(its.code, `its machine show local: ${its.stdout.slice(-300)} ${its.stderr.slice(-300)}`).toBe(0);
      const fingerprint = JSON.parse(its.stdout).fingerprint;
      const key = (await cli.json('machine', 'show', 'local')).controller_key;
      expect(fingerprint && key, 'its fingerprint and this computer\'s key').toBeTruthy();
      const allowed = await standAs(dir, 'authorize', '--json', 'machine', 'authorize', key);
      expect(allowed.code, `authorize: ${allowed.stdout.slice(-300)} ${allowed.stderr.slice(-300)}`).toBe(0);
      const reach = JSON.parse(allowed.stdout).machine?.reach;
      expect(reach, 'where it answers').toBeTruthy();
      for (const name of NAMES) {
        await cli.json('machine', 'add', name, reach, '--fingerprint', fingerprint);
        added.push(name);
      }
      await expect.poll(async () => {
        const listed = (await api.get('/v1/machines')).machines.filter((one) => NAMES.includes(one.name) && one.state === 'ready');
        return listed.length;
      }, { message: 'each name ready', timeout: 120_000 }).toBe(NAMES.length);
    });

    const tabbed = () => nav.locator('.mtab[data-machine]').evaluateAll((tabs) => tabs.map((tab) => tab.dataset.machine));
    // What the page knows of: its tabs, and how many "N more" says.
    const known = async () => (await tabbed()).length + ((await more.count()) ? Number.parseInt(await more.innerText(), 10) : 0);
    await test.step('more machines than fit: one row, the rest behind "N more"', async () => {
      for (const one of LOOKS) {
        const tag = await look(one);
        const all = await machinesNow(api);
        await expect.poll(known, { message: `${tag}: the page has all ${all.length}` }).toBe(all.length);
        await page.waitForTimeout(500);
        await page.screenshot({ path: join(shots, `overflow-${tag}.png`) });
        await page.screenshot({ path: join(shots, `overflow-bar-${tag}.png`), clip: { x: one.width / 2, y: 0, width: one.width / 2, height: 64 } });
        await expect(more, `${tag}: "N more" past what fits`).toBeVisible();
        const shown = await tabbed();
        const hidden = all.filter((name) => !shown.includes(name));
        expect(shown[0], `${tag}: this computer first`).toBe('local');
        expect(hidden.length, `${tag}: some behind "N more"`).toBeGreaterThan(0);
        await expect(more).toHaveText(`${hidden.length} more`);
        const at = await geometry(page);
        expect(Math.max(...at.kids.map((kid) => kid.top)) - Math.min(...at.kids.map((kid) => kid.top)), `${tag}: the tabs one row`).toBeLessThanOrEqual(1);
        expect(at.nav.top >= at.bar.top && at.nav.bottom <= at.bar.bottom, `${tag}: inside the bar ${JSON.stringify(at.nav)} ${JSON.stringify(at.bar)}`).toBe(true);
        expect(Math.max(...at.kids.map((kid) => kid.right)), `${tag}: no tab cut off`).toBeLessThanOrEqual(at.nav.right + 0.5);
        expect(at.nav.width, `${tag}: no wider than its room`).toBeLessThanOrEqual(one.width * 0.4 + 1);
        await plusPlaced(tag);
      }
    });

    await test.step('"N more" opens Machines: every machine, those with no tab first', async () => {
      for (const one of LOOKS) {
        const tag = await look(one);
        await expect(more).toBeVisible();
        const all = await machinesNow(api);
        const shown = await tabbed();
        const hidden = all.filter((name) => !shown.includes(name));
        await more.click();
        await expect(title).toHaveText('Machines');
        const rows = drawer.locator('li.runrow.machine[data-machine]');
        await expect(rows).toHaveCount(all.length);
        const listed = await rows.evaluateAll((items) => items.map((item) => item.dataset.machine));
        expect(new Set(listed.slice(0, hidden.length)), `${tag}: those with no tab first`).toEqual(new Set(hidden));
        expect(new Set(listed.slice(hidden.length)), `${tag}: then the tabbed`).toEqual(new Set(shown));
        await page.screenshot({ path: join(shots, `all-machines-${tag}.png`) });
        await page.keyboard.press('Escape');
        await expect(drawer).toHaveCount(0);
      }
    });

    await test.step('the + opens New machine on the rent form; Esc and a press outside close it, a press from inside never', async () => {
      for (const one of LOOKS) {
        const tag = await look(one);
        await plus.click();
        await expect(page).toHaveURL(project.url('machines/new'));
        await expect(title).toHaveText('New machine');
        const rent = drawer.locator('section').filter({ has: page.getByRole('heading', { name: 'Rent a machine', exact: true }) });
        await expect(rent).toBeVisible();
        await expect(rent.locator('li.sku').or(rent.getByText('to rent.')).first(), 'machines to choose, or signing in first').toBeVisible();
        await page.screenshot({ path: join(shots, `new-machine-${tag}.png`) });
        await page.keyboard.press('Escape');
        await expect(drawer, `${tag}: Esc closes it`).toHaveCount(0);
        await expect(page).toHaveURL(project.url());
      }
      await plus.click();
      await expect(title).toHaveText('New machine');
      const head = await title.boundingBox();
      const outside = { x: 520, y: 640 };
      await page.mouse.move(head.x + 4, head.y + head.height / 2);
      await page.mouse.down();
      await page.mouse.move(outside.x, outside.y, { steps: 6 });
      await page.mouse.up();
      await page.waitForTimeout(300);
      await expect(title, 'a press that began inside stays open').toHaveText('New machine');
      await page.mouse.click(outside.x, outside.y);
      await expect(drawer, 'a press outside closes it').toHaveCount(0);
    });
  } finally {
    await page.goto('about:blank');
    for (const name of added) await cli('--json', 'machine', 'end', name, '--force', '-y');
    await run(shots, 'stand-in-killed', 'sudo', ['pkill', '-KILL', '-u', STAND]);
  }
});
