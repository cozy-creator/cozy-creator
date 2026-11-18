import type { Gpu, Machine, MachineStatus } from "../../api/calcifer"
import { useGpuTrace } from "../../api/gpus"
import { useConnection } from "../../app/Connection"
import { Section } from "../common"
import { bytes } from "../format"
import { GpuGauges, useLive } from "./GpuGauges"
import { heatWords } from "./words"

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

/** Each GPU: its name, memory and driver, whether it works, and how hard it's working now (cl-1100, cl-1191), live off the events
 *  stream while the machines are shown; the ones the machine couldn't use, said under them. */
export function Gpus({ machine, status }: { machine: Machine; status: MachineStatus }) {
  const { stream } = useConnection()
  const trace = useGpuTrace(stream, machine.name)
  const live = useLive(trace)
  const gpus = status.gpus ?? []
  const readings = live?.gpus ?? []
  // Read but not listed: recorded readings a test replays on a machine with none.
  const unlisted = readings.filter((reading) => !gpus.some((gpu) => gpu.index === reading.index))
  return (
    <Section title="GPUs">
      {gpus.length || unlisted.length ? (
        <ul className="gpus">
          {gpus.map((gpu) => {
            const state = stateOf(gpu, status)
            const reading = readings.find((one) => one.index === gpu.index)
            return (
              <li key={gpu.index} className="gpu" data-gpu={gpu.index}>
                <div className="gpu-line">
                  <b>GPU {gpu.index}</b>
                  <span className="gpu-name">{gpu.name}</span>
                  {gpu.memory_bytes > 0 && <span className="gpu-mem">{bytes(gpu.memory_bytes)}</span>}
                  <span className={`gpu-state ${state.tone}`}>{state.word}</span>
                </div>
                {reading && <GpuGauges trace={trace} reading={reading} total={gpu.memory_bytes} />}
                {state.why && <p className={state.tone === "bad" ? "gpu-why bad" : "gpu-why hot"}>{state.why}</p>}
                {gpu.display && <p className="gpu-why muted">It also drives a screen, so renders on it make the screen lag.</p>}
                {gpu.driver && <p className="gpu-why faint">Driver {gpu.driver}</p>}
              </li>
            )
          })}
          {unlisted.map((reading) => (
            <li key={reading.index} className="gpu" data-gpu={reading.index}>
              <div className="gpu-line">
                <b>GPU {reading.index}</b>
              </div>
              <GpuGauges trace={trace} reading={reading} total={0} />
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No GPUs. Runs here use the CPU.</p>
      )}
      {status.gpu_problems?.map((problem) => (
        <p className="gpu-problem" key={problem}>
          {problem}
        </p>
      ))}
    </Section>
  )
}
