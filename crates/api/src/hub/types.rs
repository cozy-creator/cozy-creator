//! The documents cozy sends to and reads from the Hub, named as `api/openapi.yaml` names them, with
//! the fields cozy uses. The Hub and cozy are released apart, so an answer is read leniently (owner,
//! 2026-10-23): a field this does not know is ignored, a missing string, number, bool, list or map
//! is its empty value, a `null` list or map is empty, and a wrong shape names its field. A missing
//! object refuses, by name.
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// A `null` list or map is an empty one: Go writes a nil slice as `null`.
fn null_is_empty<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(
    d: D,
) -> Result<T, D::Error> {
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

/// A Hub account: its name, AuthKit user and orgs.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Account {
    pub name: String,
    /// The AuthKit user it is.
    pub user_id: String,
    /// The orgs it is a member of.
    #[serde(deserialize_with = "null_is_empty")]
    pub orgs: Vec<String>,
}

/// `GET /v1/credit` answered: the caller's credit.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CreditResponse {
    /// None for an account whose rentals never touch credit.
    pub credit: Option<CreditStatus>,
}

/// The caller's net balance (balance less holds and debt), in micros of USD.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CreditStatus {
    pub available_usd_micros: i64,
}

/// RFC 9728's metadata: the origin this Hub's tokens are for.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProtectedResource {
    pub resource: String,
    /// The issuers whose tokens it takes.
    pub authorization_servers: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DeviceKeyEnrollment {
    pub enrollment_id: String,
    pub challenge: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DeviceKeyChallenge {
    pub challenge_id: String,
    pub challenge: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AuthResult {
    pub token_set: Option<TokenSet>,
    pub device_key: Option<SignedDeviceKey>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TokenSet {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SignedDeviceKey {
    pub id: String,
}

/// A machine's DPoP-bound token for its capability (AuthKit's jwt-bearer grant).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WorkloadToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

/// Where a machine reaches this Hub (`GET /v1/execution-environment`).
#[derive(Debug, Deserialize)]
pub struct ExecutionEnvironment {
    pub environment: Environment,
}

/// The Hub's origins and CA as a run's machine receives them, by variable name.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "SCREAMING_SNAKE_CASE")]
pub struct Environment {
    pub tensorhub_origin: String,
    pub tensorhub_public_origin: String,
    /// The CA a machine trusts for the origin when a private CA signs it (a test Hub).
    pub tensorhub_ca_der_b64url: Option<String>,
}

/// The Runtime, TensorFS, cozy and PyTorch versions machines run (`GET /v1/software`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Software {
    pub runtime: String,
    pub tensorfs: String,
    /// Absent: a machine keeps its binary.
    pub cozy: Option<String>,
    /// The operator's pin, not the current worker image's.
    pub pinned: bool,
    /// The PyTorch a machine with no image installs packages on (`2.15.0`); absent (an older
    /// Hub): each keeps the newest it holds.
    pub torch: Option<String>,
}

/// The keys whose signatures a rental's machine accepts, for `lease_seconds` before it asks again.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WorkerAuthorizedKeys {
    pub worker_id: String,
    /// Ed25519 public keys, unpadded base64url.
    #[serde(deserialize_with = "null_is_empty")]
    pub authorized_keys: Vec<String>,
    pub lease_seconds: i64,
}

/// What a rental's machine measured of its own host (`POST /v1/worker/rental/speed`).
#[derive(Debug, Default, Serialize)]
pub struct WorkerSpeedReport {
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download: Option<WorkerDownload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<WorkerProbe>,
}

/// One transfer into the machine, all its streams together.
#[derive(Debug, Serialize)]
pub struct WorkerDownload {
    pub bytes: u64,
    pub seconds: f64,
}

/// A fixed probe's wall time, the same work on every host.
#[derive(Debug, Serialize)]
pub struct WorkerProbe {
    pub probe: String,
    pub seconds: f64,
}

/// `GET /v1/models` answered: one page of matching models, and how many match.
#[derive(Debug, Deserialize)]
pub struct ModelSearch {
    #[serde(default, deserialize_with = "null_is_empty")]
    pub models: Vec<ModelSummary>,
    pub search: SearchMeta,
}

/// How many matched in all, beyond the page answered.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchMeta {
    pub total: i64,
}

/// A model's row in a search, or the head of its card.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ModelSummary {
    pub org: String,
    pub name: String,
    pub family: Option<String>,
    /// Header-derived repository role: model, lora or unknown. Absent on older Hubs.
    pub role: Option<String>,
    pub created_at: String,
    /// The pinned upstream source (on a card).
    pub source: Option<String>,
    /// The org it credits (a root model) or is in (a mirror).
    pub author: Option<ModelAuthor>,
    /// Words a LoRA's prompt uses, and its recommended strength (on a card); never applied.
    pub trigger: Option<String>,
    pub strength: Option<f64>,
    /// Its labels, sorted: filled once from its source, then its owner's. `nsfw` marks one that makes sexual or nude images.
    #[serde(deserialize_with = "null_is_empty")]
    pub tags: Vec<String>,
}

