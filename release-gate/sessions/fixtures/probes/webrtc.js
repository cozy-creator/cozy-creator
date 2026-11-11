// probes.sh's browsers: a playback API link opened on its real HTTPS origin, under that page's real CSP, with its real cozy-webrtc.js, in
// Chromium, Firefox and WebKit. Playwright's WebKit on Linux is not Safari, and nothing here speaks for Safari or iOS.
//   node webrtc.js <link> <results.json>
// $PLAYWRIGHT: playwright-core's directory; $NET: net.sh; $MACHINE: the machine, by name. Prints one line per browser; results.json grows as
// each step ends. Each step is bounded, and one that fails is recorded and the next goes on.
const { execFileSync } = require('child_process');
const { writeFileSync } = require('fs');
const { chromium, firefox, webkit } = require(process.env.PLAYWRIGHT);

const [link, file] = process.argv.slice(2);
const machine = process.env.MACHINE;
const [ip, port] = new URLSearchParams(new URL(link).hash.slice(1)).get('a').split(':');
const STEP_MS = 240_000;
const results = {};
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
const save = () => writeFileSync(file, JSON.stringify(results, null, 1));
const net = (action, rate) => execFileSync(process.env.NET, [action, ip, port, ...(rate ? [rate] : [])], { encoding: 'utf8' }).trim();
const there = (script) => (machine === 'local'
  ? execFileSync('bash', ['-c', script], { encoding: 'utf8' })
  : execFileSync('cozy', ['machine', 'ssh', machine, '--', script], { encoding: 'utf8' }));
