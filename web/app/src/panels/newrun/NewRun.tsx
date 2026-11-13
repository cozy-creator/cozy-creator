import { useQuery } from "@tanstack/react-query"
import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react"
import { useNavigate, useSearchParams } from "react-router-dom"
import { useConnection } from "../../app/Connection"
import { useProjectId, useTo } from "../../app/Shell"
import type { RunRequest, Submitted, Target } from "../../api/calcifer"
import type { TargetInterface, TargetRequest } from "../../types"
import { Copyable } from "../../ui/Copyable"
import { say } from "../../ui/notes"
import { Failure } from "../common"
import { DrawerFrame } from "../Drawer"
import { DropZone, FileRow, useFiles } from "../FileDrop"
import { shown } from "../format"
import { OnMachine } from "../machines/OnMachine"
import { hideFiles, revealFiles } from "./fields"
import { RunForm, UNREADABLE } from "./RunForm"

/** Local code names a path on this computer: only a terminal can. */
const localCode = (target: string) => /^(\.{0,2}[/\\]|~|[A-Za-z]:[/\\])/.test(target) || /\.py(\/|$)/.test(target)

function Picker({ onPick }: { onPick: (target: string) => void }) {
  const { client } = useConnection()
  const [typed, setTyped] = useState("")
  const query = useDeferredValue(typed.trim())
  const { data, error } = useQuery({
    queryKey: ["packages", query, 50],
    queryFn: ({ signal }) => client.packages(query, 50, signal),
    staleTime: 60_000,
  })
  const text = typed.trim()
  return (
    <>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault()
          if (text) onPick(text)
        }}
      >
        <input className="field" autoFocus spellCheck={false} aria-label="A package or function" placeholder="A package, or package/function" value={typed} onChange={(event) => setTyped(event.target.value)} />
        <button type="submit" className="tb" disabled={!text}>
          Open
        </button>
      </form>
      <Failure error={error} lead="Couldn't search the packages" />
      <ul className="rows choose">
        {data?.packages.map((found) => (
          <li key={found.package}>
            <button type="button" onClick={() => onPick(found.package)}>
              <span className="nm">{shown(found.package)}</span>
              <small>{found.latest_release}</small>
            </button>
          </li>
        ))}
      </ul>
    </>
  )
}

