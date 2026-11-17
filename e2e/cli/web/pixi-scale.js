// Production scale protocol; SOURCE ONLY until the coordinator authorizes execution.
// Prepare: COZY=... PIXI_OUT=... PIXI_PHASE=prepare node pixi-scale.js
// Resume an interrupted preparation into a NEW receipt: add PREPARED=.../prepared.json to the prepare command.
// Measure: COZY=... CHROME=... PLAYWRIGHT=... PIXI_OUT=... PREPARED=.../prepared.json PIXI_PHASE=measure node pixi-scale.js
// Optional SOURCE_DIR, CANDIDATE_DIST, CHROME_ARGS (JSON array), FFMPEG, DPR=1|2.
// Uses the existing default-home Calcifer, public CLI imports and real /canvas writes. Never seeds a database, rents, or restarts services.
// All projects and receipts survive failure. The only browser is headless. Three repetitions use the SAME prepared files and layout.
// Explicit protocol budgets: measurement API/input 30s, setup API 120s, visible-image loading 15min, each preparation command 30min.
// Env *_MS may override setup/liveness deadlines; frame and response acceptance targets stay separate.
// Interaction thresholds are reported separately from those liveness deadlines. CPU/PSS and screenshots are sampled OUTSIDE frame timing.
const { execFileSync } = require('node:child_process');
const { createHash, randomUUID } = require('node:crypto');
const { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } = require('node:fs');
const { homedir, cpus, loadavg, release, totalmem } = require('node:os');
const { join, resolve } = require('node:path');

const COUNTS = [100, 1000, 5000], WIDTH = 1600, HEIGHT = 1000, REPEATS = 3;
const STEP = Number(process.env.STEP_MS ?? 30_000), LOAD = Number(process.env.LOAD_MS ?? 900_000), PREPARE = Number(process.env.PREPARE_MS ?? 1_800_000);
const PREPARE_READ = Number(process.env.PREPARE_READ_MS ?? 120_000);
const FFMPEG = process.env.FFMPEG ?? 'ffmpeg', NAME = i => `photo_${String(i).padStart(5, '0')}`;
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const pause = ms => new Promise(done => setTimeout(done, ms));
const secrets = [], failures = [], seen = { script_sha256: hash(readFileSync(__filename)), protocol: { counts: COUNTS, repetitions: REPEATS, width: WIDTH, height: HEIGHT, step_ms: STEP, loading_ms: LOAD, prepare_command_ms: PREPARE, response_ms: 1000, timing: 'input dispatch through settled camera; loading, screenshots, process samples excluded', cache: 'fresh browser process for cohort; server and OS caches uncontrolled; each warm case waits for visible textures; transitions and subsequent loading recorded separately' } };
const clean = value => secrets.reduce((text, secret) => text.replaceAll(secret, '[redacted]'), String(value));
let out, browser, page, during = 'configuration';
function save(name, value) { writeFileSync(join(out, name), `${clean(JSON.stringify(value, null, 2))}\n`); }
function check(name, ok, detail) { if (!ok) failures.push({ name, detail }); process.stderr.write(`[pixi-scale] ${name}: ${ok ? 'pass' : 'FAIL'}\n`); }
async function until(label, read, accept, budget = STEP) {
  during = label;
  const started = performance.now(); let value;
  do { value = await read(); if (accept(value)) return value; await pause(100); } while (performance.now() - started < budget);
  throw new Error(`${label}: explicit ${budget}ms protocol budget exhausted; last=${JSON.stringify(value)}`);
}
function webpSize(bytes) {
  if (bytes.toString('ascii', 0, 4) !== 'RIFF' || bytes.toString('ascii', 8, 12) !== 'WEBP') throw new Error('fixture is not WebP');
  for (let at = 12; at + 8 < bytes.length;) {
    const type = bytes.toString('ascii', at, at + 4), length = bytes.readUInt32LE(at + 4), p = at + 8;
    if (type === 'VP8X') return [1 + bytes.readUIntLE(p + 4, 3), 1 + bytes.readUIntLE(p + 7, 3)];
    if (type === 'VP8 ') return [bytes.readUInt16LE(p + 6) & 16383, bytes.readUInt16LE(p + 8) & 16383];
    if (type === 'VP8L') { const packed = bytes.readUInt32LE(p + 1); return [(packed & 16383) + 1, ((packed >>> 14) & 16383) + 1]; }
    at = p + length + (length & 1);
  }
  throw new Error('fixture dimensions missing');
}
function decode(input, width, height) {
  const file = typeof input === 'string';
  return execFileSync(FFMPEG, ['-v', 'error', '-threads', '1', '-filter_threads', '1', '-i', file ? input : 'pipe:0', '-frames:v', '1', ...(width ? ['-vf', `scale=${width}:${height}:flags=area`] : []), '-threads', '1', '-pix_fmt', 'rgb24', '-f', 'rawvideo', 'pipe:1'], { input: file ? undefined : input, timeout: STEP, maxBuffer: 32 * 1024 * 1024, stdio: ['pipe', 'pipe', 'pipe'] });
}
function sources(directory) {
  return Array.from({ length: 5000 }, (_, i) => {
    const file = realpathSync(join(directory, String(i), '512.webp')), bytes = readFileSync(file), [width, height] = webpSize(bytes);
    if (width !== 512 || height !== 288) throw new Error(`fixture ${i} must be 512x288, got ${width}x${height}`);
    return { index: i, file, sha256: hash(bytes), bytes: bytes.length, width, height };
  });
}
function representatives(count, columns) {
  const rows = Math.ceil(count / columns), found = new Set();
  for (const y of [0, .5, 1]) for (const x of [0, .5, 1]) found.add(Math.min(count - 1, Math.round(y * (rows - 1)) * columns + Math.round(x * (columns - 1))));
  return [...found];
}

