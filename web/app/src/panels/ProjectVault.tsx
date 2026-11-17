import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useRef, useState } from "react"
import QRCode from "react-qr-code"
import { ApiError, type Client, type VaultAsk, type VaultProgress } from "../api/client"
import { useConnection } from "../app/Connection"
import { readAddressesAgain } from "../app/project"
import { CopyButton } from "../ui/Copyable"
import { useSignal } from "../ui/signal"
import { Failure, useAsk } from "./common"
import { bytes } from "./format"

export const vaultKey = (id: string) => ["project-vault", id] as const
/** Kept by the stream: `project.vault` says each change, and `hello` has it read again. A read Calcifer couldn't answer is asked again, later
 *  each time; a refusal isn't. */
export const vaultQuery = (client: Client, id: string) => queryOptions({
  queryKey: vaultKey(id),
  queryFn: ({ signal }) => client.vault(id, signal),
  staleTime: 0,
  refetchInterval: ({ state }) => {
    const lost = state.status === "error" && state.error instanceof ApiError && (state.error.status === 0 || state.error.status >= 500)
    return lost ? Math.min(30_000, 1000 * 2 ** state.fetchFailureCount) : false
  },
})

type Pending = { action: string; event: VaultProgress | null }

function pendingLine(pending: Pending | null): string {
  if (!pending) return "Working…"
  const action: Record<string, string> = {
    encrypt: "Encrypting the project", decrypt: "Decrypting the project", rotate: "Re-encrypting the project", unlock: "Unlocking the project",
    lock: "Locking the project", passphrase: "Changing the passphrase", recovery: "Making a new recovery secret",
  }
  if (!pending.event || pending.event.type === "accepted") return `${action[pending.action] ?? "Working"}…`
  const phase: Record<string, string> = {
    preparing: "Getting the project ready", validating: "Checking the project's files", mounting: "Opening the encrypted files",
    copying: pending.action === "decrypt" ? "Decrypting files" : "Encrypting files",
    verifying: "Checking the new copy", committing: "Saving the change", opening: "Opening the project",
    cleaning_up: "Cleaning up", waiting: "Waiting for the project's work to finish",
    protecting_history: "Encrypting the project's history", compacting_history: "Compacting the project's history",
    updating_project_storage: "Updating the project's files", finishing_storage_change: "Finishing up",
  }
  const event = pending.event
  const counts = [
    ...(event.files_total > 0 ? [`${event.files_done.toLocaleString()} of ${event.files_total.toLocaleString()} files`] : event.files_done > 0 ? [`${event.files_done.toLocaleString()} files so far`] : []),
    ...(event.bytes_total > 0 ? [`${bytes(event.bytes_done)} of ${bytes(event.bytes_total)}`] : event.bytes_done > 0 ? [`${bytes(event.bytes_done)} so far`] : []),
  ]
  const said = phase[event.phase] ?? "Working"
  return counts.length ? `${said}, ${counts.join(", ")}` : `${said}…`
}

