import type { Viewport } from "@xyflow/react"

// Viewport and selection stay in the browser. They can come back empty, and the page works without them.

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

/** Whether lists show models tagged `nsfw` until a viewer says otherwise: shown, for now. */
export const SHOW_NSFW = true
export const loadShowNsfw = () => read<boolean>(NSFW_KEY) ?? SHOW_NSFW
export const saveShowNsfw = (shown: boolean) => write(NSFW_KEY, shown)

export const loadToolsShut = () => read<boolean>(TOOLS_KEY) === true
export const saveToolsShut = (shut: boolean) => write(TOOLS_KEY, shut)

export const loadViewport = (id: string | null) => (id ? read<Viewport>(viewKey(id)) : null)
export const saveViewport = (id: string, viewport: Viewport) => write(viewKey(id), viewport)
export const loadSelection = (id: string | null) => (id ? (read<string[]>(selectionKey(id)) ?? []) : [])
export const saveSelection = (id: string, names: string[]) => write(selectionKey(id), names)
export const loadMore = (id: string | null) => (id ? (read<string[]>(moreKey(id)) ?? []) : [])
export const saveMore = (id: string, names: string[]) => write(moreKey(id), names)
