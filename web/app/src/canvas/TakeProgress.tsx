import { useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import type { Machine, Operation, Run } from "../api/calcifer"
import { machinesQuery, runQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { fractionOf, useWaitsOn, waits } from "../panels/machines/activity"
import { called } from "../panels/machines/words"
import { bytesKind, countsOf, phaseOf } from "../panels/runs/words"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"
import "./Takes.css"

/** What a take on its way is doing, by name, and how far through that alone (Paul, 11-16, cl-1074): its download, then its render, each
 *  with its own share, never one number for both. Waiting, it has none. */
export type TakePhase = { phase: string; share?: number }

const share = (done: number, total: number) => (total > 0 ? Math.min(1, Math.max(0, done / total)) : undefined)

export function takeNow(run: Run | undefined, on: Operation | undefined, machine: Machine | undefined): TakePhase {
  if (!run) return { phase: "Queued" }
  if (run.cancel_requested) return { phase: "Canceling" }
  if (run.status === "completed") return { phase: "Saving", share: run.arriving ? share(run.arriving.bytes_done, run.arriving.bytes_total) : undefined }
  if (machine?.state === "booting") return { phase: machine.waiting ? "Waiting for capacity" : "Starting up" }
  // Its package installing or its weights arriving on its machine, before it runs.
  if (on) return { phase: on.kind === "download" ? "Downloading" : "Installing", share: fractionOf(on) }
  if (!run.machine || waits(run) || run.queue_position != null) return { phase: "Queued" }
  const progress = run.progress
  if (!progress) return { phase: run.machine_state === "preparing" ? "Preparing" : "Starting" }
  const phase = phaseOf(progress.stage)
  const counted = countsOf(progress) === "token" ? undefined : share(progress.completed, progress.total)
  const bytes = share(progress.bytes_done, progress.bytes_total)
  const stage = progress.stage_fraction ?? undefined
  // Bytes arriving under a stage that doesn't say so (`preparing model adapters…`) are its download all the same.
  if (phase === "downloading" || bytesKind(progress) === "download") return { phase: "Downloading", share: bytes ?? counted ?? stage }
  if (phase === "installing") return { phase: "Installing", share: counted ?? stage }
  if (phase === "loading") return { phase: "Loading", share: bytes ?? counted ?? stage }
  if (phase === "saving" || phase === "result") return { phase: "Saving", share: bytes ?? counted }
  if (phase === "queued") return { phase: "Queued" }
  if (phase && phase !== "working") return { phase: "Preparing", share: counted ?? stage }
  return { phase: "Rendering", share: counted ?? stage ?? progress.fraction ?? progress.done_fraction ?? undefined }
}

/** A take on its way, on its tile: what it's doing and how far through that, over the machine it's on (Paul, 11-16). Its own run, each
 *  tile (cl-945): the box follows only the one its frame is on. None yet: queued. */
export function TakeProgress({ view, run }: { view: ElementView; run?: number }) {
  const { client } = useConnection()
  const project = useCanvasStore().projectId
  const framed = run !== undefined && view.run?.number === run
  const { data } = useQuery({ ...runQuery(client, project ?? "", run ?? 0), enabled: Boolean(project) && run !== undefined && !framed })
  const mine = run === undefined ? undefined : framed ? view.run : data
  const on = useWaitsOn(mine)
  const { data: machines } = useQuery(machinesQuery(client))
  const machine = mine?.machine ? machines?.machines.find((held) => held.name === mine.machine) : undefined
  const now = takeNow(mine, on, machine)
  const queued = now.phase === "Queued"
  const where = machine ? called(machine) : (mine?.machine ?? undefined)
  // What it has made so far plays on its tile, silent (cl-907): a pick of the tile plays it with its sound.
  const glimpse = run !== undefined && view.glimpse?.media === "video" && view.glimpse.number === run ? view.glimpse : undefined
  return (
    <>
      {glimpse && <TileGlimpse glimpse={glimpse} />}
      {/* Its state, as its words say it (cl-945): grey while it waits, its machine's line too; blue once it's under way. */}
      <span className={`state-dot ${queued ? "queued" : "running"}`} role="img" aria-label={queued ? "Queued" : "Rendering"} />
      <span className="take-now">
        {now.share !== undefined && <b className="take-said">{Math.floor(now.share * 100)}%</b>}
        <span className="take-phase">{now.phase}</span>
      </span>
      {where && (
        <span className="take-where" title={`On ${where}`}>
          {where}
        </span>
      )}
    </>
  )
}

function TileGlimpse({ glimpse }: { glimpse: NonNullable<ElementView["glimpse"]> }) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const [src, setSrc] = useState<string>()
  useEffect(() => {
    if (!store.projectId) { setSrc(undefined); return }
    let live = true
    void client.transport.src(client.output({ project: store.projectId, run: glimpse.run }, glimpse.output, glimpse.index, glimpse.rev)).then((url) => live && setSrc(url))
    return () => {
      live = false
    }
  }, [client, store.projectId, glimpse.run, glimpse.output, glimpse.index, glimpse.rev])
  return src ? <video className="take-glimpse" src={src} muted loop autoPlay playsInline preload="metadata" /> : null
}
