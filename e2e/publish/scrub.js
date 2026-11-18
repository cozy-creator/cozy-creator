// This repo is public, so what a run prints or uploads is published. This takes out what could act as someone, whoever printed it: Playwright's
// errors list a request's headers, a session's answer holds its token, a sign-in link its code. GitHub masks only its own secrets.
//   <command> 2>&1 | node scrub.js   line by line, ::error:: lines too. It outlasts a cancel, so a session's trap can still say it ended its pod.
//   node scrub.js <dir>...           every text file under them, in place: e2e/publish does this before any upload.
//   node scrub.js --text-only [--keep <glob>]... <dir>...   fail closed: also removes every file that is not UTF-8 text (a NUL in
//                                    its head, or invalid UTF-8) and every SQLite file (.db, .sqlite, -wal, -shm, -journal),
//                                    unless it matches a --keep glob (relative to its <dir>) and no rule matches its bytes. Each
//                                    file left out is a line of <dir>/scrub-left-out.tsv.
// A private key (`-----BEGIN … PRIVATE KEY-----`, OpenSSH's too) is never scrubbed in place: in a directory its file is left out,
// whatever the mode (listed, or said); a stream prints the block's first line as [scrubbed] and none of the rest.
const { closeSync, lstatSync, openSync, readFileSync, readSync, readdirSync, rmSync, writeFileSync } = require('fs');
const { join, relative } = require('path');
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
const BINARY_RULES = RULES.map(([rule]) => rule).map((rule) => (String(rule).includes('|cap|a)') ? /([#?&](?:code|media|token|cap)=)[^\s"'&<>]{8}/ : rule));
const PRIVATE_KEY = /-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----/;
const KEY_END = /-----END (?:[A-Z0-9]+ )*PRIVATE KEY-----/;
const scrubbed = (text) => RULES.reduce((said, [rule, instead]) => said.replace(rule, instead), text);

const MOST = 64 << 20;
// A file a running program removes while this walks (a SQLite -shm as its database closes) is gone, not a failure.
const gone = (error) => error.code === 'ENOENT';
// Text only: a binary's own strings aren't anyone's, and rewriting a binary breaks it.
const binary = (head) => head.includes(0);
const SQLITE = /(\.db|\.sqlite3?|-wal|-shm|-journal)$/;
const utf8 = (bytes) => {
  try {
    new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    return true;
  } catch {
    return false;
  }
};
const glob = (g) => new RegExp(`^${g.split('**').map((p) => p.split('*').map((q) => q.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('[^/]*')).join('.*')}$`);
let textOnly = false;
const keeps = [];
const left = [];
// out: a file the upload must not carry, removed and listed.
function out(path, rel, size, why) {
  rmSync(path, { force: true });
  left.push(`${rel}\t${size}\t${why}`);
}

function file(path, rel) {
  const stat = lstatSync(path, { throwIfNoEntry: false });
  if (!stat) return;
  const kept = keeps.some((k) => k.test(rel));
  if (textOnly && SQLITE.test(rel) && !kept) return out(path, rel, stat.size, 'a SQLite file');
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
      if (binary(head.subarray(0, read))) return textOnly ? out(path, rel, stat.size, `not text, over ${MOST} bytes`) : undefined;
      if (textOnly) return out(path, rel, stat.size, `text over ${MOST} bytes, not scrubbed`);
      rmSync(path, { force: true });
      console.log(`scrub: ${path} (${stat.size} bytes) is text over ${MOST} bytes: left out of the upload`);
      return;
    }
    bytes = readFileSync(path);
  } catch (error) {
    if (gone(error)) return;
    throw error;
  }
  const text = bytes.toString('latin1');
  if (binary(bytes.subarray(0, 8192)) || (textOnly && !utf8(bytes))) {
    if (!textOnly) return;
    if (!kept) return out(path, rel, stat.size, 'not text');
    // Named as evidence: it goes up as it is, only when nothing in it looks like what the rules take out. A link's one-letter
    // `a=` key is left to text: three fixed bytes turn up by chance about once in 10 MB of compressed pixels (a P-H3-Image
    // PNG did), where every other rule needs five or more.
    const hit = BINARY_RULES.find((rule) => text.search(rule) !== -1);
    if (hit) out(path, rel, stat.size, `kept as evidence, but its bytes match ${hit}`);
    else if (PRIVATE_KEY.test(text)) out(path, rel, stat.size, 'holds a private key');
    return;
  }
  if (PRIVATE_KEY.test(text)) {
    out(path, rel, stat.size, 'holds a private key');
    if (!textOnly) console.log(`scrub: ${path} holds a private key: left out of the upload`);
    return;
  }
  const clean = scrubbed(text);
  if (clean !== text) writeFileSync(path, Buffer.from(clean, 'latin1'));
}

function walk(dir, root) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch (error) {
    if (gone(error)) return;
    throw error;
  }
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path, root);
    else if (entry.isFile()) file(path, relative(root, path));
    // A link (or any other non-file) is never read here, and the upload would follow it: text only leaves it out.
    else if (textOnly) out(path, relative(root, path), 0, entry.isSymbolicLink() ? 'a link' : 'not a file');
  }
}

const args = process.argv.slice(2);
const dirs = [];
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--text-only') textOnly = true;
  else if (args[i] === '--keep') keeps.push(glob(args[++i]));
  else dirs.push(args[i]);
}
if (dirs.length) {
  // A suite that never started has nothing here.
  for (const dir of dirs) {
    if (!lstatSync(dir, { throwIfNoEntry: false })?.isDirectory()) continue;
    left.length = 0;
    walk(dir, dir);
    if (textOnly) {
      writeFileSync(join(dir, 'scrub-left-out.tsv'), `# left out of the upload: not scrubbable text (scrub.js --text-only)\n# path\tbytes\twhy\n${left.map((l) => `${l}\n`).join('')}`);
      console.log(`scrub: ${left.length} files left out of ${dir} (scrub-left-out.tsv)`);
    }
  }
} else {
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => {});
  let key = false;
  createInterface({ input: process.stdin, crlfDelay: Infinity }).on('line', (line) => {
    if (key || PRIVATE_KEY.test(line)) {
      if (!key) process.stdout.write(`${line.slice(0, line.search(PRIVATE_KEY))}${GONE} private key\n`);
      key = !KEY_END.test(line);
      return;
    }
    process.stdout.write(`${scrubbed(line)}\n`);
  });
}
