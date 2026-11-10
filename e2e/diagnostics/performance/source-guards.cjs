const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');
const refs = {
  baseline: '14d9c5fb407bc87e678d7d35d2aa9db66a2422f9',
  current: '811b924a138a1cb1fa61128d413279688c0d1b44',
};
const webTrees = { [refs.baseline]: 'e5bc5b1ca639a149166a438a627a3dcbb676bb99', [refs.current]: '79bb409cdd792f84e231c6910eaa7db7b3508288' };
const sha = value => crypto.createHash('sha256').update(value).digest('hex');
const git = (root, args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
function verify(root, ref, scope) {
  if (!Object.values(refs).includes(ref)) throw new Error('unexpected source ref');
  if (git(root, ['rev-parse', 'HEAD']) !== ref) throw new Error('checkout HEAD is not the pinned ref');
  if (scope === 'web' && git(root, ['rev-parse', `${ref}:web`]) !== webTrees[ref]) throw new Error('unexpected pinned web tree');
  const entries = execFileSync('git', ['-C', root, 'ls-tree', '-rz', ref, '--', scope], { encoding: 'utf8' }).split('\0').filter(Boolean);
  for (const entry of entries) {
    const split = entry.indexOf('\t'), [mode, type, blob] = entry.slice(0, split).split(' '), name = entry.slice(split + 1);
    if (type !== 'blob') throw new Error(`unsupported source entry ${name}`);
    const file = path.join(root, name);
    const bytes = mode === '120000' ? Buffer.from(fs.readlinkSync(file)) : fs.readFileSync(file);
    const actual = crypto.createHash('sha1').update(Buffer.from(`blob ${bytes.length}\0`)).update(bytes).digest('hex');
    if (actual !== blob) throw new Error(`source bytes drifted: ${name}`);
  }
  return { ref, scope, tree: git(root, ['rev-parse', `${ref}:${scope}`]), checked_blobs: entries.length };
}
function files(root) {
  const found = {};
  function walk(dir) { for (const entry of fs.readdirSync(dir, { withFileTypes: true })) { const at = path.join(dir, entry.name); if (entry.isDirectory()) walk(at); else if (entry.isFile()) found[path.relative(root, at)] = sha(fs.readFileSync(at)); else throw new Error('dist symlinks are not accepted'); } }
  walk(root); return found;
}
function verifyDist(root, expected) {
  const actual = files(root);
  if (JSON.stringify(Object.keys(actual).sort()) !== JSON.stringify(Object.keys(expected).sort())) throw new Error('dist file inventory drifted');
  for (const [name, hash] of Object.entries(expected)) if (actual[name] !== hash) throw new Error(`dist bytes drifted: ${name}`);
}
module.exports = { refs, webTrees, sha, git, verify, files, verifyDist };
