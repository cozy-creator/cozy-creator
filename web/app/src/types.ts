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
  /** That peer's details of its refusal, verbatim. */
  metadata?: Record<string, unknown>
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
  /** Its bytes aren't on this computer. */
  absent?: boolean
  /** The machine that keeps them, when one does. */
  machine?: string
}

/** The file people mean by a version. */
export interface MainFile {
  output: string
  path: string
  sha256: string
  field: string
  width?: number
  height?: number
  duration_ms?: number
  /** A video's frames a second. */
  fps?: number
  alpha?: boolean
  absent?: boolean
  machine?: string
}

export interface VersionPrefix {
  number: number
  prefix: string
  /** An Edit's: the clips that made this version. */
  timeline?: TimelineNow
  /** A cut's: what it was cut from, `<asset> <version>` each. */
  from?: string[]
  /** A cut's: what it was cut without, `<piece>: <why>` each. */
  without?: string[]
  /** The take it picked up from. */
  pickup?: Pickup
  /** The machine holding its latents, which a pickup of it joins on. */
  latents?: string
  execution_ms?: number
  run?: number
  main?: MainFile
}

/** A mention that names something, and the places its text says it: `summary`, `beats.0.description`. */
export interface Read {
  name: string
  /** The one version a `<name.2>` fixes. */
  version?: number
  at?: string[]
}

export interface Asset {
  name: string
  standing: Standing
  making?: number[]
  kind: string
  noun: string
  tool?: string
  versions: number
  prefixes?: VersionPrefix[]
  /** The version that is its state. */
  version?: number
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
  alpha?: boolean
  absent?: boolean
  machine?: string
  reads?: Read[]
  /** Its inputs, flat, by each control's `field`. A person wrote them, so nothing here is more than text. */
  fields?: Record<string, unknown>
  /** What `PATCH`'s `If-Match` names. */
  etag?: string
  /** An Edit's: its clips as Calcifer reads them now. */
  timeline?: TimelineNow
  /** An Edit being rendered here: how far, 0 to 1. It has no run to ask. */
  rendering?: number
  failure?: ErrorBody
  /** A pin's: what changed since the version it holds was made. */
  changed?: string[]
  /** What its text reads that is nothing in the project. */
  missing?: string[]
}

/** A clip as an edit's `fields.timeline` writes it: `<name>`, and the part of it. A video's `in` to `out`, exclusive, in frames at the edit's rate: no `out` is to its end. A picture's `frames`, how long it's held. Its place in the list is its place in the film. */
export interface Clip {
  clip: string
  in?: number
  out?: number
  frames?: number
}

/** A clip and what it reads now. */
export interface ClipNow extends Clip {
  name?: string
  field?: string
  path?: string
  sha256?: string
  /** How many frames a video holds at the edit's rate. */
  holds?: number
  width?: number
  height?: number
  /** Why the render goes on without it. */
  missing?: string
}

export interface TimelineNow {
  fps: number
  tracks: { clips: ClipNow[] }[]
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
  /** The film that stands was cut without these: `<piece>: <why>`. */
  without?: string[]
}

/** `GET /v1/projects/{id}/assets/{name}`; what a card needs is `asset`. */
export interface AssetVersions {
  asset: Asset
}

export interface ProjectState {
  id: string
  path: string
  /** What kept its last render from reaching a machine, each once. */
  unsent?: ErrorBody[]
  name?: string
  assets: Asset[]
  entities?: Entity[]
  queue?: Queued[]
  missing?: string[]
  stale?: Stale[]
  orphans?: Orphan[]
  busy: boolean
  /** The page's own: when the read that says this began, so a read from before a write says nothing of it. */
  began?: number
  moving?: boolean
  absent?: number
  run_files?: number
  cuts?: Cut[]
  warnings?: ProjectWarning[]
}

/** What reading a project said, of a name, or with none of the file as a whole. */
export interface ProjectWarning {
  name?: string
  message: string
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
  /** What opening it did, or found, said once. */
  said?: string[]
}

export interface Asked {
  id: string
  path: string
  joined?: boolean
  added: number
  skipped?: string[]
  queued?: { name: string; ahead?: string[] }[]
}

