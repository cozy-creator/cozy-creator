// This test's browser's sockets to Calcifer, read from the runner (Linux: ss and /proc): what a pinned connection looks like from outside
// the page, whatever the page or the browser says about its requests (cl-1220; web/pieces.spec.js, web/windows.spec.js).
const { execFileSync } = require('child_process');
const { readdirSync, readFileSync } = require('fs');

/** The processes under this one: the browser it launched and the browser's own. Not itself, whose calls to Calcifer are the harness's. */
function browserProcesses() {
  const parent = new Map();
  for (const name of readdirSync('/proc')) {
    if (!/^\d+$/.test(name)) continue;
    try {
      const stat = readFileSync(`/proc/${name}/stat`, 'utf8');
      parent.set(Number(name), Number(stat.slice(stat.lastIndexOf(')') + 2).split(' ')[1]));
    } catch {}
  }
  const under = new Set([process.pid]);
  for (let grew = true; grew;) {
    grew = false;
    for (const [pid, ppid] of parent) if (!under.has(pid) && under.has(ppid)) { under.add(pid); grew = true; }
  }
  under.delete(process.pid);
  return under;
}

/** This browser's sockets to Calcifer's `port`: how many, how many hold bytes the browser hasn't read, and how many of Calcifer's ends of
 *  them hold bytes it hasn't sent; `holding`, the browser's ports of those that hold either. */
function sockets(port) {
  const mine = browserProcesses();
  const rows = execFileSync('ss', ['-H', '-t', '-n', '-p', 'state', 'established'], { encoding: 'utf8' }).split('\n').flatMap((line) => {
    const parts = line.trim().split(/\s+/);
    const at = parts.findIndex((part) => /:\d+$/.test(part));
    if (at < 2 || !/:\d+$/.test(parts[at + 1] ?? '')) return [];
    const pids = [...line.matchAll(/pid=(\d+)/g)].map((found) => Number(found[1]));
    return [{ recv: Number(parts[at - 2]), send: Number(parts[at - 1]), local: parts[at].split(':').pop(), peer: parts[at + 1].split(':').pop(), pids }];
  });
  const ours = rows.filter((row) => row.peer === String(port) && row.pids.some((pid) => mine.has(pid)));
  const ends = new Set(ours.map((row) => row.local));
  const unsent = rows.filter((row) => row.local === String(port) && ends.has(row.peer) && row.send > 0);
  const holding = [...new Set([...ours.filter((row) => row.recv > 0).map((row) => row.local), ...unsent.map((row) => row.peer)])];
  return { open: ours.length, unread: ours.filter((row) => row.recv > 0).length, unread_kb: Math.round(ours.reduce((kb, row) => kb + row.recv, 0) / 1024),
    unsent: unsent.length, unsent_kb: Math.round(unsent.reduce((kb, row) => kb + row.send, 0) / 1024), holding };
}

module.exports = { sockets };
