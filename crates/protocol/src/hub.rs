//! Calcifer's Hub resources (CALCIFER-API.md "Account", "Orgs", "Models", "Packages"): what a
//! client reads and changes at a Hub through Calcifer, which speaks to the Hub as this computer's
//! device key. Every request names its Hub (`hub`; absent, Calcifer's configured one). Records
//! a CLI prints with `--json` are these, field for field, in the order they print.
use serde::{Deserialize, Serialize};

/// `?hub=`: the Hub a read or deletion is at.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct At {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
}

/// `GET /v1/account`: who this computer signs in as at a Hub, as that Hub says. Credit is read
/// only here; an account a sign-in answers carries none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// None until it is named, and while signed out.
    pub account: Option<String>,
    /// None where the Hub does not meter it.
    pub balance_usd_micros: Option<i64>,
    /// `metered` or `unmetered`; None for an unnamed account.
    pub billing: Option<String>,
    /// The Hub's page to buy credit.
    pub buy_credits: Option<String>,
    pub device_key: Option<String>,
    pub email: Option<String>,
    /// This computer holds a key the Hub no longer accepts (revoked from another computer).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub key_refused: bool,
    /// The Hub accepts this computer's key.
    pub logged_in: bool,
    pub orgs: Vec<String>,
}

/// What an emailed code proves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// Enrolls this computer's key.
    SignIn,
    /// Enrolls a key for another computer (CI), answered to the client and kept nowhere here.
    Export,
    /// Revokes every key of the account, this computer's too.
    SignOutEverywhere,
}

/// `POST /v1/account/codes`: the Hub emails a code. `email` names the account to sign in or
/// export a key for; signing out everywhere mails the signed-in key's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    pub purpose: Purpose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// A code on its way: `POST /v1/account/codes/{id}` with what arrived finishes it, once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Code {
    pub id: String,
    pub email: String,
}

/// `POST /v1/account/codes/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAnswer {
    pub code: String,
}

/// What a finished code did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case")]
pub enum Proven {
    /// This computer's key is enrolled and kept; `account` is None until it is named.
    SignIn(Account),
    Export(ExportedKey),
    SignOutEverywhere(SignedOut),
}

/// A key for another computer: its file, which the client writes owner-only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedKey {
    pub email: String,
    pub device_key: String,
    /// The key file's text (`cozy login --key` installs it).
    pub key: String,
}

/// `PUT /v1/account/name`: names a new account. The Hub's `account.name_unavailable` and
/// `account.name_invalid` refuse a name; ask for another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Naming {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    pub name: String,
}

/// `PUT /v1/account/key`: makes a key `Export` made this computer's key at its Hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyInstall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// The key file's text.
    pub key: String,
}

/// `DELETE /v1/account`, or a finished `SignOutEverywhere`: this computer's key is erased,
/// whatever the Hub answered its revocation (`notes` say when it did not confirm).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedOut {
    pub email: Option<String>,
    pub everywhere: bool,
    pub hub: String,
    pub notes: Vec<String>,
    /// `signed out`, or `not signed in`.
    pub status: String,
}

/// `PATCH /v1/orgs/{org}` (platform admins): each field given replaces the org's own; an org that
/// does not exist yet is parked under the handles given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// Its author's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<String>>,
    /// `site:handle` (hf, github or civitai).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handles: Option<Vec<String>>,
}

/// An org's page, as `PATCH /v1/orgs/{org}` leaves it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    pub name: String,
    pub display_name: String,
    pub links: Vec<String>,
    pub handles: Vec<OrgHandle>,
    pub parked: bool,
    /// A parked org's line.
    pub reserved: Option<String>,
    pub models: Vec<String>,
    pub mirrors: Vec<String>,
    pub admin_uploads: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgHandle {
    pub site: String,
    pub handle: String,
}

/// `POST /v1/orgs/{org}/claim` (`{hub}`): the org is the caller's once a handle it lists shows
/// `code`; until then, where to post it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgClaim {
    pub claimed: bool,
    pub code: Option<String>,
    pub org: String,
    pub proofs: Vec<OrgProof>,
    pub proven_by: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgProof {
    pub checked: String,
    pub handle: String,
    pub r#where: String,
}