/** How this computer would do what a render asks: each tool it would do poorly, and the GPU to rent instead. */
export interface Fit {
  tools: ToolFit[]
  rent?: RentFor
}

export interface ToolFit {
  tool: string
  label: string
  names: string[]
  line: string
  bytes: number
  /** Render here was said of it on this computer: nothing asks again. */
  here?: boolean
}

export interface RentFor {
  sku: string
  gpus: number
  card: string
  vram_gb: number
  usd_micros_per_hour: number
  line: string
}

export interface Unpinned {
  name: string
  version?: number
  stale: string[]
  etag?: string
}

export interface Approved {
  name: string
  noun: string
  /** The version it is; 0 for a file that is none of its versions. */
  number: number
  files: Record<string, ProjectFile>
  path: string
  stale: string[]
  warnings?: string[]
  etag?: string
}

/** What discarding a take or version, or bringing it back, did. */
export interface VersionMoved {
  name: string
  number: number
  noun: string
  /** It's the approved one: its approval went with it, or came back with it. */
  approved?: boolean
  /** What the asset stands on now, else its newest. None: it has none left. */
  stands?: number
  stale?: string[]
  /** What names this version in its text. */
  read_by?: string[]
  /** A version held that file already: it's that one. */
  already?: boolean
}

export interface Renamed {
  from: string
  to: string
  versions: number
  /** A render is going: it happens as that ends. */
  queued?: boolean
}

export interface Row {
  x: number
  y: number
  w?: number
}

export interface Canvas {
  /** The project's revision these rows are of. */
  seq: number
  cards: Record<string, Row>
}

export interface Import {
  source: string
  name?: string
  at?: Row
  /** What its drop pushed aside, written with it unless another wrote a row since `seq`. */
  moves?: Record<string, Row>
  seq?: number
  description?: string
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
  /** Each of its `moves` another wrote since, as it is. */
  kept?: Record<string, Row | null>
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
  /** The median of its stage's last five steps after the first. */
  step_steady_ms?: number
  at_ms?: number
  done_fraction?: number
  bytes_per_s?: number
  /** The seconds the whole run has left, as Calcifer estimates them; every client shows this one. */
  eta_s?: number
  /** 0 to 1. */
  eta_confidence?: number
  /** `first_run` with no `eta_s`: nothing like it ran on this machine or its GPU yet. */
  eta_basis?: "machine" | "gpu" | "steps" | "first_run"
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
  url?: string
  width?: number
  height?: number
  duration_ms?: number
  export_error?: string
  checkpoint?: unknown
  stays?: boolean
}

export interface Adjustment {
  field: string
  kind: string
  requested: string
  applied: string
  reason: string
  source: string
}

export interface Call {
  run: string
  index: number
  function: string
  label?: string
  status: string
  called_at_ms: number
  finished_at_ms: number
  reason?: Reason
  memoized?: boolean
}

/** What a run was asked, as it was bound: `target` and `input`, under a new key, ask it again. */
export interface RunAsked {
  target: string
  input: Record<string, unknown>
  seed?: number
}

export interface ReportStage {
  name: string
  ms: number
  legs?: { name: string; ms: number }[]
  steps?: { count: number; each_ms: number }
  unaccounted?: boolean
}

export interface ReportModel {
  slot: string
  repository: string
  release: string
  lane: string
  precision: string
  adapters: { model: string; strength: number }[]
}

export interface ReportComponent {
  component: string
  attention: string[]
  gemm: { kernel: string; format: string; fallback?: { served: string; wanted: string; reason: string } }[]
}

export interface ReportKernel {
  gpu?: number
  arch: string
  kernel: string
  state: string
  served: boolean
  progress?: number
  compile_ms: number
  detail: string
}

export interface Moved {
  bytes: number
  gbps?: number
  ceiling_share?: number
}

export interface ReportMemory {
  gpu: number
  scope: string
  kept: Moved
  leased: Moved
  store: Moved
  derived: Moved
  stall_ms: number
  peak?: number
  torch_peak?: number
  compute?: number
  leftover?: number
}

