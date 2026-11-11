import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useRef, useState } from "react"
import QRCode from "react-qr-code"
import type { Client, VaultAsk, VaultProgress } from "../api/client"
import { useConnection } from "../app/Connection"
import { useProjectId } from "../app/Shell"
import { useSignal } from "../ui/signal"
import { DrawerFrame } from "./Drawer"
import { Failure, useAsk } from "./common"
import { bytes } from "./format"

export const vaultKey = (id: string) => ["project-vault", id] as const
export const vaultQuery = (client: Client, id: string) => queryOptions({
  queryKey: vaultKey(id),
  queryFn: ({ signal }) => client.vault(id, signal),
  staleTime: 0,
  refetchInterval: 1000,
  refetchIntervalInBackground: true,
})

type Pending = { action: string; event: VaultProgress | null }

function pendingLine(pending: Pending | null): string {
  if (!pending) return "Working…"
  const action: Record<string, string> = {
    encrypt: "Encryption", decrypt: "Decryption", rotate: "Key rotation", unlock: "Unlock",
    lock: "Lock", passphrase: "Passphrase update", recovery: "Recovery update",
  }
  const name = action[pending.action] ?? "Project change"
  if (!pending.event) return `${name}: sending request…`
  if (pending.event.type === "accepted") return `${name} request accepted…`
  const phase: Record<string, string> = {
    preparing: "Preparing project storage", validating: "Checking project storage", mounting: "Opening encrypted storage",
    copying: pending.action === "decrypt" ? "Decrypting project files" : "Encrypting project files",
    verifying: "Verifying project files", committing: "Saving project changes", opening: "Opening project",
    cleaning_up: "Finishing cleanup", waiting: "Waiting for project work to finish",
  }
  const event = pending.event
  const counts = [
    ...(event.files_done > 0 ? [`${event.files_done.toLocaleString()} ${event.files_done === 1 ? "file" : "files"} done`] : []),
    ...(event.bytes_done > 0 ? [`${bytes(event.bytes_done)} copied`] : []),
  ]
  return `${phase[event.phase] ?? "Working on project storage"}…${counts.length ? ` ${counts.join(" · ")}` : ""}`
}

export function Recovery({ secret, onDone, disabled = false }: { secret: string; onDone: () => void; disabled?: boolean }) {
  const [saved, setSaved] = useState(false)
  const [copied, setCopied] = useState(false)
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
    hint.textContent = "Store this away from the computer that holds your project."
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
      <p>Keep it on your phone, in a password manager, or on paper away from this computer. TensorHub does not receive it.</p>
      <pre className="yaml" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{secret}</pre>
      <QRCode value={secret} size={180} title="Recovery secret for transfer to your own device" />
      <div className="row">
        <button className="tb" type="button" onClick={() => void navigator.clipboard.writeText(secret).then(() => setCopied(true))}>{copied ? "Copied" : "Copy"}</button>
        <button className="tb" type="button" onClick={download}>Download</button>
        <button className="tb" type="button" onClick={print}>Print</button>
      </div>
      <label><input type="checkbox" checked={saved} onChange={(event) => setSaved(event.target.checked)} /> I saved this somewhere away from this computer</label>
      <button className="tb primary" type="button" disabled={!saved || disabled} onClick={onDone}>Done</button>
    </section>
  )
}

