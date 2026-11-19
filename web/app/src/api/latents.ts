import type { Client } from "./client"

/** How often a run's preview is asked for while none newer is said: asking is what keeps its machine making them. */
const LATENT_MS = 8000

/** The newest picture a run's machine made of its latents (cr-282): its bytes and its tag. */
export interface Latent { blob: Blob; tag: string }

export interface LatentWatch {
  /** Whether this viewer wants it now (in view, its run going): it's asked for while any does. */
  wanted(on: boolean): void
  /** The run says its newest preview is `number`: a newer one than asked for is asked for now. */
  newest(number: number): void
  stop(): void
}

interface Run {
  key: string
  viewers: Set<Viewer>
  latest?: Latent
  newest: number
  /** A newer one was said while one was being asked for: asked for again as that ask ends. */
  stale?: boolean
  loading?: AbortController
  timer?: ReturnType<typeof setTimeout>
}
interface Viewer { on: boolean; told: (latent: Latent) => void }

const runs = new Map<string, Run>()

/**
 * `run`'s newest latent preview for one viewer, asked of Calcifer once for all of them: the board's card and Focus each watch, and each is
 * told each new picture (the latest at once, if there is one). An answer with no new one leaves the last in place.
 */
export function watchLatent(client: Client, project: string, run: number, told: (latent: Latent) => void): LatentWatch {
  const key = `${project} ${run}`
  let held = runs.get(key)
  if (!held) runs.set(key, (held = { key, viewers: new Set(), newest: 0 }))
  const shared = held
  const viewer: Viewer = { on: false, told }
  shared.viewers.add(viewer)
  if (shared.latest) { const latest = shared.latest; queueMicrotask(() => { if (shared.viewers.has(viewer)) told(latest) }) }
  const wanted = () => [...shared.viewers].some((each) => each.on)
  const ask = async () => {
    clearTimeout(shared.timer)
    shared.loading?.abort()
    if (runs.get(key) !== shared || !wanted()) return
    const loading = (shared.loading = new AbortController())
    shared.stale = false
    try {
      const tag = shared.latest?.tag
      const response = await client.transport.fetch(client.runPreview({ project, run }), { signal: loading.signal, priority: "low", headers: tag ? { "If-None-Match": tag } : {} })
      const now = response.headers.get("etag")
      if (response.status !== 200 || !now || now === tag) { await response.body?.cancel(); return }
      const blob = await response.blob()
      if (loading.signal.aborted) return
      const latest = (shared.latest = { blob, tag: now })
      for (const each of [...shared.viewers]) each.told(latest)
    } catch {
      // A preview is optional while the run's own result is still being made.
    } finally {
      if (shared.loading === loading) {
        shared.loading = undefined
        if (!loading.signal.aborted && runs.get(key) === shared && wanted()) {
          if (shared.stale) void ask()
          else shared.timer = setTimeout(() => void ask(), LATENT_MS)
        }
      }
    }
  }
  return {
    wanted(on) {
      if (viewer.on === on || !shared.viewers.has(viewer)) return
      const was = wanted()
      viewer.on = on
      if (!was && on) void ask()
      else if (was && !wanted()) { shared.loading?.abort(); clearTimeout(shared.timer) }
    },
    newest(number) {
      if (!number || number <= shared.newest) return
      shared.newest = number
      // Each preview is said as it's made: a short render showed none, polled every LATENT_MS (render: "a passive latent preview").
      if (shared.loading) shared.stale = true
      else if (wanted()) void ask()
    },
    stop() {
      if (!shared.viewers.delete(viewer)) return
      if (shared.viewers.size && wanted()) return
      shared.loading?.abort()
      clearTimeout(shared.timer)
      if (!shared.viewers.size) runs.delete(key)
    },
  }
}
