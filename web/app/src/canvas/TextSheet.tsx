import { useEffect, useRef } from "react"
import type { Entity } from "../types"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { beatLength, beatsOf, clock, noHash, placeName, withMentions } from "./text"

/** Words as a person wrote them, as text: a mention stands out, and nothing is ever HTML. */
function Words({ text }: { text: string }) {
  return (
    <>
      {withMentions(text).map((part, i) => (part.mention ? <code key={i}>{part.text}</code> : <span key={i}>{part.text}</span>))}
    </>
  )
}

function Field({ place, label, text }: { place: string; label: string; text: string }) {
  return (
    <section data-place={place}>
      <h4>{label}</h4>
      <p>
        <Words text={text} />
      </p>
    </section>
  )
}

const shown = (value: unknown): string => (typeof value === "string" ? value : JSON.stringify(value))

function AssetWords({ text }: { text: Record<string, unknown> }) {
  const beats = beatsOf(text)
  let at = 0
  return (
    <>
      {Object.entries(text)
        .filter(([field]) => field !== "beats" || !beats.length)
        .map(([field, value]) => (
          <Field key={field} place={field} label={placeName(field)} text={shown(value)} />
        ))}
      {beats.map((beat, i) => {
        const from = at
        at += beatLength(beat)
        return (
          <section key={i} data-place={`beats.${i}`}>
            <h4>
              beat {i + 1}, {clock(from * 1000)} to {clock(at * 1000)}
            </h4>
            <p>
              <Words text={beat.description} />
            </p>
            {beat.writes.map((write) => (
              <p key={write.image} className="writes">
                writes <code>{write.image}</code>
                {write.at === undefined ? "" : ` at ${write.at} s`}
              </p>
            ))}
          </section>
        )
      })}
    </>
  )
}

function EntityWords({ entity }: { entity: Entity }) {
  return (
    <>
      {entity.description && <Field place="description" label="description" text={entity.description} />}
      {Object.entries(entity.links ?? {}).map(([field, target]) => (
        <Field key={field} place={field} label={field} text={`<${target}>`} />
      ))}
      {(entity.subs ?? []).map((sub) => (
        <section key={sub.name} className="sheet-sub">
          <h3>{sub.name}</h3>
          <EntityWords entity={sub} />
        </section>
      ))}
    </>
  )
}

/** An element's words, open beside the canvas, to read. Removing an edge means editing these, which `project.yaml` is for. */
function Sheet({ name, at }: { name: string; at?: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const body = useRef<HTMLDivElement>(null)
  const id = store.projectId

  useEffect(() => {
    if (!at || !body.current) return
    const places = [at, at.split(".").slice(0, 2).join("."), at.split(".")[0]]
    const found = places.map((place) => body.current!.querySelector(`[data-place="${CSS.escape(place)}"]`)).find(Boolean)
    body.current.querySelectorAll(".here").forEach((held) => held.classList.remove("here"))
    found?.classList.add("here")
    found?.scrollIntoView({ block: "nearest" })
  }, [at, view])

  return (
    <aside className="sheet" aria-label={`${name}'s words`}>
      <header>
        <b>{name}</b>
        <span className="sp" />
        {id && (
          <a className="act" href={`#/projects/${encodeURIComponent(id)}/yaml`}>
            project.yaml
          </a>
        )}
        <button type="button" className="iconb" aria-label="Close" onClick={() => store.open.set(null)}>
          ×
        </button>
      </header>
      <div className="sheet-body" ref={body}>
        {view?.warnings?.map((warning, i) => (
          <p key={i} className="warned-line">
            {noHash(warning)}
          </p>
        ))}
        {view?.asset?.failure && <p className="failed-line">{noHash(view.asset.failure.message)}</p>}
        {view?.asset && <AssetWords text={view.asset.text ?? {}} />}
        {view?.entity && <EntityWords entity={view.entity} />}
        {view?.asset && !Object.keys(view.asset.text ?? {}).length && <p className="muted">It has no words of its own.</p>}
      </div>
    </aside>
  )
}

export function TextSheet() {
  const store = useCanvasStore()
  const open = useSignal(store.open)
  return open ? <Sheet key={open.name} name={open.name} at={open.at} /> : null
}
