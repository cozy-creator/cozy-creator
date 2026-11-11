import type { Asset, Tool, ToolField } from "../api/calcifer"
import { madeHere } from "./shown"

export type BasicNodeKind = "text" | "image" | "video" | "video_edit"

export const BASIC_NODES: ReadonlyArray<{ kind: BasicNodeKind; label: string }> = [
  { kind: "text", label: "Text" },
  { kind: "image", label: "Image" },
  { kind: "video", label: "Video" },
  { kind: "video_edit", label: "VideoEdit" },
]

export interface PaletteEntry {
  key: string
  label: string
  model: string | null
  tool: Tool
  variants: Tool[]
}

const modelOf = (tool: Tool) => tool.model?.default?.split("@")[0]?.replace(/^_\//, "") || null
const mediaRequired = (fields: ToolField[]): number => fields.reduce((sum, field) => sum + (field.type === "slot" && field.required ? 1 : field.type === "group" && field.required ? mediaRequired(field.members ?? []) : 0), 0)
const hasPrompt = (fields: ToolField[]): boolean => fields.some((field) => field.type === "prompt" || (field.type === "group" && hasPrompt(field.members ?? [])))
const order = (a: Tool, b: Tool) => mediaRequired(a.fields) - mediaRequired(b.fields) || Number(Boolean(a.local)) - Number(Boolean(b.local)) || Number(!hasPrompt(a.fields)) - Number(!hasPrompt(b.fields)) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
const title = (name: string) => name.split(/[-_]+/).filter(Boolean).map((word) => !/[aeiou]/i.test(word) ? word.toUpperCase() : word[0].toUpperCase() + word.slice(1)).join(" ")

/** One model, not every function of its package. Signature-required media keeps edit/upscale helpers behind the generator. */
export function paletteEntries(tools: Tool[]): PaletteEntry[] {
  const groups = new Map<string, Tool[]>()
  const byPackage = new Map<string, Set<string>>()
  const localKinds = new Map<string, Set<string>>()
  for (const tool of tools) {
    if (madeHere(tool.id)) continue
    const model = modelOf(tool)
    const key = model ? `model:${model}` : `package:${tool.package}:${tool.makes}`
    const group = groups.get(key) ?? []
    group.push(tool)
    groups.set(key, group)
    const keys = byPackage.get(tool.package) ?? new Set<string>()
    keys.add(key)
    byPackage.set(tool.package, keys)
    if (!model) {
      const kinds = localKinds.get(tool.package) ?? new Set<string>()
      kinds.add(tool.makes)
      localKinds.set(tool.package, kinds)
    }
  }
  return [...groups].map(([key, variants]) => {
    variants.sort(order)
    const tool = variants[0]
    const model = modelOf(tool)
    const name = model?.split("/").at(-1)
    const own = tool.package.split("/").at(-1) ?? tool.package
    const related = Boolean(name?.toLowerCase().startsWith(own.toLowerCase()))
    const label = !model && (localKinds.get(tool.package)?.size ?? 0) > 1
      ? `${tool.package_title || title(own)} ${title(tool.makes)}`
      : name && ((related && name !== own) || (!related && (byPackage.get(tool.package)?.size ?? 0) > 1))
        ? title(name) : tool.package_title || title(own)
    return { key, label, model, tool, variants }
  }).sort((a, b) => a.label.localeCompare(b.label))
}

export const isVideoEdit = (asset: Asset) => asset.tool === "cozy/edit" || asset.fields?.node_kind === "video_edit"

/** An intrinsic timeline is not a generative Video node, even though both produce video files. */
export function toolFitsNode(tool: Tool, asset: Asset): boolean {
  return !isVideoEdit(asset) && !madeHere(asset.tool) && tool.id !== "text" && !madeHere(tool.id) && tool.makes === asset.kind
}