/** Run any function: a form from its interface, or YAML text. */
function Asking({ target, resolved, id, startInput }: { target: string; resolved: Target; id: string; startInput?: Record<string, unknown> }) {
  const { client } = useConnection()
  const navigate = useNavigate()
  const to = useTo()
  const files = useFiles([], id)
  const entry = (resolved.interface as TargetInterface).entrypoints?.find((candidate) => candidate.name === resolved.function)
  const fields = useMemo(() => (resolved.request as TargetRequest | null | undefined)?.fields ?? [], [resolved])
  const declared = useMemo(() => new Set(fields.map((field) => field.name)), [fields])
  const [values, setValues] = useState<Record<string, unknown>>(startInput ?? {})
  const [mode, setMode] = useState<"form" | "text">("form")
  const labels = useRef(new Map<string, string>())
  const [text, setText] = useState("")
  const [modelTerms, setModelTerms] = useState<Record<string, string>>({})
  const [loras, setLoras] = useState("")
  const [lane, setLane] = useState("")
  const [gpus, setGpus] = useState("")
  const [switchNote, setSwitchNote] = useState("")

  // What Run again brought that this release doesn't declare goes on being sent, and says so. `models` is what it chose, which `input` takes.
  const extra = Object.keys(values).filter((name) => !declared.has(name) && name !== "models")
  const unreadable = Object.values(values).includes(UNREADABLE)

  const toText = () => {
    setText(hideFiles(JSON.stringify(values, null, 2), labels.current))
    setSwitchNote("")
    setMode("text")
  }
  const toForm = () => {
    try {
      setValues(JSON.parse(revealFiles(text, labels.current) || "{}"))
      setSwitchNote("")
      setMode("form")
    } catch {
      setSwitchNote("Only JSON goes back to the form: this text stays as it is, and runs as it is.")
    }
  }

  const body = useMemo((): RunRequest => {
    const terms = Object.entries(modelTerms)
      .filter(([, ref]) => ref.trim())
      .map(([slot, ref]) => `model.${slot}=${ref.trim()}`)
    const loraList = loras
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
    return {
      target,
      project: id,
      ...(mode === "form" ? { input: values } : { input_text: revealFiles(text, labels.current) }),
      ...(terms.length ? { terms } : {}),
      ...(loraList.length ? { loras: loraList } : {}),
      ...(lane.trim() ? { lane: lane.trim() } : {}),
      ...(Number(gpus) > 0 ? { gpus: Number(gpus) } : {}),
    }
  }, [target, id, mode, values, text, modelTerms, loras, lane, gpus])

  const done = ({ run, notes, replay }: Submitted) => {
    // One note for one run, whatever it has to say.
    const lines = [replay ? `This was asked already: run ${run.number}.` : "", ...(notes ?? []).map((note) => note.message)].filter(Boolean)
    if (lines.length) say(lines.join(" "))
    navigate(to(`runs/${run.number}`))
  }

  // In text, a dropped file is named by its label, as a field's value.
  const dropInText = (chosen: File[]) => {
    files.add(chosen, (sent) => {
      if (!sent.digest) return
      const label = hideFiles(sent.digest, labels.current)
      setText((now) => (now.includes(label) ? now : `${now}${now && !now.endsWith("\n") ? "\n" : ""}${sent.name.replace(/\W+/g, "_")}: ${label}\n`))
    })
  }

  return (
    <>
      {resolved.summary && <p className="muted">{resolved.summary}</p>}
      {entry?.description && <p className="muted">{entry.description}</p>}
      <p className="muted">
        {resolved.release_from === "source" ? `From source: ${resolved.release}` : `Release ${resolved.release} (${resolved.release_from})`}
      </p>
      <div className="row tabs">
        <button type="button" className={`tab ${mode === "form" ? "on" : ""}`} onClick={() => mode !== "form" && toForm()}>
          Form
        </button>
        <button type="button" className={`tab ${mode === "text" ? "on" : ""}`} onClick={() => mode !== "text" && toText()}>
          YAML
        </button>
      </div>
      {switchNote && <p className="warn">{switchNote}</p>}
      {mode === "form" && (
        <>
          <RunForm fields={fields} values={values} files={files} target={target} onChange={(name, value) => setValues((now) => {
            const next = { ...now }
            if (value === undefined) delete next[name]
            else next[name] = value
            return next
          })} />
          {extra.length > 0 && (
            <p className="warn">
              Not in this release's interface: {extra.join(", ")}. They're sent as they were, and Calcifer warns and drops what it can't use.
            </p>
          )}
          {unreadable && <p className="warn">A field above doesn't read, so this can't run yet.</p>}
        </>
      )}
      {mode === "text" && (
        <>
          <textarea
            className="field area code"
            spellCheck={false}
            rows={14}
            aria-label="The input, as YAML"
            placeholder={"prompt: a lighthouse at dusk\nseed: 7"}
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
          <DropZone multiple onFiles={dropInText}>
            Drop files here, or choose. Each is named by its label in the text.
          </DropZone>
          {files.files
            .filter((file) => file.digest && labels.current.has(file.digest))
            .map((file) => (
              <FileRow key={file.id} file={file} label={labels.current.get(file.digest!)} onRemove={() => files.remove(file.id)} />
            ))}
          {files.files
            .filter((file) => !file.digest)
            .map((file) => (
              <FileRow key={file.id} file={file} onRemove={() => files.remove(file.id)} />
            ))}
          {[...labels.current]
            .filter(([digest]) => !files.files.some((file) => file.digest === digest))
            .map(([digest, label]) => (
              <p className="muted" key={digest}>
                <code>{label}</code> is a file it was given.
              </p>
            ))}
          <p className="muted">A field the function doesn't declare is dropped with a warning, shown on the run.</p>
        </>
      )}
      <details className="sect">
        <summary>Models, lane and GPUs</summary>
        <div className="stack">
          {entry?.models?.map((slot) => {
            const param = slot.path.split(".models.")[1] ?? slot.path
            return (
              <label key={slot.path}>
                {param}
                <input
                  className="field"
                  spellCheck={false}
                  placeholder={slot.default ? shown(slot.default) : "the function's default"}
                  value={modelTerms[param] ?? ""}
                  onChange={(event) => setModelTerms((now) => ({ ...now, [param]: event.target.value }))}
                />
              </label>
            )
          })}
          <label>
            LoRAs
            <textarea className="field area" rows={2} spellCheck={false} placeholder="[slot=]org/lora[,strength], one a line" value={loras} onChange={(event) => setLoras(event.target.value)} />
          </label>
          <label>
            Lane
            <input className="field" spellCheck={false} placeholder="the Hub's pick for the machine" value={lane} onChange={(event) => setLane(event.target.value)} />
          </label>
          <label>
            GPUs
            <input className="field" type="number" min={1} placeholder="every GPU its slots support" value={gpus} onChange={(event) => setGpus(event.target.value)} />
          </label>
        </div>
      </details>
      {resolved.contract && (
        <details className="sect">
          <summary>Its contract</summary>
          <pre className="yaml">{resolved.contract}</pre>
        </details>
      )}
      <OnMachine
        body={body}
        label="Run"
        disabled={files.moving || unreadable}
        quote={(ask) => client.runQuote(ask)}
        ask={(ask, key) => client.submit(id, ask, key)}
        onDone={done}
      />
      {files.moving && <p className="muted">Waiting for its files to reach Calcifer.</p>}
    </>
  )
}