export function Recovery({ secret, onDone, disabled = false }: { secret: string; onDone: () => void; disabled?: boolean }) {
  const [saved, setSaved] = useState(false)
  const download = () => {
    const url = URL.createObjectURL(new Blob([`${secret}\n`], { type: "text/plain;charset=utf-8" }))
    const link = document.createElement("a")
    link.href = url
    link.download = "cozy-project-recovery.txt"
    link.click()
    setTimeout(() => URL.revokeObjectURL(url), 0)
  }
  const print = () => {
    const paper = window.open("", "_blank", "popup,width=520,height=360")
    if (!paper) return
    paper.document.title = "Cozy project recovery"
    const title = paper.document.createElement("h1")
    title.textContent = "Project recovery secret"
    const hint = paper.document.createElement("p")
    hint.textContent = "Keep this somewhere away from the computer your project is on."
    const code = paper.document.createElement("pre")
    code.style.whiteSpace = "pre-wrap"
    code.style.overflowWrap = "anywhere"
    code.textContent = secret
    paper.document.body.replaceChildren(title, hint, code)
    paper.addEventListener("afterprint", () => paper.close(), { once: true })
    paper.print()
  }
  return (
    <section className="sect" aria-label="Save recovery secret">
      <h3>Save your recovery secret</h3>
      <p>Save this somewhere safe. It unlocks the project if you lose your passphrase, and this is the only time you'll see it. Keep it on your phone, in a password manager, or on paper, away from this computer. The Hub never gets a copy.</p>
      <pre className="yaml" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{secret}</pre>
      <QRCode value={secret} size={180} title="Scan to save the recovery secret on your phone or another device" />
      <div className="row">
        <CopyButton text={secret} title="Copy the recovery secret" />
        <button className="tb" type="button" onClick={download}>Download</button>
        <button className="tb" type="button" onClick={print}>Print</button>
      </div>
      <label><input type="checkbox" checked={saved} onChange={(event) => setSaved(event.target.checked)} /> I saved this somewhere away from this computer</label>
      <div className="row"><button className="tb primary" type="button" disabled={!saved || disabled} onClick={onDone}>Done</button></div>
    </section>
  )
}

