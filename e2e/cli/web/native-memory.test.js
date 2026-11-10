const assert = require('node:assert/strict');
const test = require('node:test');
const { parseFootprint, closedProjectMemory } = require('./native-memory');
const MiB = 1048576;
const start = '9007199254740992';

function observed(footprintMiB, heapBytes, open = false) {
  const record = { pid: 2229, start_abstime: start, physical_bytes: Math.round(footprintMiB * MiB), resident_bytes: 403 * MiB };
  return { heap_bytes: heapBytes, project_db_open: open, physical: { holder_pid: 2229, after_holder_pid: 2229, before: { ...record }, after: { ...record } } };
}
function diagnostic() {
  // Actual42858914504 values. vmmap rounded M values exercise the oracle; they are not C-sensor runtime proof.
  const phases = ['before', 'read', 'open', 'edits', 'fitted', 'closed'];
  const rss = [23, 236, 260, 431, 402, 403];
  const footprint = [11.1, 76.1, 178.0, 357.7, 222.7, 120.0];
  const heap = [4891545, 4895310, 124307758, 154001944, 91390842, 9248827];
  return { memory: Object.fromEntries(phases.map((phase, i) => [phase, rss[i]])),
    native: Object.fromEntries(phases.map((phase, i) => [phase, observed(footprint[i], heap[i], ['open', 'edits', 'fitted'].includes(phase))])) };
}

test('same six observations retain raw RSS while Darwin uses the unchanged charged-memory budget', () => {
  const { memory, native } = diagnostic();
  assert.deepEqual(Object.values(memory), [23, 236, 260, 431, 402, 403]);
  assert.equal(closedProjectMemory('darwin', memory, native, 256), true);
  assert.equal(closedProjectMemory('linux', memory, native, 256), false);
  assert.equal(closedProjectMemory('win32', memory, native, 256), false);
  assert.equal(native.closed.heap_bytes - native.before.heap_bytes, 4357282);
});

test('256MiB is inclusive; 257MiB charged growth is refused', () => {
  const { memory, native } = diagnostic();
  const before = native.before.physical.after.physical_bytes;
  native.closed.physical.after.physical_bytes = before + 256 * MiB;
  assert.equal(closedProjectMemory('darwin', memory, native, 256), true);
  native.closed.physical.after.physical_bytes = before + 257 * MiB;
  assert.equal(closedProjectMemory('darwin', memory, native, 256), false);
});

test('open or unknown ownership, unavailable counters and failed sensors never become closed', () => {
  for (const change of [
    (value) => { value.closed.project_db_open = true; },
    (value) => { value.closed.project_db_open = null; },
    (value) => { value.closed.ownership_error = 'lsof incomplete'; },
    (value) => { value.before.ownership_error = 'unknown baseline'; },
    (value) => { value.closed.physical_error = 'proc_pid_rusage failed'; },
    (value) => { delete value.closed.physical; },
    (value) => { value.closed.heap_bytes = null; },
    (value) => { value.closed.physical.after.physical_bytes = NaN; },
    (value) => { value.closed.physical.after.physical_bytes = Infinity; },
    (value) => { value.closed.physical.after.physical_bytes = Number.MAX_SAFE_INTEGER + 1; },
  ]) {
    const { memory, native } = diagnostic(); change(native);
    assert.equal(closedProjectMemory('darwin', memory, native, 256), false);
  }
});

test('PID/start continuity is exact, including adjacent uint64 values above Number precision', () => {
  for (const change of [
    (value) => { value.closed.physical.after.pid++; },
    (value) => { value.closed.physical.after_holder_pid++; },
    (value) => { value.closed.physical.before.start_abstime = '9007199254740993'; },
    (value) => { for (const sample of ['before', 'after']) value.closed.physical[sample].start_abstime = '9007199254740993'; },
    (value) => { value.closed.physical.after.start_abstime = Number(start); },
  ]) {
    const { memory, native } = diagnostic(); change(native);
    assert.equal(closedProjectMemory('darwin', memory, native, 256), false);
  }
});

test('every intermediate phase must have known observations and the same process identity', () => {
  for (const phase of ['before', 'read', 'open', 'edits', 'fitted', 'closed']) {
    for (const change of [
      (value) => { value.native[phase].physical_error = 'sensor failed'; },
      (value) => { value.native[phase].project_db_open = null; },
      (value) => { value.native[phase].ownership_error = 'uncertain census'; },
      (value) => { value.native[phase].heap_error = 'counter unavailable'; },
      (value) => { value.native[phase].heap_bytes = NaN; },
      (value) => { value.memory[phase] = NaN; },
      (value) => { delete value.native[phase]; },
    ]) {
      const value = diagnostic(); change(value);
      assert.equal(closedProjectMemory('darwin', value.memory, value.native, 256), false, phase);
    }
  }
  for (const phase of ['read', 'open', 'edits', 'fitted']) {
    const value = diagnostic();
    for (const sample of ['before', 'after']) value.native[phase].physical[sample].start_abstime = '9007199254740993';
    assert.equal(closedProjectMemory('darwin', value.memory, value.native, 256), false, phase);
  }
});

test('typed sensor record rejects malformed, ambiguous or unsafe observations', () => {
  const record = { pid: 2229, start_abstime: '18446744073709551615', physical_bytes: 120 * MiB, resident_bytes: 403 * MiB };
  assert.deepEqual(parseFootprint(JSON.stringify(record) + '\n'), record);
  for (const text of [
    '{}', 'unavailable', JSON.stringify({ ...record, physical_bytes: null }),
    JSON.stringify({ ...record, physical_bytes: -1 }), JSON.stringify({ ...record, physical_bytes: 0 }),
    JSON.stringify({ ...record, resident_bytes: Number.MAX_SAFE_INTEGER + 1 }),
    JSON.stringify({ ...record, start_abstime: '18446744073709551616' }),
    JSON.stringify({ ...record, start_abstime: Number(start) }),
    JSON.stringify({ ...record, extra: true }), JSON.stringify(record).replace('"pid":2229', '"pid":1,"pid":2229'),
  ]) assert.throws(() => parseFootprint(text));
});

test('other OSes keep their RSS decision without requiring a Darwin sensor', () => {
  for (const platform of ['linux', 'win32']) {
    assert.equal(closedProjectMemory(platform, { before: 100, closed: 356 }, undefined, 256), true);
    assert.equal(closedProjectMemory(platform, { before: 100, closed: 357 }, undefined, 256), false);
    assert.equal(closedProjectMemory(platform, { before: 100, closed: NaN }, undefined, 256), false);
  }
});
