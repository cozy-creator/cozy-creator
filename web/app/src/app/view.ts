import type { Viewport } from "@xyflow/react"
import type { ProjectView, ProjectViewPatch } from "../api/calcifer"

// Project preferences are durable only in project.db. These maps are this tab's live view.
const privateProjects = new Set<string>()
const blockedProjects = new Set<string>()
const views = new Map<string, ProjectView>()
export const privateProject = (id: string | null) => id !== null && privateProjects.has(id)
function forgetLegacyProjectView(id: string) {
  for (const prefix of ["cozy.view:", "cozy.selection:", "cozy.more:"]) {
    try { localStorage.removeItem(`${prefix}${id}`) } catch { /* storage is optional */ }
  }
}
export function forgetProjectView(id: string) {
  blockedProjects.add(id)
  views.delete(id)
  forgetLegacyProjectView(id)
}
export function protectProjectView(id: string, encrypted: boolean) {
  if (encrypted) { privateProjects.add(id); forgetLegacyProjectView(id) }
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

/** Whether lists show models tagged `nsfw` until a viewer says otherwise: shown, for now. */
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
