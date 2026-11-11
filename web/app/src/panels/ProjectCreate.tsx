import { type FormEvent, useEffect, useRef, useState } from "react"
import type { Opened } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import { Failure } from "./common"
import { Recovery } from "./ProjectVault"

/** The same local secret/recovery setup for explicit New and an empty canvas's first import. */
export function ProjectCreate({ onCreated, onCancel, modal = false, autoFocus = false }: {
  onCreated(project: Opened): void
  onCancel?(): void
  modal?: boolean
  autoFocus?: boolean
}) {
  const { client } = useConnection()
  const [name, setName] = useState("")
  const [unencrypted, setUnencrypted] = useState(false)
  const [passphrase, setPassphrase] = useState("")
  const [confirmation, setConfirmation] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>()
  const [created, setCreated] = useState<Opened | null>(null)
  const [recovery, setRecovery] = useState<string | null>(null)
  const naming = useRef<HTMLInputElement>(null)
  const dialog = useRef<HTMLDialogElement>(null)
  const mounted = useRef(false)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => {
    if (modal) dialog.current?.showModal()
    if (autoFocus || modal) naming.current?.focus()
    return () => { dialog.current?.close() }
  }, [modal, autoFocus])

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (busy || (!unencrypted && (!passphrase || passphrase !== confirmation))) return
    const body = { name: name.trim() || "untitled", unencrypted, ...(!unencrypted ? { passphrase } : {}) }
    setBusy(true); setError(undefined); setPassphrase(""); setConfirmation("")
    try {
      const answer = await client.newProject(body)
      const { recovery_secret, ...project } = answer
      delete answer.recovery_secret
      if (!mounted.current) return
      if (unencrypted) onCreated(project)
      else if (recovery_secret) { setCreated(project); setRecovery(recovery_secret) }
      else throw new Error("The project was created. Open its encryption settings to replace and save its recovery secret.")
    } catch (failure) {
      if (mounted.current) setError(failure)
    } finally {
      if (mounted.current) setBusy(false)
    }
  }
  const content = <div className="stack">
    {modal && <h2>New project</h2>}
    <Failure error={error} plain />
    {recovery && created ? <Recovery secret={recovery} onDone={() => { setRecovery(null); onCreated(created) }} /> : <form className="stack" onSubmit={(event) => void submit(event)}>
      <label>Name<input ref={naming} className="field" aria-label="New project's name" placeholder="New project's name" spellCheck={false} value={name} onChange={(event) => setName(event.target.value)} /></label>
      <p>New projects are encrypted on this computer. Choose a passphrase and save the recovery secret away from this device.</p>
      <label><input type="checkbox" checked={unencrypted} disabled={busy} onChange={(event) => { setUnencrypted(event.target.checked); setPassphrase(""); setConfirmation("") }} /> Create without encryption</label>
      {!unencrypted && <>
        <label>Passphrase<input className="field" type="password" autoComplete="new-password" maxLength={4096} value={passphrase} onChange={(event) => setPassphrase(event.target.value)} disabled={busy} /></label>
        <label>Confirm passphrase<input className="field" type="password" autoComplete="new-password" maxLength={4096} value={confirmation} onChange={(event) => setConfirmation(event.target.value)} disabled={busy} /></label>
      </>}
      <div className="row">
        <button type="submit" className="tb primary" disabled={busy || (!unencrypted && (!passphrase || passphrase !== confirmation))}>{busy ? "Creating…" : "Create project"}</button>
        {onCancel && <button type="button" className="tb" disabled={busy} onClick={onCancel}>Cancel</button>}
      </div>
    </form>}
  </div>
  return modal ? <dialog ref={dialog} className="project-setup" onCancel={(event) => { event.preventDefault(); if (!busy && !created) onCancel?.() }}>{content}</dialog> : content
}
