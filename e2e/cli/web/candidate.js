// Serve a built candidate at the real Calcifer origin. API and media traffic, auth, and security headers stay real.
const { readFileSync, readdirSync, realpathSync } = require('node:fs');
const { extname, join } = require('node:path');
const installed = new WeakMap();
async function candidateUI(page, origin) {
  if (!process.env.CANDIDATE_DIST) return;
  let origins = installed.get(page);
  if (!origins) installed.set(page, (origins = new Set()));
  if (origins.has(origin)) return;
  origins.add(origin);
  const root = realpathSync(process.env.CANDIDATE_DIST), files = new Map();
  const read = (directory, prefix = '') => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const name = `${prefix}/${entry.name}`, path = join(directory, entry.name);
      if (entry.isDirectory()) read(path, name);
      else if (entry.isFile()) files.set(name, readFileSync(path));
      else throw new Error(`Non-regular candidate asset: ${name}`);
    }
  };
  read(root);
  const html = files.get('/index.html');
  if (!html || html.includes('@vite/client')) throw new Error('Candidate must be a production build');
  const mime = { '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.webp': 'image/webp', '.woff2': 'font/woff2', '.woff': 'font/woff', '.wasm': 'application/wasm', '.json': 'application/json', '.ico': 'image/x-icon' };
  let security;
  await page.route(`${origin}/**`, async route => {
    const request = route.request(), url = new URL(request.url());
    if (url.origin !== origin || request.method() !== 'GET' || url.pathname.startsWith('/v1')) return route.continue();
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) {
      const response = await route.fetch(), headers = response.headers();
      if (!response.ok() || !headers['content-type']?.includes('text/html') || !headers['content-security-policy']) return route.fulfill({ response });
      security = Object.fromEntries(Object.entries(headers).filter(([key]) => ['content-security-policy', 'referrer-policy', 'x-content-type-options', 'cross-origin-opener-policy', 'cross-origin-embedder-policy'].includes(key)));
      for (const key of ['content-length', 'content-encoding', 'etag', 'last-modified', 'transfer-encoding']) delete headers[key];
      return route.fulfill({ response, headers: { ...headers, 'cache-control': 'no-store' }, body: html });
    }
    const body = files.get(decodeURIComponent(url.pathname));
    if (!body || !security) return route.continue();
    return route.fulfill({ status: 200, headers: { ...security, 'cache-control': 'no-store', 'content-type': mime[extname(url.pathname)] ?? 'application/octet-stream' }, body });
  });
}
module.exports = { candidateUI };
