import { useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { Link } from "react-router-dom"
import type { Asset } from "../api/calcifer"
import { projectQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useProjectId, useTo } from "../app/Shell"
import { downloadable } from "../canvas/Media"
import { Menu, useMenuAt } from "../ui/Menu"
import { DrawerFrame } from "./Drawer"

/** Its picture, a video's poster: a small thumbnail. */
function Picture({ id, sha256 }: { id: string; sha256: string }) {
  const { client } = useConnection()
  const [src, setSrc] = useState<string>()
  useEffect(() => {
    let live = true
    void client.transport.src(client.thumb(id, sha256, 256)).then((url) => live && setSrc(url))
    return () => {
      live = false
    }
  }, [client, id, sha256])
  return src ? <img src={src} alt="" draggable={false} /> : null
}

function Row({ id, asset }: { id: string; asset: Asset }) {
  const { client } = useConnection()
  const to = useTo()
  const menu = useMenuAt()
  const file = asset.path ? { path: asset.path, absent: asset.absent } : null
  const saves = file && downloadable(file)
  return (
    <li className={`assetrow ${asset.starred ? "starred" : ""}`} data-asset={asset.name} onContextMenu={saves ? menu.open : undefined}>
      <Link to={to(`assets/${encodeURIComponent(asset.name)}`)}>
        <span className="asset-thumb">{asset.sha256 && asset.kind !== "audio" && !asset.absent && <Picture id={id} sha256={asset.sha256} />}</span>
        <span className="nm">{asset.name}</span>
        <span className="sp" />
        <small>{asset.kind}</small>
        {asset.starred && (
          <span className="star" aria-label="Starred" title="Starred">
            ★
          </span>
        )}
      </Link>
      {menu.at && file && (
        <Menu x={menu.at.x} y={menu.at.y} label={`${asset.name}'s menu`} close={menu.close}>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              client.download(client.file(id, file.path), asset.name)
              menu.close()
            }}
          >
            Download
          </button>
        </Menu>
      )}
    </li>
  )
}

/** The project's assets (cl-892): what's starred first, with a ★, then the rest as the project lists them. A click opens one on the canvas. */
export function Files() {
  const { client, stream } = useConnection()
  const id = useProjectId()!
  const { data } = useQuery(projectQuery(client, stream, id))
  const assets = data?.assets ?? []
  const listed = [...assets.filter((asset) => asset.starred), ...assets.filter((asset) => !asset.starred)]
  return (
    <DrawerFrame title="Files">
      {data && !listed.length && <p className="muted">Nothing in this project yet.</p>}
      <ul className="assetlist">
        {listed.map((asset) => (
          <Row key={asset.name} id={id} asset={asset} />
        ))}
      </ul>
    </DrawerFrame>
  )
}