export interface Measured {
  by: string
  stages: ReportStage[]
  models: ReportModel[]
  components: ReportComponent[]
  kernels: ReportKernel[]
  memory: ReportMemory[]
}

export interface Report {
  attention_pins?: string[]
  measured?: Measured[]
}

export interface Run {
  id: string
  number: number
  kind: string
  target: string
  hub: string
  machine: string | null
  project?: string
  project_id?: string
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
  adjustments?: Adjustment[]
  result?: unknown
  execution_ms?: number
  stalled?: Stalled
  calls?: Call[]
  report?: Report
  request?: RunAsked
}

export interface RunList {
  runs: Run[]
  older?: number
  machine?: string
  unreadable?: string[]
}

/** One event of `GET /v1/runs/{id}/events`, as far as the page reads them; it skips the rest. */
export type RunEvent =
  | { type: "run.log"; sequence_number: number; at_ms: number; level: string; text: string }
  | { type: "run.call"; sequence_number: number; call: Call }
  | { type: "output_item.done" | "output_item.landed"; sequence_number: number; item: OutputItem }
  | { type: "run.created" | "run.queued" | "run.in_progress" | "run.completed" | "run.failed" | "run.canceled"; sequence_number: number; run: Run }
  | { type: "output_item.added" | "output_item.delta"; sequence_number: number }
  | { type: "run.progress" | "run.waiting" }

export interface Play {
  address: string
  fingerprint: string
  token: string
  url: string
  expires_s: number
}

export interface RunList {
  runs: Run[]
  older?: number
}

// Packages

export interface FoundPackage {
  package: string
  latest_release?: string
  created_at?: string
}

export interface PackageSearch {
  packages: FoundPackage[]
  total?: number
  older?: number
}

export interface PackageRelease {
  release: string
  published?: string
  yanked: boolean
}

export interface PackageDetail {
  package: string
  latest_release?: string
  releases: PackageRelease[]
  warnings?: string[]
}

export interface PackageRemovals {
  machine: string
  runs: Run[]
}

// Models

export interface ModelAuthor {
  org: string
  name: string
  parked: boolean
  reserved?: string
}

export interface FoundModel {
  model: string
  family?: string
  release?: string
  lanes: string[]
  private: boolean
  author?: ModelAuthor
  /** Its owner's label, or (absent) its weights' family. */
  family_from?: FamilyFrom
  /** What its header says: only a LoRA's factors, a model's weights (its header verifies as a family), or a component's (a text encoder, a VAE:
   *  weights no family's). Absent: it couldn't be read. */
  kind?: ModelKind
  /** Its labels, sorted; `nsfw` marks one that makes sexual or nude images. */
  tags?: string[]
}

export type ModelKind = "lora" | "weights" | "component"

/** Where a model's family comes from: its owner's label, or its weights (its header verifies as it). */
export type FamilyFrom = "owner" | "weights"

export interface ModelSearch {
  models: FoundModel[]
  total?: number
  older?: number
}

export interface ModelLane {
  lane: string
  commit: string
  bytes: number
  kinds?: string[]
}

export interface ModelRelease {
  version: string
  created_at: string
  mirror?: boolean
  by_admin?: boolean
  lanes: ModelLane[]
}

export interface ModelCommit {
  commit: string
  lane: string
  bytes: number
  releases: string[]
  created_at: string
}

export interface ModelDetail {
  model: string
  family?: string
  family_from?: FamilyFrom
  /** What its header verifies as, whatever its label. */
  weights_family?: string
  kind?: ModelKind
  created_at: string
  source?: string
  author?: ModelAuthor
  trigger?: string
  strength?: number
  tags?: string[]
  releases: ModelRelease[]
  commits: ModelCommit[]
}

/** Its owner's edit: each field given is set. `family` "none" clears the label; `tags` replaces them, then `add_tags` and `remove_tags`. */
export interface ModelEdit {
  family?: string
  tags?: string[]
  add_tags?: string[]
  remove_tags?: string[]
}

export interface ModelEdited {
  model: ModelDetail
  warnings?: string[]
}

