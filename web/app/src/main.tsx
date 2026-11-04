import "@xyflow/react/dist/base.css"
import "./ui/tokens.css"
import "./ui/app.css"
import { QueryClientProvider } from "@tanstack/react-query"
import { StrictMode, useEffect, useState } from "react"
import { createRoot } from "react-dom/client"
import { RouterProvider } from "react-router-dom"
import { Client } from "./api/client"
import { sameOrigin } from "./api/transport"
import { type Connection, ConnectionProvider, connect } from "./app/Connection"
import { router } from "./app/routes"
import { readSession, saveSession, takeCode } from "./app/session"
import { Copyable } from "./ui/Copyable"
import { useSignal } from "./ui/signal"

type Boot = { connection: Connection } | { failed: "signed_out" | "unreachable" }

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
  if (!readSession()) saveSession(null)
  return connect()
}

const booted = boot()

function SignedOut() {
  return (
    <main className="gate">
      <h1>This page isn't signed in</h1>
      <p>Run this in a terminal to open it signed in:</p>
      <Copyable text="cozy web" />
    </main>
  )
}

function Unreachable() {
  return (
    <main className="gate">
      <h1>Calcifer isn't answering</h1>
      <p>Run this in a terminal, then reload:</p>
      <Copyable text="cozy web" />
      <button type="button" className="tb" onClick={() => location.reload()}>
        Reload
      </button>
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
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ConnectionProvider>
  )
}

function App() {
  const [state, setState] = useState<Boot | null>(null)
  useEffect(() => {
    void booted.then(setState)
  }, [])
  if (!state) return <main className="gate">Connecting…</main>
  if ("connection" in state) return <Connected connection={state.connection} />
  return state.failed === "signed_out" ? <SignedOut /> : <Unreachable />
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