const cozy = (...args) => execFileSync('cozy', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
const quiet = (work) => { try { return work(); } catch (error) { return null; } };
// A rental's idle clock is held for the measurement, which is no job of its.
const keep = () => machine !== 'local' && quiet(() => cozy('machine', 'keepalive', machine));
/** TensorD's CPU ticks there, all its processes. */
const ticks = () => quiet(() => there(`for p in /proc/[0-9]*; do [ "$(cat $p/comm 2>/dev/null)" = tensord ] && cut -d' ' -f14,15 $p/stat; done`)
  .trim().split(/\s+/).reduce((sum, n) => sum + Number(n), 0));

// In the page, as an inline module: the page's own CSP judges it as it judges the player.
function library(cozy) {
  const link = cozy.parseLink(location.hash);
  const now = () => performance.now();
  const STALL_MS = 60_000;
  const make = RTCPeerConnection.prototype.createDataChannel;
  let channel = null;
  RTCPeerConnection.prototype.createDataChannel = function (...args) { return (channel = make.apply(this, args)); };
  const open = async (window) => {
    const t = now();
    const session = await cozy.connect(link, { cap: link.cap, ...(window ? { window } : {}) });
    return { session, ms: now() - t, channel };
  };
  const read = (session, offset, length, { into, moved } = {}) => new Promise((resolve, reject) => {
    const t = now();
    let first = null, got = 0, at = offset, last = t, gap = 0;
    const watch = setInterval(() => { if (now() - last > STALL_MS) { clearInterval(watch); reject(new Error(`no bytes for ${STALL_MS} ms`)); } }, 1000);
    const finish = (settle) => (value) => { clearInterval(watch); settle(value); };
    session.get({ run: link.run, output: link.output, offset, length }, {
      data(position, bytes) {
        const n = now();
        first ??= n - t;
        gap = Math.max(gap, n - last);
        last = n;
        if (position !== at) return finish(reject)(new Error(`bytes at ${position}, expected ${at}`));
        into?.set(bytes, position);
        at += bytes.length;
        got += bytes.length;
        session.release(bytes.length);
        moved?.(at);
      },
      end: (m) => finish(resolve)({ ms: now() - t, first, got, gap, end: m }),
      error: finish(reject),
      close: finish(reject),
    });
  });
  const round = (x) => Math.round(x * 10) / 10;
  const spread = (values) => {
    const s = [...values].sort((a, b) => a - b);
    return { n: s.length, total_ms: round(s.reduce((a, b) => a + b, 0)), median_ms: round(s[s.length >> 1]), p95_ms: round(s[Math.floor(s.length * 0.95)]), worst_ms: round(s[s.length - 1]), at_or_over_1s: s.filter((x) => x >= 1000).length };
  };
  const hex = async (bytes) => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (b) => b.toString(16).padStart(2, '0')).join('');
  const same = async (bytes, end) => (await hex(bytes)) === String(end.sha256).replace(/^sha256:/, '');
  const SMALL = 4096;
  const measure = (s) => read(s, 0, SMALL).then((r) => r.end);
  const dropping = {};
  window.P = {
    async opens(n) {
      const ms = [];
      for (let i = 0; i < n; i++) { const o = await open(); ms.push(o.ms); o.session.close(); await new Promise((d) => setTimeout(d, 100)); }
      return spread(ms);
    },
    async bulk() {
      const { session } = await open();
      const end = await measure(session);
      const into = new Uint8Array(end.length);
      const r = await read(session, 0, end.length, { into });
      session.close();
      return { bytes: r.got, ms: round(r.ms), mb_s: round(r.got / 1e3 / r.ms), first_byte_ms: round(r.first), worst_gap_ms: round(r.gap), sha256_matches: await same(into, r.end) };
    },
    async smalls(n) {
      const { session } = await open();
      const { length } = await measure(session);
      const ms = [];
      for (let i = 0; i < n; i++) ms.push((await read(session, Math.floor(Math.random() * (length - SMALL)), SMALL)).ms);
      session.close();
      return spread(ms);
    },
    /** Small reads, one after another, while one bulk read shares the session: what an API over it would feel. */
    async underLoad(megabytes, window) {
      const { session } = await open(window);
      const { length } = await measure(session);
      const size = megabytes ? Math.min(megabytes * 1e6, length) : length;
      let done = false, lag = 0, tick = now();
      const beat = setInterval(() => { const n = now(); lag = Math.max(lag, n - tick - 50); tick = n; }, 50);
      const bulk = read(session, 0, size).finally(() => { done = true; });
      const ms = [];
      while (!done) {
        ms.push((await read(session, Math.floor(Math.random() * (length - SMALL)), SMALL)).ms);
        await new Promise((d) => setTimeout(d, 100));
      }
      const r = await bulk;
      clearInterval(beat);
      session.close();
      return { window: window || 8 << 20, small: spread(ms), bulk_mb: size / 1e6, bulk_mb_s: round(r.got / 1e3 / r.ms), page_main_thread_worst_lag_ms: round(lag) };
    },
    /** cozy/1 has no upload: padded no-op messages are what the channel carries up, unforwarded. The ordered reply says the pod took them all. */
    async uplink(megabytes) {
      const opened = await open();
      const up = opened.channel;
      const message = JSON.stringify({ t: 'credit', bytes: 0, pad: 'x'.repeat(60000) });
      up.bufferedAmountLowThreshold = 1 << 20;
      const t = now();
      let sent = 0;
      while (sent < megabytes * 1e6) {
        if (up.bufferedAmount > 4 << 20) await new Promise((d) => up.addEventListener('bufferedamountlow', d, { once: true }));
        up.send(message);
        sent += message.length;
      }
      await measure(opened.session);
      const ms = now() - t;
      opened.session.close();
      return { bytes: sent, ms: round(ms), mb_s: round(sent / 1e3 / ms) };
    },
    async dropBegin() {
      const { session } = await open();
      const end = await measure(session);
      Object.assign(dropping, { session, end, at: 0, lost: null, code: null, mark: null, done: null, failed: null, into: new Uint8Array(end.length) });
      session.closed.then((reason) => { dropping.lost = now(); dropping.code = reason.code; });
      dropping.read = read(session, 0, end.length, { into: dropping.into, moved: (at) => { dropping.at = at; } })
        .then((r) => { dropping.done = r; }, (error) => { dropping.failed = String(error.message ?? error); });
    },
    dropState: () => ({ at: dropping.at, lost: dropping.lost !== null, done: dropping.done !== null, failed: dropping.failed ?? null }),
    dropMark() { dropping.mark = now(); },
    async dropFinish() {
      const out = { lost: dropping.lost !== null, code: dropping.code, detect_ms: dropping.lost === null ? null : round(dropping.lost - dropping.mark), at_mb: round(dropping.at / 1e6) };
      if (dropping.done) {
        out.survived = true;
        out.worst_gap_ms = round(dropping.done.gap);
        out.sha256_matches = await same(dropping.into, dropping.done.end);
        dropping.session.close();
        return out;
      }
      dropping.session.close();
      const again = await open();
      out.reopen_ms = round(again.ms);
      const rest = await read(again.session, dropping.at, dropping.end.length - dropping.at, { into: dropping.into });
      out.first_resumed_byte_after_drop_ms = round(now() - rest.ms + rest.first - dropping.mark);
      out.sha256_matches = await same(dropping.into, rest.end);
      again.session.close();
      return out;
    },
  };
}

