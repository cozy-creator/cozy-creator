// Instrument a verified copy of the actual 800 harness; no gate or product edits.
const { readFileSync, writeFileSync, copyFileSync } = require('node:fs');
const { createHash } = require('node:crypto');
const { join } = require('node:path');
const root = process.argv[2];
const path = join(root, 'e2e/cli/web/scale.js');
const original = readFileSync(path, 'utf8');
if (createHash('sha256').update(original).digest('hex') !== '42a59eb62da9e3a2f38656c14bc4ce4c574dd51b06e9a7cfb32e9ae444ed7726') throw new Error('scale harness does not match exact800');
const replacements = [
  ["const failures = [];", "const macDiagnostic = require('./mac-scale-memory.cjs');\nconst failures = [];"],
  ['function calciferMb() {', 'function calciferMb() {\n  macDiagnostic.beforeRss();'],
  ["return Math.round(Number(execFileSync('ps', ['-o', 'rss=', '-p', String(pid)], { encoding: 'utf8' }).trim()) / 1024);", "return macDiagnostic.afterRss(Math.round(Number(execFileSync('ps', ['-o', 'rss=', '-p', String(pid)], { encoding: 'utf8' }).trim()) / 1024));"],
  ['async function nativeMemory(base, folder) {', 'async function nativeMemory(base, folder, phase) {'],
  ['sample.finished_at_ms = Date.now();', 'sample.finished_at_ms = Date.now();\n  try { sample.diagnostic = await macDiagnostic.sample(base, folder, phase); }\n  catch { sample.diagnostic = { phase, error: "diagnostic unavailable; attribution unknown" }; }'],
  ['before: await nativeMemory(base, folder)', "before: await nativeMemory(base, folder, 'before')"],
  ...['read', 'open', 'edits', 'fitted', 'closed'].map((phase) => [`numbers.calcifer_native.${phase} = await nativeMemory(base, folder);`, `numbers.calcifer_native.${phase} = await nativeMemory(base, folder, '${phase}');`]),
];
let source = original;
for (const [from, to] of replacements) {
  if (source.split(from).length !== 2) throw new Error(`diagnostic injection is not unique: ${from}`);
  source = source.replace(from, to);
}
let reversed = source;
for (const [from, to] of [...replacements].reverse()) reversed = reversed.replace(to, from);
if (reversed !== original) throw new Error('diagnostic patch modified original behavior');
writeFileSync(path, source);
copyFileSync(join(__dirname, 'mac-scale-memory.cjs'), join(root, 'e2e/cli/web/mac-scale-memory.cjs'));
console.log(JSON.stringify({ source: '80077215ac26f2ac9337a7d4871327080d6d232a', original_scale_sha256: createHash('sha256').update(original).digest('hex'), instrumented_scale_sha256: createHash('sha256').update(source).digest('hex'), reverse_exact: true, bars_and_assertions_unchanged: true }));
