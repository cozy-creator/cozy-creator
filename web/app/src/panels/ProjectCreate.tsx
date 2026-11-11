import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useCallback, useEffect, useRef, useState } from "react"
import type { Opened } from "../api/calcifer"
import { useConnection } from "../app/Connection"
import { Failure } from "./common"
import { Recovery, vaultKey, vaultQuery } from "./ProjectVault"
import { useSignal } from "../ui/signal"

/** The same local secret/recovery setup for explicit New and an empty canvas's first import. */
export function ProjectCreate({ onCreated, onCancel, modal = false, autoFocus = false }: {
  onCreated(project: Opened): void
  onCancel?(): void
  modal?: boolean
  autoFocus?: boolean
}) {
  const { client, bridge, stream } = useConnection()
  const queries = useQueryClient()
  const health = useSignal(stream.health)
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
  const authority = useRef<{ id: string; epoch: number } | null>(null)
  const protection = useQuery({ ...vaultQuery(client, created?.id ?? ""), enabled: Boolean(created) })
  const revoke = useCallback(() => {
    authority.current = null
    if (!mounted.current) return
    setRecovery(null); setCreated(null); setPassphrase(""); setConfirmation("")
    setError(new Error("Project access changed. Unlock the created project and replace/save its recovery secret before adding private files."))
  }, [])
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; authority.current = null } }, [])
  useEffect(() => stream.subscribe((event) => {
    const held = authority.current
    if (!held) return
    if (event.type === "hello" || event.type === "resync" || (event.type === "project.vault" && event.id === held.id && event.state !== "unlocked")) revoke()
  }), [stream, revoke])
  useEffect(() => {
    if (authority.current && health.state !== "live") revoke()
  }, [health, revoke])
  useEffect(() => {
    if (created && (!protection.data?.encrypted || protection.data.state !== "unlocked" || protection.isError || bridge.projectEpoch(created.id) !== authority.current?.epoch)) revoke()
  }, [created, protection.data, protection.isError, bridge, revoke])
  useEffect(() => {
    if (modal) dialog.current?.showModal()
    if (autoFocus || modal) naming.current?.focus()
    return () => { dialog.current?.close() }
  }, [modal, autoFocus])

  const verify = async (held: { id: string; epoch: number }) => {
    const status = await client.vault(held.id)
    if (!mounted.current || authority.current !== held) return false
    if (!status.encrypted || status.state !== "unlocked" || bridge.projectEpoch(held.id) !== held.epoch || stream.health.get().state !== "live") {
      revoke()
      return false
    }
    queries.setQueryData(vaultKey(held.id), status)
    return true
  }
  const finish = async () => {
    const held = authority.current
    if (!created || !held || held.id !== created.id) return
    setBusy(true)
    try {
      if (await verify(held)) {
        authority.current = null
        setRecovery(null)
        onCreated(created)
      }
    } catch { revoke() }
    finally { if (mounted.current) setBusy(false) }
  }

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
      else if (recovery_secret) {
        const held = { id: project.id, epoch: bridge.projectEpoch(project.id) }
        authority.current = held
        void queries.invalidateQueries({ queryKey: ["projects"] })
        if (await verify(held)) { setCreated(project); setRecovery(recovery_secret) }
      }
      else throw new Error("Calcifer did not confirm encrypted creation. Update or restart it, then inspect the new project's protection before adding private files.")
    } catch (failure) {
      authority.current = null
      if (mounted.current) setError(failure)
    } finally {
      if (mounted.current) setBusy(false)
    }
  }
  const content = <div className="stack">
    {modal && <h2>New project</h2>}
    <Failure error={error} plain />
    {recovery && created ? protection.data?.encrypted && protection.data.state === "unlocked" && !protection.isError && health.state === "live" && bridge.projectEpoch(created.id) === authority.current?.epoch
      ? <Recovery secret={recovery} disabled={busy} onDone={() => void finish()} /> : <p>Checking project protection…</p>
      : <form className="stack" onSubmit={(event) => void submit(event)}>
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
  return modal ? <dialog ref={dialog} className="project-setup" aria-label="New project" onCancel={(event) => { event.preventDefault(); if (!busy && !created) onCancel?.() }}>{content}</dialog> : content
}
