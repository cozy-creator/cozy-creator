import { QueryClient } from "@tanstack/react-query"
import { createContext, useContext } from "react"
import { Client } from "../api/client"
import { EventStream } from "../api/events"
import { sameOrigin } from "../api/transport"
import type { Machine, MachineList, Run, ServerEvent, Status } from "../types"
import { signal } from "../ui/signal"
import { readSession } from "./session"

/** Stream events into the query cache. The stream invalidates, so nothing goes stale by itself. */
class Bridge {
  /** Calcifer came back as another version: the page offers Reload. */
  readonly newVersion = signal<string | null>(null)
  private queries: QueryClient
  private version: string | null = null
  private revs = new Map<string, string>()
  private refreshes = new Map<string, { timer?: ReturnType<typeof setTimeout>; flying: boolean; again: boolean }>()
  private runs = new Map<number, Run>()
  private runListeners = new Set<(run: Run) => void>()
  private flushing = false

  constructor(queries: QueryClient) {
    this.queries = queries
  }

  setVersion(version: string) {
    this.version = version
  }

  /** New state wanted: 200 ms of quiet, one read at a time, and one more if it changed during that read. */
  refresh(id: string) {
    let slot = this.refreshes.get(id)
    if (!slot) this.refreshes.set(id, (slot = { flying: false, again: false }))
    const held = slot
    clearTimeout(held.timer)
    held.timer = setTimeout(async () => {
      if (held.flying) {
        held.again = true
        return
      }
      held.flying = true
      await Promise.all([
        this.queries.refetchQueries({ queryKey: ["project", id] }),
        this.queries.invalidateQueries({ queryKey: ["yaml", id] }),
      ])
      held.flying = false
      if (held.again) {
        held.again = false
        this.refresh(id)
      }
    }, 200)
  }

  onRun(listener: (run: Run) => void): () => void {
    this.runListeners.add(listener)
    return () => this.runListeners.delete(listener)
  }

  private flushRuns = () => {
    this.flushing = false
    const batch = [...this.runs.values()]
    this.runs.clear()
    for (const run of batch) {
      this.queries.setQueryData(["run", run.number], run)
      for (const listener of this.runListeners) listener(run)
    }
    this.queries.invalidateQueries({ queryKey: ["runs"] })
  }

  private machines(change: (machines: Machine[]) => Machine[]) {
    this.queries.setQueryData<MachineList>(["machines"], (list) => list && { ...list, machines: change(list.machines) })
  }

  handle = (event: ServerEvent) => {
    switch (event.type) {
      case "hello":
        if (this.version && event.version !== this.version) this.newVersion.set(event.version)
        void this.queries.invalidateQueries(undefined, { cancelRefetch: false })
        break
      case "resync":
        void this.queries.invalidateQueries()
        break
      case "run.updated":
        this.runs.set(event.run.number, event.run)
        if (!this.flushing) {
          this.flushing = true
          setTimeout(this.flushRuns, 16)
        }
        break
      case "machine.updated":
        this.machines((machines) => {
          const known = machines.some((machine) => machine.name === event.machine.name)
          return known ? machines.map((machine) => (machine.name === event.machine.name ? event.machine : machine)) : [...machines, event.machine]
        })
        break
      case "machine.removed":
        this.machines((machines) => machines.filter((machine) => machine.name !== event.name))
        break
      case "project.changed":
        if (this.revs.get(event.id) !== event.rev) {
          this.revs.set(event.id, event.rev)
          this.refresh(event.id)
        }
        break
    }
  }
}

export interface Connection {
  /** Which Calcifer: names the stream's lock and what's kept in the browser. */
  scope: string
  client: Client
  queries: QueryClient
  stream: EventStream
  bridge: Bridge
  /** Which try a call is on, when it isn't the first; 0 otherwise. */
  attempt: ReturnType<typeof signal<number>>
  signedOut: ReturnType<typeof signal<boolean>>
  status: Status
}

const REMOTE_KEY = "cozy.calcifer"

/** Which Calcifer this tab looks at: this computer's, or a machine's through it. Kept per tab. */
function chosenRemote(): string | null {
  try {
    return sessionStorage.getItem(REMOTE_KEY)
  } catch {
    return null
  }
}

/** One per Calcifer, with its own `QueryClient`, so one Calcifer's world never shows as another's. */
export async function connect(): Promise<{ connection: Connection } | { failed: "signed_out" | "unreachable"; client: Client }> {
  const remote = chosenRemote()
  const scope = remote ?? "here"
  const attempt = signal(0)
  const signedOut = signal(false)
  const transport = sameOrigin(readSession, remote ? `/v1/machines/${encodeURIComponent(remote)}/calcifer` : "")
  const client = new Client(transport, {
    signedOut: () => signedOut.set(true),
    attempt: (n) => attempt.set(n),
  })
  let status: Status
  try {
    status = await client.status()
  } catch {
    return { failed: signedOut.get() ? "signed_out" : "unreachable", client }
  }
  const queries = new QueryClient({
    defaultOptions: {
      queries: { staleTime: Infinity, retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false },
    },
  })
  const stream = new EventStream(client, scope)
  const bridge = new Bridge(queries)
  bridge.setVersion(status.version)
  stream.subscribe(bridge.handle)
  stream.start()
  return { connection: { scope, client, queries, stream, bridge, attempt, signedOut, status } }
}

const Context = createContext<Connection | null>(null)
export const ConnectionProvider = Context.Provider

export function useConnection(): Connection {
  const connection = useContext(Context)
  if (!connection) throw new Error("no connection")
  return connection
}
