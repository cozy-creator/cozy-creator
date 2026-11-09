import { useEffect, useRef, useState } from "react"
import { useConnection } from "../../app/Connection"
import type { Run, RunEvent } from "../../api/calcifer"
import { LogView, plain, Section } from "../common"
import { Failure } from "../common"
import { follow, followingLine, type Following } from "../follow"
import { span } from "../format"

/** The most a drawer keeps: what scrolls past this is in `cozy run show`. */
const KEPT = 5000

const LIFECYCLE: Record<string, string> = {
  "run.in_progress": "running",
  "run.completed": "completed",
  "run.failed": "failed",
  "run.canceled": "canceled",
}

/** One line of a run's story, or none for what the page already shows live. */
function line(event: RunEvent, start: number): string | null {
  switch (event.type) {
    case "run.log": {
      const at = span((event.at_ms - start) / 1000).padStart(6)
      const level = event.level && event.level !== "info" ? `${event.level}: ` : ""
      return `${at}  ${level}${plain(event.text)}`
    }
    case "run.call":
      return `        call ${event.call.index} ${event.call.function}: ${event.call.status}${event.call.reason ? `, ${event.call.reason.message}` : ""}`
    case "output_item.landed":
      return `        ${event.item.name}${event.item.index ? ` ${event.item.index}` : ""} landed here`
    case "output_item.done":
      return event.item.export_error ? `        ${event.item.name} will not land: ${event.item.export_error}` : null
    case "run.in_progress":
    case "run.completed":
    case "run.failed":
    case "run.canceled":
      return `        ${LIFECYCLE[event.type]}`
    default:
      return null
  }
}

/** A run's log and events from `GET /v1/runs/{id}/events`, one such stream across tabs. */
export function RunLog({ run }: { run: Run }) {
  const { client } = useConnection()
  const [text, setText] = useState("")
  const [following, setFollowing] = useState<Following>({ state: "waiting" })
  const start = useRef(run.accepted_at_ms ?? run.created_at_ms)
  start.current = run.accepted_at_ms ?? run.created_at_ms

  useEffect(() => {
    const stop = new AbortController()
    const lines: string[] = []
    let after = 0
    let ended = false
    let dropped = 0
    let timer: ReturnType<typeof setTimeout> | undefined
    // A settled run replays its whole log in one go: the page redraws for it ten times a second, not once a line.
    const flush = () => {
      timer = undefined
      setText((dropped ? [`(${dropped} earlier lines: cozy run show ${run.number})`, ...lines] : lines).join("\n"))
    }
    setText("")
    void follow(
      stop.signal,
      () => client.runEvents(run.id, after, stop.signal),
      (data) => {
        const event = JSON.parse(data) as RunEvent
        if ("sequence_number" in event) {
          if (event.sequence_number <= after) return
          after = event.sequence_number
        }
        if (event.type === "run.completed" || event.type === "run.failed" || event.type === "run.canceled") ended = true
        const said = line(event, start.current)
        if (said === null) return
        lines.push(said)
        if (lines.length > KEPT) {
          lines.shift()
          dropped += 1
        }
        timer ??= setTimeout(flush, 100)
      },
      () => ended,
      setFollowing,
    )
    return () => {
      stop.abort()
      clearTimeout(timer)
    }
  }, [client, run.id, run.number])

  const note = followingLine(following, "a log")
  return (
    <Section title="Log">
      <LogView text={text} empty={following.state === "ended" ? "It logged nothing." : "Nothing yet."} />
      {note && <p className="muted">{note}</p>}
      {following.state === "refused" && <Failure error={following.error} lead="Couldn't read its log" />}
    </Section>
  )
}
