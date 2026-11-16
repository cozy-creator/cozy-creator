import type { Gpu, MachineStatus } from "../../api/calcifer"
import { Section } from "../common"
import { bytes } from "../format"
import { heatWords } from "./words"

/** Whether it takes runs, at a glance: out of service (cl-935), held back for heat on its last run (cl-913), else ready. */
function stateOf(gpu: Gpu, status: MachineStatus): { word: string; tone: "ok" | "hot" | "bad"; why?: string } {
  if (gpu.out_of_service)
    return { word: "Not working", tone: "bad", why: `It isn't working: ${gpu.out_of_service.replace(/\.$/, "")}. Runs go to its other GPUs or to other machines.` }
  const hot = status.heat?.find((heat) => heat.gpu === gpu.index)
  if (hot) return { word: "Slowed by heat", tone: "hot", why: `Held back ${heatWords(hot)}.` }
  return { word: "Ready", tone: "ok" }
}

/** Each GPU: its name, memory and driver, and whether it works; the ones the machine couldn't use, said under them. */
export function Gpus({ status }: { status: MachineStatus }) {
  const gpus = status.gpus ?? []
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
      {status.gpu_problems?.map((problem) => (
        <p className="gpu-problem" key={problem}>
          {problem}
        </p>
      ))}
    </Section>
  )
}
