import { useQueryClient } from "@tanstack/react-query"
import { useConnection } from "../../app/Connection"
import type { Machine, MachineList } from "../../api/calcifer"
import { Copyable } from "../../ui/Copyable"
import { say } from "../../ui/notes"
import { Confirm, Failure, useAsk } from "../common"
import { usd } from "../format"
import { called, handle, stableName } from "./words"

/** Puts what an ask answered into the list at once; the stream says the same a moment later. */
function useKeepInList() {
  const queries = useQueryClient()
  return (machine: Machine) =>
    queries.setQueryData<MachineList>(["machines"], (list) => list && { ...list, machines: list.machines.map((held) => (held.name === machine.name ? machine : held)) })
}

/** Ends it: what it waits for, then until the Hub affirms the pod gone. Nothing here says it's gone until the Hub has. */
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
      if (ended.abandoned?.length) say(`Gave up outputs that had not landed: ${ended.abandoned.join(", ")}`)
    })

  if (machine.kind === "local" || machine.state === "ended") return null
  if (machine.kind === "added") {
    return (
      <>
        <Confirm
          label="Remove"
          disabled={ask.busy}
          says={`Remove ${machine.name} from this computer's machines? It goes on running, and its own computer keeps what it holds.`}
          yes="Remove it"
          onYes={() => end(false)}
        />
        <Failure error={ask.error} lead="Couldn't remove it" />
      </>
    )
  }
  if (machine.ordered === false) {
    return (
      <>
        <Confirm
          label="End"
          disabled={ask.busy}
          says={`End ${it}? Another computer rented it, and whatever it's doing for that computer stops.${hourly ? ` It bills ${hourly}/h until the Hub affirms it gone.` : ""}`}
          yes="End it"
          onYes={() => end(false)}
        />
        <Failure error={ask.error} lead="Couldn't end it" />
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
              ? `Stop ${machine.placeholder ? "this order" : `the order for ${machine.name}`}? It buys nothing more, and anything it already placed is ended.`
              : `End ${it}? Outputs of its runs that haven't landed here land first, then it shuts down. Its files on the pod go.${hourly ? ` It bills ${hourly}/h until the Hub affirms it gone.` : ""}`
          }
          yes="End it"
          onYes={() => end(false)}
        />
      )}
      {machine.state === "ending" && machine.ending && (
        <>
          <p>Waiting to end it: {machine.ending}</p>
          <Confirm
            label="End now"
            disabled={ask.busy}
            says={`End ${it} now? Outputs of its runs that haven't landed here are given up for good.`}
            yes="End it and give them up"
            onYes={() => end(true)}
          />
        </>
      )}
      {machine.state === "ending" && !machine.ending && (
        <p>Shutting down.{order ? " It buys nothing more, and leaves this list once its Hub answers." : ` It bills${hourly ? ` ${hourly}/h` : ""} until its Hub affirms the pod gone, which this page shows when it does.`}</p>
      )}
      {machine.end_failed && (
        <div className="failure" role="alert">
          <span>{called(machine)} was not shut down: {machine.end_failed}</span>
          <span className="remedy">Nothing was ended, and it may still be billing. End it again to ask again.</span>
          <Copyable text={`cozy machine end ${handle(machine)}`} />
        </div>
      )}
      <Failure error={ask.error} lead="Couldn't end it" />
    </>
  )
}
