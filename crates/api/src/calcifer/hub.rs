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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CodeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    pub purpose: Purpose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// A code on its way: `POST /v1/account/codes/{id}` with what arrived finishes it, once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Code {
    pub id: String,
    pub email: String,
    /// Asked again too soon: `id` is the code already out, and the Hub sends another after this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_s: Option<u64>,
}

/// `POST /v1/account/codes/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CodeAnswer {
    pub code: String,
}

/// `POST /v1/account/codes/{id}/step`: the step a Hub's AuthKit named, refusing that code with
/// its `metadata`. `path` is under AuthKit's mount (`account/recovery/confirm`); `body` goes as is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeStep {
    pub path: String,
    #[serde(default)]
    pub body: serde_json::Value,
}

/// What a finished code did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "purpose", rename_all = "snake_case")]
pub enum Proven {
    /// This computer's key is enrolled and kept; `account` is None until it is named.
    SignIn(Account),
    Export(ExportedKey),
    SignOutEverywhere(SignedOut),
}

/// A key for another computer: its file, which the client writes owner-only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

/// An account the org lists on another site (hf, github or civitai).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgHandle {
    pub site: String,
    pub handle: String,
}

/// `POST /v1/orgs/{org}/claim` (`{hub}`): the org is the caller's once a handle it lists shows
/// `code`; until then, where to post it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OrgClaim {
    pub claimed: bool,
    pub code: Option<String>,
    pub org: String,
    pub proofs: Vec<OrgProof>,
    pub proven_by: Option<String>,
}

/// One of the org's handles to post the claim code at: where, and the Hub's check of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OrgProof {
    pub checked: String,
    pub handle: String,
    pub r#where: String,
}

/// `GET /v1/models?q=&family=&limit=&offset=`: the Hub's models the caller can see, one row each with its
/// newest release; signed in, also its own and its orgs' while private (no release yet). `q` that
/// is exactly `org/name` is that one model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelSearch {
    pub models: Vec<FoundModel>,
    /// How many models matched, shown or not.
    pub total: i64,
    /// How many matched beyond those shown.
    pub older: i64,
}

/// One model a search found, with its newest release, lanes, author and kind.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FoundModel {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The label its owner gave it, else what its header verifies as.
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_from: Option<FamilyFrom>,
    pub release: Option<String>,
    /// The newest release's lanes; a private model's commits' lanes.
    pub lanes: Vec<String>,
    pub private: bool,
    /// Who made it: the org it credits, or the parked org it mirrors into.
    pub author: Option<ModelAuthor>,
    /// What its header says it is. None: it couldn't be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "kind")]
    pub content_kind: Option<ModelContentKind>,
    /// Stable model identity kind; absent when an older source cannot classify it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_kind: Option<crate::model_ref::ModelKind>,
    /// Its labels, sorted; `nsfw` among them marks one that makes sexual or nude images.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Where a model's family comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum FamilyFrom {
    /// Its owner's label, which its weights don't say (or weren't read).
    Owner,
    /// Its weights: its header verifies as it.
    Weights,
}

/// What a checkpoint is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ModelContentKind {
    /// Only a LoRA's factors: it goes on a model.
    Lora,
    /// A model's weights, its header verified as a family's: it stands in for a model.
    Weights,
    /// Weights no family's model is (a text encoder, a VLM, a VAE, an upsampler, a layout no table reads): a part, never a box's model.
    Component,
}

/// Who made a model: the org it credits, and whether that org is parked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelDetail {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The label its owner gave it, else what its header verifies as.
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_from: Option<FamilyFrom>,
    /// What its header verifies as, whatever its label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights_family: Option<String>,
    /// What its header says it is. None: it couldn't be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "kind")]
    pub content_kind: Option<ModelContentKind>,
    /// Stable model identity kind; absent when an older source cannot classify it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_kind: Option<crate::model_ref::ModelKind>,
    pub created_at: String,
    /// The pinned upstream source.
    pub source: Option<String>,
    pub author: Option<ModelAuthor>,
    /// Words a LoRA's prompt uses, and its recommended strength (a run's default stays 1.0).
    pub trigger: Option<String>,
    pub strength: Option<f64>,
    /// Its labels, sorted: filled once from its source, then its owner's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Newest first.
    pub releases: Vec<ModelRelease>,
    /// Newest first; empty to anyone but the model's members.
    #[serde(alias = "commits")]
    pub checkpoints: Vec<ModelCheckpoint>,
}

/// One release of a model: its version, each lane's commit, and whether it is a mirror.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelRelease {
    pub version: String,
    #[serde(default)]
    pub visibility: crate::model_ref::ReleaseVisibility,
    pub created_at: String,
    /// Published while its org was parked: not its author's.
    pub mirror: bool,
    /// Published by a platform admin where it is no member.
    pub by_admin: bool,
    pub lanes: Vec<ModelLane>,
}

/// One lane of a release: its commit, size, header kinds and provenance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelLane {
    pub lane: String,
    /// `sha256:<hex>`.
    #[serde(alias = "commit")]
    pub checkpoint: String,
    pub bytes: i64,
    /// Its header's schemes and dtypes.
    pub kinds: Vec<String>,
    /// The recipe that wrote it, as its header records.
    pub provenance: Option<Provenance>,
}

