import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useDeferredValue, useState } from "react"
import { Link, useNavigate, useParams } from "react-router-dom"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import type { ModelCheckpoint, ModelDetail, ModelEdit, Submitted } from "../../api/calcifer"
import { say } from "../../ui/notes"
import { NSFW, Tags, isNsfw, useShowNsfw } from "../../ui/Tags"
import { Confirm, Failure, Section, useAsk, useSignedIn } from "../common"
import { DropZone, FileRow, useFiles } from "../FileDrop"
import { bytes, shown, stamp } from "../format"
import { OnMachine } from "../machines/OnMachine"
import { LibraryFrame } from "./Library"

/** Follows the run the ask became. A Hub checkpoint is only recorded there: Calcifer's own run, on no machine. */
function useFollowRun() {
  const navigate = useNavigate()
  const to = useTo()
  return ({ run }: Submitted) => navigate(to(`runs/${run.number}`))
}

/** A dropped file becomes a private checkpoint of a model, on a machine that makes and uploads it. */
function Upload({ destination }: { destination?: string }) {
  const { client } = useConnection()
  const follow = useFollowRun()
  const files = useFiles()
  const [typed, setTyped] = useState("")
  const [lane, setLane] = useState("")
  const file = files.files[0]
  const to = destination ?? typed.trim()
  return (
    <div className="stack">
      {!destination && (
        <label>
          Model
          <input className="field" spellCheck={false} placeholder="org/name" value={typed} onChange={(event) => setTyped(event.target.value)} />
        </label>
      )}
      <label>
        Lane
        <input className="field" spellCheck={false} placeholder="bf16, fp8, int8, …" value={lane} onChange={(event) => setLane(event.target.value)} />
      </label>
      {file ? <FileRow file={file} onRemove={() => files.remove(file.id)} /> : <DropZone onFiles={(chosen) => files.add(chosen)}>Drop its weights here, or choose</DropZone>}
      <OnMachine
        body={{ source: file?.digest ?? "", destination: to, lane: lane.trim() }}
        label="Upload"
        disabled={!file?.digest || !to || !lane.trim()}
        quote={(body) => client.jobQuote("cozy/model_upload", body)}
        ask={(body, key) => client.job("cozy/model_upload", body, key)}
        onDone={follow}
      />
    </div>
  )
}

export function Models() {
  const { client } = useConnection()
  const to = useTo()
  const signedIn = useSignedIn()
  const [typed, setTyped] = useState("")
  const q = useDeferredValue(typed.trim())
  const [limit, setLimit] = useState(50)
  const [showNsfw] = useShowNsfw()
  const { data, error } = useQuery({
    queryKey: ["models", q, limit, showNsfw],
    queryFn: ({ signal }) => client.models(q, limit, signal, { noTag: showNsfw ? undefined : NSFW }),
    staleTime: 60_000,
  })
  return (
    <LibraryFrame on="models">
      <div className="row center">
        <input className="field wide" aria-label="Search models" placeholder="Search models, tags, or org/name" spellCheck={false} value={typed} onChange={(event) => setTyped(event.target.value)} />
      </div>
      <Failure error={error} lead="Couldn't search the models" />
      <ul className="list">
        {data?.models
          .filter((found) => showNsfw || !isNsfw(found.tags))
          .map((found) => (
            <li key={found.model}>
              <Link to={to(`models/${encodeURIComponent(found.model)}`)}>
                <b>{shown(found.model)}</b>
                <small>
                  {[found.private ? "private" : found.release, found.family, found.lanes.join(", "), found.author?.name].filter(Boolean).join(" · ")}{" "}
                  <Tags tags={found.tags} />
                </small>
              </Link>
            </li>
          ))}
      </ul>
      {data && !data.models.length && <p className="muted">{q ? `No model matches ${q}.` : "No models you can see."}</p>}
      {Boolean(data?.older) && (
        <button type="button" className="tb" onClick={() => setLimit(Math.min(100, limit + 25))}>
          {data?.older} more
        </button>
      )}
      {signedIn && (
        <details className="sect">
          <summary>Upload a file</summary>
          <Upload />
        </details>
      )}
    </LibraryFrame>
  )
}

