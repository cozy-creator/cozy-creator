// Run by the isolated native fixture: actual modules, HTTP handlers, Web Locks and BroadcastChannel.
import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import fs from "node:fs"
import { createRequire } from "node:module"
import path from "node:path"
import { DatabaseSync } from "node:sqlite"
import { pathToFileURL, fileURLToPath } from "node:url"
import { locks } from "node:worker_threads"

const app = path.dirname(path.dirname(fileURLToPath(import.meta.url)))
const require = createRequire(path.join(app, "package.json"))
const ts = require("typescript")
const temporary = fs.mkdtempSync(path.join(process.env.COZY_PRESENCE_ROOT, "modules-"))
const sourceHashes = {}
const compiled = new Map()
const baseline = process.env.COZY_PRESENCE_QUERY_BASELINE === "1"
function compile(filename) {
  if (compiled.has(filename)) return compiled.get(filename)
  const target = path.join(temporary, path.relative(path.join(app, "src"), filename).replace(/\.tsx?$/, ".mjs"))
  compiled.set(filename, target)
  let source = fs.readFileSync(filename, "utf8")
  if (baseline && filename.endsWith("/api/queries.ts")) source = execFileSync("git", ["show", "a411ddba0:web/app/src/api/queries.ts"], { cwd: app, encoding: "utf8" })
  sourceHashes[path.relative(app, filename)] = createHash("sha256").update(source).digest("hex")
  // Observe Bridge's unchanged production implementation without exporting it in the product.
  if (filename.endsWith("/app/Connection.tsx")) source += "\nexport { Bridge };\n"
  let { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX } })
  outputText = outputText.replace(/\bfrom\s+(["'])([^"']+)\1/g, (_, quote, specifier) => {
    let resolved
    if (specifier.startsWith(".")) {
      const base = path.resolve(path.dirname(filename), specifier)
      const next = [base, `${base}.ts`, `${base}.tsx`].find((candidate) => fs.existsSync(candidate) && fs.statSync(candidate).isFile())
      assert.ok(next, specifier)
      resolved = compile(next)
    } else {
      resolved = require.resolve(specifier)
      if (resolved.endsWith(".cjs") && fs.existsSync(resolved.slice(0, -4) + ".js")) resolved = resolved.slice(0, -4) + ".js"
    }
    return `from ${quote}${pathToFileURL(resolved).href}${quote}`
  })
  fs.mkdirSync(path.dirname(target), { recursive: true })
  fs.writeFileSync(target, outputText)
  return target
}

Object.defineProperty(navigator, "locks", { value: locks })
const { Client, ApiError } = await import(pathToFileURL(compile(path.join(app, "src/api/client.ts"))))
const { EventStream } = await import(pathToFileURL(compile(path.join(app, "src/api/events.ts"))))
const { projectQuery } = await import(pathToFileURL(compile(path.join(app, "src/api/queries.ts"))))
const { Bridge } = await import(pathToFileURL(compile(path.join(app, "src/app/Connection.tsx"))))
const { QueryClient, QueryObserver } = await import(pathToFileURL(require.resolve("@tanstack/react-query").replace(/\.cjs$/, ".js")))
const base = process.env.COZY_PRESENCE_URL
const token = process.env.COZY_PRESENCE_TOKEN
assert.ok(base && token && process.env.COZY_PRESENCE_ROOT)
const headers = { Authorization: `Bearer ${token}` }
const deferred = () => { let done; const promise = new Promise((resolve) => { done = resolve }); return { promise, done } }
const events = []
const eventWaiters = new Set()
const getRequests = []
const showRequests = []
const requestWaiters = new Set()
const owned = []
let delayedShow = null
let delayCreation = null
let legacy = false
let failShow = null
let connection = new AbortController()
const acknowledgements = []
const monitor = new BroadcastChannel("cozy-events")
monitor.onmessage = ({ data }) => { if (data.shown) acknowledgements.push(data.shown) }

