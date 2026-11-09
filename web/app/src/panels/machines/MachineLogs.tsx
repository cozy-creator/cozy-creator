import { useEffect, useState } from "react"
import { useConnection } from "../../app/Connection"
import type { Machine } from "../../api/calcifer"
import type { MachineLogName } from "../../types"
import { Failure, LogView, plain } from "../common"
import { follow, followingLine, type Following } from "../follow"

/** The most a drawer keeps, in characters: what scrolls past this is in `cozy machine logs`. */
const KEPT = 400_000
const LINES = 500

/** A machine's log, following its end: the one log stream across tabs. */
function Followed({ machine, log }: { machine: Machine; log: MachineLogName }) {
  const { client } = useConnection()
  const [text, setText] = useState("")
  const [following, setFollowing] = useState<Following>({ state: "waiting" })
  const name = machine.name

  useEffect(() => {
    const stop = new AbortController()
    let held = ""
    let after: string | null = null
    let timer: ReturnType<typeof setTimeout> | undefined
    setText("")
    void follow(
      stop.signal,
      () => client.machineLogEvents(name, log, LINES, after, stop.signal),
      (data) => {
        const event = JSON.parse(data) as { type: string; after?: string; text?: string }
        if (event.type !== "lines") return
        after = event.after ?? after
        held = (held + plain(event.text ?? "")).slice(-KEPT)
        timer ??= setTimeout(() => {
          timer = undefined
          setText(held)
        }, 100)
      },
      () => false,
      setFollowing,
    )
    return () => {
      stop.abort()
      clearTimeout(timer)
    }
  }, [client, name, log])

  const note = followingLine(following, "a log")
  return (
    <>
      <LogView text={text} empty="Nothing yet." />
      {note && <p className="muted">{note}</p>}
      {following.state === "refused" && <Failure error={following.error} lead="Couldn't read its log" />}
    </>
  )
}

/** Followed only once opened: there's one log stream to go round. */
export function MachineLogs({ machine }: { machine: Machine }) {
  const [log, setLog] = useState<MachineLogName>("tensord")
  const [open, setOpen] = useState(false)
  return (
    <details className="sect" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>Logs</summary>
      {open && (
        <>
          <select className="field small logs" aria-label="Which log" value={log} onChange={(event) => setLog(event.target.value as MachineLogName)}>
            <option value="tensord">TensorD</option>
            <option value="tensorfs">TensorFS</option>
            <option value="memory">Memory</option>
            {machine.kind === "local" && <option value="calcifer">Calcifer</option>}
          </select>
          <Followed machine={machine} log={log} />
        </>
      )}
    </details>
  )
}
