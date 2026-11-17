import { useQuery, useQueryClient } from "@tanstack/react-query"
import { type FormEvent, useEffect, useState } from "react"
import { useNavigate } from "react-router-dom"
import type { Appearance } from "../api/calcifer"
import { appearanceQuery, projectQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useProjectId } from "../app/project"
import { say } from "../ui/notes"
import { DrawerFrame } from "./Drawer"
import { vaultQuery, VaultBody } from "./ProjectVault"
import { AppearanceChoice } from "./Settings"
import { Confirm, Failure, Section, useAsk } from "./common"

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path

/** Its name, which is its folder's: renamed where it is (cl-1097). An unlocked encrypted project locks to move. */
function Name({ id, path, encryptedOpen }: { id: string; path: string; encryptedOpen: boolean }) {
  const { client } = useConnection()
  const queries = useQueryClient()
  const ask = useAsk()
  const was = folderName(path)
  const [name, setName] = useState(was)
  useEffect(() => setName(was), [was])
  const rename = (event: FormEvent) => {
    event.preventDefault()
    void ask.run(async () => {
      await client.renameProject(id, name.trim())
      await queries.invalidateQueries({ queryKey: ["projects"] })
      await queries.invalidateQueries({ queryKey: ["project-vault", id] })
    })
  }
  return (
    <Section title="Name">
      <form className="row" onSubmit={rename}>
        <input className="field" aria-label="Project name" spellCheck={false} maxLength={255} value={name} onChange={(event) => setName(event.target.value)} />
        <button className="tb" type="submit" disabled={ask.busy || !name.trim() || name.trim() === was}>Rename</button>
      </form>
      <p className="muted">{encryptedOpen ? "Its folder is renamed too, and the project locks. Unlock it again after." : "Its folder is renamed too."}</p>
      <Failure error={ask.error} plain />
    </Section>
  )
}

/** How the page looks with it open: as everywhere, or its own (cl-1097). Kept in the project, so it's read only while it's unlocked. */
function Look({ id, locked }: { id: string; locked: boolean }) {
  const { client, stream } = useConnection()
  const queries = useQueryClient()
  const { data: state } = useQuery({ ...projectQuery(client, stream, id), enabled: !locked })
  const { data: everywhere } = useQuery(appearanceQuery(client))
  if (locked) return <Section title="Appearance"><p className="muted">Unlock the project to change how it looks.</p></Section>
  if (!state) return <Section title="Appearance"><p className="muted">Loading…</p></Section>
  const own = state.appearance ?? null
  const keep = (appearance: Appearance | null) => {
    queries.setQueryData(["project", id], { ...state, appearance })
    client.setProjectAppearance(id, appearance).catch((error) => {
      void queries.invalidateQueries({ queryKey: ["project", id] })
      say("Couldn't save how this project looks", error)
    })
  }
  return (
    <Section title="Appearance">
      <div className="row choices" role="radiogroup" aria-label="This project's appearance">
        <button type="button" role="radio" aria-checked={!own} className={`tab ${own ? "" : "on"}`} onClick={() => own && keep(null)}>Same as everywhere</button>
        <button type="button" role="radio" aria-checked={Boolean(own)} className={`tab ${own ? "on" : ""}`} onClick={() => !own && keep(everywhere ?? { theme: "paper" })}>Its own</button>
      </div>
      {own && <AppearanceChoice label="This project's own appearance" value={own} onChange={keep} />}
    </Section>
  )
}

/** Its folder moved to the trash, whole, after an in-place confirm that names it (cl-1097). A locked one needs no passphrase. */
function Delete({ id, path }: { id: string; path: string }) {
  const { client, status } = useConnection()
  const queries = useQueryClient()
  const navigate = useNavigate()
  const ask = useAsk()
  const trash = status.os === "windows" ? "Recycle Bin" : "Trash"
  const remove = () => void ask.run(async () => {
    await client.deleteProject(id)
    await queries.invalidateQueries({ queryKey: ["projects"] })
    navigate("/")
    say(`Moved ${folderName(path)} to the ${trash}.`)
  })
  return (
    <Section title="Delete">
      <Confirm label="Delete project" disabled={ask.busy} says={<>Delete this project? Its folder <code>{path}</code> moves to the {trash}.</>} yes="Delete project" onYes={remove} />
      {ask.busy && <p role="status">Moving it to the {trash}…</p>}
      <Failure error={ask.error} plain />
    </Section>
  )
}

/** One project's settings, from the menu (Paul, 11-16): its name, how it looks, its encryption, and deleting it. Each works whether or
 *  not it's locked, except what's kept inside it, which says so. */
export function ProjectSettings() {
  const id = useProjectId()
  const { client } = useConnection()
  const { data: vault, error, refetch } = useQuery({ ...vaultQuery(client, id ?? ""), enabled: Boolean(id) })
  if (!id) return <DrawerFrame title="Project settings"><p>Open a project to see its settings.</p></DrawerFrame>
  if (!vault) return (
    <DrawerFrame title="Project settings">
      <p>Loading…</p>
      <Failure error={error} />
      {error ? <button className="tb" type="button" onClick={() => void refetch()}>Try again</button> : null}
    </DrawerFrame>
  )
  const locked = vault.encrypted && vault.state !== "unlocked"
  return (
    <DrawerFrame title="Project settings">
      <Name id={id} path={vault.path} encryptedOpen={vault.encrypted && !locked} />
      <Look id={id} locked={locked} />
      <Section title="Encryption"><VaultBody key={id} id={id} /></Section>
      <Delete id={id} path={vault.path} />
    </DrawerFrame>
  )
}