/** A function, from a package, a typed name, the Library, or a run being run again. */
export function NewRun() {
  const { client } = useConnection()
  const id = useProjectId()!
  const to = useTo()
  const [params] = useSearchParams()
  const again = params.get("again")
  const [picked, setPicked] = useState<string | null>(params.get("target"))

  const { data: asked, error: askedError } = useQuery({
    queryKey: ["project-run", id, Number(again), "full", "again"],
    queryFn: ({ signal }) => client.runFull({ project: id, run: Number(again) }, signal),
    enabled: Boolean(id && again),
  })
  const askedTarget = asked?.request?.target
  useEffect(() => {
    if (askedTarget) setPicked(askedTarget)
  }, [askedTarget])

  const local = picked ? localCode(picked) : false
  const { data: resolved, error } = useQuery({
    queryKey: ["target", picked],
    queryFn: ({ signal }) => client.target(picked!, signal),
    enabled: Boolean(picked) && !local,
    staleTime: 60_000,
  })

  // A package of one function is that function.
  const only = resolved && !resolved.function && resolved.functions?.length === 1 ? resolved.functions[0].function : null
  useEffect(() => {
    if (only && resolved) setPicked(`${resolved.package}/${only}`)
  }, [only, resolved])

  const startInput = useMemo(() => {
    const input = asked?.request?.input
    return input && Object.fromEntries(Object.entries(input).filter(([name, value]) => !(name === "seed" && value === undefined)))
  }, [asked])

  const title = picked ? `Run ${shown(picked)}` : "New run"
  return (
    <DrawerFrame title={title} back={picked && !again ? undefined : to("runs")}>
      {picked && !again && (
        <button type="button" className="more topback" onClick={() => setPicked(null)}>
          ← Another function
        </button>
      )}
      <Failure error={askedError} lead={`Couldn't read run ${again}`} />
      {again && asked && !asked.request && <p className="warn">This Calcifer has no record of what run {again} was asked.</p>}
      {again && asked?.request && <p className="muted">From run {again}: its input, models and LoRAs. Not its lane, GPUs or attention kernel, which are for you to say below.</p>}
      {!picked && !again && <Picker onPick={setPicked} />}
      {local && picked && (
        <>
          <p className="muted">Local code runs from a terminal: it names a path on this computer.</p>
          <Copyable text={`cozy run ${picked}`} />
        </>
      )}
      <Failure error={error} lead={`Couldn't read ${picked}`} />
      {resolved && !resolved.function && (resolved.functions?.length ?? 0) > 1 && (
        <ul className="rows choose">
          {resolved.functions?.map((choice) => (
            <li key={choice.function}>
              <button type="button" onClick={() => setPicked(`${resolved.package}/${choice.function}`)}>
                <span className="nm">{choice.function}</span>
                <small>{choice.description}</small>
              </button>
            </li>
          ))}
        </ul>
      )}
      {resolved?.function && picked && (!again || asked) && (
        <Asking key={`${picked}:${again ?? ""}`} target={picked} resolved={resolved} id={id} startInput={startInput} />
      )}
    </DrawerFrame>
  )
}