/// The org a model credits or is in, and whether its author has claimed it.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelAuthor {
    pub org: String,
    /// The author's display name.
    pub name: String,
    /// True until the author claims the org.
    pub parked: bool,
    /// A parked org's line.
    pub reserved: Option<String>,
}

/// A model recorded as a mirror of `source`, and the author it credits.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ModelMirror {
    pub model: String,
    pub source: String,
    pub author: Option<ModelAuthor>,
    /// The source's handle, when it was new to the org (`site:handle`).
    pub added_handle: Option<String>,
}

/// An org as `PUT /v1/orgs/{org}` leaves it: its names, links, claim handles and models.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OrgPage {
    pub name: String,
    pub display_name: String,
    #[serde(deserialize_with = "null_is_empty")]
    pub links: Vec<String>,
    #[serde(deserialize_with = "null_is_empty")]
    pub handles: Vec<OrgHandle>,
    pub parked: bool,
    pub reserved: Option<String>,
    #[serde(deserialize_with = "null_is_empty")]
    pub models: Vec<String>,
    #[serde(deserialize_with = "null_is_empty")]
    pub mirrors: Vec<String>,
    #[serde(deserialize_with = "null_is_empty")]
    pub admin_uploads: Vec<String>,
}

/// A claim handle: its site (hf, github or civitai) and the handle there.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OrgHandle {
    pub site: String,
    pub handle: String,
}

/// A parked org's claim: claimed, or its code and where each handle posts it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct OrgClaim {
    pub org: String,
    pub claimed: bool,
    pub code: Option<String>,
    #[serde(deserialize_with = "null_is_empty")]
    pub proofs: Vec<OrgProof>,
    pub proven_by: Option<String>,
}

/// Where one handle posts the claim code, and what the last check of its page found.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OrgProof {
    pub handle: String,
    pub r#where: String,
    pub checked: String,
}

/// `PATCH /v1/models/{org}/{name}`: each field given is set. `tags` replaces them; then `add_tags`, then `remove_tags`.
#[derive(Debug, Default, Serialize)]
pub struct ModelUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub add_tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove_tags: Vec<String>,
}

/// A model as an edit left it.
#[derive(Debug, Deserialize)]
pub struct ModelAnswer {
    pub model: ModelSummary,
    /// What the Hub clamped or cut (a display edit).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// A model with its releases and, to its members, its commits (`GET /v1/models/{org}/{name}`).
#[derive(Debug, Deserialize)]
pub struct ModelCard {
    pub model: ModelSummary,
    /// Newest first.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub releases: Vec<ModelRelease>,
    /// Every commit, newest first, to the model's members; empty to anyone else.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub commits: Vec<ModelCommit>,
}

/// A published release of a model: each lane label's commit.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ModelRelease {
    pub version: String,
    pub created_at: String,
    /// Published while its org was parked: not its author's.
    pub mirror: bool,
    /// Published by a platform admin where it is no member.
    pub by_admin: bool,
    #[serde(deserialize_with = "null_is_empty")]
    pub lanes: BTreeMap<String, ModelReleaseLane>,
    /// What publishing it should tell its publisher; it stands regardless.
    #[serde(deserialize_with = "null_is_empty")]
    pub warnings: Vec<String>,
}

/// One lane's commit in a release: its tree, header, closure bytes and kinds.
#[derive(Debug, Deserialize)]
pub struct ModelReleaseLane {
    /// `sha256:<hex>`
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub length: i64,
    pub header: ObjectRef,
    #[serde(default)]
    pub bytes: i64,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub provenance: Option<ModelLaneProvenance>,
}

