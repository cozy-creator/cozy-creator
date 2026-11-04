// cl-633 / cl-774 Part E probe: WebRTC-direct from a real HTTPS origin, in three engines.
// It loads the PUBLISHED player page (a real github.io HTTPS origin, with the player's own strict
// CSP), imports the real cozy-webrtc.js from that same origin, and drives its Session API against
// the rental's WebRTC listener over the public internet. For each of Chromium, Firefox and WebKit
// it records: whether it connects at all (and any mixed-content or CSP console refusal), time to
// open, a 100 MB bulk read in MB/s, a hundred 4 KiB reads (total and median), a hundred small reads
// issued while the bulk read runs (worst and median wait), and a reconnect.
//   node webrtc.js
// env: PLAYWRIGHT (the installed `playwright` package dir), PAGE (the published player origin, e.g.
//   https://cozy-creator.github.io/cozy-creator/play/), LINK (a real `cozy run play` link for the
//   blob output), BLOB_LEN (the blob output's byte length), OUT (where per-engine JSON is written).
const fs = require('fs');
const path = require('path');

const { chromium, firefox, webkit } = require(process.env.PLAYWRIGHT);
const PAGE = process.env.PAGE;
const LINK = process.env.LINK;
const BLOB_LEN = Number(process.env.BLOB_LEN);
const OUT = process.env.OUT;

// Runs in the page, on the real HTTPS origin, with the real library. Returns a plain object.
async function measure({ link, blobLen }) {
  const mod = await import('./cozy-webrtc.js');
  const result = { steps: {} };
  const parsed = mod.parseLink(new URL(link).hash);
  const descriptor = { address: parsed.address, fingerprint: parsed.fingerprint };
  const cap = parsed.cap, run = parsed.run, output = parsed.output;
  const deadline = (p, ms, what) =>
    Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error(`${what}: timed out after ${ms} ms`)), ms))]);

  // One get of [offset, offset+length) of an output; resolves {bytes, ms}.
  const getRange = (session, offset, length) =>
    new Promise((resolve, reject) => {
      const t0 = performance.now();
      let got = 0;
      session.get(
        { run, output, offset, length },
        {
          open() {},
          data(_o, b) { got += b.length; session.release(b.length); },
          end() { resolve({ bytes: got, ms: performance.now() - t0 }); },
          error(e) { reject(new Error(e.code + ': ' + e.message)); },
          close(e) { reject(new Error('closed: ' + (e && e.message))); },
        },
      );
    });

  const t0 = performance.now();
  let session;
  try {
    session = await deadline(mod.connect(descriptor, { cap, window: 8 << 20 }), 30000, 'connect');
  } catch (e) {
    result.connected = false;
    result.error = String(e.message || e);
    return result;
  }
  result.connected = true;
  result.open_ms = Math.round(performance.now() - t0);
  result.machine = session.welcome && session.welcome.machine;

  // A 100 MB bulk read.
  try {
    const big = await deadline(getRange(session, 0, blobLen), 120000, 'bulk');
    result.steps.bulk = { bytes: big.bytes, ms: Math.round(big.ms), MBps: +(big.bytes / 1e6 / (big.ms / 1000)).toFixed(2) };
  } catch (e) { result.steps.bulk = { error: String(e.message || e) }; }

  // A hundred 4 KiB reads, one after another.
  try {
    const ms = [];
    for (let i = 0; i < 100; i++) {
      const r = await deadline(getRange(session, (i * 65536) % Math.max(1, blobLen - 4096), 4096), 20000, 'small');
      ms.push(r.ms);
    }
    ms.sort((a, b) => a - b);
    result.steps.small_reads = {
      count: ms.length,
      total_ms: Math.round(ms.reduce((a, b) => a + b, 0)),
      median_ms: +ms[Math.floor(ms.length / 2)].toFixed(2),
      max_ms: +ms[ms.length - 1].toFixed(2),
    };
  } catch (e) { result.steps.small_reads = { error: String(e.message || e) }; }

  // Small reads WHILE a bulk read runs: the latency an API call feels behind a transfer.
  try {
    let bulkDone = false;
    const bulkP = getRange(session, 0, blobLen).then((r) => { bulkDone = true; return r; }).catch((e) => { bulkDone = true; return { error: String(e.message || e) }; });
    const waits = [];
    const started = performance.now();
    while (!bulkDone && performance.now() - started < 60000) {
      const r = await deadline(getRange(session, (waits.length * 65536) % Math.max(1, blobLen - 4096), 4096), 20000, 'small-during-bulk');
      waits.push(r.ms);
    }
    const bulk = await bulkP;
    waits.sort((a, b) => a - b);
    result.steps.small_during_bulk = {
      count: waits.length,
      median_ms: waits.length ? +waits[Math.floor(waits.length / 2)].toFixed(2) : null,
      max_ms: waits.length ? +waits[waits.length - 1].toFixed(2) : null,
      bulk_MBps: bulk && bulk.bytes ? +(bulk.bytes / 1e6 / (bulk.ms / 1000)).toFixed(2) : null,
    };
  } catch (e) { result.steps.small_during_bulk = { error: String(e.message || e) }; }

  // Reconnect after dropping: close the session and open a fresh one. A new ufrag means a fresh
  // session, not a resumed one; that is what the player's reconnect loop does after a lost socket.
  try {
    session.close();
    const t1 = performance.now();
    const s2 = await deadline(mod.connect(descriptor, { cap, window: 8 << 20 }), 30000, 'reconnect');
    result.steps.reconnect_ms = Math.round(performance.now() - t1);
    s2.close();
  } catch (e) { result.steps.reconnect = { error: String(e.message || e) }; }

  return result;
}

