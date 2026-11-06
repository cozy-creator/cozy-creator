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
import { Client } from "./api/client"
import { sameOrigin } from "./api/transport"
import { type Boot, type Connection, ConnectionProvider, chooseCalcifer, connect } from "./app/Connection"
import { theRouter } from "./app/routes"
import { saveSession, takeCode } from "./app/session"
import { Copyable } from "./ui/Copyable"
import { useSignal } from "./ui/signal"

/** Consumes a sign-in link, then asks Calcifer who it is. Runs once, outside React. */
async function boot(): Promise<Boot> {
  const code = takeCode()
  if (code) {
    const client = new Client(sameOrigin(() => null), { signedOut() {}, attempt() {} })
    try {
      saveSession(await client.redeem(code))
    } catch {
      // A used or expired link: a session this browser already holds still stands.
    }
  }
  return connect()
}

const booted = boot()

function SignedOut() {
  return (
    <main className="gate">
      <h1>This page isn't signed in</h1>
      <p>Run this in a terminal to open it signed in:</p>
      <Copyable text="cozy open" />
    </main>
  )
}

/** Asked again every few seconds: the page opens by itself once Calcifer answers. */
function Unreachable() {
  return (
    <main className="gate">
      <h1>Calcifer isn't answering</h1>
      <p>This opens as soon as it does. If it doesn't come back, run this in a terminal:</p>
      <Copyable text="cozy open" />
    </main>
  )
}

function RemoteDown({ machine }: { machine: string }) {
  return (
    <main className="gate">
      <h1>{machine}'s Calcifer isn't answering</h1>
      <div className="row">
        <button type="button" className="tb" onClick={() => location.reload()}>
          Try again
        </button>
        <button type="button" className="tb" onClick={() => chooseCalcifer(null)}>
          Back to this computer
        </button>
      </div>
    </main>
  )
}

function Connected({ connection }: { connection: Connection }) {
  const signedOut = useSignal(connection.signedOut)
  const health = useSignal(connection.stream.health)
  if (signedOut || health.state === "signed_out") return <SignedOut />
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
  useEffect(() => {
    if (!state || !("failed" in state) || state.failed !== "unreachable") return
    const again = setTimeout(() => void connect().then(setState), ASK_AGAIN_MS)
    return () => clearTimeout(again)
  }, [state])
  if (!state) return <main className="gate">Connecting…</main>
  if ("connection" in state) return <Connected connection={state.connection} />
  if (state.failed === "remote") return <RemoteDown machine={state.machine} />
  return state.failed === "signed_out" ? <SignedOut /> : <Unreachable />
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