/// One commit of a model: its tree, lane, header and closure.
#[derive(Debug, Deserialize)]
pub struct ModelCommit {
    /// `sha256:<hex>` of the commit's tree.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub length: i64,
    #[serde(default)]
    pub lane: String,
    pub header: ObjectRef,
    /// The commit's whole closure.
    #[serde(default)]
    pub bytes: i64,
    /// Its header's schemes and dtypes (bf16, fp8.rowwise, …).
    #[serde(default, deserialize_with = "null_is_empty")]
    pub kinds: Vec<String>,
    /// The releases naming it; empty while private.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub releases: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    /// Uploaded by a platform admin where it is no member.
    #[serde(default)]
    pub by_admin: bool,
}

/// The recipe that wrote a commit: its header's `metadata.provenance`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelLaneProvenance {
    pub package: String,
    pub release: String,
    pub recipe: String,
    /// The digest of the recipe's canonical plan.
    pub fingerprint: String,
    /// The header digest of the checkpoint it was written from.
    pub source: String,
    #[serde(deserialize_with = "null_is_empty")]
    pub plan: Map<String, Value>,
}

/// A stored object by its SHA-256 and length.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ObjectRef {
    /// 64 lowercase hex.
    pub sha256: String,
    pub length: i64,
}

/// `GET /v1/packages` answered: the matching packages, and how many match.
#[derive(Debug, Deserialize)]
pub struct PackageSearch {
    #[serde(default, deserialize_with = "null_is_empty")]
    pub packages: Vec<PackageSummary>,
    pub search: SearchMeta,
}

/// A package's row in a search, or the head of its card.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageSummary {
    pub org: String,
    pub name: String,
    pub created_at: String,
    /// The newest committed release that is not yanked.
    pub latest_release: Option<String>,
}

/// A package and its releases (`GET /v1/packages/{org}/{name}`).
#[derive(Debug, Deserialize)]
pub struct PackageCard {
    pub package: PackageSummary,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub releases: Vec<CatalogRelease>,
}

/// One of a package's releases: when it was cut and whether it is yanked.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CatalogRelease {
    pub release: String,
    pub cut_at: Option<String>,
    pub yanked: Option<bool>,
    pub yanked_at: Option<String>,
}

/// The package `DELETE /v1/packages/{org}/{name}` deleted.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageDeletion {
    /// `org/name`; the root's org is `_`.
    pub package: String,
}

/// The exact files of one release: its interface, pyproject and lock as bytes, and its downloads.
#[derive(Debug, Deserialize)]
pub struct PackageInstallPlan {
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub python_version: Option<String>,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub downloads: Vec<PackageInstallFile>,
    pub package_interface: ExactDocument,
    pub pyproject: ExactDocument,
    pub uv_lock: ExactDocument,
}

/// One download of an install plan: its presigned URL, digest, release path and length.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageInstallFile {
    pub url: String,
    pub digest: String,
    pub path: String,
    pub length: i64,
}

/// A release document's exact bytes.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExactDocument {
    /// Standard base64.
    pub canonical_bytes: String,
}

/// One committed release: its interface, and what the package does in a line.
#[derive(Debug, Deserialize, Serialize)]
pub struct PackageRelease {
    pub release: PackageReleaseSummary,
    #[serde(default)]
    pub package_interface: Value,
    #[serde(default)]
    pub summary: String,
}

/// The release a card is of, by name.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PackageReleaseSummary {
    pub release: String,
}

/// One file a release declares before upload: its digest, length and release path.
#[derive(Debug, Serialize)]
pub struct PackageDeclaredFile {
    pub digest: String,
    pub length: i64,
    /// A PackageReleasePath (`uv.lock`, `artifacts/project/`, …), a directory one followed by the
    /// file's name.
    pub path: String,
}

/// A declared release's upload grants, one per file, and the publication to finalize.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageReleaseGrants {
    #[serde(deserialize_with = "null_is_empty")]
    pub files: Vec<PackageFileGrant>,
    pub publication_id: String,
}

/// One declared file's grant: already present, or where to upload it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageFileGrant {
    pub digest: String,
    pub path: String,
    /// The Hub already holds these bytes.
    pub present: bool,
    pub upload: Option<PresignedUpload>,
}

