import { loadShowNsfw, saveShowNsfw } from "../app/view"
import { signal, useSignal } from "./signal"

/** The tag of a model that makes sexual or nude images, asked for or not. */
export const NSFW = "nsfw"

export const isNsfw = (tags?: string[]) => Boolean(tags?.includes(NSFW))

/** A model's tags as chips, `nsfw` as NSFW; with `onRemove`, each one's remove button. In `small`, so a row's own spans stay its own. */
export function Tags({ tags, onRemove }: { tags?: string[]; onRemove?: (tag: string) => void }) {
  if (!tags?.length) return null
  return (
    <small className="model-tags">
      {tags.map((tag) => (
        <small key={tag} className={tag === NSFW ? "model-tag nsfw" : "model-tag"}>
          {tag === NSFW ? "NSFW" : tag}
          {onRemove && (
            <button type="button" aria-label={`Remove the tag ${tag}`} onClick={() => onRemove(tag)}>
              ×
            </button>
          )}
        </small>
      ))}
    </small>
  )
}

/** Whether this viewer's lists show models tagged `nsfw`, one switch for every list, remembered in this browser. */
export const showsNsfw = signal(loadShowNsfw())

export function useShowNsfw(): [boolean, (shown: boolean) => void] {
  return [
    useSignal(showsNsfw),
    (next: boolean) => {
      showsNsfw.set(next)
      saveShowNsfw(next)
    },
  ]
}

/** The single Settings switch: off, model and LoRA lists leave out entries tagged `nsfw`. */
export function ShowNsfw({ shown, onChange }: { shown: boolean; onChange: (shown: boolean) => void }) {
  return (
    <label className="row center">
      <input type="checkbox" checked={shown} onChange={(event) => onChange(event.target.checked)} />
      Show NSFW models and LoRAs
    </label>
  )
}
