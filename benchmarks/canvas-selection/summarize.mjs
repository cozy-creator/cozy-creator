import { readdirSync, readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { basename, resolve } from 'node:path';
import { createHash } from 'node:crypto';

const MODES = ['reactflow', 'react-dom', 'pixi', 'leafer', 'pixi-webgpu', 'gpucanvas'];
const EXPECTED = new Map([[1000, 1], [5000, 3]]);
const CASE_NAME = /^(.+)-(reactflow|react-dom|pixi-webgpu|pixi|leafer|gpucanvas)-n(\d+)-dpr(\d+(?:\.\d+)?)-r(\d+)\.json$/;
const HELP = 'Usage: node summarize.mjs --dirs=DIR1,DIR2 --out=OUTPUT_DIRECTORY\nReads case JSON files directly inside only those directories. No recursion or browser execution.';

function argumentsOf(values) {
  const result = { dirs: [], out: null, help: false };
  for (let index = 0; index < values.length; index++) {
    const value = values[index];
    if (value === '--help' || value === '-h') { result.help = true; continue; }
    if (value === '--dirs' || value.startsWith('--dirs=')) {
      const paths = value.startsWith('--dirs=') ? [value.slice(7)] : [];
      if (!paths.length) while (values[index + 1] && !values[index + 1].startsWith('--')) paths.push(values[++index]);
      result.dirs.push(...paths.flatMap(path => path.split(',')).filter(Boolean));
    } else if (value === '--out') result.out = values[++index];
    else if (value.startsWith('--out=')) result.out = value.slice(6);
    else throw new Error(`Unknown argument: ${value}\n${HELP}`);
  }
  return result;
}

const finite = value => typeof value === 'number' && Number.isFinite(value);
const numeric = value => finite(value) ? value : null;
const stable = object => JSON.stringify(Object.fromEntries(Object.entries(object).sort(([a], [b]) => a.localeCompare(b))));
const digest = object => createHash('sha256').update(stable(object)).digest('hex');
const countOf = values => Object.fromEntries([...new Set(values)].sort().map(value => [value, values.filter(item => item === value).length]));

function renderDuty(record) {
  const engines = record.active?.gpuEngineDutyPercent;
  if (!engines || typeof engines !== 'object') return null;
  const values = Object.entries(engines).filter(([name, value]) => name.endsWith(':render') && finite(value)).map(([, value]) => value);
  return values.length ? values.reduce((sum, value) => sum + value, 0) : null;
}

const METRICS = {
  activeCpuCores: { unit: 'one-core equivalents', read: record => finite(record.active?.cpuOneCorePercent) ? record.active.cpuOneCorePercent / 100 : null },
  idleCpuCores: { unit: 'one-core equivalents', read: record => finite(record.idle?.cpuOneCorePercent) ? record.idle.cpuOneCorePercent / 100 : null },
  frameP95Ms: { unit: 'ms', read: record => numeric(record.tour?.frameMs?.p95) },
  frameP99Ms: { unit: 'ms', read: record => numeric(record.tour?.frameMs?.p99) },
  traversalPssMiB: { unit: 'MiB', read: record => numeric(record.traversalMemory?.pssMiB) },
  gpuResidentClientAggregateMiB: { unit: 'MiB', read: record => numeric(record.traversalMemory?.gpu?.residentClientAggregateMiB) },
  gpuSharedClientAggregateMiB: { unit: 'MiB', read: record => numeric(record.traversalMemory?.gpu?.sharedClientAggregateMiB) },
  gpuPrivateResidentLowerBoundMiB: { unit: 'MiB', read: record => numeric(record.traversalMemory?.gpu?.privateResidentLowerBoundMiB) },
  gpuRenderEngineDutyPercent: { unit: '% summed recorded render-engine duty', read: renderDuty },
  coldReadyMs: { unit: 'ms', read: record => numeric(record.coldReadyMs) },
  initialUniqueThumbnailRequests: { unit: 'distinct source URLs', read: record => numeric(record.initialUniqueThumbnailRequests) },
  initialVisible: { unit: 'cards', read: record => numeric(record.initialReady?.visible) },
  initialLiveDomNodes: { unit: 'elements in document', read: record => numeric(record.initialReady?.domNodes) },
  initialCdpNodes: { unit: 'CDP nodes, including detached nodes', read: record => numeric(record.loadedMemory?.nodes) },
  traversalCdpNodes: { unit: 'CDP nodes, including detached nodes', read: record => numeric(record.traversalMemory?.nodes) },
  finalLiveDomNodes: { unit: 'elements in document', read: record => numeric(record.final?.domNodes) },
  finalCdpNodes: { unit: 'CDP nodes, including detached nodes', read: record => numeric(record.settledMemory?.nodes) },
  destroyedCdpNodes: { unit: 'CDP nodes, including detached nodes', read: record => numeric(record.destroyedMemory?.nodes) },
};

function inspectCase(record, file, name) {
  const reasons = [];
  const mode = record.mode ?? name[2], n = record.n ?? Number(name[3]);
  if (record.key !== basename(file, '.json') || record.mode !== name[2] || record.n !== Number(name[3])) reasons.push('filename/case identity mismatch');
  if (!MODES.includes(mode) || !EXPECTED.has(n)) reasons.push('outside the requested final matrix');
  if (record.status !== 'pass') reasons.push(`status=${record.status ?? 'missing'}`);
  if (record.initialPixelGate?.pass !== true) reasons.push('displayed-photo pixel gate missing or failing');
  if (record.initialReady?.pass !== true || record.afterTourReady?.pass !== true) reasons.push('initial/after-tour readiness missing or failing');
  const visible = record.initialReady?.visible, requests = record.initialUniqueThumbnailRequests, limit = record.startupAdmissionLimit;
  const admissionPass = finite(visible) && visible > 0 && finite(requests) && requests >= 0 && finite(limit)
    && requests <= limit && requests <= visible * 2 + 8 && limit <= visible * 2 + 8;
  if (!admissionPass) reasons.push('source admission missing or failing');
  if (record.headless !== true) reasons.push('not an explicitly headless receipt');
  const params = new URLSearchParams(record.query ?? '');
  if (params.get('floor') !== '256' || params.get('view') !== 'working') reasons.push('not the production-floor working-view cohort');
  const adapter = record.initialReady?.adapter ?? {};
  if (mode === 'reactflow' && (adapter.rfMode !== 'optimized' || adapter.rfLayer !== 1 || adapter.rfHandles !== 'empty')) reasons.push('React Flow is not optimized + measured empty-port/layer-hint control');
  if (mode === 'gpucanvas' && (adapter.sourceRetentionPolicy !== 'budget' || stable({ dimensions: adapter.nominalAtlasDimensions }) !== stable({ dimensions: [4096, 4096, 5] }))) reasons.push('GPUCanvas is not budget-retention/default-atlas control');
  const hashes = record.sourceHashes;
  const provenancePass = typeof record.sourceCommit === 'string' && /^[0-9a-f]{40}$/i.test(record.sourceCommit)
    && hashes && typeof hashes === 'object' && Object.keys(hashes).length > 0
    && Object.values(hashes).every(hash => typeof hash === 'string' && /^[0-9a-f]{64}$/i.test(hash));
  if (!provenancePass) reasons.push('source commit/hash manifest missing or malformed');
  const metrics = Object.fromEntries(Object.entries(METRICS).map(([key, metric]) => [key, metric.read(record)]));
  return {
    file, key: record.key ?? basename(file, '.json'), mode, n, repeat: record.repeat,
    qualified: reasons.length === 0, reasons, status: record.status ?? null, admissionPass,
    sourceCommit: record.sourceCommit ?? null, sourceHashes: hashes ?? null,
    sourceHashSetDigest: provenancePass ? digest(hashes) : null,
    workload: { config: record.config, dpr: record.dpr, query: record.query, tourMs: record.tourMs, width: record.initialReady?.width, height: record.initialReady?.height },
    adapter: { backend: adapter.backend, rfMode: adapter.rfMode, rfLayer: adapter.rfLayer, rfHandles: adapter.rfHandles, sourceRetentionPolicy: adapter.sourceRetentionPolicy, nominalAtlasDimensions: adapter.nominalAtlasDimensions, cacheSourceBudgetBytes: adapter.cacheSourceBudgetBytes },
    metrics, missingMetrics: Object.keys(metrics).filter(key => metrics[key] === null),
    gpuEngineDutyPercent: record.active?.gpuEngineDutyPercent ?? null,
    pageErrors: record.pageErrors ?? [], consoleErrors: record.consoleErrors ?? [], error: record.error ?? null,
  };
}

function aggregate(values) {
  const sorted = values.filter(finite).sort((a, b) => a - b);
  if (!sorted.length) return { count: 0, median: null, min: null, max: null };
  const middle = Math.floor(sorted.length / 2);
  return { count: sorted.length, median: sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2, min: sorted[0], max: sorted.at(-1) };
}

function provenanceOf(cases) {
  const commits = countOf(cases.map(item => item.sourceCommit));
  const hashSets = countOf(cases.map(item => item.sourceHashSetDigest));
  const files = [...new Set(cases.flatMap(item => Object.keys(item.sourceHashes ?? {})))].sort();
  const perFile = Object.fromEntries(files.map(file => [file, countOf(cases.map(item => item.sourceHashes?.[file] ?? '(missing)'))]));
  return {
    consistent: cases.length > 0 && Object.keys(commits).length === 1 && Object.keys(hashSets).length === 1,
    commitConsistent: Object.keys(commits).length === 1, hashSetConsistent: Object.keys(hashSets).length === 1,
    commits, hashSetDigests: hashSets, perFileHashes: perFile,
    differingFiles: files.filter(file => Object.keys(perFile[file]).length !== 1),
    scope: 'Only the source hashes recorded in receipts; not a new hash of the current worktree or fixture/browser binaries.',
  };
}

function csvCell(value) {
  const text = value === null || value === undefined ? '' : String(value);
  return /[",\r\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

function markdown(summary) {
  const lines = ['# Canonical canvas benchmark summary', '',
    `Input directories: ${summary.directories.map(path => `\`${path}\``).join(', ')}. Direct case files only; no recursive scans.`, '',
    `Qualified samples: **${summary.cases.filter(item => item.qualified).length}**. Matrix complete: **${summary.matrixComplete}**. Source consistent: **${summary.provenance.consistent}**. Workload consistent: **${summary.workloadConsistent}**.`, '',
    `Source commits: ${Object.entries(summary.provenance.commits).map(([sha, count]) => `\`${sha}\` (${count})`).join(', ') || 'none'}. Recorded source-hash sets: ${Object.keys(summary.provenance.hashSetDigests).length}.`, '',
    'Values are median [min, max] across qualified receipts; a single sample is shown as one value. Missing measurements are never replaced with zero. These are not confidence intervals.', ''];
  function value(group, key, digits = 2) {
    const stats = group.metrics[key];
    if (!stats.count) return '—';
    const format = number => number.toFixed(digits);
    const text = stats.count === 1 ? format(stats.median) : `${format(stats.median)} [${format(stats.min)}, ${format(stats.max)}]`;
    return stats.count === group.samples ? text : `${text} (${stats.count}/${group.samples})`;
  }
  const tables = [
    { title: 'Traversal timing and CPU', columns: [['Active CPU cores', 'activeCpuCores'], ['Idle CPU cores', 'idleCpuCores'], ['Frame p95 ms', 'frameP95Ms'], ['Frame p99 ms', 'frameP99Ms'], ['GPU render duty %', 'gpuRenderEngineDutyPercent'], ['Cold ready ms', 'coldReadyMs', 0]] },
    { title: 'Post-traversal memory: separate accounting domains', columns: [['Browser PSS MiB', 'traversalPssMiB', 1], ['GPU resident-client MiB', 'gpuResidentClientAggregateMiB', 1], ['GPU shared-client MiB', 'gpuSharedClientAggregateMiB', 1], ['Private-resident lower bound MiB', 'gpuPrivateResidentLowerBoundMiB', 1]] },
    { title: 'Admission and node counts', columns: [['Initial distinct URLs', 'initialUniqueThumbnailRequests', 0], ['Initial visible cards', 'initialVisible', 0], ['Initial live DOM', 'initialLiveDomNodes', 0], ['Initial CDP nodes', 'initialCdpNodes', 0], ['Traversal CDP nodes', 'traversalCdpNodes', 0], ['Final live DOM', 'finalLiveDomNodes', 0], ['Final CDP nodes', 'finalCdpNodes', 0], ['Destroyed CDP nodes', 'destroyedCdpNodes', 0]] },
  ];
  for (const table of tables) {
    lines.push(`## ${table.title}`, '', `| Cards | Mode | Samples / expected | Status | ${table.columns.map(([label]) => label).join(' | ')} |`, `| --- | --- | --- | --- | ${table.columns.map(() => '---').join(' | ')} |`);
    for (const group of summary.groups) lines.push(`| ${group.n} | ${group.mode} | ${group.samples}/${group.expectedSamples} | ${group.status} | ${table.columns.map(([, key, digits]) => value(group, key, digits)).join(' | ')} |`);
    lines.push('');
  }
  lines.push('## Interpretation limits', '', ...summary.notes.map(note => `- ${note}`), '', '## Missing, rejected, or inconsistent evidence', '');
  if (!summary.issues.length && !summary.rejected.length && summary.groups.every(group => group.status === 'complete')) lines.push('None in the specified matrix.');
  for (const issue of summary.issues) lines.push(`- ${issue}`);
  for (const group of summary.groups.filter(group => group.status !== 'complete')) lines.push(`- n=${group.n}, ${group.mode}: ${group.status}; ${group.samples}/${group.expectedSamples} qualified; missing metrics: ${group.missingMetrics.join(', ') || 'none'}.`);
  for (const item of summary.rejected) lines.push(`- \`${item.file}\`: ${item.reasons.join('; ')}${item.error ? `; ${String(item.error).replaceAll('\n', ' ')}` : ''}`);
  return lines.join('\n') + '\n';
}

function main() {
  const args = argumentsOf(process.argv.slice(2));
  if (args.help) { console.log(HELP); return; }
  if (!args.dirs.length || !args.out) throw new Error(HELP);
  const directories = [...new Set(args.dirs.map(path => resolve(path)))], out = resolve(args.out);
  const cases = [], ignored = [], issues = [], seenKeys = new Set();
  for (const directory of directories) {
    let entries;
    try { entries = readdirSync(directory, { withFileTypes: true }); }
    catch (error) { issues.push(`Cannot read specified directory ${directory}: ${error.message}`); continue; }
    for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
      const match = CASE_NAME.exec(entry.name);
      if (!entry.isFile() || !match) { ignored.push({ file: resolve(directory, entry.name), reason: 'not a direct case JSON file' }); continue; }
      const file = resolve(directory, entry.name);
      try {
        const item = inspectCase(JSON.parse(readFileSync(file, 'utf8')), file, match);
        if (seenKeys.has(item.key)) { item.qualified = false; item.reasons.push('duplicate case identity; not counted twice'); }
        else seenKeys.add(item.key);
        cases.push(item);
      } catch (error) { issues.push(`Cannot parse case ${file}: ${error.message}`); }
    }
  }
  const qualified = cases.filter(item => item.qualified), provenance = provenanceOf(qualified);
  const workloadVariants = countOf(qualified.map(item => stable(item.workload)));
  const workloadConsistent = qualified.length > 0 && Object.keys(workloadVariants).length === 1;
  if (!provenance.consistent) issues.push('Source commits or recorded hash manifests differ/missing; cross-mode comparison is not qualified.');
  if (!workloadConsistent) issues.push('Hardware/display/query/tour workload settings differ/missing; cross-mode comparison is not qualified.');
  const groups = [];
  for (const [n, expectedSamples] of EXPECTED) for (const mode of MODES) {
    const accepted = qualified.filter(item => item.n === n && item.mode === mode);
    const rejected = cases.filter(item => item.n === n && item.mode === mode && !item.qualified);
    const metrics = Object.fromEntries(Object.keys(METRICS).map(key => [key, aggregate(accepted.map(item => item.metrics[key]))]));
    const missingMetrics = Object.keys(metrics).filter(key => metrics[key].count !== accepted.length || !accepted.length);
    let status = accepted.length === expectedSamples ? 'complete' : !accepted.length ? 'missing' : accepted.length < expectedSamples ? 'incomplete' : 'unexpected-extra-samples';
    if (rejected.length) status += '+rejected';
    if (missingMetrics.length && accepted.length) status += '+missing-metrics';
    if (!provenance.consistent || !workloadConsistent) status += '+inconsistent-cohort';
    groups.push({ n, mode, samples: accepted.length, expectedSamples, rejectedSamples: rejected.length, status,
      files: accepted.map(item => item.file), sourceCommits: [...new Set(accepted.map(item => item.sourceCommit))],
      sourceHashSetDigests: [...new Set(accepted.map(item => item.sourceHashSetDigest))], missingMetrics, metrics });
  }
  const summary = {
    generated: new Date().toISOString(), directories, nonrecursive: true,
    qualification: 'status pass + initial/after-tour readiness + displayed-photo pixel pass + bounded source admission + explicitly headless canonical working-floor controls + recorded source provenance',
    expectedSamples: Object.fromEntries(EXPECTED), expectedModes: MODES,
    matrixComplete: groups.every(group => group.status === 'complete') && !issues.length,
    provenance, workloadConsistent, workloadVariants,
    metricDefinitions: Object.fromEntries(Object.entries(METRICS).map(([key, definition]) => [key, definition.unit])),
    notes: [
      'CPU cores = recorded cpuOneCorePercent / 100, summing browser-owned process CPU; not percent of all system cores.',
      'Headless 60 Hz frame intervals and p95/p99 scheduling intervals are not maximum FPS, physical presentation time, or input-to-photon latency.',
      'Each table entry aggregates per-run statistics; a median of run p95/p99 values is not a pooled percentile.',
      'Traversal memory is after tour readiness and the recorded post-traversal pause, before interaction/editor/model demonstrations.',
      'Browser PSS and DRM GPU resident/shared-client accounting overlap on integrated/shared-memory hardware: do not add them. Shared GPU buffers may appear in more than one client aggregate.',
      'Private-resident lower bound is max(0, resident-client aggregate minus shared-client aggregate), not exact physical VRAM or an upper bound.',
      'GPU render duty sums the recorded :render engine entries. It is not a universal GPU utilization or shader-cost measure and can exceed 100% across devices.',
      'CDP node counts can include detached nodes; document element counts are reported separately. Missing metrics remain null, not zero.',
      'Primary 5,000-card groups expect three qualified repeats per mode; 1,000-card groups expect one observation, not a replicated performance estimate.',
      'React Flow here is optimized uncontrolled + measured empty-port metadata + explicit layer hint. GPUCanvas uses budget source retention with the default 4096×4096×5 atlas.',
    ],
    groups, cases, rejected: cases.filter(item => !item.qualified), ignored, issues,
  };
  mkdirSync(out, { recursive: true });
  const columns = ['n', 'mode', 'samples', 'expectedSamples', 'rejectedSamples', 'status', 'sourceCommits', 'sourceHashSetDigests', 'sourceConsistent', 'workloadConsistent', ...Object.keys(METRICS).flatMap(key => ['median', 'min', 'max', 'count'].map(stat => `${key}_${stat}`))];
  const rows = groups.map(group => Object.fromEntries([
    ...columns.slice(0, 6).map(key => [key, group[key]]),
    ['sourceCommits', group.sourceCommits.join('|')], ['sourceHashSetDigests', group.sourceHashSetDigests.join('|')],
    ['sourceConsistent', provenance.consistent], ['workloadConsistent', workloadConsistent],
    ...Object.keys(METRICS).flatMap(key => ['median', 'min', 'max', 'count'].map(stat => [`${key}_${stat}`, group.metrics[key][stat]])),
  ]));
  writeFileSync(resolve(out, 'summary.csv'), [columns.join(','), ...rows.map(row => columns.map(key => csvCell(row[key])).join(','))].join('\n') + '\n');
  writeFileSync(resolve(out, 'summary.json'), JSON.stringify(summary, null, 2) + '\n');
  writeFileSync(resolve(out, 'summary.md'), markdown(summary));
  console.log(JSON.stringify({ out, qualifiedCases: qualified.length, rejectedCases: summary.rejected.length, matrixComplete: summary.matrixComplete, sourceConsistent: provenance.consistent, workloadConsistent, issues }));
  if (!summary.matrixComplete) process.exitCode = 1;
}

try { main(); }
catch (error) { console.error(error.message); process.exitCode = 1; }