/// Where to PUT a file's bytes, and the headers the PUT must carry.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PresignedUpload {
    pub url: String,
    #[serde(deserialize_with = "null_is_empty")]
    pub required_headers: BTreeMap<String, String>,
}

/// A release's finalization: its state, and why it failed.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageFinalization {
    /// queued, running, committed or failed.
    pub state: String,
    /// Why it failed: the Hub's refusal.
    pub error: Option<Value>,
    #[serde(deserialize_with = "null_is_empty")]
    pub warnings: Vec<String>,
    /// The committed release is yanked. A Hub from before finalize took `yanked` never says so.
    pub yanked: bool,
}

/// A yank or un-yank: `changed` false when the release already was.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageYank {
    pub release: String,
    pub changed: bool,
    pub yanked_at: String,
}

/// One run of a package function to place (`POST /v1/rental-placements`).
#[derive(Debug, Serialize)]
pub struct RentalPlacementRequest {
    /// `org/name`
    pub package: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
}

/// Where one run of a package function fits: its weights, the machines that hold it, and those that don't.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalPlacement {
    pub weight_bytes: i64,
    /// Whether it runs on a GPU: its own, or the internal functions it calls.
    pub accelerator: bool,
    /// Each model slot as the pick loads it, whose bytes sum to `weight_bytes`.
    #[serde(deserialize_with = "null_is_empty")]
    pub weights: Vec<RentalPlacementWeights>,
    /// The pick first.
    #[serde(deserialize_with = "null_is_empty")]
    pub machines: Vec<RentalPlacementMachine>,
    #[serde(deserialize_with = "null_is_empty")]
    pub excluded: Vec<RentalPlacementExclusion>,
    /// What the Hub changed of the ask, or wants its asker to know.
    #[serde(deserialize_with = "null_is_empty")]
    pub warnings: Vec<String>,
}

/// One model slot the run loads: its model, release, lane and bytes.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalPlacementWeights {
    /// `<function>.models.<param>`
    pub slot: String,
    pub model: String,
    pub release: String,
    pub lane: String,
    pub bytes: i64,
}

/// A machine that holds the run: its card, price, disk and speed.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalPlacementMachine {
    pub name: String,
    pub accelerator_model: String,
    pub vram_gb: i64,
    pub accelerator_count: i64,
    pub price_usd_micros_per_hour: i64,
    pub container_disk_gb: i64,
    /// Effective dense TOPS at the lane, from the card's spec sheet.
    pub speed: Option<f64>,
    pub fast: Option<bool>,
    /// Cost per run against the cheapest machine's (1).
    pub run_cost: Option<f64>,
}

/// A machine that cannot hold the run, and why.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalPlacementExclusion {
    pub name: String,
    pub accelerator_count: i64,
    pub reason: String,
}

/// A rental's product intent; a quote takes the same body.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RentalRequest {
    /// 1-32 lowercase letters, digits and hyphens, beginning with a letter.
    pub name: String,
    pub sku: String,
    /// Machines the rental may be bought as instead, in order.
    pub fallbacks: Vec<RentalFallback>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Retained only when replaying a rental request recorded by an older client.
    #[doc(hidden)]
    #[serde(rename = "image", skip_serializing_if = "Option::is_none")]
    pub legacy_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accelerator_count: Option<i64>,
    /// SHA-256 of the media token, 64 lowercase hex.
    pub media_token_sha256: String,
    /// The rental's Ed25519 public key, unpadded base64url.
    pub creator_public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_disk_gb: Option<i64>,
    /// SSH into the machine as root with this key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub development: Option<RentalDevelopment>,
    /// The price an hour agreed to: an order dearer than it rents nothing, and the refusal says the new price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_usd_micros_per_hour: Option<i64>,
}

/// The SSH key a development rental lets root log in with.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RentalDevelopment {
    /// One OpenSSH public key line.
    pub ssh_public_key: String,
}

/// A product a rental may be bought as instead, at its GPU count (absent: one).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RentalFallback {
    pub sku: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accelerator_count: Option<i64>,
}

