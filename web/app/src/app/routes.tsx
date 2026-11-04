import { useQuery } from "@tanstack/react-query"
import { createHashRouter, Navigate, type RouteObject } from "react-router-dom"
import { projectsQuery } from "../api/queries"
import { Projects } from "../panels/Projects"
import { Stub } from "../panels/Stub"
import { Yaml } from "../panels/Yaml"
import { useConnection } from "./Connection"
import { Shell } from "./Shell"

/** `#/`: the last project used, or the empty canvas it already shows. */
function Home() {
  const { client } = useConnection()
  const { data } = useQuery(projectsQuery(client))
  const last = data?.projects.find((project) => !project.missing)
  return last ? <Navigate replace to={`/projects/${last.id}`} /> : null
}

const drawer = (path: string, element: React.ReactNode, kind: "normal" | "wide" = "normal"): RouteObject => ({
  path,
  element,
  handle: { drawer: kind },
})

const drawers: RouteObject[] = [
  drawer("runs", <Stub title="Runs" />),
  drawer("runs/:number", <Stub title="Run" />),
  drawer("machines", <Stub title="Machines" />),
  drawer("machines/:name", <Stub title="Machine" />),
  drawer("packages", <Stub title="Packages" />, "wide"),
  drawer("packages/:name", <Stub title="Package" />, "wide"),
  drawer("models", <Stub title="Models" />, "wide"),
  drawer("models/:name", <Stub title="Model" />, "wide"),
  drawer("account", <Stub title="Account" />),
  drawer("projects", <Projects />),
]

export const router = createHashRouter([
  {
    path: "/",
    element: <Shell />,
    children: [
      { index: true, element: <Home /> },
      ...drawers,
      {
        path: "projects/:id",
        children: [
          { index: true, element: null },
          ...drawers,
          drawer("new", <Stub title="New run" />),
          drawer("files", <Stub title="Files" />),
          drawer("yaml", <Yaml />),
        ],
      },
      { path: "*", element: <Navigate replace to="/" /> },
    ],
  },
])
