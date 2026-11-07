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

const viewKey = (scope: string, id: string) => `cozy.view:${scope}:${id}`
const selectionKey = (scope: string, id: string) => `cozy.selection:${scope}:${id}`
const moreKey = (scope: string, id: string) => `cozy.more:${scope}:${id}`

const machineKey = (scope: string) => `cozy.machine:${scope}`
const TOOLS_KEY = "cozy.tools-shut"
const NSFW_KEY = "cozy.show-nsfw"

/** Whether lists show models tagged `nsfw` until a viewer says otherwise: shown, for now. */
export const SHOW_NSFW = true
export const loadShowNsfw = () => read<boolean>(NSFW_KEY) ?? SHOW_NSFW
export const saveShowNsfw = (shown: boolean) => write(NSFW_KEY, shown)

export const loadToolsShut = () => read<boolean>(TOOLS_KEY) === true
export const saveToolsShut = (shut: boolean) => write(TOOLS_KEY, shut)

export const loadMachine = (scope: string) => read<string>(machineKey(scope))
export const saveMachine = (scope: string, name: string) => write(machineKey(scope), name)
export const loadViewport = (scope: string, id: string | null) => (id ? read<Viewport>(viewKey(scope, id)) : null)
export const saveViewport = (scope: string, id: string, viewport: Viewport) => write(viewKey(scope, id), viewport)
export const loadSelection = (scope: string, id: string | null) => (id ? (read<string[]>(selectionKey(scope, id)) ?? []) : [])
export const saveSelection = (scope: string, id: string, names: string[]) => write(selectionKey(scope, id), names)
export const loadMore = (scope: string, id: string | null) => (id ? (read<string[]>(moreKey(scope, id)) ?? []) : [])
export const saveMore = (scope: string, id: string, names: string[]) => write(moreKey(scope, id), names)