async function runEngine(name, type) {
  const record = { engine: name, console: [] };
  let browser;
  try {
    browser = await type.launch({ headless: true });
  } catch (e) {
    record.launch_error = String(e.message || e);
    fs.writeFileSync(path.join(OUT, `webrtc-${name}.json`), JSON.stringify(record, null, 2));
    return record;
  }
  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    page.on('console', (m) => {
      const t = m.text();
      if (/mixed content|content security policy|csp|insecure|refused|blocked/i.test(t)) record.console.push(`${m.type()}: ${t}`.slice(0, 300));
    });
    page.on('pageerror', (e) => record.console.push('pageerror: ' + String(e.message || e).slice(0, 300)));
    page.on('requestfailed', (q) => record.console.push(`requestfailed: ${q.url().slice(0, 120)} ${q.failure() && q.failure().errorText}`));
    // Load the real player origin with a deliberately incomplete hash: the page's own player errors
    // out benignly (it never auto-connects), but the module and the secure origin are live for us.
    await page.goto(PAGE + '#v=1', { waitUntil: 'load', timeout: 30000 });
    record.origin = await page.evaluate(() => location.origin);
    record.secure_context = await page.evaluate(() => window.isSecureContext);
    const r = await page.evaluate(measure, { link: LINK, blobLen: BLOB_LEN });
    Object.assign(record, r);
    await context.close();
  } catch (e) {
    record.error = String(e.message || e);
  } finally {
    await browser.close().catch(() => {});
  }
  fs.writeFileSync(path.join(OUT, `webrtc-${name}.json`), JSON.stringify(record, null, 2));
  return record;
}

(async () => {
  const engines = [['chromium', chromium], ['firefox', firefox], ['webkit', webkit]];
  const all = {};
  for (const [name, type] of engines) {
    try {
      all[name] = await runEngine(name, type);
    } catch (e) {
      all[name] = { engine: name, error: String(e.message || e) };
    }
    console.error(`${name}: ${JSON.stringify(all[name]).slice(0, 400)}`);
  }
  process.stdout.write(JSON.stringify(all, null, 2));
})();
