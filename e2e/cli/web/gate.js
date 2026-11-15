// What the gate costs (web.sh's web_gate_cost; Paul, 11-14), from Calcifer's Server-Timing ([calcifer] diagnostics): its checks on one
// kept connection, and the peer lookup a new connection pays once; idle, then with <pairs> other loopback connections open in a newer
// process (web/busy.js). This process is the older one, as a browser is, so a lookup that scans processes (macOS) passes busy.js first.
//   node gate.js <base URL> <bearer> <pairs>
const http = require('http');
const { spawn } = require('child_process');
const { join } = require('path');

const [base, bearer, pairs] = process.argv.slice(2);
const timing = (header) => Object.fromEntries((header ?? '').split(',').map((part) => part.trim().match(/^(\w+);dur=([\d.]+)/)).filter(Boolean).map(([, name, ms]) => [name, Number(ms)]));

function ask(agent) {
  return new Promise((done, failed) => {
    const request = http.get(`${base}/v1/status`, { agent, headers: { Authorization: `Bearer ${bearer}` } }, (response) => {
      response.resume();
      response.on('end', () => done(timing(response.headers['server-timing'])));
    });
    request.on('error', failed);
  });
}

const stats = (values) => {
  const sorted = values.filter((value) => typeof value === 'number').sort((a, b) => a - b);
  return sorted.length ? { n: sorted.length, median: sorted[sorted.length >> 1], p95: sorted[Math.floor(sorted.length * 0.95)], max: sorted.at(-1) } : null;
};

async function measure() {
  const fresh = [];
  for (let at = 0; at < 60; at++) fresh.push(await ask(new http.Agent({ keepAlive: false })));
  const kept = new http.Agent({ keepAlive: true, maxSockets: 1 });
  const held = [];
  for (let at = 0; at < 200; at++) held.push(await ask(kept));
  kept.destroy();
  return { lookup_ms: stats(fresh.map((one) => one.peer)), checks_ms: stats(held.map((one) => one.guard)), admit_ms: stats(held.map((one) => one.admit)) };
}

(async () => {
  const idle = await measure();
  const busy = spawn(process.execPath, [join(__dirname, 'busy.js'), pairs], { stdio: ['ignore', 'pipe', 'inherit'] });
  await new Promise((done, failed) => {
    busy.stdout.on('data', (chunk) => { if (String(chunk).startsWith('ready')) done(); });
    busy.on('exit', (code) => failed(new Error(`busy.js ended: ${code}`)));
  });
  const loaded = await measure();
  busy.kill();
  console.log(JSON.stringify({ os: process.platform, sockets: 2 * Number(pairs), idle, busy: loaded }));
  process.exit(0);
})().catch((error) => { console.error(error); process.exit(1); });
