import type { Appearance } from "../api/calcifer"
import { signal } from "./signal"

export type Theme = "paper" | "dark"

/** One option of a theme: its label and its choices, the first the default. */
export interface ThemeOption {
  label: string
  choices: [value: string, label: string][]
}

/** Each theme and its own options (cl-1097): a new theme adds its entry here, and the settings show it. */
export const THEMES: Record<Theme, { label: string; options: Record<string, ThemeOption> }> = {
  paper: { label: "Paper", options: { landscape: { label: "Landscape", choices: [["shown", "Shown"], ["hidden", "Hidden"]] } } },
  dark: { label: "Dark", options: {} },
}

/** What this browser showed last, so a page opens as it was until Calcifer says how it looks: never the setting itself. */
const SHOWN = "cozy.appearance"

function read(): Appearance | null {
  try {
    const kept = JSON.parse(localStorage.getItem(SHOWN) ?? "null")
    return typeof kept?.theme === "string" ? kept : null
  } catch {
    return null
  }
}

export const theme = signal<Theme>("paper")
/** The landscape behind the canvas, on paper: shown unless it's hidden. */
export const landscape = signal(true)
/** What's shown now. */
export const shown = signal<Appearance>(read() ?? { theme: "paper" })

/** A theme this page knows, else Paper. */
export const themeOf = (appearance: Appearance): Theme => (appearance.theme in THEMES ? (appearance.theme as Theme) : "paper")

/** Option `name` as `appearance` has it, else its theme's default. */
export function optionOf(appearance: Appearance, name: string): string {
  return appearance.options?.[name] ?? THEMES[themeOf(appearance)].options[name]?.choices[0][0] ?? ""
}

/** Shows `appearance`: paper is the root's own; only dark is marked. */
export function showAppearance(appearance: Appearance) {
  const now = themeOf(appearance)
  if (now === "dark") document.documentElement.dataset.theme = "dark"
  else delete document.documentElement.dataset.theme
  theme.set(now)
  landscape.set(optionOf(appearance, "landscape") !== "hidden")
  shown.set(appearance)
  try {
    localStorage.setItem(SHOWN, JSON.stringify(appearance))
  } catch {
    // Private windows may refuse: the next page opens as Paper until Calcifer answers.
  }
}

showAppearance(shown.get())