/// The recipe that wrote a commit: its header's `metadata.provenance`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

/// One commit of a model: its lane, size and the releases naming it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelCheckpoint {
    #[serde(alias = "commit")]
    pub checkpoint: String,
    pub lane: String,
    pub bytes: i64,
    /// The releases naming it; none while private.
    pub releases: Vec<String>,
    pub created_at: String,
}

/// `PATCH /v1/models/{org}/{name}`: the fields its owner sets, each absent one kept. An empty or `none` family
/// clears the label (its weights' family shows then), an empty trigger clears it, a null strength clears it. `add_tags`
/// are added and `remove_tags` removed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub strength: Option<Option<f64>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_tags: Vec<String>,
}

impl ModelEdit {
    /// Whether it changes a model's tags.
    pub fn tags_change(&self) -> bool {
        !self.add_tags.is_empty() || !self.remove_tags.is_empty()
    }
}

/// A field that is there, null or not.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(field: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(field).map(Some)
}

/// What `PATCH /v1/models/{org}/{name}` leaves: the model as `GET` reads it, and what the Hub clamped.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelEdited {
    pub model: ModelDetail,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `GET /v1/families`: the families TensorFS tells from a header.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Families {
    pub families: Vec<String>,
}

/// `POST /v1/models/{org}/{name}/releases`: one commit per lane, under a version higher than every
/// earlier one; the Hub never reuses a version, which makes a retry safe.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelReleaseAsk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    pub version: String,
    /// `sha256:<hex>`.
    #[serde(alias = "commits")]
    pub checkpoints: Vec<String>,
}

/// The release a publish made: its version and each lane's commit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelReleased {
    pub model: String,
    pub version: String,
    pub created_at: String,
    /// Each lane's commit.
    pub lanes: Vec<(String, String)>,
    /// What the Hub tells its publisher; the release stands regardless.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// A retained model release whose public availability was withdrawn.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelUnpublished {
    pub model: String,
    pub release: String,
    /// Whether this call changed the release's visibility.
    pub unpublished: bool,
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
    #[serde(alias = "commit")]
    pub checkpoint: Option<String>,
}

/// `GET /v1/packages?q=&limit=`: the Hub's packages the caller can see.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PackageSearch {
    pub packages: Vec<FoundPackage>,
    pub total: i64,
    /// How many matched beyond those shown.
    pub older: i64,
}

/// One package a search found, with its newest release.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FoundPackage {
    pub package: String,
    pub latest_release: Option<String>,
    pub created_at: String,
    /// Authoritative update time when the Hub provides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// `GET /v1/packages/{org}/{name}?release=`: a package's releases newest first, or the one named
/// with what its publish warned of (its owner sees the warnings).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PackageDetail {
    /// As published: the Hub finds `qwen3-5` for `qwen3.5`.
    pub package: String,
    pub latest_release: Option<String>,
    pub releases: Vec<PackageRelease>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// One release of a package: when it was published and whether it is yanked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

/// A model's display metadata as set at the Hub: shown in the model browser, never applied (a
/// run's LoRA strength stays 1.0).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Displayed {
    pub model: String,
    pub trigger: Option<String>,
    pub strength: Option<f64>,
    /// Why the Hub did not take them; the upload stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// An older release an install retired, and whether it left the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retired {
    pub package: String,
    pub release: String,
    /// It left the machine; otherwise `reason` says why it stays.
    pub left: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Calcifer's own jobs (cl-841): each is a run, asked as any is, by `POST /v1/runs` of a reserved target of the package `cozy`
/// whose `input` is the record named here, quoted by `POST /v1/run-quotes`, and listed, shown, followed and canceled as a run.
/// A run's `on`, `hub`, `code` (local code, by content) and `agreed` are the request's own.
pub mod job {
    /// `input`: [`super::ModelDownload`]. A warm run that makes a model present in a machine's store.
    pub const MODEL_DOWNLOAD: &str = "cozy/model_download";
    /// `input`: [`super::ModelUpload`]. A warm run that makes the source and uploads it as a private commit; a Hub commit on no
    /// base is recorded at the Hub by Calcifer, with no machine: its result is a [`super::RecordedCheckpoint`].
    pub const MODEL_UPLOAD: &str = "cozy/model_upload";
    /// `input`: [`super::ModelQuantize`]. A run of a package's `quantize`, the request's `code` when it is local code.
    pub const MODEL_QUANTIZE: &str = "cozy/model_quantize";
    /// `input`: [`super::PackageInstall`]. A warm run of a package's environment, no weights; the request's `code` installs
    /// local code as it is now.
    pub const PACKAGE_INSTALL: &str = "cozy/package_install";
    /// `input`: [`super::PackageRemove`]. Calcifer's run: one remove run per package and Hub the machine `on` holds it from,
    /// summed in its result, a [`super::PackagesRemoved`].
    pub const PACKAGE_REMOVE: &str = "cozy/package_remove";
    /// `input`: [`super::PackageLock`], of the request's `code`. Calcifer's run, with uv in its copy; its result is a
    /// [`super::Locked`].
    pub const PACKAGE_LOCK: &str = "cozy/package_lock";
    /// `input`: [`super::PackagePublish`], of the request's `code`. Calcifer's run; its result is a [`super::Published`].
    pub const PACKAGE_PUBLISH: &str = "cozy/package_publish";

