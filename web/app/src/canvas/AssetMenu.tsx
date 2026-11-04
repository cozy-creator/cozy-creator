import { useQuery } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useConnection } from "../app/Connection"
import type { Tool } from "../types"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { makes } from "./shown"

type Open = "menu" | "tool" | "lora" | "model"

/** The models a chooser finds. Whatever the Hub calls them, as text. */
function ModelSearch({ label, onPick }: { label: string; onPick: (model: string) => void }) {
  const { client } = useConnection()
  const [typed, setTyped] = useState("")
  const { data, error } = useQuery({ queryKey: ["models", typed], queryFn: ({ signal }) => client.models(typed.trim(), 20, signal), staleTime: 60_000 })
  return (
    <>
      <input className="field wide" aria-label={label} placeholder={label} spellCheck={false} autoFocus value={typed} onChange={(event) => setTyped(event.target.value)} />
      {error ? <p className="warn">{error instanceof Error ? error.message : String(error)}</p> : null}
      <ul className="menu-list">
        {data?.models.map((found) => (
          <li key={found.model}>
            <button type="button" onClick={() => onPick(found.model)}>
              {found.model}
            </button>
          </li>
        ))}
        {data && !data.models.length && <li className="muted">Nothing by that name</li>}
      </ul>
    </>
  )
}

/** A right click on a box: what makes it, what it's made with, its name, and gone. */
export function AssetMenu() {
  const store = useCanvasStore()
  const menu = useSignal(store.menu)
  return menu ? <Menu key={`${menu.name}:${menu.x}:${menu.y}`} name={menu.name} x={menu.x} y={menu.y} /> : null
}

function Menu({ name, x, y }: { name: string; x: number; y: number }) {
  const store = useCanvasStore()
  const { client } = useConnection()
  const view = useElement(name)
  const tools = useSignal(store.tools)
  const [open, setOpen] = useState<Open>("menu")
  const shell = useRef<HTMLDivElement>(null)
  const close = () => store.menu.set(null)
  useEffect(() => {
    const away = (event: Event) => {
      if (!shell.current?.contains(event.target as globalThis.Node)) store.menu.set(null)
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") store.menu.set(null)
    }
    window.addEventListener("pointerdown", away)
    window.addEventListener("keydown", key)
    return () => {
      window.removeEventListener("pointerdown", away)
      window.removeEventListener("keydown", key)
    }
  }, [store])
  const asset = view?.asset
  const tool = asset?.tool
  const sameKind: Tool[] = [...tools.values()].filter((other) => other.kind === asset?.kind && other.id !== tool)
  const { data: target } = useQuery({
    queryKey: ["target", tool],
    queryFn: ({ signal }) => client.target(tool!, signal),
    enabled: open === "model" && Boolean(tool),
  })
  const entry = target?.interface.entrypoints?.find((held) => held.name === target.function)
  const slots = (entry?.models ?? []).map((slot) => slot.path.split(".models.")[1] ?? slot.path)
  const rendered = makes(view)
  const pickModel = (model: string) => {
    const held = store.valueOf(name, "models")
    const models = typeof held === "object" && held !== null ? held : {}
    void store.setField(name, "models", { ...models, [slots[0] ?? "model"]: { ref: model } })
    close()
  }
  const pickLora = (model: string) => {
    const held = store.valueOf(name, "lora")
    void store.setField(name, "lora", [...(Array.isArray(held) ? held : []), { ref: model, strength: 1 }])
    close()
  }
  const left = Math.min(x, window.innerWidth - 300)
  const top = Math.min(y, window.innerHeight - 340)
  return createPortal(
    <div ref={shell} className="asset-menu" role="menu" aria-label={`${name}'s menu`} style={{ left, top }}>
      {open === "menu" && (
        <>
          {rendered && (
            <>
              <button type="button" role="menuitem" onClick={() => setOpen("tool")}>
                Change the tool
              </button>
              <button type="button" role="menuitem" onClick={() => setOpen("model")}>
                Change the model
              </button>
              <button type="button" role="menuitem" onClick={() => setOpen("lora")}>
                Add a LoRA
              </button>
            </>
          )}
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              store.openName(name)
              close()
            }}
          >
            Rename
          </button>
          {asset && (
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                void store.remove([name])
                close()
              }}
            >
              Delete
            </button>
          )}
        </>
      )}
      {open === "tool" && (
        <ul className="menu-list">
          {sameKind.map((other) => (
            <li key={other.id}>
              <button
                type="button"
                title={other.description || other.id}
                onClick={() => {
                  void store.setTool(name, other.id)
                  close()
                }}
              >
                {other.label}
              </button>
            </li>
          ))}
          {!sameKind.length && <li className="muted">No other tool makes {asset?.kind === "image" ? "an image" : `a ${asset?.kind}`} here</li>}
        </ul>
      )}
      {open === "lora" && <ModelSearch label="Search for a LoRA" onPick={pickLora} />}
      {open === "model" && <ModelSearch label="Search for a model" onPick={pickModel} />}
    </div>,
    document.body,
  )
}
