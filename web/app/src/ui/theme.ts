import { signal } from "./signal"

export type Theme = "paper" | "dark"

const KEY = "cozy.theme"
const LANDSCAPE = "cozy.landscape"

function read(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function keep(key: string, value: string) {
  try {
    localStorage.setItem(key, value)
  } catch {
    // Private windows may refuse. It still holds for this tab.
  }
}

/** Paper is the root's own; only dark is marked. */
function show(value: Theme) {
  if (value === "dark") document.documentElement.dataset.theme = "dark"
  else delete document.documentElement.dataset.theme
}

export const theme = signal<Theme>(read(KEY) === "dark" ? "dark" : "paper")
show(theme.get())

export function setTheme(value: Theme) {
  theme.set(value)
  show(value)
  keep(KEY, value)
}

/** The landscape behind the canvas, on paper: shown unless it's been hidden. */
export const landscape = signal(read(LANDSCAPE) !== "hidden")

export function setLandscape(shown: boolean) {
  landscape.set(shown)
  keep(LANDSCAPE, shown ? "shown" : "hidden")
}
