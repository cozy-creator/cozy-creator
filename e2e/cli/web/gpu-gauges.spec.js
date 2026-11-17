// gpu-gauges (cl-1191), Paul 11-17: "there was a plan to show details on a machine, like its GPU utilization, like a throttle, from 0 -> 100%,
// like what runpod has. I don't see it yet on the /machines page. Why not? I want to see how much GPUs are being utilized please."
// This computer's machine on recorded GPU readings (`machine.gpu_samples`, gpu-gauges.jsonl: two 24GB GPUs, GPU 0 going 12, 47, 83, 100%
// at its power limit, 64% slowed for heat, 30%, a reading a second by the clock, GPU 1 idle), through the real path: TensorD's GET
// /v1/gpus, then Calcifer's events stream while a page shows machines. `cozy machine show local` prints each GPU's numbers now. Machines
// lists the machine with each GPU's busy share as a bar that moves with the readings, its last minute as a trace, its memory, power of its
// limit and temperature, and "Power-limited" and "Thermal" when the readings say so; its drawer the same, a row each. A stream whose page
// doesn't show GPU use hears none, and the page says it shows them only while it does: closed, it takes it back. Screenshots of both at 1440×900 and 1920×1080,
// paper and dark.
const { readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, themed } = require('./fixtures');

const SAMPLES = join(__dirname, 'gpu-gauges.jsonl');
const RECORDED = readFileSync(SAMPLES, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
const GPU0 = RECORDED.filter((sample) => sample.gpu === 0);
const BUSY = new Set(GPU0.map((sample) => sample.busy_percent));
const LOOKS = [[1440, 900, 'paper'], [1440, 900, 'dark'], [1920, 1080, 'paper'], [1920, 1080, 'dark']];

/** config.toml's text with `machine.gpu_samples` set to `path` (e2e/cli/slow.sh's slow_config). */
function withSamples(text, path) {
  const line = `gpu_samples = ${JSON.stringify(path)}\n`;
  const kept = text.replace(/^gpu_samples = .*\n/m, '');
  return /^\[machine\]$/m.test(kept) ? kept.replace(/^\[machine\]\n/m, (table) => table + line) : `${kept.replace(/\n*$/, '')}\n\n[machine]\n${line}`;
}

/** What `GET /v1/events` says for `ms`, as a stream that never says what it shows: each event parsed. */
async function heardFor(calcifer, ms) {
  const stop = new AbortController();
  const answer = await fetch(`${calcifer.origin}/v1/events`, { headers: { Authorization: `Bearer ${calcifer.token}`, Accept: 'text/event-stream' }, signal: stop.signal });
  const reader = answer.body.pipeThrough(new TextDecoderStream()).getReader();
  const until = Date.now() + ms;
  const events = [];
  let held = '';
  try {
    for (;;) {
      const next = await Promise.race([reader.read(), new Promise((done) => setTimeout(() => done(null), Math.max(0, until - Date.now())))]);
      if (!next || next.done) break;
      held += next.value;
      for (let end = held.indexOf('\n\n'); end >= 0; end = held.indexOf('\n\n')) {
        const data = held.slice(0, end).split('\n').filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trim()).join('\n');
        held = held.slice(end + 2);
        if (data) events.push(JSON.parse(data));
      }
    }
  } finally {
    stop.abort();
  }
  return events;
}

