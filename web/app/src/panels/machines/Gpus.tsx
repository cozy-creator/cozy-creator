import { useEffect, useState } from "react"
import type { Gpu, GpuReading, GpuUse, Machine, MachineStatus } from "../../api/calcifer"
import { ApiError, pause } from "../../api/client"
import { holdGpus, readEvents } from "../../api/events"
import { useConnection } from "../../app/Connection"
import { Section } from "../common"
import { bytes } from "../format"
import { heatWords } from "./words"

type Use = { state: "off" } | { state: "reading"; use?: GpuUse } | { state: "unknown"; why: string }

/** Its GPUs' use, live (cl-1100): the machine reads them each second only while this drawer is open on a machine that can say. One
 *  slot across tabs. An older machine or Cozy says it doesn't report it yet. */
function useGpuUse(name: string, watch: boolean): Use {
  const { client } = useConnection()
  const [use, setUse] = useState<Use>({ state: "off" })
  useEffect(() => {
    setUse({ state: watch ? "reading" : "off" })
    if (!watch) return
    const stop = new AbortController()
    void (async () => {
      let release: () => void
      try {
        release = await holdGpus(stop.signal)
      } catch {
        return
      }
      try {
        for (let attempt = 0; !stop.signal.aborted; attempt += 1) {
          try {
            const response = await client.machineGpuEvents(name, stop.signal)
            attempt = 0
            if (response.body) await readEvents(response.body, (data) => setUse({ state: "reading", use: JSON.parse(data) as GpuUse }))
          } catch (error) {
            if (stop.signal.aborted) return
            if (error instanceof ApiError && error.status !== 503) {
              const why = error.body.code === "unknown_route" ? "This version of Cozy doesn't report GPU use yet." : error.message
              return setUse({ state: "unknown", why })
            }
          }
          await pause(Math.min(5000, 250 * 2 ** attempt), stop.signal).catch(() => {})
        }
      } finally {
        release()
      }
    })()
    return () => stop.abort()
  }, [client, name, watch])
  return use
}

/** How hard it's working, as Runpod shows it: busy share and memory in use, each a bar. */
function Busy({ reading, total }: { reading: GpuReading; total: number }) {
  const memory = total ? reading.memory_used_bytes / total : 0
  return (
    <div className="gpu-use">
      <span>Busy</span>
      <span className="meter busy" role="meter" aria-label={`GPU ${reading.index} busy`} aria-valuenow={reading.busy_percent} aria-valuemin={0} aria-valuemax={100}>
        <i style={{ width: `${reading.busy_percent}%` }} />
      </span>
      <b>{reading.busy_percent}%</b>
      <span>Memory</span>
      <span className="meter memory" role="meter" aria-label={`GPU ${reading.index} memory in use`} aria-valuenow={Math.round(memory * 100)} aria-valuemin={0} aria-valuemax={100}>
        <i style={{ width: `${Math.min(100, memory * 100)}%` }} />
      </span>
      <b>
        {bytes(reading.memory_used_bytes)}
        {total ? ` of ${bytes(total)}` : ""}
      </b>
    </div>
  )
}

/** Whether it takes runs, at a glance: out of service (cl-935), held back for heat (cl-913) or by its host's power limit (cl-1161) on
 * its last run, else ready. */
function stateOf(gpu: Gpu, status: MachineStatus): { word: string; tone: "ok" | "hot" | "bad"; why?: string } {
  if (gpu.out_of_service)
    return { word: "Not working", tone: "bad", why: `It isn't working: ${gpu.out_of_service.replace(/\.$/, "")}. Runs go to its other GPUs or to other machines.` }
  const hot = status.heat?.find((heat) => heat.gpu === gpu.index)
  if (hot)
    return hot.power_limit_w
      ? { word: "Power-limited", tone: "hot", why: `Held ${heatWords(hot)}.` }
      : { word: "Slowed by heat", tone: "hot", why: `Held back ${heatWords(hot)}.` }
  return { word: "Ready", tone: "ok" }
}

/** Each GPU: its name, memory and driver, whether it works, and how busy it is now; the ones the machine couldn't use, said under them. */
export function Gpus({ machine, status }: { machine: Machine; status: MachineStatus }) {
  const gpus = status.gpus ?? []
  const use = useGpuUse(machine.name, gpus.length > 0 && machine.state === "ready" && !machine.offline && !machine.broken)
  const readings = use.state === "reading" ? use.use?.gpus : undefined
  return (
    <Section title="GPUs">
      {gpus.length ? (
        <ul className="gpus">
          {gpus.map((gpu) => {
            const state = stateOf(gpu, status)
            return (
              <li key={gpu.index} className="gpu" data-gpu={gpu.index}>
                <div className="gpu-line">
                  <b>GPU {gpu.index}</b>
                  <span className="gpu-name">{gpu.name}</span>
                  {gpu.memory_bytes > 0 && <span className="gpu-mem">{bytes(gpu.memory_bytes)}</span>}
                  <span className={`gpu-state ${state.tone}`}>{state.word}</span>
                </div>
                {readings?.filter((reading) => reading.index === gpu.index).map((reading) => (
                  <Busy key={reading.index} reading={reading} total={gpu.memory_bytes} />
                ))}
                {state.why && <p className={state.tone === "bad" ? "gpu-why bad" : "gpu-why hot"}>{state.why}</p>}
                {gpu.display && <p className="gpu-why muted">It also drives a screen, so renders on it make the screen lag.</p>}
                {gpu.driver && <p className="gpu-why faint">Driver {gpu.driver}</p>}
              </li>
            )
          })}
        </ul>
      ) : (
        <p className="muted">No GPUs. Runs here use the CPU.</p>
      )}
      {use.state === "unknown" && <p className="muted">{use.why}</p>}
      {readings && gpus.length > 0 && !readings.length && <p className="muted">This machine's GPUs don't report how busy they are.</p>}
      {status.gpu_problems?.map((problem) => (
        <p className="gpu-problem" key={problem}>
          {problem}
        </p>
      ))}
    </Section>
  )
}