export interface ModelReleased {
  model: string
  version: string
  warnings?: string[]
}

export interface ModelUploading {
  run?: Run
  replay: boolean
  recorded?: { commit: string; from: string; lane: string; model: string; next?: string[] }
}

// Account

export interface Account {
  account?: string
  balance_usd_micros?: number
  billing?: string
  buy_credits?: string
  email?: string
  key_refused?: boolean
  logged_in: boolean
  orgs: string[]
}

export interface Code {
  id: string
  email: string
  retry_after_s?: number
}

export type Proven = ({ purpose: "sign_in" } & Account) | { purpose: "export" } | ({ purpose: "sign_out_everywhere" } & SignedOutAnswer)

export interface SignedOutAnswer {
  email?: string
  everywhere: boolean
  hub: string
  notes: string[]
  status: string
}

export interface OrgClaim {
  claimed: boolean
  code?: string
  org: string
  proofs: { checked: string; handle: string; where: string }[]
  proven_by?: string
}

export interface RunRequest {
  target: string
  project?: string
  on?: string
  input?: Record<string, unknown>
  input_text?: string
  terms?: string[]
  files?: { name?: string; file: string }[]
  loras?: string[]
  gpus?: number
  lane?: string
  agreed?: Agreed
}

export interface Submitted {
  run: Run
  replay: boolean
  notes?: { level: string; message: string }[]
  preparing?: boolean
}

/** A field of a function's request, as its interface renders it. */
export type FieldType =
  | string
  | { literal: unknown[] }
  | { union: FieldType[]; tag_field?: string }
  | { list: FieldType }
  | { map: { key?: FieldType; value: FieldType } }
  | { fields: RequestField[] }
  | { asset: string }
  | { input: string }

export interface RequestField {
  name: string
  type: FieldType
  wire?: string
  default?: unknown
  constraints?: { min_length?: number; max_length?: number; gt?: number; ge?: number; lt?: number; le?: number; multiple_of?: number }
  asset_bound?: { max_bytes?: number; media_types?: string[] }
}

export interface Target {
  package: string
  function?: string
  release: string
  release_from: string
  summary?: string
  interface: {
    entrypoints?: {
      name: string
      description?: string
      models?: { path: string; default?: string }[]
      assets?: { parameter: string; kinds?: { kind: string; media_types?: string[]; max_count?: number }[] }
    }[]
  }
  functions?: { function: string; description?: string }[]
  contract?: string
  request?: { fields?: RequestField[] }
  next?: string[]
}

// Machines

export type MachineKind = "local" | "rental" | "added"
export type MachineState = "booting" | "ready" | "ending" | "ended" | "stopped" | "unknown"

export interface Rental {
  sku: string
  accelerator: string
  gpus: number
  usd_micros_per_hour: number
  spend_usd_micros?: number
  spend_basis?: string
  rented_at_ms: number
  ended_at_ms?: number
  disk_gb: number
  failure?: string
  placement_note?: string
  unreachable_since_ms?: number
  ssh?: string
}

export interface Gpu {
  index: number
  name: string
  memory_bytes: number
  driver?: string
  family?: string
  memory_class?: string
}

export interface LiveRun {
  id: string
  state: string
  waiting?: string
  run?: Run
}

export interface MachineStatus {
  worker_id: string
  version: string
  dev?: boolean
  platform: string
  phase: string
  update?: string
  runtime?: string
  tensorfs?: string
  torch?: string
  started_at_ms: number
  idle_deadline_ms?: number
  gpus?: Gpu[]
  disk_total_bytes?: number
  disk_free_bytes?: number
  disk_models_bytes?: number
  runs?: LiveRun[]
  gpu_problems?: string[]
  activity?: Operation[]
}

/** Weights arriving on a machine, or a package's environment built there, once however many runs wait on it. */
export interface Operation {
  id: string
  kind: "download" | "install"
  /** The model's repository (or provider source), or the package and its release. */
  subject: string
  stage?: string
  /** Bytes fetched of those it has to fetch, never what was here already. */
  bytes_done: number
  bytes_total: number
  bytes_here: number
  bytes_per_s?: number
  eta_s?: number
  started_at_ms: number
  /** The runs waiting on it, by number. */
  runs: number[]
}

