// Harness telemetry only. No DOM geometry reads, forced collection or target memory snapshots.
const fs = require('node:fs');
const path = require('node:path');
const out = process.env.PROFILE_CASE_OUTPUT;
const profiled = process.env.PROFILE_CASE.endsWith('-profiled');
let page, cdp, active, phase = 'boot', panCount = 0;
const windows = [], network = [], intervals = [], trace = [];
const pending = new Set();
let traceBytes = 0, truncated = false;
let droppedEvents = 0;
const allowedEvent = /^(RunTask|ThreadController|ThreadPool|TaskQueue|EventDispatch|FunctionCall|EvaluateScript|CompileScript|UpdateLayoutTree|Layout|PrePaint|Paint|RasterTask|CompositeLayers|Commit|BeginMain|BeginFrame|DrawFrame|ActivateLayerTree|LayerTreeHost|Scheduler|V8[.:]|MinorGC|MajorGC|GCEvent|GCTask|TimerFire|FireAnimationFrame|RequestAnimationFrame|UpdateLayerTree)/;
const safeUrl = value => { try { const url = new URL(value); return url.protocol === 'http:' || url.protocol === 'https:' ? url.pathname : url.protocol; } catch { return String(value).split(/[?#]/)[0]; } };
function init() { window.__scaleHarnessTelemetry = { frames: [], phase: 'boot', time_origin_ms: performance.timeOrigin, state_scope: 'camera/node/outbox available only from additive profiled-build attach observer; mounted/fetch per-frame unknown' }; }
async function bind(target, session) {
  page = target; cdp = session;
  await page.addInitScript(init);
  // bind runs after sign-in. Hash navigations keep this document, so it needs the same initialization now.
  await page.evaluate(init);
  page.on('request', request => { pending.add(request); network.push({ at_ms: Date.now(), phase, event: 'start', method: request.method(), route: safeUrl(request.url()).replace(/\/assets\/[^/]+$/, '/assets/<asset>'), pending: pending.size }); });
  for (const event of ['requestfinished', 'requestfailed']) page.on(event, request => { pending.delete(request); network.push({ at_ms: Date.now(), phase, event, pending: pending.size }); });
  cdp.on('Tracing.dataCollected', ({ value }) => {
    for (const event of value) {
      if (!['X','B','E','I','i','R','b','e','n'].includes(event.ph) || !allowedEvent.test(event.name)) { droppedEvents++; continue; }
      const row = { name: event.name, cat: event.cat, ph: event.ph, pid: event.pid, tid: event.tid, ts: event.ts, dur: event.dur };
      traceBytes += JSON.stringify(row).length;
      if (traceBytes <= 32 * 1024 * 1024) trace.push(row); else truncated = true;
    }
  });
}
async function mark(name) {
  phase = name;
  await page.addInitScript(name => { window.__scaleDiagnosticPhase = name; }, name);
  await page.evaluate(name => { if (window.__scaleHarnessTelemetry) window.__scaleHarnessTelemetry.phase = name; if (window.__scaleRuntimeProfile) window.__scaleRuntimeProfile.phase = name; }, name);
}
async function start(name) {
  await mark(name); active = name;
  intervals.push({ name, start_epoch_ms: Date.now(), profiled });
  if (!profiled) return;
  await cdp.send('Profiler.enable'); await cdp.send('Profiler.start');
  trace.length = 0; traceBytes = 0; truncated = false; droppedEvents = 0;
  await cdp.send('Tracing.start', { categories: 'toplevel,devtools.timeline,v8,cc,viz,renderer.scheduler', options: 'record-as-much-as-possible', transferMode: 'ReportEvents' });
}
async function stop(name = active) {
  if (!name) return;
  const observation = { name, end_epoch_ms: Date.now(), pending_requests: pending.size };
  if (profiled) {
    const { profile } = await cdp.send('Profiler.stop');
    for (const node of profile.nodes) { node.callFrame.url = safeUrl(node.callFrame.url); delete node.hitCount; }
    fs.writeFileSync(path.join(out, `${name}.cpuprofile`), JSON.stringify(profile));
    const done = new Promise(resolve => cdp.once('Tracing.tracingComplete', resolve));
    await cdp.send('Tracing.end'); await done;
    fs.writeFileSync(path.join(out, `${name}.timeline.json`), JSON.stringify({ scope: 'allowlisted event metadata only; all trace args discarded; no screenshot/network/object data', allowed_event_pattern: String(allowedEvent), dropped_events: droppedEvents, truncated, events: trace }));
  }
  observation.page = await page.evaluate(() => ({ time_origin_ms: performance.timeOrigin, frames: window.__scaleHarnessTelemetry?.frames ?? [], runtime: window.__scaleRuntimeProfile ? { stats: window.__scaleRuntimeProfile.stats, state: window.__scaleRuntimeProfile.read(), overhead: window.__scaleRuntimeProfile.overhead } : null }));
  intervals.push(observation); active = null;
  phase = `after-${name}`;
}
function frameWindow(frames, p95) { windows.push({ phase, end_epoch_ms: Date.now(), frames, p95_known: Number.isFinite(p95), p95_ms: Number.isFinite(p95) ? p95 : null }); }
async function startPan() { const name = ++panCount === 1 ? 'pan-small' : 'pan-big'; await start(name); }
async function finish() {
  if (active) await stop();
  fs.writeFileSync(path.join(out, 'profile-telemetry.json'), JSON.stringify({ profiled, phase, windows, intervals, network, pending_at_finish: pending.size, limits: ['unprofiled frontend has no attach/prototype hooks', 'per-frame mounted IDs and browser fetching count are unknown', 'profiled instrumentation adds observer/sampler overhead; does not qualify a performance pass'] }, null, 2));
}
function browserClosed() { fs.writeFileSync(path.join(out,'browser-close.json'),JSON.stringify({awaited_browser_close:true,at_ms:Date.now(),scope:'browser.close resolved; OS process absence remains independently unobserved'})); }
module.exports = { bind, startOpening: () => start('opening'), stopOpening: () => stop('opening'), frameWindow, startPan, stopPan: () => stop(), finish, browserClosed };