/// `GET /v1/models?q=&family=&limit=`: the Hub's models the caller can see, one row each with its
/// newest release; signed in, also its own and its orgs' while private (no release yet). `q` that
/// is exactly `org/name` is that one model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelSearch {
    pub models: Vec<FoundModel>,
    /// How many models matched, shown or not.
    pub total: i64,
    /// How many matched beyond those shown.
    pub older: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FoundModel {
    pub model: String,
    pub family: Option<String>,
    pub release: Option<String>,
    /// The newest release's lanes; a private model's commits' lanes.
    pub lanes: Vec<String>,
    pub private: bool,
    /// Who made it: the org it credits, or the parked org it mirrors into.
    pub author: Option<ModelAuthor>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAuthor {
    pub org: String,
    /// The author's display name.
    pub name: String,
    /// True until the author claims the org.
    pub parked: bool,
    /// A parked org's line.
    pub reserved: Option<String>,
}

/// `GET /v1/models/{org}/{name}?release=`: a model's releases (one, when named) and, to its org's
/// members, every commit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelDetail {
    pub model: String,
    pub family: Option<String>,
    pub created_at: String,
    /// The pinned upstream source.
    pub source: Option<String>,
    pub author: Option<ModelAuthor>,
    /// Words a LoRA's prompt uses, and its recommended strength (a run's default stays 1.0).
    pub trigger: Option<String>,
    pub strength: Option<f64>,
    /// Newest first.
    pub releases: Vec<ModelRelease>,
    /// Newest first; empty to anyone but the model's members.
    pub commits: Vec<ModelCommit>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelRelease {
    pub version: String,
    pub created_at: String,
    /// Published while its org was parked: not its author's.
    pub mirror: bool,
    /// Published by a platform admin where it is no member.
    pub by_admin: bool,
    pub lanes: Vec<ModelLane>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelLane {
    pub lane: String,
    /// `sha256:<hex>`.
    pub commit: String,
    pub bytes: i64,
    /// Its header's schemes and dtypes.
    pub kinds: Vec<String>,
    /// The recipe that wrote it, as its header records.
    pub provenance: Option<Provenance>,
}

/// The recipe that wrote a commit: its header's `metadata.provenance`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub package: String,
    pub release: String,
    pub recipe: String,
    /// The digest of the recipe's canonical plan.
    pub fingerprint: String,
    /// The header digest of the checkpoint it was written from.
    pub source: String,
    pub plan: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCommit {
    pub commit: String,
    pub lane: String,
    pub bytes: i64,
    /// The releases naming it; none while private.
    pub releases: Vec<String>,
    pub created_at: String,
}

/// `PATCH /v1/models/{org}/{name}`: an empty `family` clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
}

/// What `PATCH /v1/models/{org}/{name}` leaves.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFamily {
    pub model: String,
    pub family: Option<String>,
}

/// `POST /v1/models/{org}/{name}/releases`: one commit per lane, under a version higher than every
/// earlier one; the Hub never reuses a version, which makes a retry safe.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelReleaseAsk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    pub version: String,
    /// `sha256:<hex>`.
    pub commits: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelReleased {
    pub model: String,
    pub version: String,
    pub created_at: String,
    /// Each lane's commit.
    pub lanes: Vec<(String, String)>,
}

/// `DELETE /v1/models/{org}/{name}[/releases/{version}|/commits/{commit}]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDeleted {
    pub model: String,
    /// One release, by version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// One commit; absent when the whole model went.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// `GET /v1/packages?q=&limit=`: the Hub's packages the caller can see.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSearch {
    pub packages: Vec<FoundPackage>,
    pub total: i64,
    /// How many matched beyond those shown.
    pub older: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundPackage {
    pub package: String,
    pub latest_release: Option<String>,
    pub created_at: String,
}

/// `GET /v1/packages/{org}/{name}?release=`: a package's releases newest first, or the one named
/// with what its publish warned of (its owner sees the warnings).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDetail {
    /// As published: the Hub finds `qwen3-5` for `qwen3.5`.
    pub package: String,
    pub latest_release: Option<String>,
    pub releases: Vec<PackageRelease>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRelease {
    pub release: String,
    pub published: Option<String>,
    pub yanked: bool,
}

