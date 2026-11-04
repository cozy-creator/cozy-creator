import "@xyflow/react/dist/style.css"
import { sha256 } from "@noble/hashes/sha2.js"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { ReactFlow } from "@xyflow/react"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { HashRouter } from "react-router-dom"

void sha256

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={new QueryClient()}>
      <HashRouter>
        <div style={{ position: "fixed", inset: 0 }}>
          <ReactFlow defaultNodes={[]} defaultEdges={[]} />
        </div>
      </HashRouter>
    </QueryClientProvider>
  </StrictMode>,
)
