// Disposable xs159 diagnostic only. Read metadata/summaries, never memory contents.
const { spawnSync } = require('node:child_process');
const { readFileSync } = require('node:fs');
const { createHash } = require('node:crypto');
const { basename } = require('node:path');

let rssSample;
const clean = (text) => text.replace(/.\x08/g, '').replace(/0x[0-9a-f]+/gi, '<address>');
function run(command, args, timeout = 20000) {
  const began = Date.now();
  try {
    const result = spawnSync(command, args, { encoding: 'utf8', timeout, maxBuffer: 16 << 20, stdio: ['ignore', 'pipe', 'pipe'] });
    if (result.error || result.status !== 0) throw Object.assign(new Error('probe unavailable'), { status: result.status, signal: result.signal, code: result.error?.code });
    return { ok: true, ms: Date.now() - began, text: result.stdout, warning: Boolean(result.stderr.trim()) };
  } catch (error) {
    // Do not serialize arbitrary stderr from a process-memory tool.
    return { ok: false, ms: Date.now() - began, exit: error.status, signal: error.signal, code: error.code, error: 'probe unavailable; attribution unknown' };
  }
}
function holder() {
  const { pid, address } = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  if (!Number.isSafeInteger(pid) || !/^127\.0\.0\.1:\d+$/.test(address)) throw new Error('unexpected holder identity');
  return { pid, address };
}
function identity() {
  const before = Date.now();
  const held = holder();
  const start = run('/bin/ps', ['-o', 'lstart=', '-p', String(held.pid)]);
  const executable = run('/bin/ps', ['-o', 'comm=', '-p', String(held.pid)]);
  const listener = run('/usr/sbin/lsof', ['-nP', `-iTCP:${held.address.split(':')[1]}`, '-sTCP:LISTEN', '-Fp']);
  const listenerPids = listener.ok ? [...listener.text.matchAll(/^p(\d+)$/gm)].map((match) => Number(match[1])) : null;
  return { ...held, at_ms: before, finished_at_ms: Date.now(), start: start.ok ? start.text.trim() : null,
    executable: executable.ok ? executable.text.trim() : null, listener_pids: listenerPids,
    listener_known: listener.ok, listener_matches_holder: listenerPids?.length === 1 && listenerPids[0] === held.pid,
    probe_ms: { start: start.ms, executable: executable.ms, listener: listener.ms } };
}
const same = (a, b) => a.pid === b.pid && a.address === b.address && a.start !== null && a.start === b.start && a.listener_matches_holder && b.listener_matches_holder;
exports.beforeRss = () => { rssSample = { before: identity() }; };
exports.afterRss = (mb) => { rssSample.rss_mb = mb; rssSample.after = identity(); rssSample.continuous = same(rssSample.before, rssSample.after); return mb; };

function projectFile(path) {
  const part = path.split('/.cozy/projects/')[1];
  if (!part) return null;
  const leaf = basename(path);
  return { project: part.split('/')[0], kind: /^project\.db(?:-wal|-shm)?$/.test(leaf) ? leaf : (leaf.includes('.') ? leaf.split('.').at(-1) : 'no-extension'), path_sha256: createHash('sha256').update(path).digest('hex') };
}
function fds(pid) {
  const answer = run('/usr/sbin/lsof', ['-nP', '-p', String(pid), '-Fpftn']);
  if (!answer.ok) return answer;
  const records = [], pids = [], unparsed = [];
  let record;
  for (const line of answer.text.split('\n')) {
    if (line[0] === 'p') pids.push(Number(line.slice(1)));
    else if (line[0] === 'f') { record = { fd: line.slice(1) }; records.push(record); }
    else if (record && line[0] === 't') record.type = line.slice(1);
    else if (record && line[0] === 'n') record.name = line.slice(1);
    else if (line.trim()) unparsed.push(line[0]);
  }
  const types = {}, projects = [], sockets = [];
  for (const record of records) {
    types[record.type ?? 'unknown'] = (types[record.type ?? 'unknown'] ?? 0) + 1;
    const project = projectFile(record.name ?? '');
    if (project) projects.push({ fd: record.fd, ...project });
    if (['IPv4', 'IPv6'].includes(record.type)) sockets.push({ fd: record.fd, type: record.type, loopback: /127\.0\.0\.1|\[::1\]/.test(record.name ?? '') });
  }
  const protocolComplete = records.length > 0 && pids.length === 1 && pids[0] === pid && unparsed.length === 0 && records.every(({ type, name }) => type !== undefined && name !== undefined);
  return { ok: true, scope: 'all descriptor records reported by successful lsof for this PID', complete: protocolComplete && !answer.warning, protocol_complete: protocolComplete, unparsed_field_count: unparsed.length, missing_fields: records.filter(({ type, name }) => type === undefined || name === undefined).length, warning: answer.warning, ms: answer.ms, records: records.length, numeric_fds: records.filter(({ fd }) => /^\d/.test(fd)).length, types, projects, sockets };
}

