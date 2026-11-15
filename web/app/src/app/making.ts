import type { Opened } from "../api/calcifer"
import { signal } from "../ui/signal"

/** A new project asked for, made in place over the empty canvas (cl-980): by the strip's +, or by a first drop, which waits for it. */
export type Ask = { by: "plus" } | { by: "drop"; resolve(project: Opened): void; reject(why: Error): void }

export const asked = signal<Ask | null>(null)

/** The folder's name as it's typed: the strip shows it as the new project's tab at once. */
export const newFolder = signal("")

/** No new project after all: a drop waiting for one hears so. */
export function giveUp() {
  const ask = asked.get()
  asked.set(null)
  if (ask?.by === "drop") ask.reject(new Error("Project creation was canceled"))
}
