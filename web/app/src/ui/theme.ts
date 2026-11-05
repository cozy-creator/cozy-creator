import { signal } from "./signal"

export type Theme = "paper" | "dark"

const KEY = "cozy.theme"

function saved(): Theme {
  try {
    return localStorage.getItem(KEY) === "dark" ? "dark" : "paper"
  } catch {
    return "paper"
  }
}

/** Paper is the root's own; only dark is marked. */
function show(value: Theme) {
  if (value === "dark") document.documentElement.dataset.theme = "dark"
  else delete document.documentElement.dataset.theme
}

export const theme = signal<Theme>(saved())
show(theme.get())

export function setTheme(value: Theme) {
  theme.set(value)
  show(value)
  try {
    localStorage.setItem(KEY, value)
  } catch {
    // Private windows may refuse. It still holds for this tab.
  }
}