/** A project's encryption: on and off, lock and unlock, its passphrase and recovery secret (in Project settings, cl-1097). */
export function VaultBody({ id }: { id: string }) {
  const { client, outbox, bridge } = useConnection()
  const queries = useQueryClient()
  const { data: status, error, refetch } = useQuery(vaultQuery(client, id))
  const pending = useSignal(outbox.owed)
  const ask = useAsk()
  const [operation, setOperation] = useState<Pending | null>(null)
  const [skipped, setSkipped] = useState<string[]>([])
  const [passphrase, setPassphrase] = useState("")
  const [confirmation, setConfirmation] = useState("")
  const [recovery, setRecovery] = useState("")
  const [credential, setCredential] = useState<"passphrase" | "recovery">("passphrase")
  const [newRecovery, setNewRecovery] = useState<string | null>(null)
  const [sourceRemoved, setSourceRemoved] = useState(false)
  const [ended, setEnded] = useState<number[]>([])
  const [idle, setIdle] = useState("1800")
  const [maximum, setMaximum] = useState("0")
  const mounted = useRef(false)
  const request = useRef<AbortController | null>(null)
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false; request.current?.abort(); request.current = null }
  }, [])

  useEffect(() => {
    if (status?.state === "plain" || (status?.encrypted && status.state !== "unlocked")) {
      setPassphrase(""); setConfirmation(""); setRecovery(""); setNewRecovery(null)
    }
  }, [status?.encrypted, status?.state])
  useEffect(() => {
    if (!status || status.state === "plain") setSourceRemoved(false)
  }, [status?.encrypted, status?.state])
  useEffect(() => {
    if (!status || error || (status.encrypted && status.state !== "unlocked")) setSkipped([])
  }, [status?.encrypted, status?.state, error])

  const change = (action: string, fields: Partial<VaultAsk> = {}) => ask.run(async () => {
    if (action === "encrypt") setSourceRemoved(false)
    setPassphrase(""); setConfirmation(""); setRecovery("")
    setSkipped([])
    setOperation({ action, event: null })
    const stop = new AbortController()
    request.current = stop
    const epoch = bridge.projectEpoch(id)
    try {
      const answer = await client.vaultChange(id, { action, ...fields }, (event) => {
        if (mounted.current) setOperation({ action, event })
      }, stop.signal)
      if (!mounted.current) { delete answer.recovery_secret; return answer.vault }
      setEnded(answer.ended ?? [])
      if (action === "decrypt") {
        // The mutation reply cannot reopen a view: a concurrent conversion or lock
        // may already have superseded it. The status query verifies current storage.
        setNewRecovery(null)
        delete answer.recovery_secret
        await queries.invalidateQueries({ queryKey: vaultKey(id) })
        await queries.invalidateQueries({ queryKey: ["projects"] })
        if (answer.skipped_files?.length) {
          const completed = bridge.projectEpoch(id)
          const current = await client.vault(id)
          if (mounted.current && !current.encrypted && current.state === "plain" && bridge.projectEpoch(id) === completed) setSkipped(answer.skipped_files)
        }
        return answer.vault
      }
      if (bridge.projectEpoch(id) !== epoch) {
        // Conversion/rekey can briefly close their own view. Recheck that completed
        // project before showing its secret; an unrelated lock suppresses late replies.
        const completed = bridge.projectEpoch(id)
        const current = await client.vault(id)
        if (!mounted.current || !["encrypt", "rotate"].includes(action) || !current.encrypted || current.state !== "unlocked" || bridge.projectEpoch(id) !== completed) {
          delete answer.recovery_secret
          await queries.invalidateQueries({ queryKey: vaultKey(id) })
          return current
        }
        answer.vault = current
      }
      queries.setQueryData(vaultKey(id), answer.vault)
      if (action === "unlock") await readAddressesAgain(queries)
      if (answer.recovery_secret) setNewRecovery(answer.recovery_secret)
      if (action === "encrypt") setSourceRemoved(true)
      if (answer.skipped_files?.length) setSkipped(answer.skipped_files)
      delete answer.recovery_secret
      await queries.invalidateQueries({ queryKey: ["projects"] })
      return answer.vault
    } finally {
      if (request.current === stop) request.current = null
      stop.abort()
      if (mounted.current) setOperation(null)
    }
  })
  const timeout = () => ({ idle_timeout_s: Number(idle), max_duration_s: Number(maximum) })
  const submit = (event: FormEvent, action: string) => {
    event.preventDefault()
    const fields = action === "unlock"
      ? credential === "recovery" ? { recovery_secret: recovery, ...timeout() } : { passphrase, ...timeout() }
      : { passphrase, ...timeout() }
    void change(action, fields)
  }
  const timeoutControls = (
    <details className="sect">
      <summary>When to lock again</summary>
      <label>Lock after this many seconds with no activity (0 for never)<input className="field" type="number" min="0" max="604800" required value={idle} onChange={(event) => setIdle(event.target.value)} /></label>
      <label>Lock after this many seconds unlocked, even if in use (0 for never)<input className="field" type="number" min="0" max="604800" required value={maximum} onChange={(event) => setMaximum(event.target.value)} /></label>
    </details>
  )
  const passwordControls = (
    <>
      <label>Passphrase<input className="field" type="password" maxLength={4096} autoComplete="new-password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} /></label>
      <label>Confirm passphrase<input className="field" type="password" maxLength={4096} autoComplete="new-password" value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
    </>
  )

  if (client.transport.channel) return <p>You can only change a project's encryption on the computer it's on. Open the project there.</p>
  if (!status) return <><p>Checking whether this project is locked…</p><Failure error={error} /><button className="tb" type="button" onClick={() => void refetch()}>Try again</button></>
  const unlocked = status.state === "unlocked"
  return (
    <div className="stack" data-vault-state={status.state}>
      <p>{!status.encrypted ? "This project isn't encrypted. Its files are ordinary files on this computer." : unlocked ? "This project is unlocked." : status.state === "locking" ? "Couldn't lock the project yet: another app still has one of its files open. Close that app, then try again." : "This project is locked."}</p>
      <Failure error={ask.error} plain />
      {sourceRemoved && !error && status.encrypted && unlocked && <p>The unencrypted copy of the project was removed.</p>}
      {ended.length > 0 && <p className="muted">Locking the project canceled {ended.length === 1 ? "run" : "runs"} {ended.join(", ")}.</p>}
      {skipped.length > 0 && !error && (!status.encrypted || unlocked) && <div className="warn">
        <p>Done, but these files were missing, so Cozy skipped them:</p>
        <ul>{skipped.map((file, at) => <li key={`${at}:${file}`}><code>{file}</code></li>)}</ul>
      </div>}
      {newRecovery && status.encrypted && unlocked && <Recovery secret={newRecovery} onDone={() => setNewRecovery(null)} />}
      {!status.encrypted ? (
        <form className="stack" onSubmit={(event) => submit(event, "encrypt")}>
          <p className="warn">Once the encrypted copy is verified, Cozy removes the unencrypted project. Other unencrypted copies made earlier, backups and shared cache copies aren't erased. Remove or secure those yourself.</p>
          {passwordControls}
          {timeoutControls}
          <button className="tb primary" type="submit" disabled={ask.busy || !passphrase || passphrase !== confirmation || pending.count > 0}>Encrypt project</button>
        </form>
      ) : status.state === "locking" ? (
        <button className="tb" type="button" disabled={ask.busy} onClick={() => void change("lock")}>Try locking again</button>
      ) : !unlocked ? (
        <form className="stack" onSubmit={(event) => submit(event, "unlock")}>
          <label>Unlock with<select className="field" value={credential} onChange={(event) => { setCredential(event.target.value as "passphrase" | "recovery"); setPassphrase(""); setRecovery("") }}><option value="passphrase">Passphrase</option><option value="recovery">Recovery secret</option></select></label>
          {credential === "passphrase"
            ? <label>Passphrase<input className="field" type="password" maxLength={4096} autoComplete="current-password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} /></label>
            : <label>Recovery secret<input className="field" type="password" maxLength={4096} autoComplete="off" spellCheck={false} value={recovery} onChange={(event) => setRecovery(event.target.value)} /></label>}
          {timeoutControls}
          <button className="tb primary" type="submit" disabled={ask.busy || !(credential === "recovery" ? recovery : passphrase)}>Unlock project</button>
        </form>
      ) : (
        <>
          {status.view && <p>The unlocked files are in <code>{status.view}</code></p>}
          {pending.count > 0 && <p className="muted">Wait for your edits and uploads to finish saving, then lock it.</p>}
          <button className="tb" type="button" disabled={ask.busy || pending.count > 0} onClick={() => void change("lock")}>Lock project</button>
          <details className="sect"><summary>Change passphrase</summary><form className="stack" onSubmit={(event) => submit(event, "passphrase")}>{passwordControls}<button className="tb" type="submit" disabled={ask.busy || !passphrase || passphrase !== confirmation}>Save passphrase</button></form></details>
          <details className="sect"><summary>Replace recovery secret</summary><p>The old recovery secret may still unlock copies of this project's keys made before you replace it.</p><button className="tb" type="button" disabled={ask.busy} onClick={() => void change("recovery")}>Make a new recovery secret</button></details>
          <details className="sect"><summary>Decrypt project</summary><p>This turns the project back into ordinary, unencrypted files. The project keeps all its files and history. First close any files or terminals that use the unlocked folder.</p><button className="tb" type="button" disabled={ask.busy || pending.count > 0} onClick={() => void change("decrypt")}>Decrypt project</button></details>
          <details className="sect"><summary>Change the encryption key</summary><p>Cozy re-encrypts every file the project keeps, and its history, with a new key. This can take a while and needs some free disk space while it runs.</p><form className="stack" onSubmit={(event) => submit(event, "rotate")}>{passwordControls}<button className="tb" type="submit" disabled={ask.busy || !passphrase || passphrase !== confirmation || pending.count > 0}>Re-encrypt project</button></form></details>
        </>
      )}
      {ask.busy && <p role="status">{pendingLine(operation)}</p>}
    </div>
  )
}

export function ProjectLocked({ id }: { id: string }) {
  return <main className="gate"><section style={{ maxWidth: 520, width: "100%", textAlign: "left" }}><h1>Encrypted project</h1><VaultBody key={id} id={id} /></section></main>
}
