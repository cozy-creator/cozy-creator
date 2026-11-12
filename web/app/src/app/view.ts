import type { Viewport } from "@xyflow/react"
import type { Client } from "../api/client"
import type { ProjectView, ProjectViewPatch } from "../api/calcifer"

// Project preferences are durable only in project.db. These maps are this tab's live view.
const privateProjects = new Set<string>()
const blockedProjects = new Set<string>()
const views = new Map<string, ProjectView>()
export const privateProject = (id: string | null) => id !== null && privateProjects.has(id)
export function forgetProjectView(id: string) {
  blockedProjects.add(id)
  views.delete(id)
}
export function protectProjectView(id: string, encrypted: boolean) {
  if (encrypted) { privateProjects.add(id) }
  else privateProjects.delete(id)
}

/** Called only after a successful, un-aborted read under this project's current viewing lease. */
export function rememberProjectView(id: string, view: ProjectView) {
  blockedProjects.delete(id)
  views.set(id, view)
}

function updateProjectView(id: string, patch: ProjectViewPatch): boolean {
  if (blockedProjects.has(id)) return false
  const view = views.get(id) ?? { viewport: null, selection: [], more: [] }
  views.set(id, {
    viewport: patch.viewport ?? view.viewport,
    selection: patch.selection ?? view.selection,
    more: patch.more ?? view.more,
  })
  return true
}

function read<T>(key: string): T | null {
  try {
    return JSON.parse(localStorage.getItem(key) ?? "null")
  } catch {
    return null
  }
}

function write(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    // A convenience only.
  }
}

const TOOLS_KEY = "cozy.tools-shut"
const NSFW_KEY = "cozy.show-nsfw"

/** The global Settings preference starts on; an explicitly saved false remains off. */
export const SHOW_NSFW = true
export const loadShowNsfw = () => read<boolean>(NSFW_KEY) ?? SHOW_NSFW
export const saveShowNsfw = (shown: boolean) => write(NSFW_KEY, shown)

export const loadToolsShut = () => read<boolean>(TOOLS_KEY) === true
export const saveToolsShut = (shut: boolean) => write(TOOLS_KEY, shut)

export const loadViewport = (id: string | null) => id ? views.get(id)?.viewport ?? null : null
export const saveViewport = (id: string, viewport: Viewport) => updateProjectView(id, { viewport })
export const loadSelection = (id: string | null) => id ? views.get(id)?.selection ?? [] : []
export const saveSelection = (id: string, names: string[]) => updateProjectView(id, { selection: [...names] })
export const loadMore = (id: string | null) => id ? views.get(id)?.more ?? [] : []
export const saveMore = (id: string, names: string[]) => updateProjectView(id, { more: [...names] })

/** Known legacy keys are removed only after an exact successful readback from project.db.
 * Conflicting/newer project preferences leave the legacy value untouched for explicit retirement. */
export async function migrateLegacyProjectView(client: Client, id: string, view: ProjectView, signal: AbortSignal): Promise<ProjectView> {
  const entries: { key: string; raw: string; field: keyof ProjectView; value: unknown }[] = []
  try {
    for (const [prefix, field] of [["cozy.view:", "viewport"], ["cozy.selection:", "selection"], ["cozy.more:", "more"]] as const) {
      const key = `${prefix}${id}`, raw = localStorage.getItem(key)
      if (raw === null) continue
      const value = JSON.parse(raw)
      const valid = field === "viewport"
        ? value && typeof value.x === "number" && Number.isFinite(value.x) && typeof value.y === "number" && Number.isFinite(value.y) && typeof value.zoom === "number" && Number.isFinite(value.zoom) && value.zoom > 0
        : Array.isArray(value) && value.every((name) => typeof name === "string")
      if (valid) entries.push({ key, raw, field, value })
    }
  } catch { return view }
  if (!entries.length) return view
  const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)
  if (entries.some(({ field, value }) => !same(view[field], value) && view[field] !== null && !(Array.isArray(view[field]) && !view[field].length))) return view
  const patch = Object.fromEntries(entries.map(({ field, value }) => [field, value])) as ProjectViewPatch
  if (entries.some(({ field, value }) => !same(view[field], value))) await client.patchProjectView(id, patch, { once: signal })
  const copied = await client.projectView(id, signal)
  signal.throwIfAborted()
  if (!entries.every(({ field, value }) => same(copied[field], value))) return copied
  for (const { key, raw } of entries) {
    try { if (localStorage.getItem(key) === raw) localStorage.removeItem(key) } catch { /* verified copy remains in project.db */ }
  }
  return copied
}
