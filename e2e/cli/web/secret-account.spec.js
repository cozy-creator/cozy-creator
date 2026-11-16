// secret-account (cl-1066, solo): a passphrase leaves the page only for its own OS account's Calcifer. The rest of protocol 3 is gone;
// this check stays (coordinator, 11-16: "a passphrase or recovery secret reaching another OS account's process is exactly the harm the
// rule keeps, even if that Calcifer would then refuse"). Calcifer stopped and another account's process at its address, Create project
// sends that process nothing: the page asks who answers (`GET /v1/status`'s `os_user`), and says why in one line. Without the check,
// the passphrase went to it.
const { spawn, spawnSync } = require('child_process');
const { existsSync, readFileSync } = require('fs');
const { dirname, join } = require('path');
const { test, expect } = require('./fixtures');

const OTHER = 'cozy-secret-other';
const PASSPHRASE = 'not-for-another-account';
// Another account's server at an address: it notes each request (whether its body held a passphrase), and answers as its own account.
const SQUAT = `import http.server, json, os, sys
log = open(sys.argv[3], "a", buffering=1)
class Handler(http.server.BaseHTTPRequestHandler):
    def answer(self):
        size = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(size) if size else b""
        log.write(json.dumps({"method": self.command, "path": self.path, "passphrase": b"passphrase" in body}) + "\\n")
        said = json.dumps({"os_user": str(os.getuid())}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(said)))
        self.end_headers()
        self.wfile.write(said)
    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = answer
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer.allow_reuse_address = True
http.server.ThreadingHTTPServer((sys.argv[1], int(sys.argv[2])), Handler).serve_forever()`;

const asked = (log) => (existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((line) => JSON.parse(line)) : []);
const answering = (address) => fetch(`http://${address}/v1/status`, { signal: AbortSignal.timeout(2_000) }).then((r) => r.json(), () => null).catch(() => null);

test('secret-account', { tag: '@solo' }, async ({ page, project, guards, calciferControl, removeAfter }) => {
  test.skip(process.platform !== 'linux' || spawnSync('sudo', ['-n', 'true']).status !== 0, 'another OS account: Linux, with sudo');
  spawnSync('sudo', ['-n', 'useradd', '-m', OTHER]);
  const other = spawnSync('id', ['-u', OTHER], { encoding: 'utf8' }).stdout.trim();
  expect(other, 'the other account').toMatch(/^\d+$/);
  const { address } = calciferControl.holder;
  const [host, port] = [address.slice(0, address.lastIndexOf(':')), address.slice(address.lastIndexOf(':') + 1)];
  const log = `/tmp/${OTHER}-${process.pid}-${Date.now()}.log`;
  const folder = join(dirname(project.path), `web-secret-${Date.now()}`);
  removeAfter(folder);

  await page.getByRole('button', { name: 'New project', exact: true }).click();
  await page.getByLabel("New project's folder", { exact: true }).fill(folder);
  await page.getByLabel('Passphrase', { exact: true }).fill(PASSPHRASE);
  await page.getByLabel('Confirm passphrase', { exact: true }).fill(PASSPHRASE);
  const create = page.getByRole('button', { name: 'Create project', exact: true });
  await expect(create).toBeEnabled();

  // Calcifer away, the page's requests fail or reach the other account's process, as the browser says: what this cell does.
  let away = true;
  guards.allow({ test: () => away });
  await calciferControl.stop();
  const squatter = spawn('sudo', ['-n', '-u', OTHER, 'python3', '-c', SQUAT, host, port, log], { stdio: 'ignore' });
  try {
    await expect.poll(async () => (await answering(address))?.os_user, { message: `${OTHER} answers at ${address}`, timeout: 30_000 }).toBe(other);
    await create.click();
    await expect(page.getByText(/runs as another account on this computer, so nothing secret was sent/), 'said in one line').toBeVisible({ timeout: 30_000 });
    const seen = asked(log);
    expect(seen.filter((one) => one.passphrase), `what reached ${OTHER}'s process: ${JSON.stringify(seen)}`).toHaveLength(0);
    expect(seen.some((one) => one.method === 'GET' && one.path === '/v1/status'), `the page asked who answers: ${JSON.stringify(seen)}`).toBe(true);
    expect(existsSync(folder), 'and no project was made').toBe(false);
  } finally {
    // Its process alone: the account is this cell's.
    squatter.kill('SIGTERM');
    spawnSync('sudo', ['-n', 'pkill', '-u', OTHER]);
    await expect.poll(async () => (await answering(address)) === null, { message: `${OTHER}'s process gone`, timeout: 30_000 }).toBe(true);
    await calciferControl.start();
    away = false;
  }
});
