// The scale test's Darwin pressure measurement, not an application memory policy.
const { execFileSync } = require('node:child_process');
const { mkdtempSync, rmSync } = require('node:fs');
const { dirname, join } = require('node:path');

let sensor;

function validFootprint(sample) {
  return sample && Object.keys(sample).sort().join(',') === 'physical_bytes,pid,resident_bytes,start_abstime'
    && Number.isSafeInteger(sample.pid) && sample.pid > 0 && sample.pid <= 2147483647
    && typeof sample.start_abstime === 'string' && /^[1-9]\d*$/.test(sample.start_abstime)
    && BigInt(sample.start_abstime) <= 18446744073709551615n
    && Number.isSafeInteger(sample.physical_bytes) && sample.physical_bytes > 0
    && Number.isSafeInteger(sample.resident_bytes) && sample.resident_bytes > 0;
}

function parseFootprint(text) {
  const sample = JSON.parse(text);
  // The typed helper emits one fixed JSON record. Reject duplicate keys, extra output and other ambiguity.
  if (!validFootprint(sample) || JSON.stringify(sample) !== text.trim()) throw new Error('invalid native footprint observation');
  return sample;
}

function readFootprint(pid) {
  if (process.platform !== 'darwin') throw new Error('physical footprint observation is Darwin only');
  if (!Number.isSafeInteger(pid) || pid <= 0) throw new Error('invalid target PID');
  if (!sensor) {
    if (!process.env.LAST_LOOK) throw new Error('footprint observation needs the owned test output directory');
    const folder = mkdtempSync(join(dirname(process.env.LAST_LOOK), '.native-memory-'));
    const executable = join(folder, 'observe');
    try {
      execFileSync('cc', ['-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', join(__dirname, 'native-memory.c'), '-o', executable], { stdio: ['ignore', 'pipe', 'pipe'] });
    } catch {
      rmSync(folder, { recursive: true, force: true });
      throw new Error('native footprint observer compilation failed');
    }
    sensor = executable;
    process.once('exit', () => rmSync(folder, { recursive: true, force: true }));
  }
  let text;
  try { text = execFileSync(sensor, [String(pid)], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }); }
  catch { throw new Error('native footprint observation failed'); }
  const sample = parseFootprint(text);
  if (sample.pid !== pid) throw new Error('native footprint observer returned another PID');
  return sample;
}

function identity(sample) {
  const held = sample?.physical;
  if (!held || sample.physical_error || !validFootprint(held.before) || !validFootprint(held.after)) return null;
  if (held.before.pid !== held.holder_pid || held.after.pid !== held.after_holder_pid
    || held.before.pid !== held.after.pid || held.before.start_abstime !== held.after.start_abstime) return null;
  return held.after;
}

function closedProjectMemory(platform, memory, native, budgetMiB) {
  // Keep the existing resident-memory oracle on the other platforms, including its exact budget.
  if (platform !== 'darwin') return memory.closed <= memory.before + budgetMiB;
  if (!Number.isSafeInteger(budgetMiB) || budgetMiB < 0) return false;
  const phases = ['before', 'read', 'open', 'edits', 'fitted', 'closed'];
  const readings = phases.map((phase) => identity(native?.[phase]));
  if (phases.some((phase, i) => !readings[i] || native[phase].ownership_error || native[phase].heap_error
    || typeof native[phase].project_db_open !== 'boolean'
    || !Number.isSafeInteger(native[phase].heap_bytes) || native[phase].heap_bytes < 0
    || !Number.isFinite(memory?.[phase]) || memory[phase] < 0)) return false;
  const before = readings[0], closed = readings.at(-1);
  if (readings.some((sample) => sample.pid !== before.pid || sample.start_abstime !== before.start_abstime)) return false;
  if (native.before.project_db_open !== false || native.closed.project_db_open !== false) return false;
  return BigInt(closed.physical_bytes) <= BigInt(before.physical_bytes) + BigInt(budgetMiB) * 1048576n;
}

module.exports = { readFootprint, parseFootprint, closedProjectMemory };
