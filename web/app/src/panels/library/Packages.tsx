import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useDeferredValue, useRef, useState } from "react"
import { Link, useNavigate, useParams } from "react-router-dom"
import { machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId, useTo } from "../../app/Shell"
import { say } from "../../ui/notes"
import { Confirm, Failure, Section, useAsk, useSignedIn } from "../common"
import { shown, stamp } from "../format"
import { OnMachine } from "../machines/OnMachine"
import { LibraryFrame } from "./Library"

export function Packages() {
  const { client } = useConnection()
  const to = useTo()
  const [typed, setTyped] = useState("")
  const q = useDeferredValue(typed.trim())
  const [limit, setLimit] = useState(50)
  const { data, error } = useQuery({
    queryKey: ["packages", q, limit],
    queryFn: ({ signal }) => client.packages(q, limit, signal),
    staleTime: 60_000,
  })
  return (
    <LibraryFrame on="packages">
      <input className="field wide" aria-label="Search packages" placeholder="Search packages" spellCheck={false} value={typed} onChange={(event) => setTyped(event.target.value)} />
      <Failure error={error} lead="Couldn't search packages" />
      <ul className="list">
        {data?.packages.map((found) => (
          <li key={found.package}>
            <Link to={to(`packages/${encodeURIComponent(found.package)}`)}>
              <b>{shown(found.package)}</b>
              <small>{[found.latest_release, found.created_at && stamp(found.created_at)].filter(Boolean).join(" · ")}</small>
            </Link>
          </li>
        ))}
      </ul>
      {data && !data.packages.length && <p className="muted">{q ? `No packages match “${q}”.` : "No packages on the Hub yet."}</p>}
      {Boolean(data?.older) && (
        <button type="button" className="tb" onClick={() => setLimit(Math.min(200, limit + 50))}>
          Show {data?.older} more
        </button>
      )}
    </LibraryFrame>
  )
}

function Remove({ name }: { name: string }) {
  const { client } = useConnection()
  const navigate = useNavigate()
  const to = useTo()
  const ask = useAsk()
  const { data: list } = useQuery(machinesQuery(client))
  const machines = list?.machines.filter((machine) => machine.state !== "ended" && machine.state !== "ending") ?? []
  const [chosen, setChosen] = useState("")
  const on = chosen || machines[0]?.name || ""
  const asked = useRef<{ on: string; key: string } | null>(null)
  if (!machines.length) return null
  return (
    <div className="row center">
      <label className="check">
        From
        <select className="field" aria-label="Machine to remove it from" value={on} onChange={(event) => setChosen(event.target.value)}>
          {machines.map((machine) => (
            <option key={machine.name} value={machine.name}>
              {machine.name}
            </option>
          ))}
        </select>
      </label>
      <Confirm
        label="Remove"
        disabled={ask.busy || !on}
        says={`Remove every release of ${shown(name)} from ${on}, with its environment? The Hub and other machines keep it, and using it on ${on} installs it again.`}
        yes="Remove"
        onYes={() => {
          if (asked.current?.on !== on) asked.current = { on, key: `remove-${crypto.randomUUID()}` }
          const { key } = asked.current
          void ask.run(async () => {
            const { run } = await client.job("cozy/package_remove", { packages: [name], on: [on] }, key)
            asked.current = null
            navigate(to(`machines/${run.machine ?? on}`))
          })
        }}
      />
      <Failure error={ask.error} lead="Couldn't remove the package" />
    </div>
  )
}

export function PackageDrawer() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const navigate = useNavigate()
  const to = useTo()
  const id = useProjectId()
  const signedIn = useSignedIn()
  const name = useParams().name ?? ""
  const { data, error } = useQuery({ queryKey: ["package", name], queryFn: ({ signal }) => client.package(name, signal) })
  const { data: target } = useQuery({ queryKey: ["target", name], queryFn: ({ signal }) => client.target(name, signal), staleTime: 60_000 })
  const [release, setRelease] = useState("")
  const ask = useAsk()

  const again = () => queries.invalidateQueries({ queryKey: ["package", name] })
  const yank = (version: string, yanked: boolean) =>
    void ask.run(async () => {
      await (yanked ? client.unyank(name, version) : client.yank(name, version))
      await again()
    })

  return (
    <LibraryFrame on="packages" title={shown(name)} back={to("packages")}>
      <Failure error={error} lead={`Couldn't load ${shown(name)}`} />
      <Failure error={ask.error} />
      {target?.summary && <p>{target.summary}</p>}
      {data?.warnings?.map((warning) => (
        <p className="warn" key={warning}>
          {warning}
        </p>
      ))}
      {Boolean(target?.functions?.length) && (
        <Section title="Functions">
          <ul className="rows choose">
            {target?.functions?.map((choice) => {
              const runnable = id ? `${to("new")}?target=${encodeURIComponent(`${name}/${choice.function}`)}` : null
              return (
                <li className="heldrow" key={choice.function}>
                  <span className="nm">{choice.function}</span>
                  <small>{choice.description}</small>
                  {runnable && (
                    <Link className="more" to={runnable}>
                      Run
                    </Link>
                  )}
                </li>
              )
            })}
          </ul>
        </Section>
      )}
      <Section title="Install">
        <OnMachine
          body={{ package: name, ...(release ? { release } : {}) }}
          label="Install"
          quote={(body) => client.jobQuote("cozy/package_install", body)}
          ask={(body, key) => client.job("cozy/package_install", body, key)}
          onDone={({ run }) => navigate(to(run.project_id ? `runs/${run.number}` : `machines/${run.machine ?? "local"}`))}
        >
          <select className="field releasepick" aria-label="Release" value={release} onChange={(event) => setRelease(event.target.value)}>
            <option value="">Newest release</option>
            {data?.releases
              .filter((row) => !row.yanked)
              .map((row) => (
                <option key={row.release} value={row.release}>
                  {row.release}
                </option>
              ))}
          </select>
        </OnMachine>
        <Remove name={name} />
      </Section>
      <Section title="Releases">
        <table className="grid">
          <tbody>
            {data?.releases.map((row) => (
              <tr key={row.release}>
                <td className="nm">{row.release}</td>
                <td>{row.published && stamp(row.published)}</td>
                <td>{row.yanked && <span className="warn">Yanked</span>}</td>
                <td className="end">
                  {signedIn && !row.yanked && (
                    <Confirm
                      label="Yank"
                      disabled={ask.busy}
                      says={`Yank ${shown(name)} ${row.release}? Machines that have it keep it, and only a request for exactly this version installs it. You can undo this.`}
                      yes="Yank"
                      onYes={() => yank(row.release, false)}
                    />
                  )}
                  {signedIn && row.yanked && (
                    <button type="button" className="tb" disabled={ask.busy} onClick={() => yank(row.release, true)}>
                      Undo yank
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Section>
      {signedIn && data && (
        <Section title="Delete">
          <Confirm
            label={`Delete ${shown(name)}`}
            disabled={ask.busy}
            says={`Delete ${shown(name)} from the Hub, with all ${data.releases.length} of its releases? Anyone can then take its name and versions. This can't be undone. Machines keep the copies they have.`}
            yes="Delete forever"
            onYes={() =>
              void ask.run(async () => {
                await client.deletePackage(name)
                say(`Deleted ${shown(name)}.`)
                void queries.invalidateQueries({ queryKey: ["packages"] })
                navigate(to("packages"))
              })
            }
          />
        </Section>
      )}
    </LibraryFrame>
  )
}
