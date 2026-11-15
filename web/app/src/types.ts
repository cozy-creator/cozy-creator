// What the page reads that api/calcifer.ts, which api generates, does not declare: its own state, and JSON the API leaves
// untyped, read as far as the page reads it.
import type { NewMachine, ProjectState } from "./api/calcifer"



export interface Entity {
  name: string
  kind: string
  description?: string
  links?: Record<string, string>
  subs?: Entity[]
}

/** A field of a function's request, as its interface renders it. */
export type RequestFieldType =
  | string
  | { literal: unknown[] }
  | { union: RequestFieldType[]; tag_field?: string }
  | { list: RequestFieldType }
  | { map: { key?: RequestFieldType; value: RequestFieldType } }
  | { fields: RequestField[] }
  | { asset: string }
  | { input: string }

export interface RequestField {
  name: string
  type: RequestFieldType
  wire?: string
  default?: unknown
  constraints?: { min_length?: number; max_length?: number; gt?: number; ge?: number; lt?: number; le?: number; multiple_of?: number }
  asset_bound?: { max_bytes?: number; media_types?: string[] }
}

export type MachineLogName = "tensord" | "tensorfs" | "memory" | "calcifer"

/** What a picture, a video or a sound is. */
export type Media = "image" | "video" | "audio"

/** A rental order, as `POST /v1/rental-quotes` and `POST /v1/machines` take it with `kind: "rent"`. */
export type Rent = Omit<Extract<NewMachine, { kind: "rent" }>, "kind">

/** A project as this page holds it, with a page-local read-start order and any entities named by an older project file. */
/** `headsOf`: the project document's heads each asset read alone was read at; the rest were read at `heads`. */
export type Project = ProjectState & { began?: number; entities?: Entity[]; headsOf?: Record<string, string[]> }

/** What the page reads of a target's `interface` (`cozy.package.interface/1`), which the API sends as JSON. */
export interface TargetInterface {
  entrypoints?: {
    name: string
    description?: string
    models?: { path: string; default?: string }[]
    assets?: { parameter: string; kinds?: { kind: string; media_types?: string[]; max_count?: number }[] }
  }[]
}

/** What the page reads of a target's `request` schema (`--describe --json`), JSON too. */
export interface TargetRequest {
  fields?: RequestField[]
}
