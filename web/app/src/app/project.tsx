import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query"
import { createContext, type ReactNode, useCallback, useContext, useLayoutEffect } from "react"
import { Link, useLocation, useMatch, useNavigate } from "react-router-dom"
import type { Client } from "../api/client"
import type { ProjectState } from "../api/calcifer"
import { rememberProjectWarnings } from "../api/queries"
import { useConnection } from "./Connection"

type ProjectAddress = Pick<ProjectState, "id" | "code">

const CurrentProject = createContext<ProjectAddress | null>(null)
const referenceKey = (reference: string) => ["project-reference", reference]
const referenceQuery = (client: Client, reference: string) => ({
  queryKey: referenceKey(reference),
  queryFn: async ({ client: queries }: { client: QueryClient }) => {
    const head = await client.projectHead(reference)
    rememberProjectWarnings(queries, head)
    return { id: head.id, code: head.code }
  },
  staleTime: Infinity,
})

/** Browser addresses use the short code. Older records can still be opened by their id. */
export function projectPath(project: ProjectAddress, rest = ""): string {
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

  if (reference && error && !data) return <main className="gate">Couldn't open this project. <Link to="/projects">Projects</Link></main>
  if (reference && !data) return <main className="gate">Opening project…</main>
  return <CurrentProject.Provider value={reference ? data ?? null : null}>{children}</CurrentProject.Provider>
}

/** A newly created project can be navigated to before the project list refetches. */
export function rememberProject(queries: QueryClient, project: ProjectAddress) {
  queries.setQueryData(referenceKey(project.id), project)
  if (project.code) queries.setQueryData(referenceKey(project.code), project)
}