function tellEvent(event) {
  events.push(event)
  for (const waiter of eventWaiters) if (waiter.accept(event)) { eventWaiters.delete(waiter); waiter.done(event) }
}
function waitEvent(accept, after = 0) {
  const found = events.slice(after).find(accept)
  if (found) return Promise.resolve(found)
  return new Promise((done) => eventWaiters.add({ accept, done }))
}
function requestSeen() {
  for (const waiter of requestWaiters) if (waiter.accept()) { requestWaiters.delete(waiter); waiter.done() }
}
function waitRequest(accept) {
  if (accept()) return Promise.resolve()
  return new Promise((done) => requestWaiters.add({ accept, done }))
}
function holdShow(id) {
  const entered = deferred(), release = deferred()
  delayedShow = { id, entered, release }
  return delayedShow
}
async function api(route, init = {}) {
  const response = await fetch(base + route, { ...init, headers: { ...headers, "Content-Type": "application/json", ...init.headers } })
  const body = await response.json()
  assert.ok(response.ok, JSON.stringify(body))
  return body
}
async function made(name, settle = true) {
  const after = events.length
  const project = await api("/v1/projects", { method: "POST", body: JSON.stringify({ name }) })
  assert.ok(path.resolve(project.path).startsWith(path.resolve(process.env.COZY_PRESENCE_ROOT) + path.sep))
  owned.push(project.id)
  if (settle) await waitEvent((event) => event.type === "project.changed" && event.id === project.id, after)
  return project
}
function observeHealth(stream, state) {
  if (stream.health.get().state === state) return Promise.resolve()
  return new Promise((done) => { const stop = stream.health.subscribe(() => { if (stream.health.get().state === state) { stop(); done() } }) })
}
function query(client, stream, id, signal = new AbortController().signal) {
  return (baseline ? projectQuery(client, id) : projectQuery(client, stream, id)).queryFn({ signal })
}
async function open(id) { return (await api(`/fixture/open/${id}`)).open }
async function clear(...streams) {
  const before = showRequests.length
  for (const stream of streams) stream.show(null)
  await waitRequest(() => showRequests.slice(before).some((request) => request.projects.length === 0 && request.completed))
}
async function replay(stream, shown) {
  const delivered = new Promise((done) => {
    const heard = ({ data }) => {
      if (data.shown?.tab === shown.tab && data.shown?.revision === shown.revision && data.shown?.stream === shown.stream) {
        stream.channel.removeEventListener("message", heard)
        done()
      }
    }
    // Observe delivery after EventStream's own handler; no fake acknowledgement authority.
    stream.channel.addEventListener("message", heard)
  })
  monitor.postMessage({ shown })
  await delivered
}

const transport = {
  async fetch(route, init = {}) {
    if (route.startsWith("/v1/projects/") && (!init.method || init.method === "GET")) { getRequests.push(route); requestSeen() }
    const signal = route === "/v1/events" ? AbortSignal.any([init.signal, connection.signal]) : init.signal
    let actual = route
    if (failShow && init.method === "PUT" && JSON.parse(init.body).projects.includes(failShow)) {
      actual = "/v1/events/absent-stream"
      failShow = null
    }
    let response = await fetch(base + actual, { ...init, signal, headers: { ...headers, ...init.headers } })
    if (init.method === "PUT" && route.startsWith("/v1/events/")) {
      const projects = JSON.parse(init.body).projects
      const record = { projects, status: response.status, route: actual, reply: response.status === 200 ? await response.clone().json() : null, completed: false }
      showRequests.push(record)
      if (response.ok && delayedShow && projects.includes(delayedShow.id)) {
        const held = delayedShow
        delayedShow = null
        held.entered.done(record)
        await held.release.promise
      }
      record.completed = true
      requestSeen()
      if (legacy && response.ok) { await response.arrayBuffer(); response = new Response(null, { status: 204 }) }
    }
    if (route === "/v1/events" && response.body) {
      const reader = response.body.pipeThrough(new TextDecoderStream()).getReader()
      let buffered = ""
      const body = new ReadableStream({
        async start(controller) {
          try {
            for (;;) {
              const { value, done } = await reader.read()
              if (done) { controller.close(); return }
              buffered += value.replace(/\r\n/g, "\n")
              for (let end = buffered.indexOf("\n\n"); end >= 0; end = buffered.indexOf("\n\n")) {
                const frame = buffered.slice(0, end + 2)
                buffered = buffered.slice(end + 2)
                const data = frame.split("\n").find((line) => line.startsWith("data: "))?.slice(6)
                const event = data ? JSON.parse(data) : null
                if (delayCreation && event?.type === "project.changed" && event.rev === "1.none") {
                  const held = delayCreation
                  delayCreation = null
                  held.entered.done(event)
                  await held.release.promise
                }
                controller.enqueue(new TextEncoder().encode(frame))
              }
            }
          } catch (error) { controller.error(error) }
        },
        cancel(reason) { return reader.cancel(reason) },
      })
      response = new Response(body, { status: response.status, headers: response.headers })
    }
    return response
  },
}
const client = new Client(transport, { signedOut() {}, attempt() {} })
const owner = new EventStream(client)
owner.subscribe(tellEvent)
owner.start()
await observeHealth(owner, "live")
const safety = setTimeout(() => { console.error(JSON.stringify({ fixture_protocol_timeout: true, events: events.slice(-5), showRequests, getRequests })); process.exit(1) }, 60_000)