function Publish({ name, checkpoints, onDone }: { name: string; checkpoints: ModelCheckpoint[]; onDone: () => void }) {
  const { client } = useConnection()
  const ask = useAsk()
  const [version, setVersion] = useState("")
  const [chosen, setChosen] = useState<Set<string>>(new Set())
  return (
    <>
      <p className="muted">Choose one checkpoint per lane for the public release.</p>
      <table className="grid">
        <tbody>
          {checkpoints.map((checkpoint) => (
            <tr key={checkpoint.checkpoint}>
              <td>
                <input
                  type="checkbox"
                  aria-label={`Publish the ${checkpoint.lane} checkpoint of ${checkpoint.created_at}`}
                  checked={chosen.has(checkpoint.checkpoint)}
                  onChange={(event) => {
                    const next = new Set(chosen)
                    if (event.target.checked) next.add(checkpoint.checkpoint)
                    else next.delete(checkpoint.checkpoint)
                    setChosen(next)
                  }}
                />
              </td>
              <td className="nm">{checkpoint.lane}</td>
              <td>{bytes(checkpoint.bytes)}</td>
              <td>{stamp(checkpoint.created_at)}</td>
              <td>{checkpoint.releases.length ? checkpoint.releases.join(", ") : "private"}</td>
              <td className="end">
                <DeleteCheckpoint name={name} checkpoint={checkpoint} onDone={onDone} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="row center">
        <input className="field" spellCheck={false} aria-label="Version" placeholder="Version, 0.1.0" value={version} onChange={(event) => setVersion(event.target.value)} />
        <Confirm
          label="Publish"
          danger={false}
          disabled={ask.busy || !version.trim() || !chosen.size}
          says={`Publish ${shown(name)} ${version.trim()} with ${chosen.size} ${chosen.size === 1 ? "lane" : "lanes"}? Its checkpoint mapping stays fixed; you can make the release private later.`}
          yes="Publish it"
          onYes={() =>
            void ask.run(async () => {
              const released = await client.modelPublish(name, version.trim(), [...chosen])
              released.warnings?.forEach((warning) => say(warning))
              setVersion("")
              setChosen(new Set())
              onDone()
            })
          }
        />
      </div>
      <Failure error={ask.error} lead="Couldn't publish it" />
    </>
  )
}

function DeleteCheckpoint({ name, checkpoint, onDone }: { name: string; checkpoint: ModelCheckpoint; onDone: () => void }) {
  const { client } = useConnection()
  const ask = useAsk()
  if (checkpoint.releases.length) return null
  return (
    <>
      <Confirm
        label="Delete"
        disabled={ask.busy}
        says={`Delete this private ${checkpoint.lane} checkpoint of ${shown(name)}? No release names it, and the Hub reclaims what nothing else uses. It can't be undone.`}
        yes="Delete it"
        onYes={() =>
          void ask.run(async () => {
            await client.deleteModelCheckpoint(name, checkpoint.checkpoint)
            onDone()
          })
        }
      />
      <Failure error={ask.error} />
    </>
  )
}

/** Where a model's family comes from, as a person reads it. */
function familyLine(detail: ModelDetail): string | undefined {
  if (!detail.family) return detail.kind === "lora" ? "no family: a LoRA needs one to run" : undefined
  if (detail.family_from === "weights") return `${detail.family}, from its weights`
  return detail.weights_family ? `${detail.family}, set by its owner (its weights say ${detail.weights_family})` : `${detail.family}, set by its owner`
}

/** Its owner's family and tags. Picking "from its weights" (or none) clears the label; a checkpoint needs no family, a LoRA one to run. */
function Labels({ name, detail }: { name: string; detail: ModelDetail }) {
  const { client } = useConnection()
  const queries = useQueryClient()
  const ask = useAsk()
  const [adding, setAdding] = useState("")
  const { data: known } = useQuery({ queryKey: ["families"], queryFn: ({ signal }) => client.families(signal), staleTime: Infinity })
  const label = detail.family_from === "owner" ? detail.family : undefined
  const families = [...new Set([...(known?.families ?? []), ...(label ? [label] : [])])].sort()
  const tags = detail.tags ?? []
  const edit = (change: ModelEdit) =>
    void ask.run(async () => {
      const edited = await client.editModel(name, change)
      edited.warnings?.forEach((warning) => say(warning))
      queries.setQueryData(["model", name], edited.model)
      void queries.invalidateQueries({ queryKey: ["models"] })
    })
  const none = detail.weights_family ? `From its weights: ${detail.weights_family}` : detail.kind === "lora" ? "None (a LoRA needs one to run)" : "None"
  const add = () => {
    const tag = adding.trim()
    if (tag) edit({ add_tags: [tag] })
    setAdding("")
  }
  return (
    <div className="stack">
      <label>
        Family
        <select className="field" value={label ?? ""} disabled={ask.busy} onChange={(event) => edit({ family: event.target.value || "none" })}>
          <option value="">{none}</option>
          {families.map((family) => (
            <option key={family} value={family}>
              {family}
            </option>
          ))}
        </select>
      </label>
      <label className="row center">
        <input type="checkbox" checked={tags.includes(NSFW)} disabled={ask.busy} onChange={(event) => edit(event.target.checked ? { add_tags: [NSFW] } : { remove_tags: [NSFW] })} />
        NSFW: it can make sexual or nude images, asked for or not
      </label>
      <div className="row center">
        <Tags tags={tags} onRemove={(tag) => edit({ remove_tags: [tag] })} />
        <input
          className="field"
          aria-label="Add a tag"
          placeholder="Add a tag"
          spellCheck={false}
          value={adding}
          disabled={ask.busy}
          onChange={(event) => setAdding(event.target.value)}
          onKeyDown={(event) => event.key === "Enter" && add()}
        />
        <button type="button" className="tb" disabled={ask.busy || !adding.trim()} onClick={add}>
          Add
        </button>
      </div>
      <Failure error={ask.error} lead="Couldn't change it" />
    </div>
  )
}

function Releases({ detail, signedIn, onDone }: { detail: ModelDetail; signedIn: boolean; onDone: () => void }) {
  const { client } = useConnection()
  const ask = useAsk()
  return (
    <>
      <table className="grid">
        <tbody>
          {detail.releases.flatMap((release) =>
            release.lanes.map((lane, at) => (
              <tr key={`${release.version}:${lane.lane}`}>
                <td className="nm">{at === 0 ? `${release.version}${release.visibility === "private" ? " (private)" : ""}` : ""}</td>
                <td className="nm">{lane.lane}</td>
                <td>{bytes(lane.bytes)}</td>
                <td>{lane.kinds?.join(", ")}</td>
                <td>{at === 0 && release.created_at ? stamp(release.created_at) : ""}</td>
                <td className="end">
                  {at === 0 && signedIn && detail.checkpoints.length > 0 && release.visibility === "public" && (
                    <Confirm
                      label="Unpublish"
                      disabled={ask.busy}
                      says={`Make ${shown(detail.model)}@${release.version} private? Existing installed copies remain.`}
                      yes="Make private"
                      onYes={() => void ask.run(async () => {
                        await client.modelUnpublish(detail.model, release.version)
                        onDone()
                      })}
                    />
                  )}
                  {at === 0 && signedIn && detail.checkpoints.length > 0 && (
                    <Confirm
                      label="Delete"
                      disabled={ask.busy}
                      says={`Delete release ${release.version} of ${shown(detail.model)}? The version cannot be reused; its checkpoints remain with the model.`}
                      yes="Delete it"
                      onYes={() =>
                        void ask.run(async () => {
                          await client.deleteModelRelease(detail.model, release.version)
                          onDone()
                        })
                      }
                    />
                  )}
                </td>
              </tr>
            )),
          )}
        </tbody>
      </table>
      {!detail.releases.length && <p className="muted">No release yet: it's private.</p>}
      <Failure error={ask.error} lead="Couldn't change the release" />
    </>
  )
}

export function ModelDrawer() {
  const { client } = useConnection()
  const queries = useQueryClient()
  const navigate = useNavigate()
  const to = useTo()
  const follow = useFollowRun()
  const signedIn = useSignedIn()
  const name = useParams().name ?? ""
  const { data, error } = useQuery({ queryKey: ["model", name], queryFn: ({ signal }) => client.model(name, signal) })
  const again = () => void queries.invalidateQueries({ queryKey: ["model", name] })
  const [release, setRelease] = useState("")
  const [lane, setLane] = useState("")
  const [quantizeLane, setQuantizeLane] = useState("")
  const [quantizePackage, setQuantizePackage] = useState("")
  const remove = useAsk()
  const member = Boolean(data?.checkpoints.length)
  const lanes = [...new Set(data?.releases.flatMap((row) => row.lanes.map((held) => held.lane)))]
  const model = release ? `${name}@${release}` : name

  return (
    <LibraryFrame on="models" title={shown(name)} back={to("models")}>
      <Failure error={error} lead={`Couldn't read ${shown(name)}`} />
      {data && (
        <p className="muted">
          {[familyLine(data), data.author && `by ${data.author.name}${data.author.parked ? " (not claimed)" : ""}`, data.triggers?.length && `${data.triggers.length > 1 ? "triggers" : "trigger"}: ${data.triggers.map((trigger) => `“${trigger}”`).join(", ")}`, data.strength !== undefined && data.strength !== null && `strength ${data.strength}`, data.source && `from ${data.source}`]
            .filter(Boolean)
            .join(" · ")}{" "}
          {!member && <Tags tags={data.tags} />}
        </p>
      )}
      {member && data && (
        <Section title="Family and tags">
          <Labels name={name} detail={data} />
        </Section>
      )}
      <Section title="Download">
        <div className="row center">
          <select className="field" aria-label="Which release" value={release} onChange={(event) => setRelease(event.target.value)}>
            <option value="">The newest release</option>
            {data?.releases.map((row) => (
              <option key={row.version} value={row.version}>
                {row.version}
              </option>
            ))}
          </select>
          <select className="field" aria-label="Which lane" value={lane} onChange={(event) => setLane(event.target.value)}>
            <option value="">The machine's best lane</option>
            {lanes.map((held) => (
              <option key={held} value={held}>
                {held}
              </option>
            ))}
          </select>
        </div>
        <OnMachine
          body={{ model, ...(lane ? { lane } : {}) }}
          label="Download"
          quote={(body) => client.jobQuote("cozy/model_download", body)}
          ask={(body, key) => client.job("cozy/model_download", body, key)}
          onDone={follow}
        />
      </Section>
      <Section title="Releases">{data && <Releases detail={data} signedIn={signedIn} onDone={again} />}</Section>
      {signedIn && (
        <>
          <details className="sect">
            <summary>Quantize</summary>
            <div className="stack">
              <label>
                Lane
                <input className="field" spellCheck={false} placeholder="The lane to write: fp8, int8, …" value={quantizeLane} onChange={(event) => setQuantizeLane(event.target.value)} />
              </label>
              <label>
                Package
                <input className="field" spellCheck={false} placeholder="org/name, whose recipe writes it" value={quantizePackage} onChange={(event) => setQuantizePackage(event.target.value)} />
              </label>
            </div>
            <OnMachine
              body={{ model, lane: quantizeLane.trim(), package: quantizePackage.trim() }}
              label="Quantize"
              disabled={!quantizeLane.trim() || !quantizePackage.trim()}
              quote={(body) => client.jobQuote("cozy/model_quantize", body)}
              ask={(body, key) => client.job("cozy/model_quantize", body, key)}
              onDone={follow}
            />
          </details>
          <details className="sect">
            <summary>Upload a file</summary>
            <Upload destination={name} />
          </details>
          {member && data && (
            <details className="sect" open>
              <summary>Commits and publish</summary>
              <Publish name={name} checkpoints={data.checkpoints} onDone={again} />
            </details>
          )}
          <Section title="Delete">
            <Confirm
              label={`Delete ${shown(name)}`}
              disabled={remove.busy}
              says={`Delete ${shown(name)} from the Hub, with every release and checkpoint? The Hub reclaims what nothing else uses, and it can't be undone. Machines keep what they hold.`}
              yes="Delete it for good"
              onYes={() =>
                void remove.run(async () => {
                  await client.deleteModel(name)
                  say(`${shown(name)} is deleted.`)
                  void queries.invalidateQueries({ queryKey: ["models"] })
                  navigate(to("models"))
                })
              }
            />
            <Failure error={remove.error} lead="Couldn't delete the model" />
          </Section>
        </>
      )}
    </LibraryFrame>
  )
}
