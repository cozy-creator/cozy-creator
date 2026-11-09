import { useQuery } from "@tanstack/react-query"
import { useState } from "react"
import { useConnection } from "../../app/Connection"
import type { Machine } from "../../api/calcifer"
import { Failure } from "../common"
import { bytes, shown } from "../format"

/** What a machine holds, as it last said. Only asked once opened, since asking starts `local`. */
function Held({ machine }: { machine: Machine }) {
  const { client } = useConnection()
  const name = machine.name
  const packages = useQuery({ queryKey: ["machine", name, "packages"], queryFn: ({ signal }) => client.machinePackages(name, signal) })
  const models = useQuery({ queryKey: ["machine", name, "models"], queryFn: ({ signal }) => client.machineModels(name, signal) })
  return (
    <>
      <Failure error={packages.error} lead="Couldn't read its packages" />
      <ul className="rows">
        {packages.data?.packages.map((held) => (
          <li className="heldrow" key={`${held.package}@${held.release}`}>
            <span className="nm">{shown(held.package)}</span>
            <small>{[held.release, held.level, held.source === "hub" ? "" : held.source].filter(Boolean).join(" · ")}</small>
            {held.warning && <small className="warn">{held.warning}</small>}
          </li>
        ))}
      </ul>
      {packages.data && !packages.data.packages.length && <p className="muted">No packages.</p>}
      <Failure error={models.error} lead="Couldn't read its models" />
      {models.data?.silent && <p className="muted">It hasn't reported yet, so nothing is known of what it holds.</p>}
      <ul className="rows">
        {models.data?.models.map((held) => (
          <li className="heldrow" key={`${held.model}@${held.release}:${held.lane}`}>
            <span className="nm">{shown(held.model)}</span>
            <small>{[held.release, held.lane || held.precision, bytes(held.bytes)].filter(Boolean).join(" · ")}</small>
          </li>
        ))}
      </ul>
      {models.data && !models.data.silent && !models.data.models.length && <p className="muted">No models.</p>}
      {Boolean(models.data?.bytes) && (
        <p className="muted">
          {bytes(models.data?.bytes ?? 0)} in all, with shared content counted once.
        </p>
      )}
    </>
  )
}

export function Installed({ machine }: { machine: Machine }) {
  const [open, setOpen] = useState(false)
  return (
    <details className="sect" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>What's installed</summary>
      {open && <Held machine={machine} />}
    </details>
  )
}
