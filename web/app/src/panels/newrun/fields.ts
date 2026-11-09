import type { RequestFieldType, RequestField } from "../../types"

export type Widget =
  | { kind: "text" }
  | { kind: "number"; integer: boolean }
  | { kind: "yesno" }
  | { kind: "choice"; options: unknown[] }
  | { kind: "file"; asset: string; many: boolean }
  /** Only a folder on this computer can fill it: it stays in the CLI. */
  | { kind: "tree" }
  | { kind: "json" }

/** Which control a field gets. An optional field is its type or null; anything richer is JSON, since the same reader takes it. */
export function widgetOf(type: RequestFieldType): Widget {
  if (typeof type === "string") {
    if (type === "str") return { kind: "text" }
    if (type === "int" || type === "float") return { kind: "number", integer: type === "int" }
    if (type === "bool") return { kind: "yesno" }
    return { kind: "json" }
  }
  if ("literal" in type) return { kind: "choice", options: type.literal }
  if ("asset" in type) return { kind: "file", asset: type.asset, many: false }
  if ("input" in type) return type.input === "tree" ? { kind: "tree" } : { kind: "text" }
  if ("list" in type) {
    const item = widgetOf(type.list)
    return item.kind === "file" ? { ...item, many: true } : { kind: "json" }
  }
  if ("union" in type) {
    const rest = type.union.filter((branch) => branch !== "null")
    if (rest.length === 1) return widgetOf(rest[0])
  }
  return { kind: "json" }
}

export const acceptOf = (field: RequestField, asset: string) =>
  field.asset_bound?.media_types?.join(",") || (asset === "image" || asset === "video" || asset === "audio" ? `${asset}/*` : undefined)

/** Whatever a field holds, in words: `required`, or its default. */
export function hintOf(field: RequestField): string {
  if (field.wire === "omissible") return "the model chooses"
  if (field.default !== undefined && field.default !== null) return String(field.default)
  return field.wire === undefined || field.wire === "required" ? "required" : ""
}

export const isFile = (value: unknown): value is string => typeof value === "string" && /^sha256:[0-9a-f]{64}$/i.test(value)

/** Each file named in `text` hidden behind a label, and the way back: the hash is never shown, and text can still name a file. */
export function hideFiles(text: string, labels: Map<string, string>): string {
  return text.replace(/sha256:[0-9a-f]{64}/gi, (digest) => {
    if (!labels.has(digest)) labels.set(digest, `(file ${labels.size + 1})`)
    return labels.get(digest)!
  })
}

export function revealFiles(text: string, labels: Map<string, string>): string {
  let out = text
  for (const [digest, label] of labels) out = out.split(label).join(digest)
  return out
}
