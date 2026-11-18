import { useQuery } from "@tanstack/react-query"
import { machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import type { ClientRun, Machine, Operation, Run } from "../../api/calcifer"
import { leftWords, shown, span } from "../format"
import { rateWords, size, stageLine, target } from "../runs/words"
import { holdWords } from "./words"

/** One thing a machine is doing: a package installing, weights arriving, a run, a run of another project or of the open one while it's
 *  locked (`own`; its state alone), a locked project's run (`locked`: Calcifer says nothing more of it, cl-1130), a run another computer
 *  sent (`client`, as the machine says it, cl-1193), what holds it up with no job (`hold`: an SSH session, a keepalive), or the machine
 *  coming up. */
export type Doing =
  | { kind: "install" | "download"; key: string; operation: Operation }
  | { kind: "run"; key: string; run: Run; on?: Operation }
  | { kind: "other"; key: string; state: string; own?: boolean; locked?: boolean }
  | { kind: "client"; key: string; other: ClientRun }
  | { kind: "hold"; key: string; words: string }
  | { kind: "boot"; key: string; words: string }

/** `qwen/qwen-image-2.1-turbo`, or a Hugging Face source of it, as people say it: `Qwen Image 2.1 Turbo`. Anything else is said as it is. */
export function modelWords(subject: string): string {
  const repository = subject.replace(/^hf:\/\//, "").match(/^[\w.-]+\/([\w.-]+)(@|\/|$)/)?.[1]
  if (!repository) return subject
  return repository
    .split(/[-_]/)
    .filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1))
    .join(" ")
}

