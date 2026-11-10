import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useState } from "react"
import QRCode from "react-qr-code"
import type { Client, VaultAsk } from "../api/client"
import { useConnection } from "../app/Connection"
import { useProjectId } from "../app/Shell"
import { useSignal } from "../ui/signal"
import { DrawerFrame } from "./Drawer"
import { Failure, useAsk } from "./common"

export const vaultKey = (id: string) => ["project-vault", id] as const
export const vaultQuery = (client: Client, id: string) => queryOptions({
  queryKey: vaultKey(id),
  queryFn: ({ signal }) => client.vault(id, signal),
  staleTime: 0,
  refetchInterval: 1000,
  refetchIntervalInBackground: true,
})

function Recovery({ secret, onDone }: { secret: string; onDone: () => void }) {
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
      <button className="tb primary" type="button" disabled={!saved} onClick={onDone}>Done</button>
    </section>
  )
}

function VaultBody({ id }: { id: string }) {
  const { client, outbox } = useConnection()
  const queries = useQueryClient()
  const { data: status, error, refetch } = useQuery(vaultQuery(client, id))
  const pending = useSignal(outbox.owed)
  const ask = useAsk()
  const [passphrase, setPassphrase] = useState("")
  const [confirmation, setConfirmation] = useState("")
  const [recovery, setRecovery] = useState("")
  const [credential, setCredential] = useState<"passphrase" | "recovery">("passphrase")
  const [newRecovery, setNewRecovery] = useState<string | null>(null)
  const [preserved, setPreserved] = useState<string | null>(null)
  const [removePlaintext, setRemovePlaintext] = useState(false)
  const [idle, setIdle] = useState("900")
  const [maximum, setMaximum] = useState("0")
  const [output, setOutput] = useState("")

  useEffect(() => {
    if (status?.encrypted && status.state !== "unlocked") {
      setPassphrase(""); setConfirmation(""); setRecovery(""); setNewRecovery(null)
    }
  }, [status?.encrypted, status?.state])

  const change = (action: string, fields: Partial<VaultAsk> = {}) => ask.run(async () => {
    setPassphrase(""); setConfirmation(""); setRecovery("")
    const answer = await client.vaultChange(id, { action, ...fields })
    queries.setQueryData(vaultKey(id), answer.vault)
    if (answer.recovery_secret) setNewRecovery(answer.recovery_secret)
    if (answer.preserved_source) setPreserved(answer.preserved_source)
    delete answer.recovery_secret
    await queries.invalidateQueries({ queryKey: ["projects"] })
    return answer.vault
  })
  const timeout = () => ({ idle_timeout_s: Number(idle), max_duration_s: Number(maximum) })
  const submit = (event: FormEvent, action: string) => {
    event.preventDefault()
    const fields = action === "unlock"
      ? credential === "recovery" ? { recovery_secret: recovery, ...timeout() } : { passphrase, ...timeout() }
      : { passphrase, remove_plaintext: removePlaintext, ...timeout() }
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
      {preserved && <p className="warn">An unencrypted copy was preserved at {preserved}. That copy is not protected.</p>}
      {newRecovery && <Recovery secret={newRecovery} onDone={() => setNewRecovery(null)} />}
      {!status.encrypted ? (
        <form className="stack" onSubmit={(event) => submit(event, "encrypt")}>
          <p className="warn">Encryption protects this project's folder and future work. Earlier exports, backups, and shared cache copies aren't erased; protect or remove them separately. For a new project, enable encryption before creating or importing files.</p>
          {passwordControls}
          {timeoutControls}
          <label><input type="checkbox" checked={removePlaintext} onChange={(event) => setRemovePlaintext(event.target.checked)} /> Remove the verified unencrypted source after conversion</label>
          <p className="muted">If unchecked, the original copy stays unencrypted. Enabling encryption preserves the project state and history.</p>
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
          <details className="sect"><summary>Export an unencrypted copy</summary><form className="stack" onSubmit={(event) => { event.preventDefault(); void change("export", { output }) }}><label>New folder on this computer<input className="field" value={output} onChange={(event) => setOutput(event.target.value)} /></label><p>This deliberately creates an ordinary readable copy. The encrypted source remains.</p><button className="tb" type="submit" disabled={ask.busy || !output}>Export unencrypted copy</button></form></details>
          <details className="sect"><summary>Rotate project key</summary><p>Re-encrypt all retained files and history. This can take time and temporary disk space.</p><form className="stack" onSubmit={(event) => submit(event, "rotate")}>{passwordControls}<button className="tb" type="submit" disabled={ask.busy || !passphrase || passphrase !== confirmation || pending.count > 0}>Re-encrypt project</button></form></details>
        </>
      )}
      {ask.busy && <p role="status">Working…</p>}
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