// Static candidate bytes only; CDP matching excludes every API/media URL and leaves HTTP cache behavior unchanged.
async function candidate(context, origin) {
  if (!process.env.CANDIDATE_DIST) return;
  seen.candidate = await require('./candidate-cdp').candidateCDP(page, origin, process.env.CANDIDATE_DIST);
}

(async () => {
  if (!process.env.COZY || !process.env.PIXI_OUT || !['prepare', 'measure'].includes(process.env.PIXI_PHASE)) throw new Error('COZY, PIXI_OUT and PIXI_PHASE=prepare|measure are required');
  if (process.env.COZY_HOME || process.env.PAGE_ORIGIN || process.env.HOLDER) throw new Error('use the existing default-home Calcifer, without home/origin/holder overrides');
  for (const value of [STEP, LOAD, PREPARE, PREPARE_READ]) if (!Number.isFinite(value) || value <= 0) throw new Error('protocol budgets must be positive');
  mkdirSync(resolve(process.env.PIXI_OUT), { recursive: true });
  out = mkdtempSync(join(resolve(process.env.PIXI_OUT), `pixi-scale-${process.env.PIXI_PHASE}-`));
  const record = join(homedir(), '.cozy/calcifer/calcifer.lock.holder'), holder = JSON.parse(readFileSync(record, 'utf8'));
  if (!holder.token || !holder.pid || !holder.address) throw new Error('default Calcifer holder incomplete');
  secrets.push(holder.token); process.kill(holder.pid, 0);
  const origin = `http://${holder.address}`;
  const sameService = () => { const next = JSON.parse(readFileSync(record, 'utf8')); if (next.pid !== holder.pid || next.address !== holder.address) throw new Error('Calcifer changed; receipt retained'); };
  const api = async (path, body) => {
    sameService();
    const budget = process.env.PIXI_PHASE === 'prepare' ? PREPARE_READ : STEP;
    const response = await fetch(origin + path, { method: body ? 'PATCH' : 'GET', headers: { Authorization: `Bearer ${holder.token}`, ...(body ? { 'Content-Type': 'application/json', 'Idempotency-Key': randomUUID() } : {}) }, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(budget) }).catch(error => { throw new Error(`${body ? 'PATCH' : 'GET'} ${path}: ${budget}ms setup/request budget: ${error.message}`); });
    if (!response.ok) throw new Error(`API ${body ? 'PATCH' : 'GET'} status ${response.status}`);
    const value = await response.json(); sameService(); return value;
  };
  const cli = (args, cwd, budget = PREPARE) => {
    sameService(); let result;
    try { result = execFileSync(process.env.COZY, args, { cwd, encoding: 'utf8', timeout: budget, maxBuffer: 64 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] }); }
    catch (error) { throw new Error(`cozy ${args[0]} failed: ${clean(error.stderr ?? error.message)}`); }
    sameService(); return JSON.parse(result);
  };
  seen.backend = { pid: holder.pid, version: (await api('/v1/status')).version };
  if (process.env.PIXI_PHASE === 'prepare') {
    seen.protocol.prepare_read_ms = PREPARE_READ;
    during = 'verify 5000 distinct source files';
    const priorPath = process.env.PREPARED ? realpathSync(process.env.PREPARED) : null;
    const priorBytes = priorPath ? readFileSync(priorPath) : null, prior = priorBytes ? JSON.parse(priorBytes) : null;
    if (prior && (prior.version !== 1 || prior.files?.length !== 5000 || !Array.isArray(prior.projects))) throw new Error('resume requires a scale preparation manifest');
    const files = prior && !process.env.SOURCE_DIR ? prior.files.map((file, index) => {
      const path = realpathSync(file.file), bytes = readFileSync(path), [width, height] = webpSize(bytes);
      if (file.index !== index || width !== 512 || height !== 288 || hash(bytes) !== file.sha256) throw new Error(`resume source ${index} differs from its receipt`);
      return { ...file, file: path, bytes: bytes.length, width, height };
    }) : sources(process.env.SOURCE_DIR ?? '/home/fidika/cozy_v2/outputs/canvas-library-selection/fixtures/cards');
    if (new Set(files.map(file => file.sha256)).size !== 5000) throw new Error('5000 distinct content hashes required before creating projects');
    const sourceHash = hash(JSON.stringify(files.map(file => file.sha256)));
    if (prior && (sourceHash !== prior.source_set_sha256 || prior.files.some((file, index) => file.index !== index || file.sha256 !== files[index].sha256))) throw new Error('resume source set differs from its receipt');
    const manifest = { ...(prior ?? {}), version: 1, files, source_set_sha256: sourceHash, projects: prior?.projects ?? [], created: prior?.created ?? new Date().toISOString(), backend: seen.backend };
    if (prior) {
      const parent = resolve(priorPath, '..');
      seen.resume = manifest.resumed_from = { prepared: priorPath, prepared_sha256: hash(priorBytes), result: join(parent, 'result.json'), at: new Date().toISOString() };
      if (new Set(manifest.projects.map(project => project.count)).size !== manifest.projects.length || new Set(manifest.projects.map(project => project.id)).size !== manifest.projects.length || manifest.projects.some(project => !COUNTS.includes(project.count) || !project.id || !project.folder || project.columns !== Math.ceil(Math.sqrt(project.count * 1.25)))) throw new Error('resume project ownership metadata is inconsistent');
      // Copy references before publishing the new manifest, so a second interruption still leaves self-contained relative paths.
      for (const project of manifest.projects) for (const [index, reference] of Object.entries(project.references ?? {})) {
        if (reference !== `reference-${project.count}-${index}.rgb`) throw new Error('resume reference path differs from the generated receipt');
        const bytes = readFileSync(join(parent, reference));
        if (bytes.length !== 64 * 36 * 3) throw new Error('resume RGB reference is incomplete');
        writeFileSync(join(out, reference), bytes);
      }
    }
    save('prepared.json', manifest);
    const readOwned = async project => {
      const state = await api(`/v1/projects/${encodeURIComponent(project.id)}`);
      if (state.id !== project.id || realpathSync(state.path) !== realpathSync(project.folder)) throw new Error(`project ${project.count} identity/folder changed; nothing overwritten`);
      const expected = new Map(files.slice(0, project.count).map(file => [NAME(file.index), file.sha256])), byName = new Map();
      for (const asset of state.assets) {
        if (byName.has(asset.name) || !expected.has(asset.name) || asset.kind !== 'image' || asset.sha256 !== `sha256:${expected.get(asset.name)}`) throw new Error(`project ${project.count} has an unexpected or changed asset ${asset.name}; nothing overwritten`);
        byName.set(asset.name, asset);
      }
      if (state.entities?.length) throw new Error(`project ${project.count} has unrelated entities; nothing overwritten`);
      return byName;
    };
    // A resume never discovers another person's edits only after modifying an earlier project in the cohort.
    const verified = new Map();
    for (const project of manifest.projects) {
      const existing = await readOwned(project);
      verified.set(project.count, existing);
      const confirmed = (project.imports ?? []).flatMap(batch => batch.names ?? files.slice(batch.first, batch.first + batch.count).map(file => NAME(file.index)));
      if (confirmed.some(name => !existing.has(name))) throw new Error(`project ${project.count} lost a previously confirmed asset; nothing recreated`);
      if (project.cards) {
        if (existing.size !== project.count) throw new Error(`project ${project.count} completed asset set changed; nothing recreated`);
        const rows = (await api(`/v1/projects/${encodeURIComponent(project.id)}/canvas`)).cards;
        if (Object.entries(project.cards).some(([name, box]) => ['x', 'y', 'w'].some(key => rows[name]?.[key] !== box[key]))) throw new Error(`project ${project.count} completed layout changed; nothing overwritten`);
      }
    }
    for (const count of COUNTS) {
      during = `prepare ${count}`;
      let project = manifest.projects.find(project => project.count === count);
      if (project?.cards) { process.stderr.write(`[pixi-scale] verified completed ${count}-card project\n`); continue; }
      if (!project) {
        const folder = join(out, `project-${count}`), made = cli(['new', '--unencrypted', '--json', folder], out);
        if (!made.created || !made.id) throw new Error('cozy new did not create the owned project');
        project = { count, id: made.id, folder, columns: Math.ceil(Math.sqrt(count * 1.25)), imports: [], references: {} };
        manifest.projects.push(project); manifest.projects.sort((a, b) => a.count - b.count); save('prepared.json', manifest);
      }
      project.imports ??= []; project.references ??= {};
      let existing = verified.get(count) ?? await readOwned(project);
      for (let first = 0; first < count; first += 100) {
        const missing = files.slice(first, Math.min(count, first + 100)).filter(file => !existing.has(NAME(file.index)));
        if (!missing.length) continue;
        during = `prepare ${count}: import ${first}..${Math.min(count, first + 100) - 1}`;
        const path = join(out, `apply-${count}-${first}.json`), input = Object.fromEntries(missing.map(file => [NAME(file.index), { kind: 'image', takes: [{ file: file.file }] }]));
        writeFileSync(path, JSON.stringify(input));
        const input_sha256 = hash(readFileSync(path)); cli(['apply', '-f', path, '--json'], project.folder);
        project.imports.push({ first, count: missing.length, names: Object.keys(input), receipt: out, input_sha256, after_sha256: hash(readFileSync(path)) });
        save('prepared.json', manifest);
        existing = await readOwned(project);
        process.stderr.write(`[pixi-scale] ${count}: verified ${existing.size} imported assets\n`);
      }
      const path = `/v1/projects/${encodeURIComponent(project.id)}`, byName = await readOwned(project);
      if (byName.size !== count || files.slice(0, count).some(file => byName.get(NAME(file.index))?.sha256 !== `sha256:${file.sha256}`)) throw new Error('public CLI imported assets differ from verified source set');
      const cards = Object.fromEntries(files.slice(0, count).map(file => [NAME(file.index), { x: (file.index % project.columns) * 288, y: Math.floor(file.index / project.columns) * 220, w: 240 }]));
      let after = await api(`${path}/canvas`);
      const pending = Object.entries(cards).filter(([name, box]) => ['x', 'y', 'w'].some(key => after.cards[name]?.[key] !== box[key]));
      for (let first = 0; first < pending.length; first += 100) {
        during = `prepare ${count}: place ${Math.min(pending.length, first + 100)}/${pending.length} changed rows`;
        const batch = Object.fromEntries(pending.slice(first, first + 100));
        await api(`${path}/canvas`, { cards: batch, placed: Object.fromEntries(Object.keys(batch).map(name => [name, after.seq])) });
        after = await api(`${path}/canvas`);
      }
      if (Object.entries(cards).some(([name, box]) => ['x', 'y', 'w'].some(key => after.cards[name]?.[key] !== box[key]))) throw new Error('real canvas layout readback differs');
      project.cards = cards; project.canvas_seq = after.seq;
      await api(`${path}/view`, { viewport: { x: 320, y: 145, zoom: 1 }, selection: [], more: [] });
      for (const i of representatives(count, project.columns)) { const file = `reference-${count}-${i}.rgb`; writeFileSync(join(out, file), decode(files[i].file, 64, 36)); project.references[i] = file; }
      save('prepared.json', manifest);
      process.stderr.write(`[pixi-scale] prepared ${count} real cards\n`);
    }
    seen.prepared = join(out, 'prepared.json'); return;
  }

  if (!process.env.PREPARED || !process.env.PLAYWRIGHT || !process.env.CHROME) throw new Error('measure requires PREPARED, PLAYWRIGHT and CHROME');
  const preparedPath = realpathSync(process.env.PREPARED), preparedRoot = resolve(preparedPath, '..'), preparedBytes = readFileSync(preparedPath), prepared = JSON.parse(preparedBytes);
  if (prepared.version !== 1 || JSON.stringify(prepared.projects.map(project => project.count)) !== JSON.stringify(COUNTS)) throw new Error('complete prepared 100/1000/5000 cohort required');
  if (prepared.files.length !== 5000 || new Set(prepared.files.map(file => file.sha256)).size !== 5000) throw new Error('prepared unique source count differs');
  if (hash(JSON.stringify(prepared.files.map(file => file.sha256))) !== prepared.source_set_sha256) throw new Error('prepared source-set receipt differs');
  for (const file of prepared.files) if (hash(readFileSync(file.file)) !== file.sha256) throw new Error(`fixture ${file.index} changed since preparation`);
  seen.prepared = { path: preparedPath, sha256: hash(preparedBytes), source_set_sha256: prepared.source_set_sha256 };
  const dpr = Number(process.env.DPR ?? 1), extra = JSON.parse(process.env.CHROME_ARGS ?? '[]');
  if (![1, 2].includes(dpr) || !Array.isArray(extra) || extra.some(arg => typeof arg !== 'string' || /swiftshader|llvmpipe|lavapipe|disable-gpu(?:$|=)/i.test(arg))) throw new Error('DPR must be 1 or 2 and CHROME_ARGS must select hardware');
  const { chromium } = require(process.env.PLAYWRIGHT);
  seen.host = { platform: process.platform, arch: process.arch, kernel: release(), cpu: cpus()[0]?.model, logical_cpus: cpus().length, memory_bytes: totalmem(), initial_load: loadavg(), ...(process.platform === 'linux' ? { cpu_affinity: readFileSync('/proc/self/status', 'utf8').match(/^Cpus_allowed_list:\s+(.+)$/m)?.[1] } : {}) };
  seen.browser_args = ['--enable-gpu', '--disable-software-rasterizer', ...extra]; seen.protocol.dpr = dpr;
  browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true, args: seen.browser_args, timeout: STEP });
  seen.browser = browser.version();
  const context = await browser.newContext({ viewport: { width: WIDTH, height: HEIGHT }, deviceScaleFactor: dpr, serviceWorkers: 'block' });
  page = await context.newPage(); page.setDefaultTimeout(STEP); page.setDefaultNavigationTimeout(STEP);
  const errors = [], violations = [], origins = new Set(), thumbWidths = new Set(), browserCredentials = [];
  page.on('pageerror', error => errors.push(clean(error.message)));
  page.on('request', request => { const url = new URL(request.url()); if (url.pathname.startsWith('/v1')) { origins.add(url.origin); if (request.headers().authorization || url.pathname.startsWith('/v1/session')) browserCredentials.push(url.pathname); } if (url.pathname.includes('/thumbs/')) thumbWidths.add(Number(url.searchParams.get('w')) || 0); });
  await page.exposeFunction('pixiScaleCsp', directive => violations.push(directive));
  await page.addInitScript(() => document.addEventListener('securitypolicyviolation', event => window.pixiScaleCsp(event.violatedDirective)));
  await candidate(context, origin);
  const cdp = await browser.newBrowserCDPSession();
  seen.gpu_process = (await cdp.send('SystemInfo.getInfo')).gpu;
  const processes = async () => {
    const at = performance.now(), { processInfo } = await cdp.send('SystemInfo.getProcessInfo');
    return { at, host_load: loadavg(), processes: processInfo.map(process => { let memory = null; try { const text = readFileSync(`/proc/${process.id}/smaps_rollup`, 'utf8'); memory = { pss_kib: Number(text.match(/^Pss:\s+(\d+)/m)?.[1]), rss_kib: Number(text.match(/^Rss:\s+(\d+)/m)?.[1]) }; } catch { /* Explicitly unavailable on other OSs or exited processes. */ } return { id: process.id, type: process.type, cpu_seconds: process.cpuTime, memory }; }) };
  };
  const cpuSample = async () => { const start = performance.now(), { processInfo } = await cdp.send('SystemInfo.getProcessInfo'); return { at: performance.now(), sampling_ms: performance.now() - start, processes: processInfo.map(p => ({ id: p.id, type: p.type, cpu_seconds: p.cpuTime })) }; };
  const canvas = page.getByRole('region', { name: 'Project canvas', exact: true });
  const inspect = (method, arg) => canvas.evaluate((element, { method, arg }) => element.canvasInspection?.[method]?.(arg) ?? null, { method, arg });
  const camera = () => inspect('camera'), resources = () => inspect('resources');
  const cameraCenter = async project => {
    const a = (await inspect('card', NAME(0))).bounds, b = (await inspect('card', NAME(project.columns - 1))).bounds, c = (await inspect('card', NAME(project.count - 1))).bounds;
    await page.mouse.move((a.x + b.x + b.width) / 2, (a.y + c.y + c.height) / 2);
  };
  const settled = async () => { let before, same = 0; return until('camera settle', camera, value => { const next = JSON.stringify(value); same = value && next === before ? same + 1 : 0; before = next; return same >= 3; }); };
  const passive = async () => { await page.mouse.move(15, 15); await until('native overlay released', () => page.locator('.canvas-node').count(), count => count === 0); };
  const fit = async () => { await page.getByRole('button', { name: /, more zoom options$/ }).click(); await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click(); await passive(); await settled(); };
  const working = async () => { await canvas.focus(); await page.keyboard.press('ControlOrMeta+0'); await passive(); await settled(); if (Math.abs((await camera()).zoom - 1) > .001) throw new Error('working view did not reach 100%'); };
  const ready = async count => {
    const start = performance.now(); let reported = start;
    const value = await until('visible textures loaded', async () => {
      const value = await resources();
      if (value?.failed) throw new Error(`visible media entered permanent failure: ${JSON.stringify(value)}`);
      if (value && performance.now() - reported > 5000) { reported = performance.now(); process.stderr.write(`[pixi-scale] ${count}: loaded ${value.loadedCards}/${value.visibleCards}; loading ${value.loading}, queued ${value.queued}\n`); }
      return value;
    }, value => value && value.totalCards === count && value.visibleCards > 0 && value.loadedCards === value.visibleCards && value.loading === 0 && value.failed === 0, LOAD);
    return { milliseconds: performance.now() - start, resources: value };
  };
  const pan = async () => {
    const before = await camera(), positions = []; await page.mouse.move(WIDTH * .6, HEIGHT * .55);
    for (let n = 0; n < 12; n++) { await page.mouse.wheel(n < 6 ? 28 : -28, 0); await pause(35); positions.push(await camera()); }
    await passive(); await settled();
    const excursion = Math.max(...positions.map(p => Math.hypot(p.x - before.x, p.y - before.y)));
    if (excursion < 100 || new Set(positions.map(p => `${p.x},${p.y}`)).size < 4) throw new Error(`pan did not visibly move the camera: ${JSON.stringify({ before, positions })}`);
    return { before, after: await camera(), excursion_px: excursion, positions };
  };
  const quantile = (values, q) => { const sorted = [...values].sort((a, b) => a - b); return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? null; };
  const timed = async (result, stage, action, warm = null) => {
    const beforeResources = await resources(), before = await processes();
    const cpuBefore = await cpuSample();
    await page.evaluate(() => { const run = window.__pixiScale = { start: performance.now(), last: null, frames: [], handle: 0 }; const step = now => { if (run.last !== null) run.frames.push(now - run.last); run.last = now; run.handle = requestAnimationFrame(step); }; run.handle = requestAnimationFrame(step); });
    const start = performance.now(); let sample, interaction;
    try { interaction = await action(); } finally { sample = await page.evaluate(() => { const run = window.__pixiScale; cancelAnimationFrame(run.handle); delete window.__pixiScale; return { frames: run.frames, duration_ms: performance.now() - run.start }; }); }
    const duration_ms = performance.now() - start, cpuAfter = await cpuSample(), after = await processes(), previous = new Map(cpuBefore.processes.map(process => [process.id, process.cpu_seconds]));
    const cpu_seconds = cpuAfter.processes.reduce((sum, process) => sum + Math.max(0, process.cpu_seconds - (previous.get(process.id) ?? 0)), 0);
    const frame = { samples: sample.frames.length, p95_ms: quantile(sample.frames, .95), p99_ms: quantile(sample.frames, .99), max_ms: Math.max(0, ...sample.frames), raw_ms: sample.frames };
    const target = warm === 'working' ? { p95_ms: 20, p99_ms: 33.4 } : warm === 'all-visible' ? { p95_ms: 33.4, p99_ms: 100 } : null;
    const cell = { stage, cache_phase: warm ?? 'transition', target, duration_ms, frame, interaction, cpu_seconds, cpu_sample_ms: cpuAfter.at - cpuBefore.at, browser_cpu_cores: cpu_seconds / ((cpuAfter.at - cpuBefore.at) / 1000), cpu_before: cpuBefore, cpu_after: cpuAfter, process_before: before, process_after: after, resources_before: beforeResources, resources_after: await resources() };
    result.stages.push(cell); save('runs.json', seen.runs);
    check(`${result.count}/${result.repeat}/${stage}/${target ? 'warm-frame-target' : 'frame-samples'}`, frame.samples > 0 && (!target || (frame.p95_ms <= target.p95_ms && frame.p99_ms <= target.p99_ms)), { p95: frame.p95_ms, p99: frame.p99_ms, samples: frame.samples, target });
    return cell;
  };
  const pixels = async (project, label) => {
    await passive(); const png = await page.screenshot(), file = `${project.count}-${label}.png`; writeFileSync(join(out, file), png);
    const rgb = decode(png), width = WIDTH * dpr, height = HEIGHT * dpr;
    if (rgb.length !== width * height * 3) throw new Error('independent screenshot decode dimensions differ');
    const average = (bytes, wide, high, box) => {
      const result = [0, 0, 0]; let n = 0;
      for (let y = Math.max(0, Math.ceil(box.y)); y < Math.min(high, Math.ceil(box.y + box.h)); y++) for (let x = Math.max(0, Math.ceil(box.x)); x < Math.min(wide, Math.ceil(box.x + box.w)); x++) { const at = (y * wide + x) * 3; for (let c = 0; c < 3; c++) result[c] += bytes[at + c]; n++; }
      if (!n) throw new Error('empty independent pixel probe'); return result.map(value => value / n);
    };
    const probes = [];
    for (const [index, reference] of Object.entries(project.references)) {
      const box = (await inspect('card', NAME(Number(index))))?.body;
      if (!box || box.x < 0 || box.y < 0 || box.x + box.width > WIDTH || box.y + box.height > HEIGHT) throw new Error('fit-all representative is not fully on screen');
      const expected = readFileSync(join(preparedRoot, reference)), actualCells = [], expectedCells = [];
      for (const y of [.25, .75]) for (const x of [.25, .75]) { actualCells.push(average(rgb, width, height, { x: (box.x + box.width * (x - .1)) * dpr, y: (box.y + box.height * (y - .1)) * dpr, w: box.width * .2 * dpr, h: box.height * .2 * dpr })); expectedCells.push(average(expected, 64, 36, { x: 64 * (x - .1), y: 36 * (y - .1), w: 64 * .2, h: 36 * .2 })); }
      const error = actualCells.flat().reduce((sum, value, i) => sum + Math.abs(value - expectedCells.flat()[i]), 0) / 12;
      probes.push({ index: Number(index), box, mean_absolute_rgb_error: error, actual: actualCells, expected: expectedCells });
      check(`${project.count}/${label}/source-pixels-${index}`, error <= 35, { mean_absolute_rgb_error: error });
    }
    return { file, sha256: hash(png), decoder: 'independent FFmpeg software RGB decode; four interior patch means against prepared source', tolerance_rgb: 35, probes };
  };

  seen.runs = [];
  for (const project of prepared.projects) {
    const apiPath = `/v1/projects/${encodeURIComponent(project.id)}`, state = await api(apiPath), rows = (await api(`${apiPath}/canvas`)).cards;
    const byName = new Map(state.assets.map(asset => [asset.name, asset]));
    if (byName.size !== project.count || prepared.files.slice(0, project.count).some(file => byName.get(NAME(file.index))?.sha256 !== `sha256:${file.sha256}`)) throw new Error('prepared project asset hashes changed');
    if (Object.entries(project.cards).some(([name, box]) => ['x', 'y', 'w'].some(key => rows[name]?.[key] !== box[key]))) throw new Error('prepared project positions changed');
    const opened = cli(['open', '--json'], project.folder, STEP), link = new URL(opened.url);
    if (typeof state.code !== 'string' || !/^[0-9A-HJKMNP-TV-Z]{9}$/.test(state.code)) throw new Error('prepared project has no canonical code');
    const canonical = `/p/${encodeURIComponent(state.code)}`;
    if (link.origin !== origin || link.hash || link.search || ![canonical, `/projects/${encodeURIComponent(project.id)}`].includes(link.pathname)) throw new Error('CLI open did not return this project plain Calcifer URL');
    link.pathname = canonical;
    const coldStart = performance.now(), document = await page.goto(link.href);
    if (!document.headers()['content-security-policy'] || document.headers()['referrer-policy'] !== 'no-referrer') throw new Error('real page CSP verification failed');
    await until('canvas ready', camera, Boolean);
    const gpu = await page.locator('canvas.canvas-pixels').evaluate(element => { const gl = element.getContext('webgl2') || element.getContext('webgl'), ext = gl?.getExtension('WEBGL_debug_renderer_info'); return ext ? { vendor: gl.getParameter(ext.UNMASKED_VENDOR_WEBGL), renderer: gl.getParameter(ext.UNMASKED_RENDERER_WEBGL), version: gl.getParameter(gl.VERSION), dpr: devicePixelRatio } : null; });
    if (!gpu || /swiftshader|llvmpipe|lavapipe|softpipe|software|swrast/i.test(gpu.renderer)) throw new Error('actual production canvas hardware WebGL not established');
    const cold = { count: project.count, gpu, first_visible: await ready(project.count), open_ms: performance.now() - coldStart };
    const firstFit = performance.now(); await fit(); cold.first_all_visible = await ready(project.count); cold.first_all_visible_ms = performance.now() - firstFit;
    check(`${project.count}/all-visible-real-textures`, cold.first_all_visible.resources.visibleCards === project.count && cold.first_all_visible.resources.loadedCards === project.count, cold.first_all_visible.resources);
    cold.pixels = await pixels(project, 'first-fit'); (seen.cold ??= []).push(cold); save('cold.json', seen.cold);
    for (let repeat = 1; repeat <= REPEATS; repeat++) {
      const result = { count: project.count, repeat, project_id: project.id, stages: [] }; seen.runs.push(result);
      await working(); result.working_load = await ready(project.count);
      await timed(result, 'working', pan, 'working');
      await timed(result, 'fitall', fit); result.fitall_load = await ready(project.count); result.fit_camera = await camera();
      const all = result.fitall_load.resources;
      check(`${project.count}/${repeat}/all-visible`, all.visibleCards === project.count && all.loadedCards === project.count && all.pinnedTextures === project.count && all.minimumPinnedTextureWidth >= 256, all);
      result.pixels = await pixels(project, `repeat-${repeat}-fit`);
      const body = (await inspect('card', NAME(Math.floor(project.count / 2)))).body, p = { x: body.x + body.width / 2, y: body.y + body.height / 2 };
      const hit = await inspect('hitTest', p); if (hit?.name !== NAME(Math.floor(project.count / 2))) throw new Error('far-zoom target hit test differs');
      await canvas.focus(); await page.keyboard.press('Escape'); await until('selection cleared before response measurement', () => inspect('selectedNames'), names => names?.length === 0);
      const selectStart = performance.now(); await page.mouse.click(p.x, p.y); await until('far-zoom selection', () => inspect('selectedNames'), names => names?.includes(hit.name));
      result.selection_ms = performance.now() - selectStart; check(`${project.count}/${repeat}/selection-response`, result.selection_ms <= 1000, result.selection_ms);
      await passive(); await timed(result, 'pan', pan, 'all-visible');
      await working(); result.gradual_initial_load = await ready(project.count);
      await timed(result, 'gradualzoom', async () => { await cameraCenter(project); await page.keyboard.down('Control'); try { for (let n = 0; n < 26; n++) { await page.mouse.wheel(0, 35); await pause(40); } } finally { await page.keyboard.up('Control'); } await passive(); await settled(); });
      result.gradual_load = await ready(project.count);
      result.gradual_camera = await camera();
      check(`${project.count}/${repeat}/gradual-all-visible`, result.gradual_load.resources.visibleCards === project.count && result.gradual_load.resources.loadedCards === project.count, result.gradual_load.resources);
      await timed(result, 'gradualzoom-in', async () => { await cameraCenter(project); await page.keyboard.down('Control'); try { for (let n = 0; n < 26; n++) { await page.mouse.wheel(0, -35); await pause(40); } } finally { await page.keyboard.up('Control'); } await passive(); await settled(); });
      result.zoomed_in_load = await ready(project.count);
      await page.mouse.wheel(80, 50); await settled();
      const home = await timed(result, 'Home', async () => { await canvas.focus(); await page.keyboard.press('f'); await passive(); await settled(); });
      result.home_input = 'f: production Zoom to fit'; result.home_camera = await camera(); result.home_load = await ready(project.count);
      check(`${project.count}/${repeat}/Home-response`, home.duration_ms <= 1000, home.duration_ms);
      check(`${project.count}/${repeat}/Home-camera`, Math.abs(result.home_camera.x - result.fit_camera.x) < .5 && Math.abs(result.home_camera.y - result.fit_camera.y) < .5 && Math.abs(result.home_camera.zoom - result.fit_camera.zoom) < .0005, { before: result.fit_camera, after: result.home_camera });
      check(`${project.count}/${repeat}/no-refresh`, await page.locator('[data-focused-asset]').count() === 0 && (await resources()).totalCards === project.count);
      save('runs.json', seen.runs);
    }
  }
  check('all-api-real-origin', origins.size === 1 && origins.has(origin), [...origins]);
  check('page-uses-no-bearer-or-session', browserCredentials.length === 0, browserCredentials);
  check('thumbnail-source-floor', [...thumbWidths].every(width => width === 0 || width >= 256), [...thumbWidths]);
  check('no-page-errors', errors.length === 0, errors); check('no-csp-violations', violations.length === 0, violations);
  check('candidate-delivery', !seen.candidate || seen.candidate.errors.length === 0, seen.candidate?.errors);
  seen.errors = errors; seen.csp = violations; seen.thumbnail_widths = [...thumbWidths]; sameService();
})().catch(async error => {
  failures.push({ during, error: clean(error.stack ?? error) });
  if (page && out) { await page.screenshot({ path: join(out, 'failure.png'), timeout: 5000 }).catch(() => {}); seen.failed_resources = await page.locator('.canvas-root').evaluate(element => element.canvasInspection?.resources()).catch(() => null); }
}).finally(async () => {
  await seen.candidate?.dispose?.().catch(() => {});
  if (seen.candidate?.errors.length) failures.push({ name: 'candidate-cleanup', detail: seen.candidate.errors });
  await browser?.close().catch(() => {});
  if (out) save('result.json', { failures, seen, receipt: out });
  process.stdout.write(`${clean(JSON.stringify({ failures, receipt: out, prepared: seen.prepared }, null, 2))}\n`);
  process.exitCode = failures.length ? 1 : 0;
});