/// What a rental request costs an hour at the offer it would buy (`POST /v1/rental-quotes`).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RentalQuote {
    /// The one price an hour, the machine with its disk and traffic: an order sends it back as its cap.
    #[serde(default)]
    pub price_usd_micros_per_hour: i64,
    #[serde(default)]
    pub container_disk_gb: i64,
    pub expected_price: RentalPriceEstimate,
    /// Where the quoted offer is: for an ask that named its provider, or an admin's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datacenter: Option<String>,
    /// What reading the quoted request changed: a disk clamped, a fallback bought, a name fixed.
    #[serde(default, deserialize_with = "null_is_empty", skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The catalog: Calcifer answers it as the Hub does.
pub(crate) use crate::calcifer::{RentalPriceEstimate, RentalSku};

/// One product at one GPU count: sold here, buyable now, or why it is held (`GET /v1/rental-skus/{name}`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalSkuStatus {
    pub known: bool,
    pub offered: bool,
    pub sku: Option<RentalSku>,
    pub last_seen_at: Option<String>,
    /// Why compatible stock is held back (cooldown, retry_in_progress).
    pub unavailable_reason: Option<String>,
    pub retry_after: Option<String>,
    /// Why the offer that frees first is held: no_capacity or failed_boot; absent from an older Hub.
    pub held_for: Option<String>,
}

/// The caller's rentals (`GET /v1/rentals`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalListing {
    #[serde(deserialize_with = "null_is_empty")]
    pub rentals: Vec<Rental>,
}

/// One rental as its Hub answers it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Rental {
    pub rental_id: String,
    pub name: String,
    pub state: String,
    /// The product asked for, then the one its latest attempt bought.
    pub sku: Option<String>,
    /// The card asked for, then the one bought.
    pub requested_accelerator_model: String,
    pub accelerator_count: i64,
    pub provider: Option<String>,
    /// Where its latest attempt was bought, and that its pod mounted the datacenter's NFS drive:
    /// an admin's read, or a renter's who named the provider.
    pub datacenter: Option<String>,
    pub nfs_drive: bool,
    pub machine_name: Option<String>,
    pub worker_id: Option<String>,
    pub worker_address: Option<String>,
    /// `host:port` of its sshd, for a rental asked with an SSH key.
    pub ssh_address: Option<String>,
    /// It was asked with an SSH key: root logs in with it once it is ready.
    pub development: bool,
    pub cert_pem: Option<String>,
    pub base_worker_image_digest: String,
    /// Its one price an hour, never above the cap its order sent.
    pub hourly_rate_usd_micros: i64,
    pub spend_usd_micros: i64,
    pub spend_basis: String,
    /// What the provider billed Tensorhub for it, on the basis `provider_billed` or `estimate`:
    /// an operator's read only.
    pub provider_cost_usd_micros: Option<i64>,
    pub provider_cost_basis: Option<String>,
    pub container_disk_gb: i64,
    pub created_at: String,
    pub ended_at: Option<String>,
    pub release_cause: Option<String>,
    pub detail: Option<String>,
    pub failure: Option<RentalFailure>,
    /// The rental runs off the continent its request came from.
    pub placement_note: Option<String>,
    pub unreachable_since: Option<String>,
    pub boot: Option<RentalBoot>,
}

/// What went wrong with the rental, by the Hub's code (`readiness.provider_stopped`, …).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalFailure {
    pub code: String,
}

/// The current attempt's boot, while its pod is created and booted.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalBoot {
    pub attempt: u32,
    pub state: String,
    pub started_at: Option<String>,
    pub phase: Option<String>,
    pub activity: Option<String>,
    pub last_progress_at: Option<String>,
    pub observed_at: String,
    pub replanned_from: Option<ReplannedBoot>,
    pub pull: Option<RentalPull>,
}

/// The image pull as the provider's boot log measured it: the layers in flight, not the image.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalPull {
    pub observable: bool,
    pub layers_observed: i64,
    pub layers_complete: i64,
    pub download_bytes: i64,
    pub download_total_bytes: i64,
    pub download_bytes_per_second: Option<f64>,
    pub extract_bytes: i64,
    pub extract_total_bytes: i64,
    pub extract_bytes_per_second: Option<f64>,
    /// A lower bound for the layers it has seen, never when the rental is ready.
    pub remaining_seconds: Option<f64>,
}

/// The failed attempt a rental's current one replaced.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReplannedBoot {
    pub attempt: u32,
    pub ended_at: String,
    pub failure_code: String,
}
