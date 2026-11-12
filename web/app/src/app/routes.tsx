import { useQuery } from "@tanstack/react-query"
import { createBrowserRouter, Navigate, type RouteObject } from "react-router-dom"
import { projectsQuery } from "../api/queries"
import { Account } from "../panels/Account"
import { Files } from "../panels/Files"
import { ModelDrawer, Models } from "../panels/library/Models"
import { PackageDrawer, Packages } from "../panels/library/Packages"
import { MachineDrawer } from "../panels/machines/MachineDrawer"
import { Machines } from "../panels/machines/Machines"
import { NewRun } from "../panels/newrun/NewRun"
import { Projects } from "../panels/Projects"
import { ProjectVault } from "../panels/ProjectVault"
import { RunDrawer } from "../panels/runs/RunDrawer"
import { Runs } from "../panels/runs/Runs"
import { Yaml } from "../panels/Yaml"
import { useConnection } from "./Connection"
import { Shell } from "./Shell"
import { ProjectRoute, projectPath } from "./project"

/** The last project used, or the empty canvas it already shows. */
function Home() {
  const { client } = useConnection()
  const { data } = useQuery(projectsQuery(client))
  const last = data?.projects[0]
  return last ? <Navigate replace to={projectPath(last)} /> : null
}

/** Whatever threw, a person gets a line and a way back, and the file holds what was saved. */
function Broke() {
  return (
    <main className="gate">
      <h1>That broke this page</h1>
      <button type="button" className="tb" onClick={() => location.reload()}>
        Reload
      </button>
    </main>
  )
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
  drawer("packages", <Packages />, "wide"),
  drawer("packages/:name", <PackageDrawer />, "wide"),
  drawer("models", <Models />, "wide"),
  drawer("models/:name", <ModelDrawer />, "wide"),
  drawer("account", <Account />),
  drawer("projects", <Projects />),
]

/** Made once boot has taken a sign-in link out of the URL: a router reads where it is as it's made, and `to` is where the link goes. */
let router: ReturnType<typeof createBrowserRouter> | null = null

export const theRouter = () =>
  (router ??= createBrowserRouter([
    {
      path: "/",
      element: <ProjectRoute><Shell /></ProjectRoute>,
      errorElement: <Broke />,
      children: [
        { index: true, element: <Home /> },
        ...drawers,
        ...["p/:code", "projects/:id"].map((path) => ({
          path,
          children: [
            { index: true, element: null },
            // A box opened: drawn over the canvas, by the canvas, which is always mounted.
            { path: "assets/:name", element: null },
            ...drawers,
            drawer("new", <NewRun />),
            drawer("files", <Files />),
            drawer("yaml", <Yaml />),
            drawer("vault", <ProjectVault />),
          ],
        })),
        { path: "*", element: <Navigate replace to="/" /> },
      ],
    },
  ]))
