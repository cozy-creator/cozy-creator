// This repo is public, so what a run prints or uploads is published. This takes out what could act as someone, whoever printed it: Playwright's
// errors list a request's headers, a session's answer holds its token, a sign-in link its code. GitHub masks only its own secrets.
//   <command> 2>&1 | node scrub.js   line by line, ::error:: lines too. It outlasts a cancel, so a session's trap can still say it ended its pod.
//   node scrub.js <dir>...           every text file under them, in place: e2e/publish does this before any upload.
const { closeSync, lstatSync, openSync, readFileSync, readSync, readdirSync, rmSync, writeFileSync } = require('fs');
const { join } = require('path');
const { createInterface } = require('readline');

const GONE = '[scrubbed]';
// `X{n}X*`, never `X{n,}`: V8 counts a `{n,}` loop on its backtrack stack, an entry a character, and a 5.6M-character run
// overflowed it (e2e 44346142807, cpu 3/3); a `*` loop backtracks by position. `\s*[:=]?\s*` tried every split of a space run.
const RULES = [
  [/\b(Bearer|Cozy-Cap)(\s+|%20|=)[\w.~+/=-]{8}[\w.~+/=-]*/gi, `$1 ${GONE}`],
  [/(authorization["']?\s*[:=]\s*["']?)(?!Bearer|Cozy-Cap)[\w.~+/=-]{8}[\w.~+/=-]*/gi, `$1${GONE}`],
  [/([#?&](?:code|media|token|cap|a)=)[^\s"'&<>]{8}[^\s"'&<>]*/g, `$1${GONE}`],
  [/"(token|session_token|access_token|refresh_token|bearer|cap|capability|device_key)"\s*:\s*"[^"]{8}[^"]*"/g, `"$1":"${GONE}"`],
  [/"(code|media)"\s*:\s*"[0-9a-f]{32}[0-9a-f]*"/g, `"$1":"${GONE}"`],
  // A signed cap or a JWT, in a link or anywhere else.
  [/\beyJ[\w-]{8}[\w-]*\.[\w-]{8}[\w-]*(\.[\w-]+)?/g, GONE],
  [/\bcap1\.[\w.-]{40}[\w.-]*/g, GONE],
  // Not a credential, but a person's: whoami's answers and its printed record.
  [/\b(email["']?\s*(?:[:=]\s*)?["']?)[\w.+-]+@[\w.-]+/gi, `$1${GONE}`],
];
const scrubbed = (text) => RULES.reduce((said, [rule, instead]) => said.replace(rule, instead), text);

const MOST = 64 << 20;
// A file a running program removes while this walks (a SQLite -shm as its database closes) is gone, not a failure.
const gone = (error) => error.code === 'ENOENT';
// Text only: a binary's own strings aren't anyone's, and rewriting a binary breaks it.
const binary = (head) => head.includes(0);

function file(path) {
  const stat = lstatSync(path, { throwIfNoEntry: false });
  if (!stat) return;
  let bytes;
  try {
    if (stat.size > MOST) {
      // Too big to read whole: a text file not scrubbed doesn't go up.
      const head = Buffer.alloc(8192);
      const fd = openSync(path, 'r');
      let read;
      try {
        read = readSync(fd, head, 0, head.length, 0);
      } finally {
        closeSync(fd);
      }
      if (binary(head.subarray(0, read))) return;
      rmSync(path, { force: true });
      console.log(`scrub: ${path} (${stat.size} bytes) is text over ${MOST} bytes: left out of the upload`);
      return;
    }
    bytes = readFileSync(path);
  } catch (error) {
    if (gone(error)) return;
    throw error;
  }
  if (binary(bytes.subarray(0, 8192))) return;
  const text = bytes.toString('latin1');
  const clean = scrubbed(text);
  if (clean !== text) writeFileSync(path, Buffer.from(clean, 'latin1'));
}

function walk(dir) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch (error) {
    if (gone(error)) return;
    throw error;
  }
  for (const entry of entries) {
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