/// `POST /v1/packages/{org}/{name}/releases/{release}/yank`: machines that hold it keep it; only
/// an exact pin installs it. `changed` is false when it was yanked already.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageYanked {
    pub package: String,
    pub release: String,
    pub changed: bool,
    pub yanked_at: String,
}

/// `DELETE …/yank`: the unversioned default again when it is the newest release.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageUnyanked {
    pub package: String,
    pub release: String,
    pub changed: bool,
}

/// `DELETE /v1/packages/{org}/{name}`: gone with every release, its name and versions free again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDeleted {
    pub package: String,
}

/// What Calcifer did once an ask's run completed, before it settled: the run's `result.after`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AfterRun {
    /// An upload's trigger and strength, set at the Hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Displayed>,
    /// An install's older releases of its package on the same machine and Hub.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<Retired>,
}

/// A model's display metadata as set at the Hub: shown by `cozy model info`, never applied (a
/// run's LoRA strength stays 1.0).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Displayed {
    pub model: String,
    pub trigger: Option<String>,
    pub strength: Option<f64>,
    /// Why the Hub did not take them; the upload stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retired {
    pub package: String,
    pub release: String,
    /// It left the machine; otherwise `reason` says why it stays.
    pub left: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `POST /v1/model-downloads` (an `Idempotency-Key`): a warm run that makes a model present in a
/// machine's store, `202` with the run (`calcifer::Submitted`). `POST /v1/model-download-quotes`
/// takes the same body and answers a `new` run's `calcifer::RunQuote`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelDownload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// `org/name[@release]`, `org/name@sha256:<commit>`, `hf://…`, `civitai://…`, `https://…`, or
    /// an absolute path to one `.safetensors` file on this computer.
    pub model: String,
    /// `local/<alias>`: the name the machine keeps it under (required for a file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// A Hub model's lane; absent, the Hub picks what the machine runs best.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// The source's convention, a reviewed TensorFS format (`h3.lora`); usually detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    pub on: crate::calcifer::On,
    /// The quote a `new` run's renter agreed to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<crate::calcifer::Agreed>,
}

/// `POST /v1/model-uploads` (an `Idempotency-Key`): a warm run on the machine that makes the source
/// and uploads it as a new private commit of `destination` in `lane`, `202` with the run. Its
/// trigger and strength are set at the Hub once it completes (`result.after.display`).
/// `POST /v1/model-upload-quotes` takes the same body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelUpload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// A provider source, `local/<alias>`, a Hub model, an absolute path to a `.safetensors` file
    /// or a CozyTensors folder, or `run:N[#output]`: a run's weights output, uploaded from the
    /// machine that holds it. A Hub commit uploads only on a `base`; else it is recorded
    /// (`POST /v1/models/{org}/{name}/commits`).
    pub source: String,
    /// `org/name`.
    pub destination: String,
    pub lane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    /// It mirrors its author's model (admins): the upstream page; empty, the hf:// or civitai://
    /// source itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<String>,
    /// The base component a LoRA was trained on, when its file does not declare it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trained_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f64>,
    /// `MODEL[:COMPONENT,…]`: start from this model, the source's components in place of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub on: crate::calcifer::On,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<crate::calcifer::Agreed>,
}

/// `POST /v1/models/{org}/{name}/commits`: commit `from` of another model, recorded as a commit of
/// this one when the caller may read its whole closure. Nothing is uploaded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCommitAsk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// `org/name@sha256:<commit>`.
    pub from: String,
    /// The lane `from` is in.
    pub lane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordedCommit {
    pub commit: String,
    pub from: String,
    pub lane: String,
    pub model: String,
    /// What a mirror recorded first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirrored: Option<Mirrored>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Displayed>,
}

/// A destination recorded as a mirror of its author's model, before anything uploads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mirrored {
    pub model: String,
    pub source: String,
    /// The author it credits, or its parked org's line.
    pub by: Option<String>,
    /// The org it credits.
    pub org: String,
    /// The source's handle (`site:handle`), when it was new to the org's claim handles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_handle: Option<String>,
}

