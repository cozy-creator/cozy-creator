import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query"
import { createContext, type ReactNode, useCallback, useContext, useLayoutEffect, useRef } from "react"
import { useLocation, useMatch, useNavigate } from "react-router-dom"
import { ApiError, type Client } from "../api/client"
import type { ProjectAddress } from "../api/calcifer"
import { noHash } from "../canvas/text"
import { Copyable } from "../ui/Copyable"
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

/** Browser addresses use the short code. Older records can still be opened by their id. */
export function projectPath(project: Pick<ProjectAddress, "id" | "code">, rest = ""): string {
  const base = project.code ? `/p/${project.code}` : `/projects/${encodeURIComponent(project.id)}`
  return `${base}${rest && `/${rest.replace(/^\//, "")}`}`
}

export function useProjectId(): string | null {
  return useContext(CurrentProject)?.id ?? null
}

export function useTo() {
  const project = useContext(CurrentProject)
  const id = project?.id
  const code = project?.code ?? ""
  return useCallback((rest = "") => id ? projectPath({ id, code }, rest) : `/${rest}`, [id, code])
}

/** Resolve another project's address without putting its alias in API or event cache keys. */
export function useProjectHref(id: string | null | undefined, rest = ""): string | null {
  const current = useContext(CurrentProject)
  const { client } = useConnection()
  const { data } = useQuery({ ...referenceQuery(client, id ?? ""), enabled: Boolean(id && id !== current?.id) })
  const project = id === current?.id ? current : data
  return project ? projectPath(project, rest) : null
}

/** Resolve URL codes once, then expose only the canonical project id to the mounted app. */
export function ProjectRoute({ children }: { children: ReactNode }) {
  const compact = useMatch("/p/:code/*")
  const legacy = useMatch("/projects/:id/*")
  const reference = compact?.params.code ?? legacy?.params.id ?? ""
  const { client } = useConnection()
  const queries = useQueryClient()
  const location = useLocation()
  const navigate = useNavigate()
  const { data, error } = useQuery({ ...referenceQuery(client, reference), enabled: Boolean(reference) })

  useLayoutEffect(() => {
    if (!data || !reference) return
    queries.setQueryData(referenceKey(data.id), data)
    if (data.code) queries.setQueryData(referenceKey(data.code), data)
    const suffix = location.pathname.replace(/^\/(?:p|projects)\/[^/]+\/?/, "")
    const canonical = `${projectPath(data, suffix)}${location.search}`
    if (`${location.pathname}${location.search}` !== canonical) navigate(canonical, { replace: true })
  }, [data, reference, location.pathname, location.search, navigate, queries])

  // While the next one's address is read, the page shown stays as it is: the frame never goes, and the screen never goes blank (cl-980).
  // Not an encrypted project's: that's covered at once.
  const shown = useRef<{ project: ProjectAddress | null } | null>(null)
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

/** Why a project can't be opened, and what to do, in its words: in place of it, wherever the page is (cl-980). */
export function Unopened({ error }: { error: unknown }) {
  const body = error instanceof ApiError ? error.body : undefined
  const message = error instanceof Error ? error.message : String(error)
  // Its folder gone, that's the whole story: "This project's folder is gone: <path>".
  const gone = body?.code === "project_folder_gone"
  return (
    <div>
      <h2>{gone ? sentence(message) : "This project can't be opened yet"}</h2>
      {!gone && <p>{noHash(sentence(message))}</p>}
      {body?.remedy && <p>{noHash(sentence(body.remedy))}</p>}
      {body?.next?.map((line) => <Copyable key={line} text={line} />)}
    </div>
  )
}
