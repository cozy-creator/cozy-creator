import { useQuery } from "@tanstack/react-query"
import * as React from "react"
import { ApiError } from "../api/client"
import { useConnection } from "../app/Connection"
import { privateProject } from "../app/view"
import type { Asset, ToolField, Tool } from "../api/calcifer"
import type { ElementView } from "./CanvasStore"
import { useCanvasStore } from "./context"

/** What a component speaks (`crates/calcifer/docs/PACKAGE-UI.md`). One that says anything else isn't drawn, and the declared box stands. */
const API = 1

/** What a component is given. */
export interface ToolBoxProps {
  /** The app's React: a component bundles none, and needs no build step. */
  React: typeof React
  asset: Pick<Asset, "name" | "kind" | "tool" | "standing" | "versions">
  /** What it holds now, what's typed included. */
  fields: Record<string, unknown>
  /** Writes these fields to the project file, and says whether that stood. */
  setFields(fields: Record<string, unknown>): Promise<boolean>
  /** What its text can name as `<name>`. */
  mentions: { name: string; kind: string }[]
  /** What the tool declares of each field, by field. */
  limits: Record<string, ToolField>
  /** The host's colours and type, as CSS values for `style`. */
  tokens: typeof TOKENS
}

const TOKENS = {
  bg: "var(--bg)",
  card: "var(--card)",
  panel: "var(--panel)",
  line: "var(--line)",
  ink: "var(--ink)",
  mute: "var(--mute)",
  faint: "var(--faint)",
  accent: "var(--ember)",
  good: "var(--cur)",
  warn: "var(--ood)",
  busy: "var(--mak)",
  bad: "var(--bad)",
  sans: "var(--sans)",
  mono: "var(--mono)",
}

interface Own {
  /** Where it's from, so another is drawn anew. */
  module: string
  Component: React.ComponentType<ToolBoxProps>
  /** The one field it draws; absent, it draws the whole edit face. */
  field?: string
}

async function imported(module: string): Promise<Own> {
  const loaded = await import(/* @vite-ignore */ module)
  if (loaded.api !== API) throw new Error(`it speaks API ${String(loaded.api)}, and this page speaks ${API}`)
  if (typeof loaded.default !== "function") throw new Error("its default export is no component")
  return { module, Component: loaded.default, field: typeof loaded.field === "string" ? loaded.field : undefined }
}

/** A component, from this computer's Calcifer. */
function useOwn(tool: Tool | undefined) {
  const { client } = useConnection()
  const store = useCanvasStore()
  const project = store.projectId
  const id = tool?.ui && !(tool.local && privateProject(project)) ? tool.id : null
  const { data, error } = useQuery({
    queryKey: ["tool-ui", id, project],
    queryFn: async () => imported((await client.toolUi(id ?? "", project)).module),
    enabled: id !== null,
    retry: false,
    // A box mounts on each hover, and one installed or edited a moment ago should show soon.
    staleTime: 30_000,
    structuralSharing: false,
  })
  const quiet = error instanceof ApiError && (error.body.code === "tool_ui_none" || error.body.code === "tool_ui_not_installed")
  return { own: id === null ? null : (data ?? null), trouble: error && !quiet ? String(error.message ?? error) : null }
}

/** One that throws leaves the declared box, and the canvas goes on. */
class Guard extends React.Component<{ fallback: React.ReactNode; children: React.ReactNode }, { failed: string | null }> {
  state = { failed: null as string | null }

  static getDerivedStateFromError(error: unknown) {
    return { failed: String(error) }
  }

  render() {
    if (this.state.failed === null) return this.props.children
    return (
      <>
        {this.props.fallback}
        <Trouble why={this.state.failed} />
      </>
    )
  }
}

const Trouble = ({ why }: { why: string }) => (
  <p className="muted tool-ui-failed" title={why}>
    This tool's own box couldn't be drawn, so this is its plain one
  </p>
)

function Drawn({ own, name, view, controls }: { own: Own; name: string; view: ElementView; controls: ToolField[] }) {
  const store = useCanvasStore()
  const asset = view.asset as Asset
  const setFields = (fields: Record<string, unknown>) => {
    for (const [field, value] of Object.entries(fields)) store.setDraft(name, field, value)
    return store.save(name)
  }
  const mentions = store.namesLike("", name).map((one) => ({ name: one, kind: store.get(one)?.asset?.kind ?? "file" }))
  const { name: called, kind, tool, standing, versions } = asset
  return (
    <div className="tool-ui nodrag nopan nowheel">
      <own.Component
        React={React}
        asset={{ name: called, kind, tool, standing, versions }}
        fields={{ ...asset.fields, ...view.sent, ...view.drafts }}
        setFields={setFields}
        mentions={mentions}
        limits={Object.fromEntries(controls.map((control) => [control.field, control]))}
        tokens={TOKENS}
      />
    </div>
  )
}

/** A box's controls, those shown, those behind More and those only to set, grouped (`grouped`, drawn by `cell`): each drawn as declared, unless
 *  its tool's package ships a component for them all or for one, which draws that one wherever it stands. The approved face is never a
 *  component's. `children` gets each part drawn; a component for them all is the shown part, and the others are null. */
export function ToolBox({
  tool,
  name,
  view,
  shown,
  folded,
  grouped,
  row,
  cell,
  children,
}: {
  tool: Tool | undefined
  name: string
  view: ElementView
  shown: ToolField[]
  folded: ToolField[]
  grouped: ToolField[]
  row(control: ToolField): React.ReactNode
  cell(control: ToolField): React.ReactNode
  children(shown: React.ReactNode, folded: React.ReactNode, grouped: React.ReactNode): React.ReactNode
}) {
  const { own, trouble } = useOwn(tool)
  const controls = [...shown, ...folded, ...grouped]
  if (!own) {
    return children(
      shown.length || trouble ? (
        <>
          {shown.map(row)}
          {trouble && <Trouble why={trouble} />}
        </>
      ) : null,
      folded.map(row),
      grouped.length ? grouped.map(cell) : null,
    )
  }
  const drawn = <Drawn own={own} name={name} view={view} controls={controls} />
  const field = own.field
  if (!field || !controls.some((control) => control.field === field)) {
    return children(
      <Guard key={own.module} fallback={[...shown.map(row), ...folded.map(row), ...grouped.map(cell)]}>
        {drawn}
      </Guard>,
      null,
      null,
    )
  }
  const each = (draw: (control: ToolField) => React.ReactNode) => (control: ToolField) =>
    control.field === field ? (
      <Guard key={own.module} fallback={draw(control)}>
        {drawn}
      </Guard>
    ) : (
      draw(control)
    )
  return children(shown.length ? shown.map(each(row)) : null, folded.map(each(row)), grouped.length ? grouped.map(each(cell)) : null)
}
