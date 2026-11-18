import { type QueryClient, skipToken, useQuery, useQueryClient } from "@tanstack/react-query"
import { createContext, type ReactNode, useCallback, useContext, useEffect, useLayoutEffect, useRef } from "react"
import { useLocation, useMatch, useNavigate } from "react-router-dom"
import { ApiError, type Client } from "../api/client"
import type { ProjectAddress } from "../api/calcifer"
import { noHash } from "../canvas/text"
import { useConnection } from "./Connection"
import { privateProject } from "./view"

const CurrentProject = createContext<ProjectAddress | null>(null)
const referenceKey = (reference: string) => ["project-reference", reference]
const referenceQuery = (client: Client, reference: string) => ({
  queryKey: referenceKey(reference),
  // The first content read retains any one-time upgrade warnings.
  queryFn: () => client.projectAddress(reference),
  staleTime: Infinity,
})

/** Where a project was whose folder is gone, as the stream said it (`project.gone`): never read, only kept. */
export const goneKey = (id: string) => ["project-gone", id]

/** Where project `id` was, once its folder is gone; null while it's there (cl-980). */
export function useGone(id: string | null): string | null {
  const { data } = useQuery<string>({ queryKey: goneKey(id ?? ""), queryFn: skipToken })
  return id ? data ?? null : null
}

/** A project's page: `/p/<code>`, its only address (cl-1171). */
export function projectPath(project: Pick<ProjectAddress, "code">, rest = ""): string {
  return `/p/${project.code}${rest && `/${rest.replace(/^\//, "")}`}`
}

/** A node open in Focus, under its project's page: `nodes/<name>` (cl-1209). */
export const nodePath = (name: string) => `nodes/${encodeURIComponent(name)}`

export function useProjectId(): string | null {
  return useContext(CurrentProject)?.id ?? null
}

export function useTo() {
  const project = useContext(CurrentProject)
  const id = project?.id
  const code = project?.code ?? ""
  return useCallback((rest = "") => id ? projectPath({ code }, rest) : `/${rest}`, [id, code])
}

/** Resolve another project's address without putting its alias in API or event cache keys. */
export function useProjectHref(id: string | null | undefined, rest = ""): string | null {
  const current = useContext(CurrentProject)
  const { client } = useConnection()
  const { data } = useQuery({ ...referenceQuery(client, id ?? ""), enabled: Boolean(id && id !== current?.id) })
  const project = id === current?.id ? current : data
  return project ? projectPath(project, rest) : null
}

/** A code as a link may spell it (ContentKit's spellings: any case, dashes, O for 0, I or L for 1), as Calcifer keeps it. */
const spelled = (code: string) => code.toUpperCase().replace(/-/g, "").replace(/O/g, "0").replace(/[IL]/g, "1")

/** Resolve URL codes once, then expose only the canonical project id to the mounted app. */
export function ProjectRoute({ children }: { children: ReactNode }) {
  const reference = useMatch("/p/:code/*")?.params.code ?? ""
  const { client, stream } = useConnection()
  const queries = useQueryClient()
  const location = useLocation()
  const navigate = useNavigate()
  const { data, error, refetch } = useQuery({ ...referenceQuery(client, reference), enabled: Boolean(reference) })
  // A read that failed is asked again once Calcifer is back, never left standing until a reload.
  useEffect(() => (error ? stream.subscribe((event) => { if (event.type === "hello") void refetch() }) : undefined), [error, stream, refetch])
  // `/p/` takes a project's code alone: its id or its name there is no address (cl-1171).
  const notCode = Boolean(reference && data && !data.gone && data.code !== spelled(reference))

  useLayoutEffect(() => {
    if (!data || !reference || notCode) return
    queries.setQueryData(referenceKey(data.id), data)
    if (data.code) queries.setQueryData(referenceKey(data.code), data)
    const suffix = location.pathname.replace(/^\/p\/[^/]+\/?/, "")
    const canonical = `${projectPath(data, suffix)}${location.search}`
    if (`${location.pathname}${location.search}` !== canonical) navigate(canonical, { replace: true })
  }, [data, reference, notCode, location.pathname, location.search, navigate, queries])

  // While the next one's address is read, the page shown stays as it is: the frame never goes, and the screen never goes blank (cl-980).
  // Not an encrypted project's: that's covered at once.
  const shown = useRef<{ project: ProjectAddress | null } | null>(null)
  if (reference && data?.gone) return <main className="gate"><Unopened gone={data.gone} /></main>
  if (notCode) return <main className="gate"><Unopened error={new Error("There's no project with this code on this computer.")} /></main>
  if (reference && error && !data) return <main className="gate"><Unopened error={error} /></main>
  if (reference && !data && (!shown.current || privateProject(shown.current.project?.id ?? null))) return <main className="gate">Opening project…</main>
  const project = !reference ? null : data ?? shown.current?.project ?? null
  shown.current = { project }
  return <CurrentProject.Provider value={project}>{children}</CurrentProject.Provider>
}

/** A newly created project can be navigated to before the project list refetches. */
export function rememberProject(queries: QueryClient, project: Pick<ProjectAddress, "id" | "code">) {
  queries.setQueryData(referenceKey(project.id), project)
  if (project.code) queries.setQueryData(referenceKey(project.code), project)
}

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

/** Why a project can't be opened, and what to do, in its words: in place of it, wherever the page is (cl-980). Its folder `gone`, where it
 *  was, as Calcifer said it without refusing anything. */
export function Unopened({ error, gone }: { error?: unknown; gone?: string }) {
  if (gone) return (
    <div>
      <h2>This project's folder is gone: {gone}</h2>
      <p>Move the folder back, or open another project.</p>
    </div>
  )
  const body = error instanceof ApiError ? error.body : undefined
  const message = error instanceof Error ? error.message : String(error)
  // Its folder gone, that's the whole story: "This project's folder is gone: <path>".
  const folderGone = body?.code === "project_folder_gone"
  return (
    <div>
      <h2>{folderGone ? sentence(message) : "Couldn't open this project"}</h2>
      {!folderGone && <p>{noHash(sentence(message))}</p>}
      {body?.remedy && <p>{noHash(sentence(body.remedy))}</p>}
    </div>
  )
}
