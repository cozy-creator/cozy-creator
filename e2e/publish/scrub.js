// This repo is public, so what a run prints or uploads is published. This takes out what could act as someone, whoever printed it: Playwright's
// errors list a request's headers, a session's answer holds its token, a sign-in link its code. GitHub masks only its own secrets.
//   <command> 2>&1 | node scrub.js   line by line, ::error:: lines too. It outlasts a cancel, so a session's trap can still say it ended its pod.
//   node scrub.js <dir>...           every text file under them, in place: e2e/publish does this before any upload.
const { lstatSync, readFileSync, readdirSync, writeFileSync } = require('fs');
const { join } = require('path');
const { createInterface } = require('readline');

const GONE = '[scrubbed]';
const RULES = [
  [/\b(Bearer|Cozy-Cap)(\s+|%20|=)[\w.~+/=-]{8,}/gi, `$1 ${GONE}`],
  [/(authorization["']?\s*[:=]\s*["']?)(?!Bearer|Cozy-Cap)[\w.~+/=-]{8,}/gi, `$1${GONE}`],
  [/([#?&](?:code|media|token|cap|a)=)[^\s"'&<>]{8,}/g, `$1${GONE}`],
  [/"(token|session_token|access_token|refresh_token|bearer|cap|capability|device_key)"\s*:\s*"[^"]{8,}"/g, `"$1":"${GONE}"`],
  [/"(code|media)"\s*:\s*"[0-9a-f]{32,}"/g, `"$1":"${GONE}"`],
  // A signed cap or a JWT, in a link or anywhere else.
  [/\beyJ[\w-]{8,}\.[\w-]{8,}(\.[\w-]+)?/g, GONE],
];
const scrubbed = (text) => RULES.reduce((said, [rule, instead]) => said.replace(rule, instead), text);

const MOST = 64 << 20;
function file(path) {
  if (lstatSync(path).size > MOST) return;
  const bytes = readFileSync(path);
  // Text only: a binary's own strings aren't anyone's, and rewriting a binary breaks it.
  if (bytes.subarray(0, 8192).includes(0)) return;
  const text = bytes.toString('latin1');
  const clean = scrubbed(text);
  if (clean !== text) writeFileSync(path, Buffer.from(clean, 'latin1'));
}

function walk(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path);
    else if (entry.isFile()) file(path);
  }
}

const dirs = process.argv.slice(2);
if (dirs.length) {
  // A suite that never started has nothing here.
  for (const dir of dirs) if (lstatSync(dir, { throwIfNoEntry: false })?.isDirectory()) walk(dir);
} else {
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => {});
  createInterface({ input: process.stdin, crlfDelay: Infinity }).on('line', (line) => process.stdout.write(`${scrubbed(line)}\n`));
}
