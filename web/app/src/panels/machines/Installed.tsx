import type { Environment, MachineStatus, Model } from "../../api/calcifer"
import { Section } from "../common"
import { bytes, shown } from "../format"
import { packageWords } from "./activity"

/** How far in a package is, past installed, as a person says it; installed says nothing. */
const LEVEL: Record<string, string> = { downloaded: "downloaded, not installed", imported: "loaded", host: "loaded in memory", gpu: "loaded on a GPU" }

const byName = (a: Environment, b: Environment) => a.package.localeCompare(b.package) || b.release.localeCompare(a.release)

/** The packages it holds installed, each release once, live from its Status: whoever's run put them there (Paul, 11-13). */
export function Packages({ status }: { status: MachineStatus }) {
  const packages = [...(status.environments ?? [])].sort(byName)
  return (
    <Section title="Packages" actions={packages.length ? <small className="count">{packages.length}</small> : undefined}>
      {packages.length ? (
        <ul className="rows held">
          {packages.map((held) => (
            <li className="heldrow" key={held.installation || `${held.package}@${held.release}`} data-package={held.package}>
              <span className="nm">{packageWords(shown(held.package))}</span>
              <span className="what">{[held.release, LEVEL[held.level], held.hub ? "" : "local code"].filter(Boolean).join(" · ")}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No packages installed yet. A package installs here the first time a run needs it.</p>
      )}
    </Section>
  )
}

/** One model's checkpoints in a few words: its releases, precisions, a LoRA, and a download cut short. */
function modelWords(model: Model): string {
  const checkpoints = model.checkpoints ?? []
  const each = (pick: (one: (typeof checkpoints)[number]) => string | undefined) => [...new Set(checkpoints.map(pick).filter(Boolean))].join(", ")
  const parts = [each((one) => one.release), each((one) => one.precision || one.lane)]
  if (checkpoints.some((one) => one.role === "lora")) parts.push("LoRA")
  if (checkpoints.some((one) => one.complete === false)) parts.push("partly downloaded")
  return parts.filter(Boolean).join(" · ")
}

/** The models it holds, biggest first, as what fills its disk, live from its Status. */
export function Models({ status }: { status: MachineStatus }) {
  const models = [...(status.models ?? [])].sort((a, b) => b.total_bytes - a.total_bytes)
  const total = status.models_bytes || models.reduce((sum, model) => sum + model.total_bytes, 0)
  return (
    <Section title="Models" actions={models.length ? <small className="count">{`${models.length} · ${bytes(total)}`}</small> : undefined}>
      {models.length ? (
        <ul className="rows held">
          {models.map((model) => (
            <li className="heldrow" key={model.repository} data-model={model.repository}>
              <span className="nm">{shown(model.repository)}</span>
              <span className="what">{modelWords(model)}</span>
              <span className="size">{bytes(model.total_bytes)}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No models downloaded yet. A model downloads here the first time a run needs it.</p>
      )}
      {models.length > 1 && <p className="muted">Files that models share are counted once in the total.</p>}
    </Section>
  )
}