test('gpu-gauges', { tag: '@solo' }, async ({ page, project, calcifer, cli, calciferControl }) => {
  test.setTimeout(600_000);
  // What the page says it shows, each time it says it.
  const said = [];
  page.on('request', (request) => {
    if (request.method() === 'PUT' && /\/v1\/events\/[^/]+$/.test(new URL(request.url()).pathname)) said.push(JSON.parse(request.postData() || '{}'));
  });

  await test.step("this computer's machine on recorded readings", async () => {
    writeFileSync(calciferControl.config, withSamples(readFileSync(calciferControl.config, 'utf8'), SAMPLES));
    await calciferControl.stopMachine();
    await calciferControl.startMachine();
  });

  await test.step('cozy machine show local: each GPU now, as the machine read it that second', async () => {
    const busy = new Set();
    for (let i = 0; i < 3; i++) {
      const shown = await cli.json('machine', 'show', 'local');
      expect(shown.gpu_use?.gpus?.map((gpu) => gpu.index), `its GPUs' use: ${JSON.stringify(shown.gpu_use)}`).toEqual([0, 1]);
      const want = GPU0[Math.floor(shown.gpu_use.at_unix_ms / 1000) % GPU0.length];
      expect(shown.gpu_use.gpus[0], 'GPU 0 as recorded for that second').toMatchObject({
        busy_percent: want.busy_percent,
        memory_used_bytes: want.used_bytes,
        memory_total_bytes: want.total_bytes,
        power_w: want.power_w,
        power_limit_w: want.power_limit_w,
        temperature_c: want.temperature_c,
        sm_mhz: want.sm_mhz,
        sm_mhz_max: want.sm_mhz_limit,
        throttles: want.reasons.filter((reason) => reason !== 'idle'),
      });
      busy.add(shown.gpu_use.gpus[0].busy_percent);
      await page.waitForTimeout(1_100);
    }
    expect(busy.size, 'it changes from one second to the next').toBeGreaterThan(1);
    const printed = await cli('machine', 'show', 'local');
    expect(printed.code, printed.stderr).toBe(0);
    expect(printed.stdout).toMatch(/GPU 0 now +\d{1,3}% busy · [\d.]+ GiB of 24 GiB · \d+ of 450 W · \d+ °C · \d+ of 3105 MHz/);
    expect(printed.stdout).toMatch(/GPU 1 now +\d% busy · 512 MiB of 24 GiB · 21 of 450 W · 38 °C · 210 of 3105 MHz/);
  });

  const row = page.locator('.drawer li.runrow.machine[data-machine="local"]');
  const busy = row.getByRole('meter', { name: 'GPU 0 busy' });
  await test.step("Machines: each GPU's busy share moves with its readings, beside its last minute, memory, power and heat", async () => {
    await page.goto(project.url('machines'));
    await expect(busy, "local's row shows GPU 0's busy share").toBeVisible({ timeout: 60_000 });
    const seen = new Set();
    for (let i = 0; i < 20 && seen.size < 4; i++) {
      seen.add(Number(await busy.getAttribute('aria-valuenow')));
      await page.waitForTimeout(500);
    }
    expect([...seen].filter((value) => !BUSY.has(value)), 'only what was recorded').toEqual([]);
    expect(seen.size, 'the bar moves with the readings').toBeGreaterThanOrEqual(3);
    await expect(row.getByRole('meter', { name: 'GPU 1 busy' })).toBeVisible();
    await expect(row.locator('.gauge[data-gpu="0"] .gauge-busy')).toHaveText(/^\d{1,3}%$/);
    await expect(row.getByRole('img', { name: /^GPU 0 busy over the last minute: \d+% to \d+%$/ })).toBeVisible();
    await expect(row.locator('.gauge[data-gpu="0"] .gauge-fact')).toHaveText([/^[\d.]+\/24GB$/, /^\d+\/450W$/, /^\d+°C$/]);
    await expect(row.locator('.gauge-limit', { hasText: 'Power-limited' }), 'at its power limit, it says so').toBeVisible({ timeout: 15_000 });
    await expect(row.locator('.gauge-limit', { hasText: 'Thermal' }), 'slowed for heat, it says so').toBeVisible({ timeout: 15_000 });
    expect(said.some((shown) => shown.gpus === true), `the page says it shows GPU use: ${JSON.stringify(said)}`).toBe(true);
  });

  await test.step('a stream whose page shows no GPU use hears none', async () => {
    const heard = await heardFor(calcifer, 4_000);
    expect(heard.filter((event) => event.type === 'machine.gpus')).toEqual([]);
  });

  await test.step('Machines, looked at', async () => {
    for (const [width, height, theme] of LOOKS) {
      await page.setViewportSize({ width, height });
      await project.look(theme);
      await themed(page, theme);
      // A trace a minute long by the last look: each look at GPU 0 at its power limit.
      await expect(busy).toHaveAttribute('aria-valuenow', '100', { timeout: 15_000 });
      await page.screenshot({ path: test.info().outputPath(`machines-${width}x${height}-${theme}.png`) });
    }
  });

  const section = page.locator('.drawer section.sect').filter({ has: page.locator('h3 > span', { hasText: /^GPUs$/ }) });
  await test.step("its drawer: each GPU's gauges, a row each", async () => {
    await page.setViewportSize({ width: 1440, height: 900 });
    // Opened from its row, as a person does: the minute the page heard is the drawer's trace too.
    await row.getByRole('link', { name: 'Details' }).click();
    const gpu0 = section.locator('li.gpu[data-gpu="0"]');
    await expect(gpu0.getByRole('meter', { name: 'GPU 0 busy' })).toBeVisible({ timeout: 60_000 });
    await expect(gpu0.getByRole('meter', { name: 'GPU 0 memory in use' })).toBeVisible();
    await expect(gpu0.getByRole('meter', { name: 'GPU 0 power of its limit' })).toBeVisible();
    await expect(gpu0.getByRole('img', { name: /^GPU 0 busy over the last minute/ })).toBeVisible();
    await expect(gpu0.locator('.gpu-use b').nth(1)).toHaveText(/^[\d.]+GB of 24GB$/);
    await expect(gpu0.locator('.gpu-heat')).toHaveText(/^\d+°C · clock [\d,]+ of 3,105 MHz$/);
    await expect(gpu0.locator('.gpu-limit', { hasText: 'Power-limited now:' })).toBeVisible({ timeout: 15_000 });
    for (const [width, height, theme] of LOOKS) {
      await page.setViewportSize({ width, height });
      await project.look(theme);
      await themed(page, theme);
      await expect(gpu0.getByRole('meter', { name: 'GPU 0 busy' })).toHaveAttribute('aria-valuenow', '100', { timeout: 15_000 });
      await page.screenshot({ path: test.info().outputPath(`drawer-${width}x${height}-${theme}.png`) });
    }
  });

  await test.step('the drawer closed, the page no longer says it shows GPU use', async () => {
    const before = said.length;
    await page.locator('.drawer').getByRole('link', { name: 'Close' }).click();
    await expect(page.locator('.drawer')).toHaveCount(0);
    await expect.poll(() => said.slice(before).some((shown) => !shown.gpus), { message: 'the page took back that it shows GPU use', timeout: 30_000 }).toBe(true);
  });
});

