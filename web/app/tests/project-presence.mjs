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
const baselineEvents = process.env.COZY_PRESENCE_EVENTS_BASELINE === "1"
const baselineCoordination = process.env.COZY_PRESENCE_COORDINATION_BASELINE === "1"
const baselineBridge = process.env.COZY_PRESENCE_BRIDGE_BASELINE === "1"
const ackOnly = process.env.COZY_PRESENCE_ACK_ONLY === "1"
function compile(filename, variant = "current") {
  const key = `${variant}:${filename}`
  if (compiled.has(key)) return compiled.get(key)
  const target = path.join(temporary, variant, path.relative(path.join(app, "src"), filename).replace(/\.tsx?$/, ".mjs"))
  compiled.set(key, target)
  let source = fs.readFileSync(filename, "utf8")
  if (variant === "legacy" && /\/api\/(events|client)\.ts$/.test(filename)) source = execFileSync("git", ["show", `a411ddba0:web/app/${path.relative(app, filename)}`], { cwd: app, encoding: "utf8" })
  if (baseline && filename.endsWith("/api/queries.ts")) source = execFileSync("git", ["show", "a411ddba0:web/app/src/api/queries.ts"], { cwd: app, encoding: "utf8" })
  if (baselineEvents && filename.endsWith("/api/events.ts")) source = execFileSync("git", ["show", "0e7cd997b:web/app/src/api/events.ts"], { cwd: app, encoding: "utf8" })
  if (baselineCoordination && variant === "current" && filename.endsWith("/api/events.ts")) source = execFileSync("git", ["show", "021e52427:web/app/src/api/events.ts"], { cwd: app, encoding: "utf8" })
  if (baselineBridge && filename.endsWith("/app/Connection.tsx")) source = execFileSync("git", ["show", "0e7cd997b:web/app/src/app/Connection.tsx"], { cwd: app, encoding: "utf8" })
  sourceHashes[`${variant}/${path.relative(app, filename)}`] = createHash("sha256").update(source).digest("hex")
  // Observe Bridge's unchanged production implementation without exporting it in the product.
  if (filename.endsWith("/app/Connection.tsx")) source += "\nexport { Bridge };\n"
  if (filename.endsWith("/api/events.ts") && source.includes("const COORDINATION =")) source += "\nexport { COORDINATION };\n"
  let { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX } })
  outputText = outputText.replace(/\bfrom\s+(["'])([^"']+)\1/g, (_, quote, specifier) => {
    let resolved
    if (specifier.startsWith(".")) {
      const base = path.resolve(path.dirname(filename), specifier)
      const next = [base, `${base}.ts`, `${base}.tsx`].find((candidate) => fs.existsSync(candidate) && fs.statSync(candidate).isFile())
      assert.ok(next, specifier)
      resolved = compile(next, variant)
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
const { EventStream, COORDINATION } = await import(pathToFileURL(compile(path.join(app, "src/api/events.ts"))))
const { projectQuery, withAssets, yamlQuery } = await import(pathToFileURL(compile(path.join(app, "src/api/queries.ts"))))
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
const showStarts = []
const requestWaiters = new Set()
const owned = []
let delayedShow = null
let delayedSnapshot = null
let delayedWhole = null
let legacy = false
let failShow = null
let connection = new AbortController()
const acknowledgements = []
const monitor = new BroadcastChannel(COORDINATION ?? "cozy-events")
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
async function made(name) {
  const project = await api("/v1/projects", { method: "POST", body: JSON.stringify({ name }) })
  assert.ok(path.resolve(project.path).startsWith(path.resolve(process.env.COZY_PRESENCE_ROOT) + path.sep))
  owned.push(project.id)
  return project
}
function observeHealth(stream, state) {
  if (stream.health.get().state === state) return Promise.resolve()
  return new Promise((done) => { const stop = stream.health.subscribe(() => { if (stream.health.get().state === state) { stop(); done() } }) })
}
const queryCache = new QueryClient()
function query(client, stream, id, signal = new AbortController().signal) {
  return (baseline ? projectQuery(client, id) : projectQuery(client, stream, id)).queryFn({ client: queryCache, signal })
}
async function open(id) { return (await api(`/fixture/open/${id}`)).open }
function readsOf(bridge) {
  const records = [], waits = new Set()
  const notify = () => { for (const wait of waits) if (wait.test()) { waits.delete(wait); wait.done() } }
  const original = bridge.read
  bridge.read = async function (...args) {
    const record = { done: false }
    records.push(record); notify()
    try { return await original.apply(this, args) }
    finally { record.done = true; notify() }
  }
  const wait = (test) => test() ? Promise.resolve() : new Promise((done) => waits.add({ test, done }))
  return { mark: () => records.length, entered: (mark) => wait(() => records.length > mark), done: (mark) => wait(() => records.slice(mark).some((record) => record.done)) }
}
async function writeYaml(id, count, label, changed = count, previous = label) {
  let text = "version: 4\nassets:\n"
  for (let i = 0; i < count; i++) text += `  w${i}: {text: "${i < changed ? label : previous} ${i}"}\n`
  const response = await fetch(`${base}/v1/projects/${id}/yaml`, { method: "PUT", headers: { ...headers, "Content-Type": "application/yaml" }, body: text })
  const result = await response.text()
  assert.ok(response.ok, `${response.status} ${result}`)
}
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
    if (init.method === "PUT" && route.startsWith("/v1/events/")) showStarts.push(route)
    if (route.startsWith("/v1/projects/") && (!init.method || init.method === "GET")) { getRequests.push(route); requestSeen() }
    const signal = route === "/v1/events" ? AbortSignal.any([init.signal, connection.signal]) : init.signal
    let actual = route
    if (failShow && init.method === "PUT" && JSON.parse(init.body).projects.includes(failShow)) {
      actual = "/v1/events/absent-stream"
      failShow = null
    }
    let response = await fetch(base + actual, { ...init, signal, headers: { ...headers, ...init.headers } })
    if (delayedSnapshot && route === `/v1/projects/${delayedSnapshot.id}` && (!init.method || init.method === "GET")) {
      const held = delayedSnapshot
      delayedSnapshot = null
      held.entered.done(await response.clone().json())
      await held.release.promise
    }
    if (init.method === "PUT" && route.startsWith("/v1/events/")) {
      const projects = JSON.parse(init.body).projects
      const record = { projects, status: response.status, route: actual, reply: response.status === 200 ? await response.clone().json() : null, completed: false }
      showRequests.push(record)
      if (response.ok && delayedShow && projects.includes(delayedShow.id)) {
        const held = delayedShow
        delayedShow = null
        held.entered.done(record)
        await held.release.promise
        if (held.rejectAfter) throw new TypeError("controlled deferred old-stream failure")
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
                if (delayedWhole && event?.type === "project.changed" && event.id === delayedWhole.id && !event.assets) {
                  const held = delayedWhole
                  delayedWhole = null
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
if (process.env.COZY_PRESENCE_OLD_HOLDER === "1") {
  const { EventStream: OldEvents } = await import(pathToFileURL(compile(path.join(app, "src/api/events.ts"), "legacy")))
  const { Client: OldClient } = await import(pathToFileURL(compile(path.join(app, "src/api/client.ts"), "legacy")))
  const old = new OldEvents(new OldClient(transport, { signedOut() {}, attempt() {} }))
  old.subscribe(tellEvent)
  old.start()
  await observeHealth(old, "live")
  const oldProject = await made("old-page-reader")
  const oldBefore = showRequests.length
  old.show(oldProject.id)
  await waitRequest(() => showRequests.slice(oldBefore).some((request) => request.projects.includes(oldProject.id) && request.completed))
  assert.equal(await open(oldProject.id), true)
  const target = await made("old-page-overlap")
  const current = new EventStream(client)
  const confirmed = deferred()
  for (const [label, actor] of [["old", old], ["current", current]]) {
    const original = actor.tell
    actor.tell = function (...args) {
      const task = original.apply(this, args)
      task.then(() => {
        const key = typeof this.told === "string" ? this.told : this.told?.key
        if (key?.split(",").includes(target.id)) confirmed.done(label)
      }, () => {})
      return task
    }
  }
  const deadline = setTimeout(() => { console.error("old/new coordination protocol did not complete"); process.exit(1) }, 60_000)
  try {
    current.start()
    const before = getRequests.length
    const state = query(client, current, target.id)
    current.show(target.id)
    const holder = await confirmed.promise
    assert.equal(await open(target.id), true)
    console.log(JSON.stringify({ exact_old_holder: "a411ddba0", holder_for_new_query: holder, state_requests_after_ack: getRequests.length - before }))
    assert.equal(holder, "current", "old page owns incompatible coordination and cannot acknowledge the new follower")
    assert.equal((await state).id, target.id)
    assert.notEqual(old.stream, current.stream)
    await clear(current)
    assert.equal(await open(target.id), false)
    assert.equal(await open(oldProject.id), true, "new cohort closed the old cohort's Reader")
    const hiddenBefore = showRequests.length
    old.show(null)
    await waitRequest(() => showRequests.slice(hiddenBefore).some((request) => request.projects.length === 0 && request.completed))
    assert.equal(await open(oldProject.id), false)
    assert.equal(old.health.get().state, "live")
    const closeBefore = showRequests.length
    old.show(oldProject.id)
    current.show(target.id)
    await waitRequest(() => [oldProject.id, target.id].every((id) => showRequests.slice(closeBefore).some((request) => request.projects.includes(id) && request.completed)))
    assert.equal(await open(oldProject.id), true)
    assert.equal(await open(target.id), true)
    console.log(JSON.stringify({ old_page_preserved: true, new_query_completed: true, independent_streams: true, hide_releases_each_reader: true, close_on_exit: [oldProject.id, target.id], source_hashes: sourceHashes }))
    clearTimeout(deadline)
    fs.rmSync(temporary, { recursive: true, force: true })
    process.exit(0)
  } catch (error) {
    clearTimeout(deadline)
    fs.rmSync(temporary, { recursive: true, force: true })
    console.error(error)
    process.exit(1)
  }
}
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
  const failedState = await fetch(`${base}/v1/projects/${unreadable.id}`, { headers })
  assert.equal(failedState.ok, false)
  assert.equal("covered" in await failedState.json(), false)
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

  const staleFailure = await made("stale-error")
  const oldEpoch = owner.stream
  const oldGate = holdShow(staleFailure.id)
  oldGate.rejectAfter = true
  const oldDone = deferred()
  const tell = owner.tell
  let captured = false
  owner.tell = function (...args) {
    const capture = !captured && !this.telling && this.stream === oldEpoch
    if (capture) captured = true
    const task = tell.apply(this, args)
    if (capture) task.then(oldDone.done, oldDone.done)
    return task
  }
  const staleGet = getRequests.length
  const stalePending = query(client, owner, staleFailure.id)
  owner.show(staleFailure.id)
  await oldGate.entered.promise
  const reconnectFrom = events.length
  connection.abort()
  connection = new AbortController()
  const newHello = await waitEvent((event) => event.type === "hello" && event.stream !== oldEpoch, reconnectFrom)
  const starts = showStarts.length
  const newGate = holdShow(staleFailure.id)
  oldGate.release.done()
  await oldDone.promise
  assert.equal(showStarts.length, starts + 1, "stale show rejection stranded the newer live stream")
  assert.ok(showStarts.at(-1).endsWith(newHello.stream))
  await newGate.entered.promise
  assert.equal(getRequests.length, staleGet)
  newGate.release.done()
  assert.equal((await stalePending).id, staleFailure.id)
  owner.tell = tell
  await clear(owner)
  console.log(JSON.stringify({ deferred_old_rejection_reconciles_new_hello: true, no_retry_on_old_stream: true }))

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

  if (ackOnly) {
    for (const id of owned) assert.equal(await open(id), false)
    clearTimeout(safety)
    fs.rmSync(temporary, { recursive: true, force: true })
    console.log(JSON.stringify({ ack_only_complete: true, readers_released: owned, modules_removed: true }))
    process.exit(0)
  }

  // Delay a real whole-project frame across its full snapshot, keeping YAML invalidation.
  const entered = deferred(), release = deferred()
  const late = await made("late-creation")
  owner.show(late.id)
  await query(client, owner, late.id)
  delayedWhole = { id: late.id, entered, release }
  await writeYaml(late.id, 257, "before")
  const creation = await entered.promise
  const queries = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const bridge = new Bridge(queries, client)
  let unsubscribe = owner.subscribe(bridge.handle)
  const reads = readsOf(bridge)
  const options = projectQuery(client, owner, late.id)
  const observer = new QueryObserver(queries, options)
  const stop = observer.subscribe(() => {})
  owner.show(late.id)
  await queries.fetchQuery(options)
  const yOptions = yamlQuery(client, late.id)
  const yObserver = new QueryObserver(queries, yOptions)
  const yStop = yObserver.subscribe(() => {})
  await queries.fetchQuery(yOptions)
  const loaded = getRequests.filter((route) => route === `/v1/projects/${late.id}`).length
  const yamlReads = getRequests.filter((route) => route === `/v1/projects/${late.id}/yaml`).length
  const initialRead = reads.mark()
  release.done()
  await reads.done(initialRead)
  assert.equal(getRequests.filter((route) => route === `/v1/projects/${late.id}`).length, loaded, "covered pre-snapshot event reloaded the whole project")
  assert.ok(getRequests.filter((route) => route === `/v1/projects/${late.id}/yaml`).length > yamlReads)
  const key = ["project", late.id]
  const firstSnapshot = queries.getQueryData(key)
  assert.equal(firstSnapshot.covered.generation, creation.cursor.generation)
  assert.ok(firstSnapshot.covered.sequence >= creation.cursor.sequence)
  console.log(JSON.stringify({ delayed_native_whole_event: creation, snapshot_covered: firstSnapshot.covered, redundant_project_gets: 0, yaml_invalidation_preserved: true }))
  // A real other writer still refreshes the asset instead of being suppressed as an own echo.
  unsubscribe()
  const eventFrom = events.length
  const externalReads = getRequests.length
  await api(`/v1/projects/${late.id}/assets`, { method: "POST", body: JSON.stringify({ name: "outside", tool: "", fields: { text: "external" } }) })
  const named = await waitEvent((event) => event.type === "project.changed" && event.id === late.id && event.assets?.includes("outside"), eventFrom)
  const mixed = reads.mark()
  bridge.changed(late.id, { assets: null, cursor: creation.cursor })
  bridge.handle(named)
  await reads.done(mixed)
  assert.ok(queries.getQueryData(key).assets.some((asset) => asset.name === "outside"))
  assert.equal(getRequests.filter((route) => route === `/v1/projects/${late.id}`).length, loaded)
  assert.ok(getRequests.slice(externalReads).includes(`/v1/projects/${late.id}/assets/outside`))
  assert.deepEqual(queries.getQueryData(key).covered, firstSnapshot.covered, "head delta promoted full asset coverage")
  const realHead = await client.projectHead(late.id)
  assert.equal(realHead.covered, undefined)
  assert.deepEqual(withAssets(firstSnapshot, [], [], { ...realHead, covered: { ...firstSnapshot.covered, sequence: firstSnapshot.covered.sequence + 100 } }).covered, firstSnapshot.covered)
  unsubscribe = owner.subscribe(bridge.handle)
  console.log(JSON.stringify({ covered_whole_plus_newer_named_delta_retained: true, head_never_promotes_full_coverage: true }))

  // The response snapshot is captured, but not delivered, when a newer genuine whole event emits.
  const snapshotGate = { id: late.id, entered: deferred(), release: deferred() }
  delayedSnapshot = snapshotGate
  const pendingSnapshot = queries.refetchQueries({ queryKey: key })
  const snapshotData = await snapshotGate.entered.promise
  const duringMark = reads.mark(), duringEvents = events.length
  await writeYaml(late.id, 257, "during")
  const during = await waitEvent((event) => event.type === "project.changed" && event.id === late.id && !event.assets, duringEvents)
  assert.equal(during.cursor.generation, snapshotData.covered.generation)
  assert.ok(during.cursor.sequence > snapshotData.covered.sequence, "snapshot claimed a later emission")
  await reads.entered(duringMark)
  snapshotGate.release.done()
  await pendingSnapshot
  await reads.done(duringMark)
  assert.equal(queries.getQueryData(key).assets.length, 257)
  const beforeAfter = queries.getQueryData(key)
  const afterMark = reads.mark(), afterEvents = events.length
  await writeYaml(late.id, 258, "after")
  const after = await waitEvent((event) => event.type === "project.changed" && event.id === late.id && !event.assets, afterEvents)
  assert.ok(after.cursor.sequence > beforeAfter.covered.sequence)
  await reads.done(afterMark)
  assert.equal(queries.getQueryData(key).assets.length, 258)
  console.log(JSON.stringify({ newer_emission_while_snapshot_response_pending: during.cursor, captured_before_read: snapshotData.covered, after_cached_snapshot: after.cursor, genuine_whole_refreshes_preserved: true }))

  async function boundary(label, causes, expected, cache, handle = false) {
    if (cache) queries.setQueryData(key, cache)
    const count = getRequests.filter((route) => route === `/v1/projects/${late.id}`).length
    const mark = reads.mark()
    for (const cause of causes) {
      if (handle) bridge.handle(cause)
      else bridge.changed(late.id, cause)
    }
    await reads.done(mark)
    assert.equal(getRequests.filter((route) => route === `/v1/projects/${late.id}`).length - count, expected, label)
    console.log(JSON.stringify({ coverage_boundary: label, whole_gets: expected }))
  }
  const full = queries.getQueryData(key)
  await boundary("same generation covered maximum", [{ assets: null, cursor: creation.cursor }, { assets: null, cursor: after.cursor }], 0)
  await boundary("older whole plus newer whole", [{ assets: null, cursor: creation.cursor }, { assets: null, cursor: after.cursor }], 1, beforeAfter)
  await boundary("unknown cause poisons covered maximum", [{ assets: null, cursor: after.cursor }, { assets: null }, { assets: null, cursor: creation.cursor }], 1)
  await boundary("different generation", [{ assets: null, cursor: { ...after.cursor, generation: "another-controller-generation" } }], 1)
  await boundary("record prior opaque revision", [creation], 0, undefined, true)
  await boundary("same opaque revision with new generation", [{ ...creation, cursor: { ...creation.cursor, generation: "restarted-controller-generation" } }], 1, undefined, true)
  await boundary("same opaque revision with newer emission", [{ ...after, rev: creation.rev }], 1, beforeAfter, true)
  const { covered: ignored, ...legacySnapshot } = full
  void ignored
  await boundary("older snapshot without cursor", [{ assets: null, cursor: creation.cursor }], 1, legacySnapshot)
  await boundary("head-only cache", [{ assets: null, cursor: creation.cursor }], 1, { ...realHead, began: performance.now() })
  queries.getQueryCache().find({ queryKey: key }).setState({ data: undefined })
  await boundary("no cached full state", [{ assets: null, cursor: creation.cursor }], 1)

  // Count fallback remains conservative even when a later full read covers all 65 names.
  unsubscribe()
  const manyEvents = events.length
  await writeYaml(late.id, 258, "sixtyfive", 65, "after")
  const many = await waitEvent((event) => event.type === "project.changed" && event.id === late.id && event.assets?.length === 65, manyEvents)
  const newest = await api(`/v1/projects/${late.id}`)
  assert.ok(newest.covered.sequence >= many.cursor.sequence)
  await boundary("more than64 names", [many], 1, { ...newest, began: performance.now() })
  unsubscribe = owner.subscribe(bridge.handle)
  // End a real SSE connection: the new hello and show acknowledgement must still refresh.
  const requested = getRequests.length
  const stateRequests = getRequests.filter((route) => route === `/v1/projects/${late.id}`).length
  const oldAck = acknowledgements.findLast((ack) => ack.project === late.id && ack.tab === owner.tab)
  assert.ok(oldAck)
  const reconnected = holdShow(late.id)
  connection.abort()
  connection = new AbortController()
  await reconnected.entered.promise
  await replay(owner, oldAck)
  assert.equal(getRequests.filter((route) => route === `/v1/projects/${late.id}`).length, stateRequests, "old stream acknowledgement released a reconnected state query")
  reconnected.release.done()
  await waitRequest(() => getRequests.slice(requested).includes(`/v1/projects/${late.id}`))
  assert.equal(owner.health.get().state, "live")
  stop(); yStop(); unsubscribe(); queries.clear()
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
