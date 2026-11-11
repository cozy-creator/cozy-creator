import type { Asset, Tool, ToolField } from "../api/calcifer"
import { madeHere } from "./shown"

export type BasicNodeKind = "text" | "image" | "video" | "video_edit" | "audio"

export const BASIC_NODES: ReadonlyArray<{ kind: BasicNodeKind; label: string }> = [
  { kind: "image", label: "Image" },
  { kind: "video", label: "Video" },
  { kind: "video_edit", label: "Video Editor" },
  { kind: "audio", label: "Audio" },
  { kind: "text", label: "Text" },
]

export const OPERATIONS = [
  { key: "image", label: "Image", makes: "image" },
  { key: "edit_image", label: "Edit Image", makes: "image" },
  { key: "upscale_image", label: "Upscale Image", makes: "image" },
  { key: "video", label: "Video", makes: "video" },
  { key: "edit_video", label: "Edit Video", makes: "video" },
  { key: "upscale_video", label: "Upscale Video", makes: "video" },
  { key: "llm", label: "LLM", makes: "text" },
] as const

export type Operation = typeof OPERATIONS[number]["key"]
export interface PaletteEntry {
  key: Operation
  label: string
  makes: string
  tool?: Tool
  variants: Tool[]
}

const hasSource = (fields: ToolField[], kind: string): boolean => fields.some((field) =>
  (field.type === "slot" && field.required && field.accept === kind) ||
  (field.type === "group" && field.required && hasSource(field.members ?? [], kind)))

/** Operations describe the result. Image input on a VLM never makes it an image tool. */
export function operationOf(tool: Tool): Operation | null {
  if (madeHere(tool.id) || tool.id === "text") return null
  if (tool.makes === "text") return "llm"
  if (tool.makes !== "image" && tool.makes !== "video") return null
  const name = `${tool.function} ${tool.label}`.toLowerCase()
  if (/upscale|super.?resolution/.test(name)) return tool.makes === "image" ? "upscale_image" : "upscale_video"
  if (/edit|inpaint|outpaint|img2img|vid2vid/.test(name) || hasSource(tool.fields, tool.makes)) return tool.makes === "image" ? "edit_image" : "edit_video"
  return tool.makes
}

export const modelLabel = (tool: Tool) => {
  const model = tool.model?.default?.split("@")[0]?.replace(/^_\//, "")
  return model ? `${tool.package_title || model} (${model})` : `${tool.package_title || tool.package} · ${tool.label}`
}

/** Stable operation rows; executable choices live in the node's model control. */
export function paletteEntries(tools: Tool[]): PaletteEntry[] {
  return OPERATIONS.map((operation) => {
    const variants = tools.filter((tool) => operationOf(tool) === operation.key)
      .sort((a, b) => Number(Boolean(a.local)) - Number(Boolean(b.local)) || a.id.localeCompare(b.id))
    return { ...operation, tool: variants[0], variants }
  })
}

export const isVideoEdit = (asset: Asset) => asset.tool === "cozy/edit" || asset.fields?.node_kind === "video_edit"

/** An intrinsic timeline is not a generative Video node, even though both produce video files. */
export function toolFitsNode(tool: Tool, asset: Asset): boolean {
  return !isVideoEdit(asset) && !madeHere(asset.tool) && tool.id !== "text" && !madeHere(tool.id) && tool.makes === asset.kind
}