async function drop(page, kind) {
  await page.evaluate('P.dropBegin()');
  const state = () => page.evaluate('P.dropState()');
  while ((await state()).at < 20e6) {
    const s = await state();
    if (s.done || s.lost || s.failed) throw new Error(`before the drop: ${JSON.stringify(s)}`);
    await pause(20);
  }
  await page.evaluate('P.dropMark()');
  const settled = async (ms) => {
    for (const end = Date.now() + ms; Date.now() < end; await pause(50)) {
      const s = await state();
      if (s.lost || s.done || s.failed) return s;
    }
    return state();
  };
  let how;
  if (kind === 'reset') {
    how = net('reset');
    await settled(90_000);
  } else {
    how = 'iptables DROP';
    net('block');
    try {
      await (kind === 'blip' ? pause(3000) : settled(90_000));
    } finally {
      net('unblock');
    }
    if (kind === 'blip') await settled(90_000);
  }
  return { how, ...(await page.evaluate('P.dropFinish()')) };
}

async function shaped(page, rate, megabytes, window = 0) {
  try {
    net('shape', rate);
  } catch (error) {
    quiet(() => net('unshape'));
    return { not_measured: `this runner could not shape: ${String(error.message).slice(0, 200)}` };
  }
  try {
    return { rate, ...(await page.evaluate(`P.underLoad(${megabytes}, ${window})`)) };
  } finally {
    quiet(() => net('unshape'));
  }
}

