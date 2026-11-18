// Chromium-only candidate delivery without Playwright routing (which disables HTTP caching).
// Install before navigation, on a fresh browser/context with no page/context routes:
//   const receipt = await candidateCDP(page, origin, builtDirectory);
//   ...; await receipt.dispose();
// Frozen static bytes may be cached normally. This helper does not establish that a cache hit occurred;
// qualification must independently verify actual HTTP cache behavior. API/media requests are never matched.
// Protocol signatures checked against playwright-core 1.55.0 types/protocol.d.ts (Fetch and Page).
const { createHash } = require('node:crypto');
const { readFileSync, readdirSync, realpathSync } = require('node:fs');
const { extname, join } = require('node:path');

const installed = new WeakMap();
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const literal = value => value.replace(/[\\*?]/g, '\\$&');
const securityNames = new Set(['content-security-policy', 'content-security-policy-report-only', 'referrer-policy', 'x-content-type-options', 'x-frame-options', 'cross-origin-opener-policy', 'cross-origin-embedder-policy', 'cross-origin-resource-policy', 'permissions-policy', 'origin-agent-cluster', 'strict-transport-security']);
const bodyHeaders = new Set(['content-length', 'content-encoding', 'transfer-encoding', 'etag', 'last-modified', 'content-md5', 'digest', 'content-digest', 'repr-digest', 'content-range']);
const mime = { '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.webp': 'image/webp', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.avif': 'image/avif', '.gif': 'image/gif', '.woff': 'font/woff', '.woff2': 'font/woff2', '.ttf': 'font/ttf', '.wasm': 'application/wasm', '.json': 'application/json', '.map': 'application/json', '.txt': 'text/plain', '.ico': 'image/x-icon' };

async function candidateCDP(page, origin, directory) {
  const address = new URL(origin);
  if (!['http:', 'https:'].includes(address.protocol) || address.username || address.password || address.pathname !== '/' || address.search || address.hash) throw new Error('candidateCDP requires a plain HTTP(S) origin');
  origin = address.origin;
  const root = realpathSync(directory), previous = installed.get(page);
  if (previous) {
    if (previous.origin !== origin || previous.directory !== root) throw new Error('this page already owns a different frozen candidate');
    return previous;
  }
  const files = new Map();
  const walk = (folder, parts = []) => {
    for (const entry of readdirSync(folder, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const names = [...parts, entry.name], path = `/${names.join('/')}`, file = join(folder, entry.name);
      // Even a malformed build must never add a /v1 interception pattern.
      if (names[0] === 'v1') throw new Error('candidate contains the reserved /v1 namespace');
      if (entry.isDirectory()) walk(file, names);
      else if (entry.isFile()) {
        const bytes = readFileSync(file), digest = sha(bytes);
        files.set(new URL(`/${names.map(encodeURIComponent).join('/')}`, origin).href, { path, bytes: bytes.length, sha256: digest, body: bytes.toString('base64'), etag: `"${digest}"`, mime: mime[extname(path).toLowerCase()] ?? 'application/octet-stream' });
      } else throw new Error(`non-regular candidate asset: ${path}`);
    }
  };
  walk(root);
  let html = files.get(`${origin}/index.html`);
  if (!html || /@vite\/client|__COZY_DEVELOPMENT_NONCE__/.test(Buffer.from(html.body, 'base64').toString('utf8'))) throw new Error('candidate must contain a production index.html');
  const receipt = {
    origin, directory: root,
    files: [...files.values()].map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })),
    documents: [], static_requests: [], errors: [], aborted: [],
    cachemode: { browser_cache: 'unchanged', static_cache_control: 'public, max-age=31536000, immutable', cache_hits: 'unverified; coordinator must observe actual browser cache hits', interception: 'page CDP Fetch; no Playwright routes or Network.setCacheDisabled' },
    disposed: false,
  };
  // CDP patterns have include globs, not exclusions. These are the actual UI routes in app/routes.tsx;
  // limiting Document interception to them avoids pausing even a top-level /v1 API/media navigation.
  const routes = ['/', '/index.html', '/p', '/projects', '/runs', '/machines', '/packages', '/models', '/account'];
  const patterns = routes.flatMap(path => [
    { urlPattern: literal(origin + path), resourceType: 'Document', requestStage: 'Response' },
    { urlPattern: `${literal(origin + path)}\\?*`, resourceType: 'Document', requestStage: 'Response' },
    ...(path === '/' || path === '/index.html' ? [] : [{ urlPattern: `${literal(origin + path)}/*`, resourceType: 'Document', requestStage: 'Response' }]),
  ]);
  for (const [url, file] of files) if (!/\.html?$/i.test(file.path)) patterns.push({ urlPattern: literal(url), requestStage: 'Request' });
  const session = await page.context().newCDPSession(page);
  let closing = false, cleanup, topFrame, security;
  const pending = new Set();
  const error = (operation, failure) => receipt.errors.push({ operation, name: failure?.name ?? 'Error', message: String(failure?.message ?? failure).replace(/https?:\/\/[^\s)]+/g, '[url]') });
  const navigated = ({ frame }) => { if (!frame.parentId) topFrame = frame.id; };
  // A request the page cancelled itself (a picture whose box re-rendered, a live theme change: 44307867239) is gone before its
  // answer: the browser says so (Network.loadingFailed, canceled), and answering it fails `Invalid InterceptionId`. That is no
  // delivery failure, so it is counted in `aborted`, by path. Any other failed answer stays an error.
  const cancels = new Map();
  const cancelled = networkId => {
    if (!networkId) return Promise.resolve(false);
    let entry = cancels.get(networkId);
    if (!entry) { entry = {}; entry.seen = new Promise(done => { entry.done = done; }); cancels.set(networkId, entry); }
    return Promise.race([entry.seen, new Promise(done => setTimeout(done, 2000, false))]);
  };
  const net = new Map(), pw = [];
  const note = (id, what) => { const e = net.get(id) ?? []; e.push(what); net.set(id, e); };
  page.on('requestfailed', r => pw.push({ url: r.url(), error: r.failure()?.errorText }));
  const failed = ({ requestId, canceled, errorText, blockedReason }) => {
    note(requestId, `failed:${errorText}:${canceled}:${blockedReason ?? ''}`);
    if (!canceled) return;
    const entry = cancels.get(requestId);
    if (entry) entry.done(true); else cancels.set(requestId, { seen: Promise.resolve(true), done() {} });
  };
  const paused = event => {
    const task = handle(event).catch(async failure => {
      if (/Invalid InterceptionId/.test(String(failure?.message))) {
        await new Promise(done => setTimeout(done, 2500));
        receipt.diag = receipt.diag ?? [];
        receipt.diag.push({ networkId: event.networkId ?? null, stage: event.responseStatusCode !== undefined ? 'response' : 'request', resource: event.resourceType,
          path: new URL(event.request.url).pathname, net: net.get(event.networkId) ?? null, pw: pw.filter(x => x.url === event.request.url) });
      }
      if (/Invalid InterceptionId/.test(String(failure?.message)) && await cancelled(event.networkId)) {
        receipt.aborted.push({ path: new URL(event.request.url).pathname, resource: event.resourceType });
        return;
      }
      if (!closing && !page.isClosed()) error('Fetch.requestPaused', failure);
      // Failed candidate delivery stays a failed receipt; unblock only this request with its real response.
      if (!closing) await session.send('Fetch.continueRequest', { requestId: event.requestId }).catch(failure => { if (!page.isClosed()) error('Fetch.continueRequest', failure); });
    });
    pending.add(task); void task.finally(() => pending.delete(task));
  };
  async function handle(event) {
    if (closing) return;
    const { requestId, request, responseStatusCode: status, responseHeaders: headers = [] } = event;
    const url = new URL(request.url);
    if (url.origin !== origin || url.pathname === '/v1' || url.pathname.startsWith('/v1/') || request.method !== 'GET') return session.send('Fetch.continueRequest', { requestId });
    const response = status !== undefined || event.responseErrorReason !== undefined;
    if (response) {
      if (event.resourceType !== 'Document' || event.frameId !== topFrame) return session.send('Fetch.continueRequest', { requestId });
      security = undefined;
      const values = name => headers.filter(header => header.name.toLowerCase() === name).map(header => header.value);
      const csp = values('content-security-policy'), contentType = values('content-type').join(';');
      const replace = !event.responseErrorReason && status >= 200 && status < 300 && contentType.split(';')[0].trim().toLowerCase() === 'text/html' && csp.some(value => value.trim());
      const document = { status: status ?? null, replaced: Boolean(replace), csp_sha256: csp.length ? sha(csp.join('\n')) : null };
      receipt.documents.push(document);
      if (!replace) return session.send('Fetch.continueRequest', { requestId });
      security = headers.filter(header => securityNames.has(header.name.toLowerCase()));
      const replacement = headers.filter(header => !bodyHeaders.has(header.name.toLowerCase()));
      replacement.push({ name: 'Content-Length', value: String(html.bytes) });
      await session.send('Fetch.fulfillRequest', { requestId, responseCode: status, ...(event.responseStatusText ? { responsePhrase: event.responseStatusText } : {}), responseHeaders: replacement, body: html.body });
      document.sha256 = html.sha256;
      return;
    }
    const file = files.get(url.href);
    if (!file || /\.html?$/i.test(file.path)) return session.send('Fetch.continueRequest', { requestId });
    if (!security) {
      error('static-before-document', new Error('static candidate requested before a valid real HTML/CSP response'));
      return session.send('Fetch.continueRequest', { requestId });
    }
    const validators = Object.entries(request.headers).filter(([name]) => name.toLowerCase() === 'if-none-match').map(([, value]) => String(value));
    const unchanged = validators.some(value => value.split(',').some(tag => tag.trim().replace(/^W\//, '') === file.etag));
    const responseHeaders = [...security, { name: 'Content-Type', value: file.mime }, { name: 'Cache-Control', value: receipt.cachemode.static_cache_control }, { name: 'ETag', value: file.etag }];
    if (!unchanged) responseHeaders.push({ name: 'Content-Length', value: String(file.bytes) });
    await session.send('Fetch.fulfillRequest', { requestId, responseCode: unchanged ? 304 : 200, responseHeaders, ...(unchanged ? {} : { body: file.body }) });
    receipt.static_requests.push({ path: file.path, status: unchanged ? 304 : 200, sha256: file.sha256, bytes: unchanged ? 0 : file.bytes });
  }
  const dispose = () => {
    if (cleanup) return cleanup;
    closing = true;
    cleanup = (async () => {
      page.off('close', closed);
      // Fetch.disable releases this session's paused requests; no server/browser/context cache state is changed.
      await session.send('Fetch.disable').catch(failure => { if (!page.isClosed()) error('Fetch.disable', failure); });
      await Promise.allSettled([...pending]);
      session.off('Fetch.requestPaused', paused); session.off('Page.frameNavigated', navigated); session.off('Network.loadingFailed', failed);
      await session.detach().catch(failure => { if (!page.isClosed()) error('CDPSession.detach', failure); });
      files.clear(); cancels.clear(); html = undefined; security = undefined;
      receipt.disposed = true; installed.delete(page);
    })();
    return cleanup;
  };
  const closed = () => { void dispose(); };
  Object.defineProperty(receipt, 'dispose', { value: dispose });
  session.on('Network.requestWillBeSent', ({ requestId }) => note(requestId, 'sent'));
  session.on('Network.loadingFinished', ({ requestId }) => note(requestId, 'finished'));
  session.on('Network.responseReceived', ({ requestId, response }) => note(requestId, `response:${response.status}`));
  session.on('Fetch.requestPaused', paused); session.on('Page.frameNavigated', navigated); session.on('Network.loadingFailed', failed);
  page.once('close', closed);
  try {
    await session.send('Page.enable');
    // Only to hear which requests the browser cancelled (loadingFailed); it changes no cache behaviour.
    await session.send('Network.enable');
    topFrame = (await session.send('Page.getFrameTree')).frameTree.frame.id;
    await session.send('Fetch.enable', { patterns, handleAuthRequests: false });
    installed.set(page, receipt);
    return receipt;
  } catch (failure) {
    error('installation', failure); await dispose(); throw failure;
  }
}

module.exports = { candidateCDP };