/** A package as a box says it: this computer's own without its `local/`, the Hub's own without its `_/`. */
export const packageWords = (pkg: string) => shown(pkg).replace(/^local\//, "")

export const operationName = (operation: Operation) => (operation.kind === "download" ? modelWords(operation.subject) : packageWords(operation.subject))

export const waits = (run: Run) => run.status === "queued" || run.machine_state === "queued"

/** What a machine does that `project`'s views see: what it installs and downloads, whichever runs wait on it (cl-1205, cl-1218), naming
 *  this computer's own runs and that project's (another project's it only counts, and one naming another shown project's isn't here). */
export const ownActivity = (machine: Machine | undefined, project: string | null | undefined) =>
  (machine?.status?.activity ?? []).filter((operation) => !operation.project || operation.project === project)

/** The downloads and installs `run` waits on there, as its progress names them, installs first (cl-1205, cl-1218; Paul, 11-17: "it
 *  should show the download occurring, and then it should also show the 3 jobs that are like 'waiting for download...'"): each its
 *  machine's one, shown once. */
export const waitedOf = (activity: Operation[], run: Run) => {
  const ids = run.progress?.waiting_on ?? []
  const named = ids.length ? activity.filter((operation) => ids.includes(operation.id) && (operation.project ?? null) === (run.project_id ?? null)) : []
  return named.sort((a, b) => Number(a.kind === "download") - Number(b.kind === "download"))
}

/** What `run` waits on there: its install (the one it builds or waits for) before its downloads. A project's operations name its runs
 *  by its own numbers, this computer's by its own. */
export const waitsOnOf = (activity: Operation[], run: Run) => {
  const its = activity.filter((operation) => (operation.project ?? null) === (run.project_id ?? null))
  return its.find((operation) => operation.kind === "install" && operation.runs.includes(run.number)) ?? waitedOf(its, run)[0]
}

/** What a run waiting on `on` names and leads to: that install alone, else every download its progress names. */
export const namedOf = (activity: Operation[], run: Run, on: Operation | undefined): Operation[] =>
  !on ? [] : on.kind === "install" ? [on] : waitedOf(activity, run).filter((operation) => operation.kind === "download")

/** A run waiting on these, in words: `Waiting for MiniMax H3 to download`, `Waiting for MiniMax H3 and Qwen3 VL to download`,
 *  `Waiting for sdxl to install`. */
export function waitingWords(on: Operation[]): string {
  const names = on.map(operationName)
  const listed = names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}` : (names[0] ?? "")
  return `Waiting for ${listed} to ${on[0]?.kind === "install" ? "install" : "download"}`
}

/** A box or a take waiting on its machine's install or download says only that, never how far it is: that is the machine's, shown
 *  once there (cl-1205, cl-1218). */
export const boxWaitWords = (kind: string) => (kind === "install" ? "Waiting for install" : "Waiting for download")

/** A download's or an install's row, by its id, in a list that shows it: what a run waiting on it links to (`#install-…`). */
export const anchorOf = (id: string) => id.replace(/[^\w-]+/g, "-")

/** The row of download or install `id` in the list holding `from`, scrolled to and lit a moment: where a waiting run's words lead. */
export function showRow(from: HTMLElement, id: string) {
  const row = from.closest("ul")?.querySelector<HTMLElement>(`[data-operation="${CSS.escape(id)}"]`)
  if (!row) return
  row.scrollIntoView({ block: "nearest" })
  row.classList.remove("lit")
  void row.offsetWidth
  row.classList.add("lit")
  setTimeout(() => row.classList.remove("lit"), 1400)
}

/** Who waits on it, as this reader may say (cl-1205): its runs by name, then how many it can't name (another project's, another
 *  client's): `run KEY-1`, `runs KEY-1, KEY-2 and 1 job more`, `3 jobs`. What it fetches is the machine's, shown to every reader. */
export function waitersWords(operation: Operation): string {
  const named = operation.names?.length ? operation.names : operation.runs.map(String)
  const others = operation.unnamed ?? 0
  const jobs = (n: number) => `${n} job${n === 1 ? "" : "s"}`
  if (!named.length) return jobs(others)
  const runs = named.length === 1 ? `run ${named[0]}` : `runs ${named.join(", ")}`
  return others ? `${runs} and ${jobs(others)} more` : runs
}

/** What a download fetches, for its row's title: its repository, release and components. */
export const fetchesWords = (operation: Operation) =>
  [operation.subject, operation.release, operation.components?.join(", ")].filter(Boolean).join(" · ")

/** Its own work under way: begun, waiting on nothing. */
export const working = (run: Run, on?: Operation) => run.started_at_ms != null && !waits(run) && !on

/** What a machine is doing now, as its status says, kept by the machines' stream: each install and download once, however many runs wait
 *  on it, then its runs, those going first. `project`'s runs and this computer's own by name; another project's by its state alone, and
 *  `project`'s too while it's `locked` (cl-1099), whose installs and downloads aren't shown. */
export function doingOf(machine: Machine, project: string | null, locked = false): Doing[] {
  const activity = ownActivity(machine, locked ? null : project)
  const booting: Doing[] = machine.state === "booting" ? [{ kind: "boot", key: "boot", words: machine.boot ?? "booting" }] : []
  const operations: Doing[] = [...activity]
    .sort((a, b) => Number(a.kind === "download") - Number(b.kind === "download"))
    .map((operation) => ({ kind: operation.kind as "install" | "download", key: operation.id, operation }))
  const runs: Doing[] = (machine.status?.runs ?? []).flatMap(({ id = "", state, run, locked: sealed }, at): Doing[] => {
    if (sealed) return [{ kind: "other", key: `locked ${at}`, state, locked: true }]
    if (!run) return []
    if (run.project_id && (run.project_id !== project || locked)) return [{ kind: "other", key: id, state, own: run.project_id === project }]
    return [{ kind: "run", key: `run ${run.project_id ?? ""} ${run.number}`, run, on: waitsOnOf(activity, run) }]
  })
  const going = (doing: Doing) => Number(doing.kind === "run" ? !working(doing.run, doing.on) : doing.kind === "other" && doing.state !== "running")
  const clients: Doing[] = [...(machine.status?.others ?? [])]
    .sort((a, b) => Number(Boolean(b.running)) - Number(Boolean(a.running)))
    .map((other, at) => ({ kind: "client", key: `client ${at}`, other }))
  const holds: Doing[] = holdWords(machine, Date.now()).map((words) => ({ kind: "hold", key: `hold ${words}`, words }))
  return [...booting, ...operations, ...runs.sort((a, b) => going(a) - going(b)), ...clients, ...holds]
}

/** Whether a machine works for `project` now, or the project prefers it (`chosen`): a run, an install or a download of it is that
 *  project's (Paul, 11-17: "machines that are working on our current project (the project I have opened) should be to the left and
 *  more visible (next to local)"). */
export function forProject(machine: Machine, project: string | null, chosen: string[]): boolean {
  if (chosen.includes(machine.name)) return true
  if (!project) return false
  return (machine.status?.runs ?? []).some((held) => held.run?.project_id === project) || (machine.status?.activity ?? []).some((operation) => operation.project === project)
}

/** The machines in Paul's order (11-17): `local`, then those working for the open project, then every other one of the account (other
 *  projects' runs, other computers', idle), each group as listed. */
export function byRelation(project: string | null, chosen: string[]) {
  const rank = (machine: Machine) => (machine.kind === "local" ? 0 : forProject(machine, project, chosen) ? 1 : 2)
  return (a: Machine, b: Machine) => rank(a) - rank(b)
}

/** Another computer's run by what it runs (cl-1193): its function, or that it's an encrypted project's, which says nothing more. */
export const clientFunction = (other: ClientRun) => (other.function ? shown(other.function) : "An encrypted project's run")

/** Who sent it: the computer as it names itself. */
export const clientFrom = (other: ClientRun) => `From ${other.client || "another computer"}`

/** How far another computer's run is, as its machine says it: `Rendering, step 7 of 30 · 2m 10s`, its weights still to come, `Queued`. */
export function clientWords(other: ClientRun, now: number): string {
  if (!other.running) {
    if (!other.bytes_left) return "Queued"
    const models = (other.models ?? []).map(modelWords).join(", ")
    return `Waiting for ${size(other.bytes_left)}${models ? ` of ${models}` : ""} to download`
  }
  const said = other.stage || other.steps ? stageLine({ stage: other.stage ?? "", completed: other.step, total: other.steps, bytes_done: 0, bytes_total: 0, step_ms: 0, step_steady_ms: 0, at_ms: 0 }) : "Running"
  return other.started_at_ms ? `${said} · ${span((now - other.started_at_ms) / 1000)}` : said
}

/** How far its steps are, for a bar; none before it says. */
export const clientFraction = (other: ClientRun) => (other.running && other.steps ? Math.min(1, other.step / other.steps) : undefined)

/** What a run's row is called: its box, where the open project has it, else what it runs. */
export const runName = (run: Run, boxes: Map<number, string>) => (run.project_id ? boxes.get(run.number) : undefined) ?? target(run)

/** How far a download is, as the run face says it (cl-1074): `6.6GB / 9.4GB · 29MB/s · 3 min left · 8.8GB already here`. */
export function fetchedWords(operation: Operation): string {
  const parts = [operation.bytes_total ? `${size(operation.bytes_done)} of ${size(operation.bytes_total)}` : size(operation.bytes_done)]
  if (operation.bytes_per_s) parts.push(rateWords(operation.bytes_per_s))
  if (operation.eta_s != null) parts.push(leftWords(operation.eta_s))
  if (operation.bytes_here) parts.push(`${size(operation.bytes_here)} already on this machine`)
  return parts.join(" · ")
}

export const fractionOf = (operation: Operation) => (operation.bytes_total ? operation.bytes_done / operation.bytes_total : undefined)

/** What `run` waits on, kept by the machines' stream: a box says it while it waits. */
export function useWaitsOn(run: Run | undefined): Operation | undefined {
  const { client } = useConnection()
  const { data } = useQuery({
    ...machinesQuery(client),
    select: (list) => (run ? waitsOnOf(ownActivity(list.machines.find((machine) => machine.name === run.machine), run.project_id), run) : undefined),
  })
  return data
}

/** Package `pkg` installing on a machine for a job this reader can name (its project's, or this computer's own): the install, and
 *  where. One only other projects' jobs wait on is the machine's to show, and no reason for this project's box to wait. */
export function installingOf(machines: Machine[], pkg: string | undefined, project: string | null | undefined): Installing | undefined {
  if (!pkg) return undefined
  for (const machine of machines) {
    const operation = ownActivity(machine, project).find((one) => one.kind === "install" && one.subject === pkg && one.names.length > 0)
    if (operation) return { machine: machine.name, operation }
  }
  return undefined
}

export interface Installing {
  machine: string
  operation: Operation
}

/** Package `pkg` installing on any machine: a box whose render waits for it before its run is sent says so. */
export function useInstalling(pkg: string | undefined, project: string | null): Installing | undefined {
  const { client } = useConnection()
  const { data } = useQuery({
    ...machinesQuery(client),
    select: (list) => installingOf(list.machines, pkg, project),
  })
  return data
}