async function one(name, type) {
  const r = (results[name] = {});
  const step = async (key, work) => {
    keep();
    try {
      r[key] = await Promise.race([work(), pause(STEP_MS).then(() => { throw new Error(`no answer in ${STEP_MS} ms`); })]);
    } catch (error) {
      r[key] = { error: String(error.message ?? error).slice(0, 400) };
    }
    save();
    console.error(`${new Date().toISOString()} ${name} ${key}: ${JSON.stringify(r[key]).slice(0, 300)}`);
    return !r[key].error;
  };
  const browser = await type.launch({ headless: true });
  try {
    r.version = browser.version();
    const context = await browser.newContext();
    await context.addInitScript(() => {
      window.__refused = [];
      document.addEventListener('securitypolicyviolation', (e) => window.__refused.push(`${e.violatedDirective}: ${e.blockedURI}`));
    });
    const page = await context.newPage();
    r.console = [];
    page.on('console', (m) => { if (m.type() === 'error' || /content.security|mixed.content|insecure/i.test(m.text())) r.console.push(m.text().slice(0, 300)); });
    page.on('pageerror', (e) => r.console.push(`pageerror: ${String(e.message).slice(0, 300)}`));

    // The real link in the real player, as a person opens it. The output isn't a film: it's closed once connected.
    const opened = await step('player', async () => {
      const t = Date.now();
      await page.goto(link, { waitUntil: 'load' });
      for (;;) {
        const state = await page.evaluate(`document.getElementById('status').dataset.state`);
        if (state === 'connected' || state === 'playing') break;
        if (state === 'error') throw new Error(await page.evaluate(`document.getElementById('status').textContent`));
        await pause(20);
      }
      return { connected_ms: Date.now() - t };
    });
    await page.evaluate('window.cozyPlayer?.close()').catch(() => {});
    r.page = await page.evaluate(`({ origin: location.origin, secure_context: isSecureContext, csp: document.querySelector('meta[http-equiv=Content-Security-Policy]')?.content, webrtc: !!window.RTCPeerConnection, user_agent: navigator.userAgent })`);
    await page.addScriptTag({ type: 'module', content: `import * as cozy from "./cozy-webrtc.js"; (${library})(cozy);` });
    for (let i = 0; i < 100 && !(await page.evaluate('!!window.P')); i++) await pause(50);
    if (!(await step('opens', () => page.evaluate('P.opens(5)')))) { r.stopped = 'it never opened'; return; }
    r.opened_in_player = opened;

    const before = ticks();
    await step('bulk', () => page.evaluate('P.bulk()'));
    const after = ticks();
    if (before !== null && after !== null && r.bulk.bytes) r.bulk.tensord_cpu_ms_per_mb = Math.round(((after - before) * 10) / (r.bulk.bytes / 1e6));
    await step('bulk_again', () => page.evaluate('P.bulk()'));
    await step('smalls', () => page.evaluate('P.smalls(100)'));
    await step('under_load', () => page.evaluate('P.underLoad(0)'));
    // The client's credit is how much can sit ahead of a small read: the player's 8 MiB above, then the design's 4 MiB and less.
    await step('under_load_window_4mib', () => page.evaluate(`P.underLoad(0, ${4 << 20})`));
    await step('under_load_window_1mib', () => page.evaluate(`P.underLoad(0, ${1 << 20})`));
    await step('uplink_noop_messages', () => page.evaluate('P.uplink(100)'));
    await step('drop_reset', () => drop(page, 'reset'));
    await step('drop_blip_3s', () => drop(page, 'blip'));
    await step('drop_blackhole', () => drop(page, 'blackhole'));
    await step('under_load_20mbit', () => shaped(page, '20mbit', 25));
    await step('under_load_5mbit', () => shaped(page, '5mbit', 6));
    await step('under_load_20mbit_window_1mib', () => shaped(page, '20mbit', 25, 1 << 20));
    await step('under_load_20mbit_window_256kib', () => shaped(page, '20mbit', 25, 256 << 10));
    await step('under_load_5mbit_window_256kib', () => shaped(page, '5mbit', 6, 256 << 10));
    r.refused = await page.evaluate('window.__refused');
  } finally {
    await browser.close().catch(() => {});
    save();
  }
}

(async () => {
  for (const [name, type] of [['chromium', chromium], ['firefox', firefox], ['webkit', webkit]]) {
    try {
      await one(name, type);
    } catch (error) {
      results[name].fatal = String(error.message ?? error).slice(0, 400);
      save();
    }
    const r = results[name];
    console.log(`${name} ${r.version ?? ''}: open ${r.opens?.median_ms} ms, bulk ${r.bulk?.mb_s}/${r.bulk_again?.mb_s} MB/s, 100 small ${r.smalls?.total_ms} ms median ${r.smalls?.median_ms}, under load worst ${r.under_load?.small?.worst_ms} median ${r.under_load?.small?.median_ms}, refused ${JSON.stringify(r.refused ?? null)}, ${r.fatal ?? r.stopped ?? r.opens?.error ?? 'ran'}`);
  }
  process.exit(0); // A step's bound still ticking holds nothing up.
})();