function VaultBody({ id }: { id: string }) {
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
  const [preserved, setPreserved] = useState<string | null>(null)
  const [sourceRemoved, setSourceRemoved] = useState(false)
  const [ended, setEnded] = useState<number[]>([])
  const [preservePlaintext, setPreservePlaintext] = useState(false)
  const [idle, setIdle] = useState("900")
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
    if (status?.encrypted) setPreservePlaintext(false)
    if (!status || status.state === "plain") setSourceRemoved(false)
  }, [status?.encrypted, status?.state])
  useEffect(() => {
    if (!status || error || (status.encrypted && status.state !== "unlocked")) setSkipped([])
  }, [status?.encrypted, status?.state, error])

  const change = (action: string, fields: Partial<VaultAsk> = {}) => ask.run(async () => {
    if (action === "encrypt") { setSourceRemoved(false); setPreserved(null) }
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
      if (answer.recovery_secret) setNewRecovery(answer.recovery_secret)
      if (answer.preserved_source) setPreserved(answer.preserved_source)
      if (action === "encrypt") { setSourceRemoved(!answer.preserved_source); if (!answer.preserved_source) setPreserved(null) }
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
      : { passphrase, ...(action === "encrypt" ? { ...(preservePlaintext ? { preserve_plaintext: true } : {}), remove_plaintext: !preservePlaintext } : {}), ...timeout() }
    void change(action, fields)
  }
  const timeoutControls = (
    <details className="sect">
      <summary>Lock timing</summary>
      <label>Lock after inactivity (seconds; 0 disables)<input className="field" type="number" min="0" max="604800" required value={idle} onChange={(event) => setIdle(event.target.value)} /></label>
      <label>Maximum unlock duration (seconds; 0 disables)<input className="field" type="number" min="0" max="604800" required value={maximum} onChange={(event) => setMaximum(event.target.value)} /></label>
    </details>
  )
  const passwordControls = (
    <>
      <label>Passphrase<input className="field" type="password" maxLength={4096} autoComplete="new-password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} /></label>
      <label>Confirm passphrase<input className="field" type="password" maxLength={4096} autoComplete="new-password" value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
    </>
  )

  if (client.transport.channel) return <p>Open this project on its computer to manage local encryption.</p>
  if (!status) return <><p>Checking project protection…</p><Failure error={error} /><button className="tb" type="button" onClick={() => void refetch()}>Try again</button></>
  const unlocked = status.state === "unlocked"
  return (
    <div className="stack" data-vault-state={status.state}>
      <p>{!status.encrypted ? "Encryption is off. This project uses ordinary local storage." : unlocked ? "This project is unlocked." : status.state === "locking" ? "Locking: another application is still using a file. Close it and try again." : "This project is locked."}</p>
      <Failure error={ask.error} plain />
      {sourceRemoved && !error && status.encrypted && unlocked && <p>The verified unencrypted source was removed.</p>}
      {preserved && <p className="warn">An unencrypted copy was preserved at {preserved}. That copy is not protected.</p>}
      {ended.length > 0 && <p className="muted">Locking ended the project's runs {ended.join(", ")}.</p>}
      {skipped.length > 0 && !error && (!status.encrypted || unlocked) && <div className="warn">
        <p>The operation finished, but these missing files were skipped:</p>
        <ul>{skipped.map((file, at) => <li key={`${at}:${file}`}><code>{file}</code></li>)}</ul>
      </div>}
      {newRecovery && status.encrypted && unlocked && <Recovery secret={newRecovery} onDone={() => setNewRecovery(null)} />}
      {!status.encrypted ? (
        <form className="stack" onSubmit={(event) => submit(event, "encrypt")}>
          <p className="warn">Encryption removes this project's verified unencrypted source after conversion succeeds. Earlier independent plaintext copies, backups, and shared cache copies aren't erased; protect or remove them separately.</p>
          {passwordControls}
          {timeoutControls}
          <label><input type="checkbox" checked={preservePlaintext} onChange={(event) => setPreservePlaintext(event.target.checked)} /> Keep an unencrypted copy after encryption</label>
          <p className="muted">By default, the verified original is deleted after encryption succeeds. Select this only if you need an additional unencrypted copy. Project state and history are preserved.</p>
          <button className="tb primary" type="submit" disabled={ask.busy || !passphrase || passphrase !== confirmation || pending.count > 0}>Enable encryption</button>
        </form>
      ) : status.state === "locking" ? (
        <button className="tb" type="button" disabled={ask.busy} onClick={() => void change("lock")}>Try lock again</button>
      ) : !unlocked ? (
        <form className="stack" onSubmit={(event) => submit(event, "unlock")}>
          <label>Unlock with<select className="field" value={credential} onChange={(event) => { setCredential(event.target.value as "passphrase" | "recovery"); setPassphrase(""); setRecovery("") }}><option value="passphrase">Passphrase</option><option value="recovery">Saved recovery secret</option></select></label>
          {credential === "passphrase"
            ? <label>Passphrase<input className="field" type="password" maxLength={4096} autoComplete="current-password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} /></label>
            : <label>Recovery secret<input className="field" type="password" maxLength={4096} autoComplete="off" spellCheck={false} value={recovery} onChange={(event) => setRecovery(event.target.value)} /></label>}
          {timeoutControls}
          <button className="tb primary" type="submit" disabled={ask.busy || !(credential === "recovery" ? recovery : passphrase)}>Unlock project</button>
        </form>
      ) : (
        <>
          {status.view && <p>Readable folder: <code>{status.view}</code></p>}
          {pending.count > 0 && <p className="muted">Wait for pending edits and uploads to finish before locking.</p>}
          <button className="tb" type="button" disabled={ask.busy || pending.count > 0} onClick={() => void change("lock")}>Lock project</button>
          <details className="sect"><summary>Change passphrase</summary><form className="stack" onSubmit={(event) => submit(event, "passphrase")}>{passwordControls}<button className="tb" type="submit" disabled={ask.busy || !passphrase || passphrase !== confirmation}>Save passphrase</button></form></details>
          <details className="sect"><summary>Replace recovery secret</summary><p>The old secret may still open previously copied keybags.</p><button className="tb" type="button" disabled={ask.busy} onClick={() => void change("recovery")}>Create replacement recovery secret</button></details>
          <details className="sect"><summary>Decrypt project</summary><p>Convert this project folder to ordinary, unencrypted storage. Its files and history stay in the same project. Close files and shells using the readable folder first.</p><button className="tb" type="button" disabled={ask.busy || pending.count > 0} onClick={() => void change("decrypt")}>Decrypt project</button></details>
          <details className="sect"><summary>Rotate project key</summary><p>Re-encrypt all retained files and history. This can take time and temporary disk space.</p><form className="stack" onSubmit={(event) => submit(event, "rotate")}>{passwordControls}<button className="tb" type="submit" disabled={ask.busy || !passphrase || passphrase !== confirmation || pending.count > 0}>Re-encrypt project</button></form></details>
        </>
      )}
      {ask.busy && <p role="status">{pendingLine(operation)}</p>}
    </div>
  )
}

export function ProjectVault() {
  const id = useProjectId()
  return <DrawerFrame title="Project encryption">{id ? <VaultBody key={id} id={id} /> : <p>Open a project first.</p>}</DrawerFrame>
}

export function ProjectLocked({ id }: { id: string }) {
  return <main className="gate"><section style={{ maxWidth: 520, width: "100%", textAlign: "left" }}><h1>Project protection</h1><VaultBody key={id} id={id} /></section></main>
}