try {
  const project = await made("gated")
  const gate = holdShow(project.id)
  const before = getRequests.length
  const read = query(client, owner, project.id)
  owner.show(project.id)
  const installed = await gate.entered.promise
  assert.ok(installed.reply.held.includes(project.id))
  assert.equal(await open(project.id), true)
  assert.equal(getRequests.length, before, "state GET escaped before exact show acknowledgement")
  gate.release.done()
  assert.equal((await read).id, project.id)
  assert.equal(getRequests.length, before + 1)
  console.log(JSON.stringify({ exact_ack_gates_actual_query: true }))

  const follower = new EventStream(client)
  follower.subscribe(() => {})
  follower.start()
  const puts = showRequests.length
  const second = query(client, follower, project.id)
  follower.show(project.id)
  assert.equal((await second).id, project.id)
  assert.equal(showRequests.length, puts, "duplicate project set made an unnecessary PUT")
  const followerAck = acknowledgements.findLast((ack) => ack.project === project.id && ack.tab !== owner.tab)
  assert.ok(followerAck)
  owner.show(null)
  assert.equal(await open(project.id), true, "one hidden tab dropped another tab's Reader")
  await clear(follower)
  assert.equal(await open(project.id), false)
  console.log(JSON.stringify({ native_locks_and_broadcast_follower: true, duplicate_set_puts: 0, last_presence_closes: true }))

  const staleGate = holdShow(project.id)
  const staleAbort = new AbortController()
  const staleStart = getRequests.length
  const staleRead = query(client, follower, project.id, staleAbort.signal)
  const staleRejected = assert.rejects(staleRead, (error) => error.name === "AbortError")
  follower.show(project.id)
  await staleGate.entered.promise
  await replay(follower, followerAck)
  assert.equal(getRequests.length, staleStart, "a previous presence revision acknowledged this query")
  follower.show(null)
  await staleRejected
  staleGate.release.done()
  await waitRequest(() => showRequests.at(-1)?.completed && showRequests.at(-1)?.projects.length === 0)
  assert.equal(await open(project.id), false)

  const healthy = await made("healthy")
  const unreadable = await made("unreadable")
  const db = new DatabaseSync(path.join(unreadable.path, "project.db"))
  db.exec("PRAGMA user_version=99")
  db.close()
  const good = query(client, owner, healthy.id)
  owner.show(healthy.id)
  assert.equal((await good).id, healthy.id)
  const bad = query(client, follower, unreadable.id)
  const refused = assert.rejects(bad, (error) => error instanceof ApiError && /schema 99/.test(error.message))
  follower.show(unreadable.id)
  await refused
  assert.equal(await open(healthy.id), true)
  assert.equal(await open(unreadable.id), false)
  await clear(owner, follower)
  console.log(JSON.stringify({ healthy_and_schema99_independent: true }))

  const hidden = await made("hidden")
  const delayed = holdShow(hidden.id)
  const hiddenBefore = getRequests.length
  const hiddenRead = query(client, owner, hidden.id)
  const hiddenRejected = assert.rejects(hiddenRead, (error) => error.name === "AbortError")
  owner.show(hidden.id)
  await delayed.entered.promise
  owner.show(null)
  await hiddenRejected
  delayed.release.done()
  await waitRequest(() => showRequests.at(-1)?.completed && showRequests.at(-1)?.projects.length === 0)
  assert.equal(getRequests.length, hiddenBefore)
  assert.equal(await open(hidden.id), false)

  const aborted = await made("aborted")
  const pending = holdShow(aborted.id)
  const abort = new AbortController()
  const abortBefore = getRequests.length
  const canceled = query(client, owner, aborted.id, abort.signal)
  const abortRejected = assert.rejects(canceled, (error) => error.name === "AbortError")
  owner.show(aborted.id)
  await pending.entered.promise
  abort.abort()
  await abortRejected
  pending.release.done()
  await waitRequest(() => showRequests.at(-1)?.completed && showRequests.at(-1)?.projects.includes(aborted.id))
  assert.equal(getRequests.length, abortBefore)
  assert.equal(await open(aborted.id), true, "a query waiter owned the Reader")
  await clear(owner)
  assert.equal(await open(aborted.id), false)
  console.log(JSON.stringify({ hidden_ack_cannot_fetch: true, unmount_cancels_waiter_not_presence: true }))

  const recovery = await made("show-failure")
  const recoveredGate = holdShow(recovery.id)
  const recoveredBefore = getRequests.length
  failShow = recovery.id
  const recovered = query(client, owner, recovery.id)
  owner.show(recovery.id)
  await recoveredGate.entered.promise
  assert.ok(showRequests.some((request) => request.projects.includes(recovery.id) && request.status === 404))
  assert.equal(getRequests.length, recoveredBefore, "failed show acknowledged the pending query")
  recoveredGate.release.done()
  assert.equal((await recovered).id, recovery.id)
  await clear(owner)
  console.log(JSON.stringify({ failed_show_uses_existing_reconnect_before_query: true, stale_revision_rejected: true }))

  const old = await made("legacy")
  legacy = true
  const oldRead = query(client, owner, old.id)
  owner.show(old.id)
  assert.equal((await oldRead).id, old.id)
  await clear(owner)
  legacy = false
  let calls = 0
  const once = new Client({ async fetch(route, init) { calls++; return transport.fetch(route, init) } }, { signedOut() {}, attempt() {} })
  await assert.rejects(once.show("absent-stream", [project.id], new AbortController().signal), (error) => error instanceof ApiError && error.status === 404)
  assert.equal(calls, 1)
  const failed = new Client({ async fetch() { calls++; throw new TypeError("controlled transport failure") } }, { signedOut() {}, attempt() {} })
  await assert.rejects(failed.show("absent-stream", [], new AbortController().signal), (error) => error instanceof ApiError && error.status === 0)
  assert.equal(calls, 2)
  console.log(JSON.stringify({ legacy204_completion: true, checked_status_and_network_failure: true, no_added_retry: true }))

  // Preserve the real Bridge fallback: delay the native creation frame until after its snapshot.
  const entered = deferred(), release = deferred()
  delayCreation = { entered, release }
  const late = await made("late-creation", false)
  const creation = await entered.promise
  const queries = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const bridge = new Bridge(queries, client)
  const unsubscribe = owner.subscribe(bridge.handle)
  const options = projectQuery(client, owner, late.id)
  const observer = new QueryObserver(queries, options)
  const stop = observer.subscribe(() => {})
  owner.show(late.id)
  await queries.fetchQuery(options)
  const loaded = getRequests.length
  release.done()
  await waitRequest(() => getRequests.slice(loaded).includes(`/v1/projects/${late.id}`))
  await new Promise((done) => {
    const check = () => { if (queries.getQueryState(["project", late.id])?.fetchStatus === "idle") { remove(); done() } }
    const remove = queries.getQueryCache().subscribe(check)
    check()
  })
  console.log(JSON.stringify({ delayed_native_creation_event: creation, independent_whole_fetch_after_snapshot: true, project_summary_has_revision: "rev" in queries.getQueryData(["project", late.id]) }))
  // A real other writer still refreshes the asset instead of being suppressed as an own echo.
  const eventFrom = events.length
  const externalReads = getRequests.length
  await api(`/v1/projects/${late.id}/assets`, { method: "POST", body: JSON.stringify({ name: "outside", tool: "", fields: { text: "external" } }) })
  await waitEvent((event) => event.type === "project.changed" && event.id === late.id && event.assets?.includes("outside"), eventFrom)
  await new Promise((done) => {
    const check = () => { if (queries.getQueryData(["project", late.id])?.assets.some((asset) => asset.name === "outside")) { remove(); done() } }
    const remove = queries.getQueryCache().subscribe(check)
    check()
  })
  assert.ok(getRequests.slice(externalReads).includes(`/v1/projects/${late.id}/assets/outside`))
  // End a real SSE connection: the new hello and show acknowledgement must still refresh.
  const requested = getRequests.length
  const oldAck = acknowledgements.findLast((ack) => ack.project === late.id && ack.tab === owner.tab)
  assert.ok(oldAck)
  const reconnected = holdShow(late.id)
  connection.abort()
  connection = new AbortController()
  await reconnected.entered.promise
  await replay(owner, oldAck)
  assert.equal(getRequests.length, requested, "old stream acknowledgement released a reconnected query")
  reconnected.release.done()
  await waitRequest(() => getRequests.slice(requested).includes(`/v1/projects/${late.id}`))
  assert.equal(owner.health.get().state, "live")
  stop(); unsubscribe(); queries.clear()
  await clear(owner, follower)
  for (const id of owned) assert.equal(await open(id), false, `Reader retained after hide: ${id}`)
  console.log(JSON.stringify({ genuine_external_refresh_and_new_hello_preserved: true, stale_stream_ack_rejected: true, readers_released: owned, source_hashes: sourceHashes }))
  clearTimeout(safety)
  fs.rmSync(temporary, { recursive: true, force: true })
  console.log(JSON.stringify({ compiled_modules_removed: !fs.existsSync(temporary), no_user_profile_or_service: true }))
  process.exit(0)
} catch (error) {
  clearTimeout(safety)
  fs.rmSync(temporary, { recursive: true, force: true })
  console.error(error)
  process.exit(1)
}
