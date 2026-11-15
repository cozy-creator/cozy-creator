// Words written as the page went (`keepalive`), kept in this tab until Calcifer answers them: the next load sends each again under its own
// key, and Calcifer does it once whether or not the first reached it (cl-973). Only a plain project's: an encrypted project's words never go
// into the browser's storage. Kept as the session is, through a reload and gone with the tab.
import type { AssetPatch } from "../api/calcifer"
import { plainProject } from "../app/view"

export interface Left {
  /** The box, as Calcifer called it. */
  name: string
  body: AssetPatch
  /** Its `Idempotency-Key`'s action: the same ask again is the same key. */
  action: string
  at: number
}

/** Less than Calcifer keeps a write's answer under its key (a day): sent again after that, it could be done twice. */
const KEPT_MS = 20 * 60 * 60 * 1000
const keyOf = (project: string) => `cozy.left:${project}`

function read(project: string): Left[] {
  try {
    const held: unknown = JSON.parse(sessionStorage.getItem(keyOf(project)) ?? "[]")
    if (!Array.isArray(held)) return []
    return held.filter((left): left is Left => typeof left?.name === "string" && typeof left.action === "string" && typeof left.body === "object" && left.body !== null && Date.now() - left.at < KEPT_MS)
  } catch {
    return []
  }
}

function write(project: string, left: Left[]) {
  try {
    if (left.length) sessionStorage.setItem(keyOf(project), JSON.stringify(left))
    else sessionStorage.removeItem(keyOf(project))
  } catch {
    // Storage refused: the keepalive write is the only one.
  }
}

export function keepLeft(project: string, left: Left[]) {
  if (left.length && plainProject(project)) write(project, [...read(project), ...left])
}

/** What this tab wrote as it last went, to send again. */
export const leftOver = (project: string): Left[] => read(project)

export function answered(project: string, action: string) {
  write(project, read(project).filter((left) => left.action !== action))
}
