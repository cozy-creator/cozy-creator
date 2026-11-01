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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CreditResponse {
    /// None for an account whose rentals never touch credit.
    pub credit: Option<CreditStatus>,
}

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

#[derive(Debug, Deserialize)]
pub struct ExecutionEnvironment {
    pub environment: Environment,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "SCREAMING_SNAKE_CASE")]
pub struct Environment {
    pub tensorhub_origin: String,
    pub tensorhub_public_origin: String,
    /// The CA a machine trusts for the origin when a private CA signs it (a test Hub).
    pub tensorhub_ca_der_b64url: Option<String>,
}

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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WorkerAuthorizedKeys {
    pub worker_id: String,
    /// Ed25519 public keys, unpadded base64url.
    #[serde(deserialize_with = "null_is_empty")]
    pub authorized_keys: Vec<String>,
    pub lease_seconds: i64,
}

#[derive(Debug, Serialize)]
pub struct WorkerEndpointReport {
    pub public_host: String,
    /// Private TCP port (a decimal string) to its public port.
    pub ports: BTreeMap<String, u16>,
}

#[derive(Debug, Deserialize)]
pub struct ModelSearch {
    #[serde(default, deserialize_with = "null_is_empty")]
    pub models: Vec<ModelSummary>,
    pub search: SearchMeta,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchMeta {
    pub total: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ModelSummary {
    pub org: String,
    pub name: String,
    pub family: Option<String>,
    pub created_at: String,
    /// The pinned upstream source (on a card).
    pub source: Option<String>,
    /// The org it credits (a root model) or is in (a mirror).
    pub author: Option<ModelAuthor>,
    /// Words a LoRA's prompt uses, and its recommended strength (on a card); never applied.
    pub trigger: Option<String>,
    pub strength: Option<f64>,
}

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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ModelMirror {
    pub model: String,
    pub source: String,
    pub author: Option<ModelAuthor>,
    /// The source's handle, when it was new to the org (`site:handle`).
    pub added_handle: Option<String>,
}

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

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OrgHandle {
    pub site: String,
    pub handle: String,
}

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

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OrgProof {
    pub handle: String,
    pub r#where: String,
    pub checked: String,
}

#[derive(Debug, Deserialize)]
pub struct ModelAnswer {
    pub model: ModelSummary,
}

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
}

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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ObjectRef {
    /// 64 lowercase hex.
    pub sha256: String,
    pub length: i64,
}

#[derive(Debug, Deserialize)]
pub struct PackageSearch {
    #[serde(default, deserialize_with = "null_is_empty")]
    pub packages: Vec<PackageSummary>,
    pub search: SearchMeta,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageSummary {
    pub org: String,
    pub name: String,
    pub created_at: String,
    /// The newest committed release that is not yanked.
    pub latest_release: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PackageCard {
    pub package: PackageSummary,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub releases: Vec<CatalogRelease>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CatalogRelease {
    pub release: String,
    pub cut_at: Option<String>,
    pub yanked: Option<bool>,
    pub yanked_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageDeletion {
    /// `org/name`; the root's org is `_`.
    pub package: String,
}

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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageInstallFile {
    pub url: String,
    pub digest: String,
    pub path: String,
    pub length: i64,
}

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

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PackageReleaseSummary {
    pub release: String,
}

#[derive(Debug, Serialize)]
pub struct PackageDeclaredFile {
    pub digest: String,
    pub length: i64,
    /// A PackageReleasePath (`uv.lock`, `artifacts/project/`, …), a directory one followed by the
    /// file's name.
    pub path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageReleaseGrants {
    #[serde(deserialize_with = "null_is_empty")]
    pub files: Vec<PackageFileGrant>,
    pub publication_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageFileGrant {
    pub digest: String,
    pub path: String,
    /// The Hub already holds these bytes.
    pub present: bool,
    pub upload: Option<PresignedUpload>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PresignedUpload {
    pub url: String,
    #[serde(deserialize_with = "null_is_empty")]
    pub required_headers: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageFinalization {
    /// queued, running, committed or failed.
    pub state: String,
    /// Why it failed: the Hub's refusal.
    pub error: Option<Value>,
    #[serde(deserialize_with = "null_is_empty")]
    pub warnings: Vec<String>,
}

/// A yank or un-yank: `changed` false when the release already was.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PackageYank {
    pub release: String,
    pub changed: bool,
    pub yanked_at: String,
}

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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalPlacement {
    pub weight_bytes: i64,
    /// The pick first.
    #[serde(deserialize_with = "null_is_empty")]
    pub machines: Vec<RentalPlacementMachine>,
    #[serde(deserialize_with = "null_is_empty")]
    pub excluded: Vec<RentalPlacementExclusion>,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
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
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RentalDevelopment {
    /// One OpenSSH public key line.
    pub ssh_public_key: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RentalFallback {
    pub sku: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accelerator_count: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RentalQuote {
    #[serde(default)]
    pub price_usd_micros_per_hour: i64,
    #[serde(default)]
    pub storage_usd_micros_per_hour: i64,
    #[serde(default)]
    pub bandwidth_usd_micros_per_gb: Option<i64>,
    #[serde(default)]
    pub container_disk_gb: i64,
    pub expected_price: RentalPriceEstimate,
}

/// What a rental is expected to cost at its prices, for the expected hours plus its download.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RentalPriceEstimate {
    pub hours: f64,
    pub download_gb: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_mbps: Option<i64>,
    pub download_hours: f64,
    pub gpu_usd_micros: i64,
    pub disk_usd_micros: i64,
    pub download_usd_micros: i64,
    pub total_usd_micros: i64,
}

/// One product (a GPU card, or the CPU machine) with the widths buyable now.
#[derive(Debug, Deserialize, Serialize)]
pub struct RentalSku {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub accelerator_model: String,
    #[serde(default)]
    pub base_worker_profile: String,
    /// major.minor; empty for the CPU product.
    #[serde(default)]
    pub compute_capability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_class: Option<String>,
    #[serde(default)]
    pub vram_gb: i64,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub python_provisionable_minors: Vec<String>,
    #[serde(default, deserialize_with = "null_is_empty")]
    pub python_interpreters: Vec<RentalPythonInterpreter>,
    /// The GPU counts buyable now, ascending.
    #[serde(default, deserialize_with = "null_is_empty")]
    pub widths: Vec<RentalSkuWidth>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RentalPythonInterpreter {
    pub version: String,
    pub abi: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RentalSkuWidth {
    #[serde(default)]
    pub accelerator_count: i64,
    #[serde(default)]
    pub price_usd_micros_per_hour: i64,
    #[serde(default)]
    pub storage_usd_micros_per_hour: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth_usd_micros_per_gb: Option<i64>,
    #[serde(default)]
    pub container_disk_gb: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcpu_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<i64>,
    pub expected_price: RentalPriceEstimate,
}

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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalListing {
    #[serde(deserialize_with = "null_is_empty")]
    pub rentals: Vec<Rental>,
}

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
    pub machine_name: Option<String>,
    pub worker_id: Option<String>,
    pub worker_address: Option<String>,
    /// `host:port` of its sshd, for a rental asked with an SSH key.
    pub ssh_address: Option<String>,
    /// It was asked with an SSH key: root logs in with it once it is ready.
    pub development: bool,
    pub cert_pem: Option<String>,
    pub base_worker_image_digest: String,
    pub hourly_rate_usd_micros: i64,
    pub compute_usd_micros_per_hour: Option<i64>,
    pub bandwidth_usd_micros_per_gb: Option<i64>,
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RentalFailure {
    pub code: String,
}

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
}

/// The failed attempt a rental's current one replaced.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReplannedBoot {
    pub attempt: u32,
    pub ended_at: String,
    pub failure_code: String,
}
