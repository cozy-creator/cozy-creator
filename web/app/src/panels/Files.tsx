import { useQuery } from "@tanstack/react-query"
import { Link } from "react-router-dom"
import type { Asset } from "../api/calcifer"
import { projectQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useProjectId, useTo } from "../app/Shell"
import { useAskedImage } from "../app/media"
import { downloadable } from "../canvas/Media"
import { nodeState, StateDot } from "../canvas/StateDot"
import { Menu, useMenuAt } from "../ui/Menu"
import { DrawerFrame } from "./Drawer"

/** Its picture, a video's poster: the thumbnail its box draws, asked for as every file is (`useAskedImage`). */
function Picture({ id, sha256 }: { id: string; sha256: string }) {
  const { client } = useConnection()
  const image = useAskedImage(id, sha256, client.thumb(id, sha256))
  return image.src ? <img key={image.request} src={image.src} alt="" draggable={false} onLoad={image.onLoad} onError={image.onError} /> : null
}

function Row({ id, asset, waiting }: { id: string; asset: Asset; waiting: boolean }) {
  const { client } = useConnection()
  const to = useTo()
  const menu = useMenuAt()
  const file = asset.path ? { path: asset.path, absent: asset.absent } : null
  const saves = file && downloadable(file)
  return (
    <li className={`assetrow ${asset.starred ? "starred" : ""}`} data-asset={asset.name} onContextMenu={saves ? menu.open : undefined}>
      <Link to={to(`assets/${encodeURIComponent(asset.name)}`)}>
        <span className="asset-thumb">{asset.sha256 && asset.kind !== "audio" && !asset.absent && <Picture id={id} sha256={asset.sha256} />}</span>
        <StateDot state={nodeState(asset, { waiting, failureShown: !asset.pinned })} unseen={asset.unseen?.length ?? 0} noun={asset.noun} />
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

/** The project's assets (cl-892; "Assets", Paul 11-16): what's starred first, with a ★, then the rest as the project lists them. A click
 *  opens one on the canvas. */
export function Files() {
  const { client, stream } = useConnection()
  const id = useProjectId()!
  const { data } = useQuery(projectQuery(client, stream, id))
  const assets = data?.assets ?? []
  const listed = [...assets.filter((asset) => asset.starred), ...assets.filter((asset) => !asset.starred)]
  // In the project's line and not started: queued (its dot, as on the canvas).
  const queued = new Set((data?.queue ?? []).filter((item) => !item.running).map((item) => item.name))
  return (
    <DrawerFrame title="Assets">
      {data && !listed.length && <p className="muted">No assets yet. Drop a file on the canvas to add one.</p>}
      <ul className="assetlist">
        {listed.map((asset) => (
          <Row key={asset.name} id={id} asset={asset} waiting={queued.has(asset.name)} />
        ))}
      </ul>
    </DrawerFrame>
  )
}