/// `POST /v1/model-uploads` answered: the run, as `calcifer::Submitted` says it, and the mirror
/// recorded before it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelUploading {
    pub run: crate::calcifer::Run,
    pub replay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirrored: Option<Mirrored>,
}

/// `POST /v1/package-installs` (an `Idempotency-Key`): a warm run that installs a package's
/// environment on a machine, no weights, `202` with it. A published package's older releases there
/// go once it is in (`result.after.retired`). `POST /v1/package-install-quotes` takes the same body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackageInstall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// `org/name`, or the absolute path of a package directory or a script: installed live, its
    /// files read as they are at each run.
    pub package: String,
    /// Absent, the Hub's newest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub on: crate::calcifer::On,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<crate::calcifer::Agreed>,
}

/// `POST /v1/package-removals` (an `Idempotency-Key`): one remove run per package and Hub the
/// machine holds it from, every release; `202` with them. Each run's result is a
/// `{removed, reclaimed_bytes, retained}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRemoval {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// `org/name`, or `local/name`.
    pub packages: Vec<String>,
    pub machine: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackageRemovals {
    pub machine: String,
    pub runs: Vec<crate::calcifer::Run>,
}

/// `POST /v1/model-quantizations` (an `Idempotency-Key`): a new commit of `model` in `lane`,
/// written from a commit (or a release's original weights) by the recipe `package` declares for
/// that lane, as a run of its `quantize` on the machine, which uploads it. `202` with the run
/// (`calcifer::Submitted`); local code not yet made ready answers its install (`preparing`):
/// follow it, then ask again under the same key. `POST /v1/model-quantization-quotes` takes the
/// same body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelQuantize {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// `org/name@sha256:<commit>`, or `org/name[@release]` (none: the newest).
    pub model: String,
    /// The lane to write: a recipe the package declares for this model.
    pub lane: String,
    /// `org/name[@release]`, or the absolute path of a package directory nobody published.
    pub package: String,
    pub on: crate::calcifer::On,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<crate::calcifer::Agreed>,
}

/// `POST /v1/package-locks` (an `Idempotency-Key`): locks the project in `dir` for the Hub and the
/// account it publishes to (`root`: the root namespace, Hub admins), with uv on this computer;
/// writes back only `uv.lock`. `202` with the work (`Authoring`); followed on
/// `GET /v1/package-locks/{id}/events`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockAsk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// The project's absolute directory.
    pub dir: std::path::PathBuf,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub root: bool,
    /// Dependencies that may move to their newest compatible release.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upgrade: Vec<String>,
}

/// `POST /v1/package-publications` (an `Idempotency-Key`): publishes the project in `dir` as
/// `<account>/<name>@<version>` (`root`: `<name>@<version>`): staged, relocked for this Hub, built,
/// described statically, uploaded and finalized. `202` with the work (`Authoring`); followed on
/// `GET /v1/package-publications/{id}/events`. A version already published is answered as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishAsk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    pub dir: std::path::PathBuf,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub root: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    Running,
    Done,
    Failed,
}

/// A lock or publication as it stands (`GET /v1/package-locks/{id}`,
/// `GET /v1/package-publications/{id}`), kept while Calcifer runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Authoring {
    /// `lock-…` or `publication-…`.
    pub id: String,
    pub state: WorkState,
    /// What it said as it went (uv's refusals are its `error`).
    pub notices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<Locked>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<Published>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::error::Error>,
}

/// One event of a lock's or publication's stream; the last is `locked`, `published` or `failed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthoringEvent {
    Notice { message: String },
    Locked { locked: Locked },
    Published { published: Published },
    Failed { error: crate::error::Error },
}

impl AuthoringEvent {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Notice { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locked {
    pub lock: std::path::PathBuf,
    pub account_index: Option<String>,
    pub changed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    pub package: String,
    pub release: String,
    /// `published`, or `already published`.
    pub status: String,
    pub changed: bool,
    pub uploaded_bytes: u64,
    pub hub: String,
    pub warnings: Vec<String>,
}
