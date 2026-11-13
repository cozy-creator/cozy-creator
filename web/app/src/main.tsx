import "@openrails/auth-ui/styles.css"
import "@xyflow/react/dist/base.css"
import "@cozy-creator/paper/paper.css"
import "./ui/tokens.css"
import "./ui/app.css"
import "./ui/paper.css"
import "./ui/theme"
import { QueryClientProvider } from "@tanstack/react-query"
import { StrictMode, useEffect, useState } from "react"
import { createRoot } from "react-dom/client"
import { RouterProvider } from "react-router-dom"
import { type Boot, type Connection, ConnectionProvider, connect } from "./app/Connection"
import { theRouter } from "./app/routes"
import { takeHashRoute } from "./app/session"
import { Copyable } from "./ui/Copyable"
import { tabShown, useSignal } from "./ui/signal"

/** Asks Calcifer who it is. Runs once, outside React. */
function boot(): Promise<Boot> {
  takeHashRoute()
  return connect()
}

const booted = boot()

/** Calcifer won't answer this page: it isn't Calcifer's own, or not its user's. One state, drawn where the page was, and nothing of a
 *  project in it (cl-980). */
function NoSession() {
  return (
    <main className="gate">
      <h1>No project open here</h1>
      <p>Open Cozy at its own address, as the user it runs as: in a terminal, run <code>cozy open</code> in a project folder, or <code>cozy new &lt;folder&gt;</code> to start one.</p>
    </main>
  )
}

/** Asked again every few seconds while the tab is in view: the page opens by itself once Calcifer answers. */
function Unreachable() {
  return (
    <main className="gate">
      <h1>Calcifer isn't answering</h1>
      <p>This opens as soon as it does. If it doesn't come back, run this in a terminal:</p>
      <Copyable text="cozy open" />
    </main>
  )
}

function Connected({ connection }: { connection: Connection }) {
  const signedOut = useSignal(connection.signedOut)
  const health = useSignal(connection.stream.health)
  if (signedOut || health.state === "signed_out") return <NoSession />
  return (
    <ConnectionProvider value={connection}>
      <QueryClientProvider client={connection.queries}>
        <RouterProvider router={theRouter()} />
      </QueryClientProvider>
    </ConnectionProvider>
  )
}

const ASK_AGAIN_MS = 2000

function App() {
  const [state, setState] = useState<Boot | null>(null)
  useEffect(() => {
    void booted.then(setState)
  }, [])
  const inView = useSignal(tabShown)
  useEffect(() => {
    if (!state || !("failed" in state) || state.failed !== "unreachable" || !inView) return
    const again = setTimeout(() => void connect().then(setState), ASK_AGAIN_MS)
    return () => clearTimeout(again)
  }, [state, inView])
  if (!state) return <main className="gate">Connecting…</main>
  if ("connection" in state) return <Connected connection={state.connection} />
  return state.failed === "signed_out" ? <NoSession /> : <Unreachable />
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
