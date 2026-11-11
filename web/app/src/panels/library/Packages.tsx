import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useDeferredValue, useRef, useState } from "react"
import { Link, useNavigate, useParams } from "react-router-dom"
import { machinesQuery } from "../../api/queries"
import { useConnection } from "../../app/Connection"
import { useProjectId, useTo } from "../../app/Shell"
import { say } from "../../ui/notes"
import { Confirm, Failure, Section, useAsk, useSignedIn } from "../common"
import { shown } from "../format"
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
      <Failure error={error} lead="Couldn't search the packages" />
      <ul className="list">
        {data?.packages.map((found) => (
          <li key={found.package}>
            <Link to={to(`packages/${encodeURIComponent(found.package)}`)}>
              <b>{shown(found.package)}</b>
              <small>{[found.latest_release, found.created_at].filter(Boolean).join(" · ")}</small>
            </Link>
          </li>
        ))}
      </ul>
      {data && !data.packages.length && <p className="muted">{q ? `No package matches ${q}.` : "No packages on this Hub."}</p>}
      {Boolean(data?.older) && (
        <button type="button" className="tb" onClick={() => setLimit(Math.min(200, limit + 50))}>
          {data?.older} more
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
        <select className="field" aria-label="Remove from which machine" value={on} onChange={(event) => setChosen(event.target.value)}>
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
        says={`Remove ${shown(name)} from ${on}: every release it holds, with its environment. The Hub and other machines keep it, and using it there installs it again.`}
        yes="Remove it"
        onYes={() => {
          if (asked.current?.on !== on) asked.current = { on, key: `remove-${crypto.randomUUID()}` }
          const { key } = asked.current
          void ask.run(async () => {
            const { run } = await client.job("cozy/package_remove", { packages: [name], on: [on] }, key)
            asked.current = null
            navigate(to(`runs/${run.number}`))
          })
        }}
      />
      <Failure error={ask.error} lead="Couldn't remove it" />
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
      <Failure error={error} lead={`Couldn't read ${shown(name)}`} />
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
          onDone={({ run }) => navigate(to(`runs/${run.number}`))}
        >
          <select className="field releasepick" aria-label="Which release" value={release} onChange={(event) => setRelease(event.target.value)}>
            <option value="">The newest</option>
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
                <td>{row.published}</td>
                <td>{row.yanked && <span className="warn">yanked</span>}</td>
                <td className="end">
                  {signedIn && !row.yanked && (
                    <Confirm
                      label="Yank"
                      disabled={ask.busy}
                      says={`Yank ${shown(name)} ${row.release}? Machines that hold it keep it, and only an exact pin installs it. You can take the yank back.`}
                      yes="Yank it"
                      onYes={() => yank(row.release, false)}
                    />
                  )}
                  {signedIn && row.yanked && (
                    <button type="button" className="tb" disabled={ask.busy} onClick={() => yank(row.release, true)}>
                      Take back
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
            says={`Delete ${shown(name)} from the Hub, with all ${data.releases.length} of its releases? Its name and versions go free for anyone, and it can't be undone. Machines keep what they hold.`}
            yes="Delete it for good"
            onYes={() =>
              void ask.run(async () => {
                await client.deletePackage(name)
                say(`${shown(name)} is deleted.`)
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
