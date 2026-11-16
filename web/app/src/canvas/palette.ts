import type { Asset, Tool, ToolField } from "../api/calcifer"
import { TEXT_TOOL, VIDEO_EDIT_TOOL } from "./controls"
import { madeHere } from "./shown"

// A picture, a video or a sound comes in as a file dropped, or a tool that makes it; words come in as Text. There's no list of blank
// boxes (Paul, 11-16: "if you want an image node, just drag an image on there, or drag an image tool on there").
export const OPERATIONS = [
  { key: "image", label: "Image", makes: "image" },
  { key: "upscale_image", label: "Upscale Image", makes: "image" },
  { key: "video", label: "Video", makes: "video" },
  { key: "upscale_video", label: "Upscale Video", makes: "video" },
  { key: "timeline", label: "Video Timeline", makes: "video" },
  { key: "llm", label: "LLM", makes: "text" },
  { key: "text", label: "Text", makes: "text" },
] as const

/** The tools listed, with Calcifer's own: Text, and the Video Timeline last. */
export const withBuiltIns = (tools: Tool[]): Tool[] => [TEXT_TOOL, ...tools.filter((tool) => tool.id !== TEXT_TOOL.id && tool.id !== VIDEO_EDIT_TOOL.id), VIDEO_EDIT_TOOL]

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

const nameOf = (tool: Tool) => `${tool.function} ${tool.label}`.toLowerCase()

/** A tool that changes a video it's given: one of the Video group, called what it does (`modelLabel`). */
const editsVideo = (tool: Tool) => tool.makes === "video" && (/edit|inpaint|outpaint|vid2vid/.test(nameOf(tool)) || hasSource(tool.fields, "video"))

/** Operations describe the result. Image input on a VLM never makes it an image tool. What makes pictures is one group, whether or not it
 *  can edit one (Paul, 11-16: two groups of the same models was "confusing af"), and so is what makes video, a video edit its own row of it
 *  (Paul, 11-16: "eliminate the Edit Video section, and instead place it inside of 'Video' section"). */
export function operationOf(tool: Tool): Operation | null {
  if (tool.id === VIDEO_EDIT_TOOL.id) return "timeline"
  if (tool.id === TEXT_TOOL.id) return "text"
  if (madeHere(tool.id)) return null
  if (tool.makes === "text") return "llm"
  if (tool.makes !== "image" && tool.makes !== "video") return null
  if (/upscale|super.?resolution/.test(nameOf(tool))) return tool.makes === "image" ? "upscale_image" : "upscale_video"
  return tool.makes
}

/** A tool's row in its group: its model, and for a video edit what it does too, as MiniMax H3 Edit beside MiniMax H3. */
export const modelLabel = (tool: Tool) => {
  if (tool.id === VIDEO_EDIT_TOOL.id) return "Video Timeline"
  const title = tool.package_title || tool.label
  const model = tool.model?.default?.split("@")[0]?.split("/").at(-1)
  if (model === "p-h3-image") return "P-H3 Image"
  const version = model?.match(/(\d+(?:\.\d+)+)$/)?.[1]
  const named = version && /\d+(?:\.\d+)*$/.test(title) ? title.replace(/\d+(?:\.\d+)*$/, version) : title
  const edit = operationOf(tool) === "video" && editsVideo(tool) && tool.label && !named.toLowerCase().endsWith(tool.label.toLowerCase())
  return edit ? `${named} ${tool.label}` : named
}

/** Stable operation rows; executable choices live in the node's model control. */
export function paletteEntries(tools: Tool[]): PaletteEntry[] {
  const available = withBuiltIns(tools)
  return OPERATIONS.map((operation) => {
    const variants = available.filter((tool) => operationOf(tool) === operation.key)
      .sort((a, b) => Number(Boolean(a.local)) - Number(Boolean(b.local)) || a.id.localeCompare(b.id))
    return { ...operation, tool: variants[0], variants }
  })
}

export const isVideoEdit = (asset: Asset) => asset.tool === "cozy/edit" || asset.fields?.node_kind === "video_edit"

/** A tool attaches to the kind it produces. Timeline is a removable video tool. */
export function toolFitsNode(tool: Tool, asset: Asset): boolean {
  return tool.id !== "text" && (!madeHere(tool.id) || tool.id === VIDEO_EDIT_TOOL.id) && tool.makes === asset.kind
}
