import { useCallback } from "react"
import type { MainFile } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import { Menu } from "../ui/Menu"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { downloadable } from "./Media"
import { shownOf } from "./shown"
import { versionIn } from "./versions"

/** A right click on a box: Download, its star, a copy of it, its name, and gone. On a take, Download alone. */
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
