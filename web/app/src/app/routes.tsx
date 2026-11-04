import { useQuery } from "@tanstack/react-query"
import { createHashRouter, Navigate, type RouteObject } from "react-router-dom"
import { projectsQuery } from "../api/queries"
import { MachineDrawer } from "../panels/machines/MachineDrawer"
import { Machines } from "../panels/machines/Machines"
import { NewRun } from "../panels/newrun/NewRun"
import { Projects } from "../panels/Projects"
import { RunDrawer } from "../panels/runs/RunDrawer"
import { Runs } from "../panels/runs/Runs"
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
  drawer("runs", <Runs />),
  drawer("runs/:number", <RunDrawer />),
  drawer("machines", <Machines />),
  drawer("machines/:name", <MachineDrawer />),
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
          drawer("new", <NewRun />),
          drawer("files", <Stub title="Files" />),
          drawer("yaml", <Yaml />),
        ],
      },
      { path: "*", element: <Navigate replace to="/" /> },
    ],
  },
])
