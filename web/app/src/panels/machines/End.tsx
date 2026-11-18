import { useQueryClient } from "@tanstack/react-query"
import { useConnection } from "../../app/Connection"
import type { Machine, MachineList } from "../../api/calcifer"
import { say } from "../../ui/notes"
import { Confirm, Failure, useAsk } from "../common"
import { usd } from "../format"
import { called, stableName } from "./words"

/** Puts what an ask answered into the list at once; the stream says the same a moment later. */
function useKeepInList() {
  const queries = useQueryClient()
  return (machine: Machine) =>
    queries.setQueryData<MachineList>(["machines"], (list) => list && { ...list, machines: list.machines.map((held) => (held.name === machine.name ? machine : held)) })
}

/** Ends it: what it waits for, then until the Hub confirms it has ended. Nothing here says it's gone until the Hub has. */
export function End({ machine }: { machine: Machine }) {
  const { client } = useConnection()
  const keep = useKeepInList()
  const ask = useAsk()
  const id = stableName(machine)
  const hourly = machine.rental && usd(machine.rental.usd_micros_per_hour)
  // A rental its Hub hasn't named a host for is said as what it rents, which a question can't name.
  const it = machine.placeholder ? "this rental" : machine.name

  const end = (force: boolean) =>
    void ask.run(async () => {
      const ended = await client.endMachine(id, force)
      keep(ended.machine)
      if (ended.abandoned?.length) say(`Ended without downloading these results: ${ended.abandoned.join(", ")}`)
    })

  if (machine.kind === "local" || machine.state === "ended") return null
  if (machine.kind === "added") {
    return (
      <>
        <Confirm
          label="Remove"
          disabled={ask.busy}
          says={`Remove ${machine.name} from your machines? It keeps running, and nothing on it is deleted.`}
          yes="Remove"
          onYes={() => end(false)}
        />
        <Failure error={ask.error} lead={`Couldn't remove ${machine.name}`} />
      </>
    )
  }
  const order = !machine.rental_id
  return (
    <>
      {machine.state !== "ending" && (
        <Confirm
          label="End"
          disabled={ask.busy}
          says={
            order
              ? `Cancel ${machine.placeholder ? "this order" : `the order for ${machine.name}`}? Nothing more will be rented, and anything it already rented will be ended.`
              : `End ${it}? Cozy downloads any results still on it first, then shuts it down and deletes its files.${hourly ? ` It costs ${hourly}/h until the Hub confirms it has ended.` : ""}`
          }
          yes={order ? "Cancel order" : "End rental"}
          onYes={() => end(false)}
        />
      )}
      {machine.state === "ending" && machine.ending && (
        <>
          <p>Ending once this finishes: {machine.ending}</p>
          <Confirm
            label="End now"
            disabled={ask.busy}
            says={`End ${it} now? Results that haven't downloaded yet will be lost.`}
            yes="End now and lose them"
            onYes={() => end(true)}
          />
        </>
      )}
      {machine.state === "ending" && !machine.ending && (
        <p>Shutting down.{order ? " Nothing more will be rented, and it leaves this list once the Hub confirms." : ` It costs money${hourly ? ` (${hourly}/h)` : ""} until the Hub confirms it has ended. This page shows when it does.`}</p>
      )}
      {machine.end_failed && (
        <div className="failure" role="alert">
          <span>{called(machine)} didn't shut down: {machine.end_failed}</span>
          <span className="remedy">It may still be costing money. Press End to try again.</span>
        </div>
      )}
      <Failure error={ask.error} lead={order ? "Couldn't cancel the order" : "Couldn't end the rental"} />
    </>
  )
}
