import { useCallback } from "react"
import type { MainFile } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import { Menu } from "../ui/Menu"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { downloadable } from "./Media"
import { shownOf } from "./shown"
import { versionIn } from "./versions"
import { retimed } from "./timeline/model"

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

/** A right click on a box: an Edit's settings, Download, its star, a copy of it, its name, and gone. On a take, Download alone. */
export function AssetMenu() {
  const store = useCanvasStore()
  const menu = useSignal(store.menu)
  return menu ? <BoxMenu key={`${menu.name}:${menu.number}:${menu.x}:${menu.y}`} name={menu.name} x={menu.x} y={menu.y} number={menu.number} /> : null
}

function BoxMenu({ name, x, y, number }: { name: string; x: number; y: number; number?: number }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const view = useElement(name)
  const close = useCallback(() => store.menu.set(null), [store])
  const asset = view?.asset
  const edit = Boolean(asset?.timeline)
  // The box saves as its name, `hei.png`; a take as its version's, `hei.2.png`.
  const file: MainFile | null | undefined = number === undefined ? view && shownOf(view) : versionIn(asset, number)?.main
  const download = file && downloadable(file) && store.projectId && (
    <button
      type="button"
      role="menuitem"
      onClick={() => {
        client.download(client.file(store.projectId!, file.path), number === undefined ? name : `${name}.${number}`)
        close()
      }}
    >
      Download
    </button>
  )
  if (number !== undefined) {
    return (
      <Menu x={x} y={y} label={`${name}.${number}'s menu`} close={close}>
        {download}
      </Menu>
    )
  }
  return (
    <Menu x={x} y={y} label={`${name}'s menu`} close={close}>
      {edit && <EditSettings name={name} />}
      {download}
      {asset && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            store.star(name, !asset.starred)
            close()
          }}
        >
          {asset.starred ? "Unstar" : "Star"}
        </button>
      )}
      {asset && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            store.duplicate([name])
            close()
          }}
        >
          Duplicate
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
    </Menu>
  )
}
