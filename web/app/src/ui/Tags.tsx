import { useState } from "react"
import { loadShowNsfw, saveShowNsfw } from "../app/view"

/** The tag of a model that makes sexual or nude images, asked for or not. */
export const NSFW = "nsfw"

export const isNsfw = (tags?: string[]) => Boolean(tags?.includes(NSFW))

/** A model's tags as chips, `nsfw` as NSFW; with `onRemove`, each one's remove button. */
export function Tags({ tags, onRemove }: { tags?: string[]; onRemove?: (tag: string) => void }) {
  if (!tags?.length) return null
  return (
    <span className="chips">
      {tags.map((tag) => (
        <span key={tag} className={tag === NSFW ? "chip nsfw" : "chip"}>
          {tag === NSFW ? "NSFW" : tag}
          {onRemove && (
            <button type="button" aria-label={`Remove the tag ${tag}`} onClick={() => onRemove(tag)}>
              ×
            </button>
          )}
        </span>
      ))}
    </span>
  )
}

/** Whether this viewer's lists show models tagged `nsfw`, remembered in this browser. */
export function useShowNsfw(): [boolean, (shown: boolean) => void] {
  const [shown, setShown] = useState(loadShowNsfw)
  return [
    shown,
    (next: boolean) => {
      setShown(next)
      saveShowNsfw(next)
    },
  ]
}

/** The switch: off, a list leaves out models tagged `nsfw`. */
export function ShowNsfw({ shown, onChange }: { shown: boolean; onChange: (shown: boolean) => void }) {
  return (
    <label className="row center">
      <input type="checkbox" checked={shown} onChange={(event) => onChange(event.target.checked)} />
      Show NSFW
    </label>
  )
}
