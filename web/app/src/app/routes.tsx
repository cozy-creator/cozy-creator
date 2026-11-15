import { createBrowserRouter, Navigate, type RouteObject } from "react-router-dom"
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
import { guardRedirects } from "./redirect"
import { Shell } from "./Shell"
import { ProjectRoute } from "./project"

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

/** Made once boot has taken a link's code out of the URL: a router reads where it is as it's made, and `to` is where the link goes. */
let router: ReturnType<typeof createBrowserRouter> | null = null

export const theRouter = () =>
  (router ??= guardRedirects(createBrowserRouter([
    {
      path: "/",
      element: <ProjectRoute><Shell /></ProjectRoute>,
      errorElement: <Broke />,
      children: [
        // The empty canvas: `/` opens no project by itself (cl-980).
        { index: true, element: null },
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
            drawer("vault", <ProjectVault />),
          ],
        })),
        { path: "*", element: <Navigate replace to="/" /> },
      ],
    },
  ])))