// GPU_MACHINE (release-gate/sessions/gpu-gauges.sh: a rental, SDXL renders on it one after another): Machines open from before the renders
// to after them. Its row's GPU 0 bar reaches 50% during them and falls to 10% after; each look's bar, figures and limits logged twice a
// second to GAUGE_LOG; Machines and the rental's drawer at 1440×900 and 1920×1080, paper and dark, during and after.
test('gpu-gauges: a rental under a render', { tag: '@gpu' }, async ({ page, project }) => {
  const name = process.env.GPU_MACHINE;
  test.skip(!name, 'GPU_MACHINE names a rental');
  test.setTimeout(1_200_000);
  const row = page.locator(`.drawer li.runrow.machine[data-machine="${name}"]`);
  const busy = row.locator('.gauge[data-gpu="0"]').getByRole('meter', { name: 'GPU 0 busy' });
  const log = [];
  let phase = 'before';
  const looks = setInterval(async () => {
    const look = await row.evaluate((li) => ({
      busy: Number(li.querySelector('.gauge[data-gpu="0"] .meter.busy')?.getAttribute('aria-valuenow') ?? NaN),
      facts: [...li.querySelectorAll('.gauge[data-gpu="0"] .gauge-fact, .gauge-limit')].map((fact) => fact.textContent),
    })).catch(() => null);
    if (look) log.push({ at: Date.now(), phase, ...look });
  }, 500);
  const shoot = async (label) => {
    for (const [width, height, theme] of LOOKS) {
      await page.setViewportSize({ width, height });
      await project.look(theme);
      await themed(page, theme);
      await page.screenshot({ path: test.info().outputPath(`gauges-${label}-machines-${width}x${height}-${theme}.png`) });
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    await project.look(null);
  };
  // Its drawer, from its row and back, as a person goes: the page keeps the minute it heard.
  const drawer = async (label) => {
    await row.getByRole('link', { name: 'Details' }).click();
    await expect(page.locator('.drawer li.gpu[data-gpu="0"]').getByRole('meter', { name: 'GPU 0 busy' })).toBeVisible({ timeout: 60_000 });
    await page.screenshot({ path: test.info().outputPath(`gauges-${label}-drawer-1440x900-paper.png`) });
    await page.locator('.drawer').getByRole('link', { name: 'Back' }).click();
    await expect(busy).toBeVisible({ timeout: 60_000 });
  };
  try {
    await page.goto(project.url('machines'));
    await expect(busy, `${name}'s row shows GPU 0's busy share`).toBeVisible({ timeout: 120_000 });
    phase = 'waiting';
    await expect.poll(async () => Number(await busy.getAttribute('aria-valuenow')), { message: 'busy during the renders', timeout: 600_000, intervals: [500] }).toBeGreaterThanOrEqual(50);
    phase = 'during';
    await shoot('during');
    await drawer('during');
    phase = 'falling';
    await expect.poll(async () => Number(await busy.getAttribute('aria-valuenow')), { message: 'idle after the renders', timeout: 900_000, intervals: [500] }).toBeLessThanOrEqual(10);
    phase = 'after';
    await page.waitForTimeout(10_000);
    await shoot('after');
    await drawer('after');
  } finally {
    clearInterval(looks);
    if (process.env.GAUGE_LOG) writeFileSync(process.env.GAUGE_LOG, log.map((look) => JSON.stringify(look)).join('\n') + '\n');
  }
  const during = log.filter((look) => look.phase === 'during' || look.phase === 'waiting').map((look) => look.busy);
  expect(Math.max(...during), 'the bar rose during the renders').toBeGreaterThanOrEqual(50);
  expect(log.filter((look) => look.phase === 'after').every((look) => look.busy <= 30), 'and stayed down after').toBe(true);
});