    /// Every job, by its target.
    pub const ALL: [&str; 7] = [MODEL_DOWNLOAD, MODEL_UPLOAD, MODEL_QUANTIZE, PACKAGE_INSTALL, PACKAGE_REMOVE, PACKAGE_LOCK, PACKAGE_PUBLISH];
}

/// `cozy/model_download`'s input.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelDownload {
    /// `org/name[@release]`, `org/name@sha256:<commit>`, `hf://…`, `civitai://…`, `https://…`, or
    /// a held `.safetensors` file's `sha256:<hex>` (`PUT /v1/files`).
    pub model: String,
    /// A held file's name where its client has it: the object's name, and its alias's when none is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `local/<alias>`: the name the machine keeps it under (required for a file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// A Hub model's lane; absent, the Hub picks what the machine runs best.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// The source's convention, a reviewed TensorFS format (`h3.lora`); usually detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
}

/// `cozy/model_upload`'s input. Its trigger and strength are set at the Hub once it completes (`result.after.display`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelUpload {
    /// A provider source, `local/<alias>`, a Hub model, a held file's `sha256:<hex>` (`PUT /v1/files`): a `.safetensors` file, or a
    /// CozyTensors folder's tree document, its files held too, or `run:N[#output]`: a run's weights output, uploaded from the
    /// machine that holds it. A Hub commit uploads only on a `base`; else it is recorded, and no machine runs.
    pub source: String,
    /// A held source's name where its client has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
}

/// A Hub commit recorded as a commit of another model when the caller may read its whole closure: nothing uploaded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecordedCheckpoint {
    #[serde(alias = "commit")]
    pub checkpoint: String,
    pub from: String,
    pub lane: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Displayed>,
    /// Commands to run next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// `cozy/package_install`'s input: the Hub's newest release when none is named. A published package's older releases on its
/// machine go once it is in (`result.after.retired`). With the request's `code`, local code, and `package` is empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PackageInstall {
    /// `org/name`; empty with `code`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
}

/// `cozy/package_remove`'s input: every release of each package off the machine the request's `on` names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PackageRemove {
    /// `org/name`, or `local/name`.
    pub packages: Vec<String>,
}

/// `cozy/package_remove`'s result: what left the machine, what that freed, and what did not go.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PackagesRemoved {
    pub machine: String,
    pub removed: Vec<RemovedRelease>,
    pub reclaimed_bytes: u64,
    /// Files a running executor still uses stay until it ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retained: bool,
    /// Each remove run that did not complete, as `package: machine: reason (code)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

/// One release a machine no longer holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RemovedRelease {
    pub package: String,
    pub release: String,
    /// The Hub it was installed from; empty for local code.
    #[serde(default)]
    pub hub: String,
}

/// `cozy/model_quantize`'s input: a new commit of `model` in `lane`, written from a commit (or a release's original weights) by
/// the recipe the package declares for that lane, as a run of its `quantize` on the machine, which uploads it. The package is
/// `package`, or the request's `code`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ModelQuantize {
    /// `org/name@sha256:<commit>`, or `org/name[@release]` (none: the newest).
    pub model: String,
    /// The lane to write: a recipe the package declares for this model.
    pub lane: String,
    /// `org/name[@release]`; empty with `code`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package: String,
}

/// `cozy/package_lock`'s input: the package the request's `code` is, locked for the Hub and the account it publishes to
/// (`root`: the root namespace, Hub admins), with uv in Calcifer's copy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub root: bool,
    /// Dependencies that may move to their newest compatible release.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upgrade: Vec<String>,
}

/// `cozy/package_publish`'s input: the package the request's `code` is, published as `<account>/<name>@<version>` (`root`:
/// `<name>@<version>`): staged, relocked for this Hub, built, described statically, uploaded and finalized. A version already
/// published is answered as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePublish {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub root: bool,
    /// Published yanked: a gated candidate, never the package's newest release.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yanked: bool,
}

/// `cozy/package_lock`'s result: the new `uv.lock` and whether it changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locked {
    /// The new `uv.lock`, for the client to write into the package's folder.
    pub lock: String,
    pub account_index: Option<String>,
    /// It differs from the `uv.lock` sent.
    pub changed: bool,
}

/// `cozy/package_publish`'s result: the release, its status, bytes uploaded and the Hub's warnings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    pub package: String,
    pub release: String,
    /// `published`, `published yanked`, or `already published`.
    pub status: String,
    pub changed: bool,
    pub uploaded_bytes: u64,
    pub hub: String,
    pub warnings: Vec<String>,
    /// The `uv.lock` it made first, the package having none, for the client to write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<String>,
}
