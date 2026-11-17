import { useQuery, useQueryClient } from "@tanstack/react-query"
import type { Appearance } from "../api/calcifer"
import { appearanceQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { say } from "../ui/notes"
import { THEMES, type Theme, optionOf, themeOf } from "../ui/theme"
import { DrawerFrame } from "./Drawer"
import { Section } from "./common"
import "./Settings.css"

/** A theme and its own options, each a row of choices (cl-1097). */
export function AppearanceChoice({ value, onChange, label }: { value: Appearance; onChange: (next: Appearance) => void; label: string }) {
  const now = themeOf(value)
  return (
    <div className="appearance" role="group" aria-label={label}>
      <div className="row choices" role="radiogroup" aria-label="Theme">
        {(Object.keys(THEMES) as Theme[]).map((theme) => (
          <button key={theme} type="button" role="radio" aria-checked={theme === now} className={`tab ${theme === now ? "on" : ""}`}
            onClick={() => theme !== now && onChange({ theme })}>
            {THEMES[theme].label}
          </button>
        ))}
      </div>
      {Object.entries(THEMES[now].options).map(([name, option]) => (
        <div key={name} className="row choices" role="radiogroup" aria-label={option.label}>
          <span className="choices-label">{option.label}</span>
          {option.choices.map(([choice, words]) => {
            const on = optionOf(value, name) === choice
            return (
              <button key={choice} type="button" role="radio" aria-checked={on} className={`tab ${on ? "on" : ""}`}
                onClick={() => !on && onChange({ theme: now, options: { ...value.options, [name]: choice } })}>
                {words}
              </button>
            )
          })}
        </div>
      ))}
    </div>
  )
}

/** This computer's settings for every page (cl-1097): how the page looks, unless a project has its own. */
export function Settings() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const { data } = useQuery(appearanceQuery(client))
  const value = data ?? { theme: "paper" }
  const change = (next: Appearance) => {
    queries.setQueryData(["appearance"], next)
    client.setAppearance(next).catch((error) => {
      void queries.invalidateQueries({ queryKey: ["appearance"] })
      say("Couldn't save how the page looks", error)
    })
  }
  return (
    <DrawerFrame title="Settings">
      <Section title="Appearance">
        <p className="muted">How the page looks everywhere. A project can have its own in its settings.</p>
        <AppearanceChoice label="Appearance everywhere" value={value} onChange={change} />
      </Section>
    </DrawerFrame>
  )
}
