import { type EdgeProps, getBezierPath } from "@xyflow/react"
import { memo, useCallback, useSyncExternalStore } from "react"
import { useSignal } from "../ui/signal"
import type { CanvasStore, GraphEdge } from "./CanvasStore"
import { useCanvasStore } from "./context"
import type { EdgeSpec } from "./graph"
import { beatsOf, noHash, placeName, textAt } from "./text"

const Line = memo(function Line({ id, kind, path }: { id: string; kind: string; path: string }) {
  const store = useCanvasStore()
  const subscribe = useCallback((listener: () => void) => store.subscribeEdge(id, listener), [store, id])
  const lit = useSyncExternalStore(subscribe, () => store.isLit(id))
  return (
    <g className={`ge ${kind} ${lit ? "lit" : ""}`} data-edge={id}>
      <path className="ge-line" d={path} />
      <path
        className="ge-hit"
        d={path}
        onPointerEnter={(event) => store.tip.set({ id, x: event.clientX, y: event.clientY })}
        onPointerLeave={() => store.tip.set(null)}
        onClick={() => store.openEdge(id)}
      />
    </g>
  )
})

/** Runs per frame for what moved, and no other edge. */
function GraphEdgeView({ id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data }: EdgeProps<GraphEdge>) {
  const [path] = getBezierPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition })
  return <Line id={id} kind={data?.kind ?? "mention"} path={path} />
}

export const edgeTypes = { graph: GraphEdgeView }

const clipped = (text: string) => (text.length > 160 ? `${text.slice(0, 160).trimEnd()}…` : text)

/** The words that made an edge, named. */
function said(store: CanvasStore, edge: EdgeSpec): { where: string; words?: string } {
  if (edge.kind === "link") return { where: `${edge.reader}'s ${edge.at[0]}`, words: `<${edge.read}>` }
  const text = store.get(edge.reader)?.asset?.fields
  if (edge.kind === "write") {
    const beats = beatsOf(text)
    const beat = beats.findIndex((held) => held.writes.some((write) => write.image === edge.read))
    const at = beats[beat]?.writes.find((write) => write.image === edge.read)?.at
    return { where: `${edge.reader}${beat >= 0 ? `'s beat ${beat + 1}` : ""}`, words: `writes ${edge.read}${at === undefined ? "" : ` at ${at} s`}` }
  }
  const [first, ...rest] = edge.at
  if (!first) return { where: `${edge.reader} reads ${edge.read}` }
  const words = textAt(text, first)
  const more = rest.length ? ` and ${rest.length} more` : ""
  return { where: `${edge.reader}'s ${placeName(first)}${more}`, words: words ? clipped(words) : `<${edge.read}>` }
}

/** One tip for every edge, so hovering renders nothing else. */
export function EdgeTip() {
  const store = useCanvasStore()
  const tip = useSignal(store.tip)
  const edge = tip && store.edge(tip.id)
  if (!tip || !edge) return null
  const { where, words } = said(store, edge)
  return (
    <div className="edge-tip" style={{ left: tip.x + 12, top: tip.y + 12 }}>
      <b>{where}</b>
      {words && <span>{noHash(words)}</span>}
    </div>
  )
}
