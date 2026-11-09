import { useEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import type { Tool } from "../api/calcifer"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { makes, toolName } from "./shown"
import { retimed } from "./timeline/model"

type Open = "menu" | "tool"

const FRAME_RATES = [24, 25, 30, 48, 50, 60]
const ASPECTS = ["16:9", "9:16", "1:1", "4:3", "3:2", "2.39:1"]
const RESOLUTIONS = ["720p", "1080p", "1440p", "2160p"]

/** One of an Edit's settings. `From its first clip` is the field cleared. */
function Setting({ label, value, options, unset, onPick }: { label: string; value: unknown; options: (string | number)[]; unset?: string; onPick: (value: string | null) => void }) {
  const held = value === undefined || value === null ? "" : String(value)
  return (
    <label className="menu-row">
      {label}
      <select className="field nodrag" aria-label={label} value={held} onChange={(event) => onPick(event.target.value || null)}>
        {unset && <option value="">{unset}</option>}
        {[...new Set([...options.map(String), ...(held ? [held] : [])])].map((option) => (
          <option key={option} value={option}>
            {option}
          </option>
        ))}
      </select>
    </label>
  )
}

/** An Edit's film: its frame rate, its shape and its size. A clip's points are times, so they move with the rate. */
function EditSettings({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(name)
  const fps = Number(store.valueOf(name, "fps") ?? view?.asset?.timeline?.fps ?? 24)
  const timeline = store.valueOf(name, "timeline") as Parameters<typeof retimed>[0] | undefined
  return (
    <>
      <Setting
        label="Frame rate"
        value={fps}
        options={FRAME_RATES}
        onPick={(picked) => {
          const to = Number(picked)
          void store.setFields(name, { fps: to, ...(timeline ? { timeline: retimed(timeline, fps, to) } : {}) })
        }}
      />
      <Setting label="Aspect ratio" value={store.valueOf(name, "aspect_ratio")} options={ASPECTS} unset="Its first clip's" onPick={(picked) => void store.setField(name, "aspect_ratio", picked)} />
      <Setting label="Resolution" value={store.valueOf(name, "resolution")} options={RESOLUTIONS} unset="Its first clip's" onPick={(picked) => void store.setField(name, "resolution", picked)} />
    </>
  )
}

/** A right click on a box: an Edit's settings, what makes it, its name, and gone. */
export function AssetMenu() {
  const store = useCanvasStore()
  const menu = useSignal(store.menu)
  return menu ? <Menu key={`${menu.name}:${menu.x}:${menu.y}`} name={menu.name} x={menu.x} y={menu.y} /> : null
}

function Menu({ name, x, y }: { name: string; x: number; y: number }) {
  const store = useCanvasStore()
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
  const sameKind: Tool[] = [...tools.values()].filter((other) => other.makes === asset?.kind && other.id !== tool)
  const edit = Boolean(asset?.timeline)
  const rendered = makes(view) && !edit
  const left = Math.min(x, window.innerWidth - 300)
  const top = Math.min(y, window.innerHeight - 340)
  return createPortal(
    <div ref={shell} className="asset-menu" role="menu" aria-label={`${name}'s menu`} style={{ left, top }}>
      {open === "menu" && (
        <>
          {edit && <EditSettings name={name} />}
          {rendered && (
            <button type="button" role="menuitem" onClick={() => setOpen("tool")}>
              Change the tool
            </button>
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
                store.remove([name])
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
                  store.putTool(name, other)
                  close()
                }}
              >
                {toolName(other)}
              </button>
            </li>
          ))}
          {!sameKind.length && <li className="muted">No other tool makes {asset?.kind === "image" ? "an image" : `a ${asset?.kind}`} here</li>}
        </ul>
      )}
    </div>,
    document.body,
  )
}