export interface Attempt {
  number: number
  state: string
  replaced?: { number: number; failure: string; ended_at_ms: number }
}

export interface Machine {
  name: string
  kind: MachineKind
  state: MachineState
  hub?: string
  rental_id?: string
  alias?: string
  address?: string
  fingerprint?: string
  reason?: string
  offline?: string
  rental?: Rental
  ssh?: { args: string[]; line: string }
  status?: MachineStatus
  unreachable?: string
  unreachable_since_ms?: number
  boot?: string
  attempt?: Attempt
  running?: number
  queued?: number
  /** False: a rental another client of this account ordered; reached as this computer's own, never placed on unasked. */
  ordered?: boolean
  unverified?: boolean
  reach?: string
  ending?: string
}

export interface MachineList {
  machines: Machine[]
  hubs?: { hub: string; listed_at_ms: number }[]
  unanswered?: { hub: string; code: string; message: string }[]
  signed_out?: string[]
}

export interface RentalSkuWidth {
  accelerator_count: number
  price_usd_micros_per_hour: number
  vcpu_count?: number
  memory_gb?: number
}

export interface RentalSku {
  name: string
  accelerator_model: string
  vram_gb: number
  widths: RentalSkuWidth[]
}

/** The Hub's price for one exact order. A person is shown `gpu_usd_micros_per_hour`, what we charge; `agreed` goes back with the order as it came. */
export interface Quote {
  gpu_usd_micros_per_hour: number
  agreed?: Agreed
  warnings?: string[]
}

export interface Agreed {
  usd_micros_per_hour: number
  usd_micros_per_gb: number
}

export interface Alternative {
  sku: string
  gpus: number
  label?: string
  time: number
  usd_micros_per_hour: number
}

/** What a `new` run would rent, its price, and why. */
export interface RunQuote {
  sku: string
  gpus?: number
  quote: Quote
  why?: string
  alternatives?: Alternative[]
  warnings?: string[]
}

export interface Rent {
  sku: string
  gpus?: number
  ssh_key?: string
}

export interface Rented {
  machine: Machine
  quote: Quote
  replay: boolean
}

export interface Ended {
  machine: Machine
  changed: boolean
  abandoned?: string[]
}

export interface Keepalive {
  acknowledged_at_ms: number
  idle_deadline_ms?: number
}

export type Component = { version: string }

export interface MachineUpdate {
  runtime?: Component
  tensorfs?: Component
  tensord?: Component
}

export interface Updating {
  machine: Machine
  run?: Run
  target: string
  warnings?: string[]
}

export type MachineLogName = "tensord" | "tensorfs" | "memory" | "calcifer"

export interface HeldPackage {
  package: string
  release: string
  runtime: string
  level: string
  source: string
  warning?: string
}

export interface MachinePackages {
  machine: string
  packages: HeldPackage[]
}

export interface HeldModel {
  model: string
  release: string
  lane: string
  precision: string
  bytes: number
  shared_bytes: number
}

export interface MachineModels {
  machine: string
  models: HeldModel[]
  silent?: boolean
  repositories?: number
  bytes?: number
}

// The one stream

export type ServerEvent =
  | { type: "hello"; version: string }
  | { type: "run.updated"; run: Run }
  | { type: "machine.updated"; machine: Machine }
  | { type: "machine.removed"; name: string }
  | { type: "project.changed"; id: string; rev: string; assets?: string[]; gone?: string[]; head?: boolean }
  | { type: "canvas"; id: string; seq: number; cards: Record<string, Row | null> }
  | { type: "history"; id: string; undo?: HistoryEntry; redo?: HistoryEntry }
  | { type: "resync" }

/** Who made an edit: the page (`client` names its tab), the CLI, a hand in project.yaml, or another client. */
export interface By {
  via: "page" | "cli" | "file" | "api"
  client?: string
}

