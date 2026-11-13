import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useCallback, useEffect, useRef, useState } from "react"
import { ApiError } from "../api/client"
import type { Opened } from "../api/calcifer"
import { projectsQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { newFolder } from "../app/making"
import { Failure } from "./common"
import { Recovery, vaultKey, vaultQuery } from "./ProjectVault"
import { useSignal } from "../ui/signal"

/** What a folder refusal is: said beside the Folder field, in Calcifer's words, never in a dialog. */
const OF_FOLDER = new Set(["project_exists", "project_not_empty", "project_permission", "project_io", "project_path", "project_own_folder", "vault_nested_project"])

/** `<the most recently opened project's parent>/untitled-N`, the first N no project has; this person's home folder when there's none yet. */
export function defaultFolder(paths: string[], home: string): string {
  const last = paths[0]
  const near = last ?? home
  const sep = near.includes("\\") && !near.includes("/") ? "\\" : "/"
  const parent = (last ? last.slice(0, Math.max(last.lastIndexOf("/"), last.lastIndexOf("\\"))) : home).replace(/[\\/]+$/, "")
  const taken = new Set(paths)
  for (let n = 1; ; n++) {
    const folder = `${parent}${sep}untitled-${n}`
    if (!taken.has(folder)) return folder
  }
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)
const baseName = (folder: string) => folder.split(/[\\/]/).filter(Boolean).pop() ?? folder

/** A new project, made in place: its folder (suggested, edited where it is), its encryption, and its recovery secret shown once. The
 *  strip's +, the Projects drawer and a first drop on an empty canvas all make one here, as `cozy new` does: `POST /v1/projects {path}`. */
export function ProjectCreate({ onCreated, onCancel, onHolding, autoFocus = false }: {
  onCreated(project: Opened): void
  onCancel?(): void
  /** Whether it's making one, or showing its recovery secret: then nothing outside it may put it away. */
  onHolding?(holding: boolean): void
  autoFocus?: boolean
}) {
  const { client, bridge, stream, status } = useConnection()
  const queries = useQueryClient()
  const health = useSignal(stream.health)
  const { data: listed } = useQuery(projectsQuery(client))
  const suggested = listed ? defaultFolder(listed.projects.map((project) => project.path), status.home) : null
  const [typed, setTyped] = useState<string | null>(null)
  const folder = typed ?? suggested ?? ""
  const [unencrypted, setUnencrypted] = useState(false)
  const [passphrase, setPassphrase] = useState("")
  const [confirmation, setConfirmation] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<unknown>()
  const [created, setCreated] = useState<Opened | null>(null)
  const [recovery, setRecovery] = useState<string | null>(null)
  const field = useRef<HTMLInputElement>(null)
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
  useEffect(() => { newFolder.set(baseName(folder)) }, [folder])
  useEffect(() => () => newFolder.set(""), [])
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
    if (autoFocus) field.current?.focus()
  }, [autoFocus])
  useEffect(() => { onHolding?.(busy || Boolean(recovery)) }, [onHolding, busy, recovery])

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

  const mismatch = !unencrypted && confirmation !== "" && passphrase !== confirmation
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (busy || !folder.trim() || (!unencrypted && (!passphrase || passphrase !== confirmation))) return
    const body = { path: folder.trim(), unencrypted, ...(!unencrypted ? { passphrase } : {}) }
    setBusy(true); setError(undefined); setPassphrase(""); setConfirmation("")
    try {
      const answer = await client.newProject(body)
      const { recovery_secret, ...project } = answer
      delete answer.recovery_secret
      if (!mounted.current) return
      void queries.invalidateQueries({ queryKey: ["projects"] })
      if (unencrypted) onCreated(project)
      else if (recovery_secret) {
        const held = { id: project.id, epoch: bridge.projectEpoch(project.id) }
        authority.current = held
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
  const code = error instanceof ApiError ? error.body.code : undefined
  const ofFolder = Boolean(code && OF_FOLDER.has(code))
  const said = (failure: unknown) => sentence(failure instanceof Error ? failure.message : String(failure))

  if (recovery && created) {
    return protection.data?.encrypted && protection.data.state === "unlocked" && !protection.isError && health.state === "live" && bridge.projectEpoch(created.id) === authority.current?.epoch
      ? <Recovery secret={recovery} disabled={busy} onDone={() => void finish()} />
      : <p>Checking project protection…</p>
  }
  return (
    <form className="stack" onSubmit={(event) => void submit(event)}>
      <label className="said-beside">
        Folder
        <input ref={field} className="field wide" aria-label="New project's folder" spellCheck={false} value={folder} disabled={busy}
          onChange={(event) => { setTyped(event.target.value); if (ofFolder) setError(undefined) }} />
        {ofFolder && <span className="beside" role="alert">{said(error)}</span>}
      </label>
      <label><input type="checkbox" checked={unencrypted} disabled={busy} onChange={(event) => { setUnencrypted(event.target.checked); setPassphrase(""); setConfirmation("") }} /> Create without encryption</label>
      {!unencrypted && <>
        <label>Passphrase<input className="field" type="password" autoComplete="new-password" maxLength={4096} value={passphrase} onChange={(event) => setPassphrase(event.target.value)} disabled={busy} /></label>
        <label className="said-beside">
          Confirm passphrase
          <input className="field" type="password" autoComplete="new-password" maxLength={4096} value={confirmation} onChange={(event) => setConfirmation(event.target.value)} disabled={busy} />
          {mismatch && <span className="beside" role="alert">The passphrases differ.</span>}
        </label>
      </>}
      {!ofFolder && <Failure error={error} plain />}
      <div className="row">
        <button type="submit" className="tb primary" disabled={busy || !folder.trim() || (!unencrypted && (!passphrase || passphrase !== confirmation))}>{busy ? "Creating…" : "Create project"}</button>
        {onCancel && <button type="button" className="tb" disabled={busy} onClick={onCancel}>Cancel</button>}
      </div>
    </form>
  )
}