const manuals = {};
for (const name of ['vmmap', 'heap']) {
  const result = run('/usr/bin/man', ['-P', 'cat', name]);
  manuals[name] = { ok: result.ok, ms: result.ms, text: result.ok ? clean(result.text) : '' };
}
exports.sample = async (base, folder, phase) => {
  const began = Date.now(), before = identity();
  const result = { phase, at_ms: began, before, rss: rssSample, original_base_matches_holder: base === `http://${before.address}`, project: basename(folder), fds: fds(before.pid), logical_readers: 'not exposed by this binary; FD/maps census cannot enumerate Reader objects' };
  // vmmap default output is region metadata only, retained solely as counts/path membership.
  const maps = run('/usr/bin/vmmap', ['-w', String(before.pid)]);
  if (maps.ok) {
    const categories = {}, projects = [];
    let regions = 0, candidates = 0;
    for (const line of maps.text.split('\n')) {
      if (/\b(?:0x)?[0-9a-fA-F]{6,}-(?:0x)?[0-9a-fA-F]{6,}\b/.test(line)) candidates++;
      const match = line.match(/^(.+?)\s+(?:0x)?[0-9a-fA-F]+-(?:0x)?[0-9a-fA-F]+\s+\[([^\]]+)\]/);
      if (!match) continue;
      regions++;
      const category = match[1].trim();
      categories[category] = (categories[category] ?? 0) + 1;
      if (line.includes('/.cozy/projects/')) {
        const path = line.slice(line.indexOf('/Users/')).trim();
        const project = projectFile(path);
        if (project) projects.push({ category, sizes: match[2].trim(), ...project });
      }
    }
    result.maps = { ok: true, ms: maps.ms, parsed_regions: regions, candidate_regions: candidates, warning: maps.warning, complete: null, scope: 'all returned vmmap -w metadata inspected in memory; full VM/submap coverage not independently validated', matched_rows_parsed: regions > 0 && regions === candidates && !maps.warning, categories, projects };
  } else result.maps = maps;
  const summaryAllowed = manuals.vmmap.ok && manuals.vmmap.text.includes('-summary');
  const summary = summaryAllowed ? run('/usr/bin/vmmap', ['-summary', String(before.pid)]) : { ok: false, error: 'runner manual does not verify -summary; attribution unknown' };
  result.vm_summary = summary.ok ? { ok: true, ms: summary.ms, warning: summary.warning, complete: summary.warning ? false : null, lines: clean(summary.text).split('\n').filter((line) => !/Process:|Path:|Identifier:|Binary Images|Load Address/.test(line)) } : summary;
  const heapAllowed = manuals.heap.ok && ['-noContent', '-zones', '-sortBySize'].every((flag) => manuals.heap.text.includes(flag));
  const heap = heapAllowed ? run('/usr/bin/heap', ['-s', '-z', '-noContent', String(before.pid)]) : { ok: false, error: 'runner manual does not verify summary/no-content flags; attribution unknown' };
  result.malloc_summary = heap.ok ? { ok: true, ms: heap.ms, warning: heap.warning, complete: heap.warning ? false : null, lines: clean(heap.text).split('\n').filter((line) => !/^Process:|^Path:|^Load Address/.test(line)) } : heap;
  result.verified_flags = { vm_summary: summaryAllowed, heap_summary_no_content: heapAllowed };
  result.after = identity();
  result.continuous = same(before, result.after) && result.rss?.continuous && same(result.rss.after, before);
  result.finished_at_ms = Date.now();
  result.probe_duration_ms = Date.now() - began;
  return result;
};