/** One edit of a project, as its history keeps it (`GET /v1/projects/{id}/history`). */
export interface HistoryEntry {
  id: number
  at_ms: number
  by: By
  words: string
  names?: string[]
  /** The page's own ids for it (`Cozy-Edit`). */
  edits?: string[]
  undone?: { at_ms: number; by: By }
}

export interface History {
  entries: HistoryEntry[]
  undo?: number
  redo?: number
}

export interface HistoryStepped {
  entries: HistoryEntry[]
  already?: boolean
  stopped?: ErrorBody
  undo?: number
  redo?: number
}

export interface AssetMade {
  name: string
  etag: string
  /** Each of its `moves` another wrote since, as it is. */
  kept?: Record<string, Row | null>
}

export interface AssetPatched {
  name: string
  etag: string
  was: string
  warnings?: string[]
}

export interface AssetDeleted {
  name: string
  /** Opaque: what `PUT` takes to put it back. */
  text: string
  at?: Row
}

export type BoxType = "words" | "prompt" | "toggle" | "number" | "choice" | "shape" | "slot" | "group" | "seed" | "timeline"

/** What a picture, a video or a sound is. */
export type Media = "image" | "video" | "audio"

/** One field of a tool's box, as `Tool.fields` declares it (`tools.md`, On the wire). A box is built from these, and nothing about any function. */
export interface Field {
  field: string
  type: BoxType
  label: string
  required?: boolean
  default?: unknown
  /** `| None` on a toggle or a number: Auto. */
  auto?: boolean
  /** An expert setting: under More, in its Advanced section. */
  advanced?: boolean
  placeholder?: string
  /** prompt: what its mentions may send. None: any. */
  takes?: Media[]
  /** number. */
  min?: number
  max?: number
  step?: number
  unit?: string
  integer?: boolean
  /** A number in a list group: the box shows its sum. */
  total?: boolean
  /** choice: what it can be, in order, its own words for some, and a line on each that makes it a dropdown, both by value. */
  options?: unknown[]
  words?: Record<string, string>
  descriptions?: Record<string, string>
  /** shape: the ratios it allows, or the bounds of any. */
  ratios?: string[]
  /** slot: what it takes. */
  accept?: Media
  /** slot: the file and its description. */
  described?: boolean
  multiple?: boolean
  /** slot: the list group whose total its video's length matches. */
  spans?: string
  /** group: its fields, whether it's rows or one block, and how few and how many rows. */
  members?: Field[]
  list?: boolean
  min_items?: number
  max_items?: number
}

/** The frames a pickup can land on: `offset + k * every`, from `first` up. */
export interface Snap {
  every: number
  offset: number
  first: number
}

export interface Tool {
  id: string
  package: string
  function: string
  release: string
  /** What a new box of it is called, `shot_1`. A Calcifer that doesn't say goes by `label`. */
  name?: string
  /** What it does within its package: `Image`, `Edit`, `Upscale`. */
  label: string
  /** Its package in words, heading its tools in the list: `Qwen Image 2`. */
  package_title: string
  /** What a box of it is. */
  makes: Media | "text"
  description?: string
  /** Local code a project's assets name. */
  local?: boolean
  /** The package a machine installs, where `package` is local code's path. */
  installs?: string
  fields: Field[]
  /** The path in its package of a component that draws its box. */
  ui?: string
  /** It takes a pickup: Pick up from here, landing on these frames. */
  pickup?: Snap
  /** What a checkpoint dropped on its box goes on: the one model its function runs. */
  model?: { slot: string; default?: string; family?: string }
}

/** Pick up from here: a take kept up to `out_frame`, and the rest rendered again. Sent for one render, never written. */
export interface Pickup {
  /** Any box's version, as a mention: `<shot_1.3>`. */
  take: string
  out_frame: number
  beats_from?: number
}

/** Where a tool's component is, for `import()`. */
export interface ToolUi {
  module: string
}

export interface ToolList {
  packages: string[]
  without?: string[]
}

export interface Browsed {
  packages: { package: string; summary?: string; tools: string[] }[]
  older: number
}

export interface Tools {
  tools: Tool[]
  /** Why it holds only what's installed. */
  note?: string
}
