// `protocol::calcifer` and `protocol::project` by hand, as far as the page reads them. A reader ignores unknown fields, and an
// unknown standing or state shows as unknown.

export interface ErrorBody {
  kind: string
  code: string
  message: string
  remedy?: string
  next?: string[]
  cause?: string
  retry_after_s?: number
}

export interface Session {
  token: string
  media: string
}

export interface Status {
  version: string
  os: string
  local_usable: boolean
  signed_in: boolean
  rental?: { machine_id: string; idle_deadline_ms?: number }
}

// Projects

export type Standing = "current" | "out_of_date" | "making" | "missing"

export interface ProjectFile {
  path: string
  sha256: string
}

export interface Kept {
  take: number
  until_s: number
}

export interface VersionPrefix {
  number: number
  prefix: string
  kept?: Kept
}

export interface Asset {
  name: string
  standing: Standing
  making?: number[]
  kind: string
  noun: string
  with?: string
  versions: number
  prefixes?: VersionPrefix[]
  approved?: number
  pinned?: boolean
  inputs?: string
  files?: Record<string, ProjectFile>
  path?: string
  made_by?: string
  status: string
  sha256?: string
  field?: string
  width?: number
  height?: number
  duration_ms?: number
}

export interface Queued {
  name: string
  running?: boolean
  run?: number
  busy?: boolean
}

export interface Entity {
  name: string
  kind: string
  description?: string
  links?: Record<string, string>
  subs?: Entity[]
}

export interface Stale {
  name: string
  why: string
}

export interface Orphan {
  name: string
  kind: string
  versions: number
  bytes: number
  renamed_to?: string
}

export interface Cut {
  cut: string
  path: string
  fps: number
  cut_at_ms?: number
  frames?: number
  retimed?: string[]
  skips?: string[]
}

export interface ProjectState {
  id: string
  path: string
  name?: string
  assets: Asset[]
  entities?: Entity[]
  queue?: Queued[]
  missing?: string[]
  stale?: Stale[]
  orphans?: Orphan[]
  busy: boolean
  moving?: boolean
  absent?: number
  run_files?: number
  cuts?: Cut[]
  warnings?: string[]
}

export interface ProjectRow {
  id: string
  name: string
  path: string
  used_at_ms: number
  missing?: boolean
  busy?: boolean
}

export interface ProjectList {
  projects: ProjectRow[]
  older?: number
}

export interface Opened {
  id: string
  path: string
  name?: string
}

export interface Renamed {
  from: string
  to: string
  versions: number
}

export interface Row {
  x: number
  y: number
  w?: number
}

export interface Canvas {
  etag: string
  cards: Record<string, Row>
}

export interface Import {
  source: string
  name?: string
  at?: Row
  remove_background?: boolean
  on?: string
}

export interface Imported {
  name: string
  sha256: string
  field: string
  path: string
  already?: boolean
  run?: Run
  note?: string
}

export interface Held {
  digest: string
  length: number
  media_type: string
}

// Runs

export type RunStatus = "queued" | "in_progress" | "completed" | "failed" | "canceled"
export type ItemStatus = "in_progress" | "completed" | "incomplete"

export interface Reason {
  code: string
  message: string
  origin?: string
  detail?: string
}

export interface Warning {
  code?: string
  message: string
}

export interface Stalled {
  since_ms: number
  machine: string
  usd_micros_per_hour?: number
}

export interface Progress {
  stage: string
  fraction?: number
  stage_fraction?: number
  completed?: number
  total?: number
  bytes_done?: number
  bytes_total?: number
  step_ms?: number
  step_avg_ms?: number
  at_ms?: number
  done_fraction?: number
  bytes_per_s?: number
  eta_s?: number
}

export interface OutputItem {
  id: string
  name: string
  index: number
  rev: number
  length: number
  media_type: string
  label?: string
  status: ItemStatus
  sha256?: string
  path?: string
  export_error?: string
  stays?: boolean
}

export interface Run {
  id: string
  number: number
  kind: string
  target: string
  hub: string
  machine: string | null
  project?: string
  status: RunStatus
  cancel_requested?: boolean
  machine_state?: string
  reason?: Reason
  output?: OutputItem[]
  created_at_ms: number
  accepted_at_ms?: number
  started_at_ms?: number
  settled_at_ms?: number
  waiting?: string
  progress?: Progress
  queue_position?: number
  warnings?: Warning[]
  execution_ms?: number
  stalled?: Stalled
}

// Machines

export type MachineKind = "local" | "rental" | "added"
export type MachineState = "booting" | "ready" | "ending" | "ended" | "stopped" | "unknown"

export interface Machine {
  name: string
  kind: MachineKind
  state: MachineState
  hub?: string
  rental_id?: string
  alias?: string
  address?: string
  reason?: string
  offline?: string
  unreachable?: string
  running?: number
  queued?: number
  recorded?: boolean
}

export interface MachineList {
  machines: Machine[]
}

// The one stream

export type ServerEvent =
  | { type: "hello"; version: string }
  | { type: "run.updated"; run: Run }
  | { type: "machine.updated"; machine: Machine }
  | { type: "machine.removed"; name: string }
  | { type: "project.changed"; id: string; rev: string }
  | { type: "canvas"; id: string; etag: string; cards: Record<string, Row | null> }
  | { type: "resync" }
