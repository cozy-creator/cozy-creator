// Coordinator-only runner. Owns an output harness/proxy; never installs or starts a daemon.
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const { spawn, execFileSync } = require('node:child_process');
const guards = require('./source-guards.cjs');
const {forwardHeaders,upstreamUrl} = require('./proxy-headers.cjs');
const attribution = require('./attribution.cjs');
async function main() {
const order = ['current-plain', 'baseline-plain', 'baseline-profiled', 'current-profiled'];
const name = process.env.PROFILE_CASE;
if (!order.includes(name)) throw new Error('PROFILE_CASE must be one of the exact four approved labels');
for (const key of ['PROFILE_FRONTEND_MANIFEST','PROFILE_DIST_ROOT','PROFILE_HARNESS_ROOT','PROFILE_OUTPUT_ROOT','HOLDER','COZY','CHROME','PLAYWRIGHT']) if (!process.env[key]) throw new Error(`${key} is required`);
const root = path.resolve(process.env.PROFILE_OUTPUT_ROOT), dist = path.resolve(process.env.PROFILE_DIST_ROOT), harnessRoot = path.resolve(process.env.PROFILE_HARNESS_ROOT);
fs.mkdirSync(root, { recursive: true });
const ledgerPath = path.join(root, 'matrix.json');
const ledger = fs.existsSync(ledgerPath) ? JSON.parse(fs.readFileSync(ledgerPath, 'utf8')) : { order, cases: [], limits: ['targeted matching warmup differs from the required fullsuite', 'profiled sampler/observer overhead cannot qualify a performance pass', 'if current plain is not performance-red, no regression/fix attribution'] };
ledger.attribution = attribution(ledger.current_unprofiled_performance_red);
if (order[ledger.cases.length] !== name) throw new Error('matrix order/retry guard rejected this case');
const kind = name.startsWith('current') ? 'current' : 'baseline', mode = name.endsWith('-profiled') ? 'profiled' : 'plain';
const manifest = JSON.parse(fs.readFileSync(process.env.PROFILE_FRONTEND_MANIFEST, 'utf8'));
if (manifest.ref !== guards.refs[kind] || manifest.api_tree !== 'defe5c4ba569f362bda6b681ee116e25bc88b6ac' || manifest.connection_blob !== '956c734658ceeb894d98d2beadc5903d02e14ad4') throw new Error('frontend/API cohort guard failed');
if (manifest.package_sha256 !== '204d0be690cf8a199f19ad1e79e7d0a1e5c45bba7c205c9e19e5f63fc309af76' || manifest.lock_sha256 !== 'a4f4b22d68a945dd25025b543413c1e0efd7ca9ef312bcbdb75178fdb7ee995d') throw new Error('dependency parity guard failed');
if (manifest.variants[mode].observer !== (mode === 'profiled')) throw new Error('wrong frontend observer variant');
guards.verifyDist(dist, manifest.variants[mode].files);
guards.verify(harnessRoot, guards.refs.current, 'e2e');
const { instrument } = require('./harness-injection.cjs');
const original = fs.readFileSync(path.join(harnessRoot, 'e2e/cli/web/scale.js'), 'utf8');
const shared = path.join(root, 'shared-harness'), scale = path.join(shared, 'e2e/cli/web/scale.js');
const { text: instrumented, guard } = instrument(original);
const provenance = { ref: guards.refs.current, ...guard, hook_sha256: guards.sha(fs.readFileSync(path.join(__dirname, 'profile-hooks.cjs'))) };
if (!fs.existsSync(shared)) {
  fs.mkdirSync(shared, { recursive: true }); fs.cpSync(path.join(harnessRoot, 'e2e'), path.join(shared, 'e2e'), { recursive: true });
  fs.writeFileSync(scale, instrumented); fs.copyFileSync(path.join(__dirname, 'profile-hooks.cjs'), path.join(shared, 'e2e/cli/web/profile-hooks.cjs'));
  fs.writeFileSync(path.join(shared, 'provenance.json'), JSON.stringify(provenance, null, 2));
} else {
  const held = JSON.parse(fs.readFileSync(path.join(shared, 'provenance.json'), 'utf8'));
  if (JSON.stringify(held) !== JSON.stringify(provenance) || guards.sha(fs.readFileSync(scale)) !== provenance.instrumented_sha256 || guards.sha(fs.readFileSync(path.join(shared, 'e2e/cli/web/profile-hooks.cjs'))) !== provenance.hook_sha256) throw new Error('shared harness bytes drifted');
}
const output = path.join(root, name);
if (fs.existsSync(output)) throw new Error('preserve case directory; no retry');
fs.mkdirSync(output);
const row = { case: name, source: manifest.ref, mode, output, status: 'RUNNING', started_ms: Date.now(), frontend_manifest_sha256: guards.sha(fs.readFileSync(process.env.PROFILE_FRONTEND_MANIFEST)), harness: provenance };
ledger.cases.push(row); fs.writeFileSync(ledgerPath, JSON.stringify(ledger, null, 2));
const loadHolder = () => {
  const held = JSON.parse(fs.readFileSync(process.env.HOLDER, 'utf8'));
  if (!/^127\.0\.0\.1:\d+$/.test(held.address) || !Number.isSafeInteger(held.pid) || !held.token) throw new Error('invalid owned holder');
  return held;
};
let holder, nativeBase;
const policy = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
let server, code = 2;
const upstreams = new Set(), proxyCounts = { get: 0, write: 0, forbidden: 0 }, cleanup = {};
try {
  holder = loadHolder(); nativeBase = `http://${holder.address}`;
  const version = execFileSync(process.env.COZY, ['--version'], { encoding: 'utf8' }).trim();
  if (version !== 'cozy 0.23.1+dev.ga2333ba5') throw new Error('wrong native binary version');
  row.native = { version, executable_sha256: guards.sha(fs.readFileSync(process.env.COZY)), initial_pid: holder.pid, build_input_digest: '84229a326f82b1c84498955f735c76012f685470', artifact: 13788927202, artifact_archive_sha256: '3c22c2b888481aee488e5927861a9a0cde06238cda3663591c93c6a5bc186935' };
  row.runtime = { node: process.version, platform: process.platform, arch: process.arch, image_os: process.env.ImageOS, image_version: process.env.ImageVersion, chrome: execFileSync(process.env.CHROME, ['--version'], {encoding:'utf8'}).trim() };
  row.cache = 'fresh Chrome process/context from original harness; OS/native caches shared in recorded matrix order; no forced cache/GC/purge';
  const statusReply = awaitFetch(`${nativeBase}/v1/status`, holder.token);
  const nativeStatus = await statusReply;
  if (nativeStatus.local_usable !== false) throw new Error('this diagnostic requires the real Mac non-inference cohort');
  server = http.createServer((request, response) => {
    const own = `127.0.0.1:${request.socket.localPort}`;
    if (request.headers.host !== own || (request.headers.origin && request.headers.origin !== `http://${own}`) || (request.headers['sec-fetch-site'] && !['same-origin','none'].includes(request.headers['sec-fetch-site']))) { response.writeHead(403); response.end('owned origin only'); return; }
    const pathname = new URL(request.url, `http://${own}`).pathname;
    if (pathname.startsWith('/v1/')) {
      const write = !['GET','HEAD'].includes(request.method);
      if (write && (/^\/v1\/(runs|rentals?|run-quote|machines|models|billing|payments)(\/|$)/.test(pathname) || /^\/v1\/projects\/[^/]+\/(runs|fit|push|pull)(\/|$)/.test(pathname))) { proxyCounts.forbidden++; response.writeHead(403); response.end('provider/model work forbidden'); return; }
      proxyCounts[write ? 'write' : 'get']++;
      const headers = forwardHeaders(request.headers,nativeBase);
      const upstream = http.request(upstreamUrl(request.url, nativeBase), { method: request.method, headers }, answer => { response.writeHead(answer.statusCode, answer.headers); answer.pipe(response); answer.on('close', () => { if (!answer.complete) response.destroy(); }); });
      upstreams.add(upstream); upstream.on('close', () => upstreams.delete(upstream));
      upstream.on('error', () => { if (!response.headersSent) response.writeHead(502); response.end('owned native unavailable'); });
      response.on('close', () => upstream.destroy()); request.pipe(upstream); return;
    }
    try {
      const file = pathname === '/' ? path.join(dist, 'index.html') : path.resolve(dist, `.${decodeURIComponent(pathname)}`);
      if (!file.startsWith(dist + path.sep)) throw new Error('outside owned dist');
      const bytes = fs.readFileSync(file), type = { '.html':'text/html', '.js':'text/javascript', '.css':'text/css', '.svg':'image/svg+xml', '.png':'image/png', '.webp':'image/webp', '.gif':'image/gif', '.ico':'image/x-icon', '.woff':'font/woff', '.woff2':'font/woff2', '.wasm':'application/wasm', '.json':'application/json', '.map':'application/json' }[path.extname(file)] ?? 'application/octet-stream';
      response.writeHead(200, { 'Content-Type':type, 'Content-Length':bytes.length, 'Referrer-Policy':'no-referrer', 'Cache-Control':pathname==='/'?'no-store':'public,max-age=31536000,immutable', ...(pathname==='/'?{'Content-Security-Policy':policy}:{}) });
      response.end(request.method === 'HEAD' ? undefined : bytes);
    } catch { response.writeHead(404); response.end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const stdout = fs.createWriteStream(path.join(output, 'web-scale.json')), stderr = fs.createWriteStream(path.join(output, 'web-scale.err'));
  const env = { ...process.env, PAGE_ORIGIN:origin, PROFILE_CASE_OUTPUT:output, LAST_LOOK:path.join(output,'web-scale'), SHOT:path.join(output,'web-scale'), ARGV_LOG:path.join(output,'argv.log') };
  delete env.ALONE; delete env.ENGINE; delete env.RESTART;
  code = await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [scale], { cwd: harnessRoot, env, stdio:['ignore','pipe','pipe'] });
    child.stdout.pipe(stdout); child.stderr.pipe(stderr); child.on('error',reject); child.on('close',(exit,signal)=>{row.child_signal=signal;resolve(exit??2)});
  });
  await Promise.all([new Promise(resolve => stdout.end(resolve)), new Promise(resolve => stderr.end(resolve))]);
  cleanup.harness_process_exited = true;
} catch (error) { row.error = String(error.message).split(/[?#]/)[0]; code = 2; }
finally {
  for (const upstream of upstreams) upstream.destroy(); cleanup.proxy_upstreams_destroyed = true;
  if (server) { server.closeAllConnections?.(); await new Promise(resolve => server.close(resolve)); cleanup.proxy_closed = true; }
  const marker=path.join(output,'browser-close.json');
  cleanup.awaited_browser_close=fs.existsSync(marker)?JSON.parse(fs.readFileSync(marker,'utf8')).awaited_browser_close:null;
  cleanup.chrome_os_absence='unknown; coordinator must independently verify, especially exit2';
  cleanup.exit2_source_fallback='Playwright1.55 processLauncher exitHandler invokes registered process-group kill; not an observed absence receipt';
  row.finished_ms = Date.now(); row.exit_code = code; row.status = code === 0 ? 'PASS' : 'FAIL'; row.cleanup = cleanup; row.proxy_counts = proxyCounts;
  if (fs.existsSync(path.join(output,'web-scale.json'))) { try {
    const result = JSON.parse(fs.readFileSync(path.join(output,'web-scale.json'),'utf8'));
    row.failures=result.failures; row.native_closed=result.seen?.scale?.calcifer_native?.closed;
    row.native_closed_sample_reached=Boolean(row.native_closed);
    if(name==='current-plain')ledger.current_unprofiled_performance_red=result.failures.some(failure=>['it_settles_quickly','size_adds_nothing_to_a_pan'].includes(failure));
    const folder=result.seen?.project?.folder;
    const projects=path.resolve(path.dirname(path.dirname(process.env.HOLDER)),'projects');
    if(folder && path.dirname(path.resolve(folder))===projects && /^web-scale-\d+$/.test(path.basename(folder)) && row.native_closed?.project_db_open===false) { fs.rmSync(folder,{recursive:true,force:true});cleanup.generated_project_removed=true; }
    else cleanup.generated_project_removed=false;
  } catch { row.output_parse_error=true; } }
  ledger.attribution = attribution(ledger.current_unprofiled_performance_red);
  fs.writeFileSync(path.join(output,'case.json'),JSON.stringify(row,null,2)); fs.writeFileSync(ledgerPath,JSON.stringify(ledger,null,2));
  execFileSync(process.execPath,[path.join(shared,'e2e/publish/scrub.js'),output],{stdio:['ignore','pipe','pipe']});
  console.log(JSON.stringify({case:name,exit_code:code,failures:row.failures,output,cleanup,attribution:ledger.attribution??'pending matched analysis'}));
  process.exitCode=code;
}
async function awaitFetch(url, token) { const response=await fetch(url,{headers:{Authorization:`Bearer ${token}`},signal:AbortSignal.timeout(10000)});if(!response.ok)throw new Error('owned status refused');return response.json(); }
}
main().catch(error => { console.error(String(error.message).split(/[?#]/)[0]); process.exitCode = 2; });
