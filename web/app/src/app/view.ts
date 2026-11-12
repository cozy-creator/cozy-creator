import type { Viewport } from "@xyflow/react"

// Ordinary-project view preferences can stay in the browser. Encrypted-project names and
// selections are never persisted; a locked project also removes preferences saved before conversion.
const privateProjects = new Set<string>()
export const privateProject = (id: string | null) => id !== null && privateProjects.has(id)
export function forgetProjectView(id: string) {
  for (const prefix of ["cozy.view:", "cozy.selection:", "cozy.more:"]) {
    try { localStorage.removeItem(`${prefix}${id}`) } catch { /* storage is optional */ }
  }
}
export function protectProjectView(id: string, encrypted: boolean) {
  if (encrypted) { privateProjects.add(id); forgetProjectView(id) }
  else privateProjects.delete(id)
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

const viewKey = (id: string) => `cozy.view:${id}`
const selectionKey = (id: string) => `cozy.selection:${id}`
const moreKey = (id: string) => `cozy.more:${id}`
const TOOLS_KEY = "cozy.tools-shut"
const NSFW_KEY = "cozy.show-nsfw"

/** The global Settings preference starts on; an explicitly saved false remains off. */
export const SHOW_NSFW = true
export const loadShowNsfw = () => read<boolean>(NSFW_KEY) ?? SHOW_NSFW
export const saveShowNsfw = (shown: boolean) => write(NSFW_KEY, shown)

export const loadToolsShut = () => read<boolean>(TOOLS_KEY) === true
export const saveToolsShut = (shut: boolean) => write(TOOLS_KEY, shut)

export const loadViewport = (id: string | null) => (id && !privateProject(id) ? read<Viewport>(viewKey(id)) : null)
export const saveViewport = (id: string, viewport: Viewport) => privateProject(id) ? undefined : write(viewKey(id), viewport)
export const loadSelection = (id: string | null) => (id && !privateProject(id) ? (read<string[]>(selectionKey(id)) ?? []) : [])
export const saveSelection = (id: string, names: string[]) => privateProject(id) ? undefined : write(selectionKey(id), names)
export const loadMore = (id: string | null) => (id && !privateProject(id) ? (read<string[]>(moreKey(id)) ?? []) : [])
export const saveMore = (id: string, names: string[]) => privateProject(id) ? undefined : write(moreKey(id), names)
