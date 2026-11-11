//! One reviewed operator transaction. No command in the installed Cozy CLI calls this.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use api::config::Origin;
use api::hub::{Client, Credential, types::{ModelCard, ModelRelease}};
use api::model_ref::{ModelKind, ReleaseVisibility};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tensorfs::fetch::{self, Endpoint, Payload, Request, SourcePolicy, TokenProvider};
use tensorfs::format::{Header, ObjectRef, Tree};

const HUB: &str = "https://tensorhub.com";
const MANIFEST_SHA: &str = "1084442c54eeb589ed85e2ba00bad8870ecb1ac16335e7f450433f7c1fc7a7b1";
const H3: &str = "minimaxai/minimax-h3";
const UPSCALER: &str = "lbh-123-ai/h3-latent-upscaler";
const USAGE: &str = "Usage: xs170-artifact-operator [check|admit|publish] --manifest <file> --inputs <inventory.json> --hub https://tensorhub.com --user-home <existing user home> --receipt <new file> [--model <reviewed model>]\ncheck reads both model cards; admit confirms 17 objects and admits six checkpoints; publish requires one --model.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase { Check, Admit, Publish }

#[derive(Debug)]
struct Options {
    phase: Phase,
    manifest: PathBuf,
    inputs: PathBuf,
    user_home: PathBuf,
    receipt: PathBuf,
    model: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct Failure {
    code: &'static str,
    message: String,
    next: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
}
type Result<T> = std::result::Result<T, Failure>;

fn fail(code: &'static str, message: impl Into<String>, next: impl Into<String>) -> Failure {
    Failure { code, message: message.into(), next: next.into(), status: None }
}
fn require(condition: bool, code: &'static str, message: impl Into<String>) -> Result<()> {
    if condition { Ok(()) } else { Err(fail(code, message, "Review the frozen proposal; do not change an existing release")) }
}
fn local_error(message: &'static str) -> Failure { fail("local_input", message, "Check the supplied artifact and receipt paths") }
fn hub_error(error: api::hub::Error, action: &str) -> Failure {
    let (status, code) = match error {
        api::hub::Error::Hub { status: 409, .. } => (Some(409), "immutable_conflict"),
        api::hub::Error::Hub { status, .. } => (Some(status), "hub_refused"),
        api::hub::Error::Credential(_) => (None, "sign_in_unavailable"),
        api::hub::Error::Answer(_) => (None, "hub_answer_invalid"),
        api::hub::Error::Call(_) => (None, "hub_unreachable"),
    };
    Failure { code, status, message: format!("Could not {action}"), next: if code == "immutable_conflict" {
        "Inspect the existing immutable version; do not overwrite it or choose another version automatically".into()
    } else { "Inspect the receipt and the trusted Hub before retrying the same phase".into() } }
}

impl Options {
    fn parse(mut args: Vec<String>) -> Result<Self> {
        let phase = if args.first().is_some_and(|arg| !arg.starts_with('-')) {
            match args.remove(0).as_str() { "check" => Phase::Check, "admit" => Phase::Admit, "publish" => Phase::Publish,
                _ => return Err(fail("usage", "Choose check, admit or publish", USAGE)) }
        } else { Phase::Check };
        let mut values = BTreeMap::new();
        let mut words = args.into_iter();
        while let Some(key) = words.next() {
            if !["--manifest", "--inputs", "--hub", "--user-home", "--receipt", "--model"].contains(&key.as_str()) {
                return Err(fail("usage", "Unknown operator option", USAGE));
            }
            let value = words.next().ok_or_else(|| fail("usage", "An operator option needs a value", USAGE))?;
            if values.insert(key, value).is_some() { return Err(fail("usage", "Do not repeat operator options", USAGE)); }
        }
        let mut needed = |name: &str| values.remove(name).ok_or_else(|| fail("usage", format!("Supply {name}"), USAGE));
        let manifest = PathBuf::from(needed("--manifest")?);
        let inputs = PathBuf::from(needed("--inputs")?);
        let hub = needed("--hub")?;
        let user_home = PathBuf::from(needed("--user-home")?);
        let receipt = PathBuf::from(needed("--receipt")?);
        require(hub == HUB, "hub_not_trusted", "This operator only accepts the explicit trusted TensorHub origin")?;
        let model = values.remove("--model");
        require(user_home.is_absolute(), "usage", "Supply the existing user's absolute home directory")?;
        require((phase == Phase::Publish && model.as_deref().is_some_and(|name| [H3, UPSCALER].contains(&name)))
            || (phase != Phase::Publish && model.is_none()), "usage", "Only publish takes --model, and it must name one of the two reviewed models")?;
        Ok(Self { phase, manifest, inputs, user_home, receipt, model })
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Mapping {
    model: String, lane: String, old_release: String, proposed_release: String,
    old_checkpoint: ObjectRef, new_checkpoint: ObjectRef, old_header: ObjectRef, new_header: ObjectRef,
}
#[derive(Deserialize, Serialize)]
struct Asset { name: String, object: ObjectRef }
#[derive(Deserialize, Serialize)]
struct ObjectAt { object: ObjectRef, path: PathBuf }
#[derive(Deserialize, Serialize)]
struct Manifest {
    format: String, prepared_only: bool, mappings: Vec<Mapping>, processor_assets: Vec<Asset>,
    new_unique_objects: Vec<ObjectAt>, new_unique_object_count: usize, total_new_bytes: u64,
}
#[derive(Deserialize, Serialize)]
struct Original {
    model: String, lane: String, release: String, old_checkpoint: String,
    old_tree: PathBuf, old_tree_length: u64, old_header: PathBuf, old_header_ref: ObjectRef,
}
#[derive(Deserialize, Serialize)]
struct Inventory { inputs: Vec<Original> }
struct Proposal { manifest: Manifest, objects: BTreeMap<ObjectRef, Bytes> }

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|_| local_error("Could not read the artifact input"))?;
    serde_json::from_slice(&bytes).map_err(|_| local_error("The artifact input is not valid JSON"))
}
fn verified(path: &Path, expected: &ObjectRef, root: &Path) -> Result<Vec<u8>> {
    let canonical = path.canonicalize().map_err(|_| local_error("An artifact object is missing"))?;
    require(canonical.starts_with(root), "input_outside_bundle", "An input path leaves its reviewed artifact directory")?;
    let bytes = fs::read(canonical).map_err(|_| local_error("Could not read an artifact object"))?;
    require(ObjectRef::of(&bytes) == *expected, "object_checksum_changed", "An artifact object's digest or length changed")?;
    Ok(bytes)
}
fn format_error() -> Failure { local_error("A checkpoint header or tree is invalid") }

impl Proposal {
    fn read(manifest_path: &Path, inputs_path: &Path, expected_hash: &str) -> Result<Self> {
        let bytes = fs::read(manifest_path).map_err(|_| local_error("Could not read the reviewed manifest"))?;
        require(ObjectRef::of(&bytes).sha256 == expected_hash, "manifest_checksum_changed", "The manifest differs from the reviewed proposal")?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| local_error("The manifest is not valid JSON"))?;
        require(manifest.format == "xs170.required-data-reissue/1" && manifest.prepared_only, "manifest_invalid", "This is not a prepared XS170 proposal")?;
        let base = manifest_path.parent().ok_or_else(|| local_error("The manifest has no artifact directory"))?.canonicalize().map_err(|_| local_error("The artifact directory is missing"))?;
        let mut objects = BTreeMap::new();
        for item in &manifest.new_unique_objects {
            require(item.path == PathBuf::from("objects").join(&item.object.sha256), "object_path_changed", "An object path does not match its content digest")?;
            let bytes = verified(&base.join(&item.path), &item.object, &base)?;
            require(objects.insert(item.object.clone(), Bytes::from(bytes)).is_none(), "object_repeated", "The manifest repeats an upload object")?;
        }
        require(objects.len() == 17 && manifest.new_unique_object_count == 17
            && objects.values().map(|bytes| bytes.len() as u64).sum::<u64>() == manifest.total_new_bytes,
            "object_set_changed", "The reviewed set must contain exactly 17 new objects")?;
        let expected: BTreeSet<_> = ["bf16", "bf16-full", "fp8", "int8", "nvfp4"].into_iter().collect();
        let lanes: BTreeSet<_> = manifest.mappings.iter().filter(|row| row.model == H3).map(|row| row.lane.as_str()).collect();
        require(manifest.mappings.len() == 6 && lanes == expected
            && manifest.mappings.iter().filter(|row| row.model == UPSCALER && row.lane == "bf16").count() == 1,
            "mapping_set_changed", "The proposal must contain the five H3 lanes and one upscaler lane")?;
        let inventory: Inventory = read_json(inputs_path)?;
        require(inventory.inputs.len() == 6, "input_set_changed", "The original inventory must contain six checkpoints")?;
        let inputs_root = inputs_path.parent().ok_or_else(|| local_error("The inventory has no input directory"))?.canonicalize().map_err(|_| local_error("The input directory is missing"))?;
        let mut expected_objects: BTreeSet<_> = manifest.processor_assets.iter().map(|asset| asset.object.clone()).collect();
        require(expected_objects.len() == 5, "processor_set_changed", "The proposal must contain five shared processor assets")?;
        for mapping in &manifest.mappings {
            let original = inventory.inputs.iter().find(|row| row.model == mapping.model && row.lane == mapping.lane).ok_or_else(|| local_error("The original inventory does not match a proposed lane"))?;
            require(original.release == mapping.old_release && original.old_checkpoint == mapping.old_checkpoint.id()
                && original.old_tree_length == mapping.old_checkpoint.length && original.old_header_ref == mapping.old_header,
                "original_mapping_changed", "The original inventory differs from the reviewed model mapping")?;
            let old_header_bytes = verified(&original.old_header, &mapping.old_header, &inputs_root)?;
            let old_header = Header::from_cbor(&old_header_bytes).map_err(|_| format_error())?;
            let old_tree_bytes = verified(&original.old_tree, &mapping.old_checkpoint, &inputs_root)?;
            let old_tree = Tree::from_json(&old_tree_bytes).map_err(|_| format_error())?;
            require(old_tree.lane == mapping.lane && old_tree.header().map_err(|_| format_error())?.1 == &mapping.old_header,
                "old_tree_changed", "The original tree differs from the reviewed lane or header")?;
            let new_header_bytes = objects.get(&mapping.new_header).ok_or_else(|| local_error("A new header object is missing"))?;
            let new_header = Header::from_cbor(new_header_bytes).map_err(|_| format_error())?;
            let new_tree_bytes = objects.get(&mapping.new_checkpoint).ok_or_else(|| local_error("A new tree object is missing"))?;
            let mut new_tree = Tree::from_json(new_tree_bytes).map_err(|_| format_error())?;
            let (path, header) = new_tree.header().map_err(|_| format_error())?;
            require(new_tree.lane == mapping.lane && header == &mapping.new_header, "new_tree_changed", "The new tree differs from the reviewed lane or header")?;
            let header_path = path.to_owned();
            new_tree.files.insert(header_path, mapping.old_header.clone());
            require(new_tree.to_json().map_err(|_| format_error())? == old_tree_bytes, "supporting_files_changed", "The proposed tree changed an existing supporting file")?;
            let mut restored = new_header.components().to_vec();
            match mapping.model.as_str() {
                H3 => {
                    require(mapping.old_release == "1.0.0-rc.7" && mapping.proposed_release == "1.0.0-rc.8" && restored.len() == 5,
                        "h3_mapping_changed", "The H3 proposal differs from the reviewed release or component layout")?;
                    let encoder = restored.iter_mut().find(|component| component.name == "text_encoder").ok_or_else(|| format_error())?;
                    for asset in &manifest.processor_assets {
                        require(encoder.assets.remove(&asset.name) == Some(asset.object.clone()), "processor_changed", "A proposed processor asset differs from the reviewed object")?;
                    }
                }
                UPSCALER => {
                    require(mapping.old_release == "0.1.0" && mapping.proposed_release == "0.1.1" && restored.len() == 2,
                        "upscaler_mapping_changed", "The upscaler proposal differs from the reviewed release or component layout")?;
                    let model = restored.iter_mut().find(|component| component.name == "model").ok_or_else(|| format_error())?;
                    require(model.config.remove("latent_normalization").is_some(), "normalization_missing", "The new upscaler lacks learned normalization")?;
                }
                _ => return Err(local_error("The proposal names an unreviewed model")),
            }
            let restored = Header::new(new_header.chunks.clone(), new_header.pipeline().cloned(), restored, new_header.metadata.clone()).map_err(|_| format_error())?;
            require(restored.to_cbor().map_err(|_| format_error())? == old_header_bytes && old_header.to_cbor().map_err(|_| format_error())? == old_header_bytes,
                "existing_model_data_changed", "The proposed checkpoint changed existing model data")?;
            expected_objects.insert(mapping.new_header.clone()); expected_objects.insert(mapping.new_checkpoint.clone());
        }
        require(expected_objects == objects.keys().cloned().collect(), "unexpected_upload_object", "The upload set contains an unexpected object")?;
        Ok(Self { manifest, objects })
    }
    fn models(&self, selected: Option<&str>) -> BTreeMap<String, Vec<&Mapping>> {
        let mut groups = BTreeMap::new();
        for row in &self.manifest.mappings {
            if selected.is_none_or(|model| row.model == model) { groups.entry(row.model.clone()).or_insert_with(Vec::new).push(row); }
        }
        groups
    }
}

#[derive(Serialize)]
struct Receipt {
    format: &'static str, phase: Phase, hub: &'static str, manifest_sha256: String,
    selected_model: Option<String>, succeeded: bool, counts: Counts, events: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Failure>,
}
#[derive(Default, Serialize)]
struct Counts { objects_confirmed: usize, object_payloads_sent: usize, checkpoints_admitted: usize, release_puts_confirmed: usize, public_release_mappings_verified: usize }
struct Journal { path: PathBuf, data: Receipt }
impl Journal {
    fn new(path: &Path, phase: Phase, selected: Option<String>) -> Result<Self> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(|_| fail("receipt_unavailable", "The receipt path exists or cannot be created", "Choose a new receipt file in an existing directory"))?;
        platform::durable::set_file_mode(&file, 0o600).map_err(|_| local_error("Could not protect the receipt file"))?;
        let data = Receipt { format: "xs170.artifact-operator-receipt/1", phase, hub: HUB, manifest_sha256: MANIFEST_SHA.into(), selected_model: selected, succeeded: false, counts: Counts::default(), events: vec![], error: None };
        file.write_all(&serde_json::to_vec_pretty(&data).map_err(|_| local_error("Could not encode the receipt"))?).map_err(|_| local_error("Could not write the receipt"))?;
        file.sync_all().map_err(|_| local_error("Could not flush the receipt"))?;
        Ok(Self { path: path.into(), data })
    }
    fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.data).map_err(|_| local_error("Could not encode the receipt"))?;
        platform::durable::replace(&self.path, &bytes, 0o600).map_err(|_| fail("receipt_write_failed", "Could not save the receipt after an operator step", "Inspect the trusted Hub and the partial receipt before retrying"))
    }
    fn note(&mut self, event: Value) -> Result<()> {
        match event.get("step").and_then(Value::as_str) {
            Some("object_confirmed") => { self.data.counts.objects_confirmed += 1; self.data.counts.object_payloads_sent += usize::from(event["payload_sent"].as_bool() == Some(true)); }
            Some("checkpoint_admitted") => self.data.counts.checkpoints_admitted += 1,
            Some("release_mapping_confirmed") => self.data.counts.release_puts_confirmed += 1,
            Some("public_mapping_verified") => self.data.counts.public_release_mappings_verified += 1,
            _ => {}
        }
        self.data.events.push(event); self.save()
    }
}

fn same_version(left: &str, right: &str) -> bool {
    left == right || left.parse::<pep440_rs::Version>().ok().zip(right.parse::<pep440_rs::Version>().ok()).is_some_and(|(left, right)| left == right)
}
fn matches_ref(actual: &api::hub::types::ObjectRef, expected: &ObjectRef) -> bool {
    actual.sha256 == expected.sha256 && u64::try_from(actual.length).ok() == Some(expected.length)
}
fn verify_release(release: &ModelRelease, rows: &[&Mapping], new: bool) -> Result<()> {
    require(release.visibility == ReleaseVisibility::Public && release.lanes.len() == rows.len(), "release_mapping_changed", "A model release is private or has different lanes")?;
    for row in rows {
        let lane = release.lanes.get(&row.lane).ok_or_else(|| fail("release_mapping_changed", "A reviewed lane is absent", "Inspect the current model card before continuing"))?;
        let (checkpoint, header) = if new { (&row.new_checkpoint, &row.new_header) } else { (&row.old_checkpoint, &row.old_header) };
        require(lane.checkpoint == checkpoint.id() && u64::try_from(lane.length).ok() == Some(checkpoint.length) && matches_ref(&lane.header, header),
            "release_mapping_changed", "A model release differs from the reviewed checkpoint and header mapping")?;
    }
    Ok(())
}
fn verify_card(card: &ModelCard, model: &str, rows: &[&Mapping], admitted: bool) -> Result<()> {
    require(format!("{}/{}", card.model.org, card.model.name) == model && card.model.kind == Some(ModelKind::Base),
        "model_identity_changed", "A model identity or stable kind differs from the reviewed proposal")?;
    let old = card.releases.iter().find(|release| same_version(&release.version, &rows[0].old_release)).ok_or_else(|| fail("source_release_missing", "The old public release is unavailable", "Inspect the current model card before continuing"))?;
    verify_release(old, rows, false)?;
    require(!card.releases.iter().any(|release| same_version(&release.version, &rows[0].proposed_release)),
        "proposed_version_exists", "A proposed release version already exists",)?;
    if admitted {
        for row in rows {
            let checkpoint = card.checkpoints.iter().find(|item| item.id == row.new_checkpoint.id()).ok_or_else(|| fail("checkpoint_not_admitted", "A proposed checkpoint is not admitted", "Complete and review admit before publishing this model"))?;
            require(checkpoint.lane == row.lane && u64::try_from(checkpoint.length).ok() == Some(row.new_checkpoint.length) && matches_ref(&checkpoint.header, &row.new_header),
                "admission_mapping_changed", "An admitted checkpoint differs from the reviewed lane or header")?;
        }
    }
    Ok(())
}
async fn preconditions(client: &Client, proposal: &Proposal, selected: Option<&str>, admitted: bool, journal: &mut Journal) -> Result<()> {
    for (model, rows) in proposal.models(selected) {
        let (org, name) = model.split_once('/').ok_or_else(|| local_error("A model name is invalid"))?;
        let card = client.get_model(org, name).await.map_err(|error| hub_error(error, "read the current authenticated model card"))?;
        verify_card(&card, &model, &rows, admitted)?;
        journal.note(json!({"step":"preconditions", "model":model,"old_release":rows[0].old_release,"proposed_release":rows[0].proposed_release,"proposed_version_absent":true,"old_public_mapping_verified":true,"new_admissions_verified":admitted}))?;
    }
    Ok(())
}

struct SignedUser(Client);
impl TokenProvider for SignedUser {
    fn headers<'a>(&'a self, _: &'a Request<'a>) -> fetch::Boxed<'a, tensorfs::err::Result<Vec<(String, String)>>> {
        Box::pin(async move {
            let Credential::User(session) = self.0.credential() else {
                return Err(tensorfs::err::Refusal { code: tensorfs::err::Code::CREDENTIAL_REQUIRED, detail: "An existing user sign-in is required".into() });
            };
            let bearer = session.bearer(&self.0.anonymous()).await.map_err(|_| tensorfs::err::Refusal { code: tensorfs::err::Code::CREDENTIAL_REQUIRED, detail: "The existing user sign-in is unavailable".into() })?;
            Ok(vec![("authorization".into(), format!("Bearer {}", bearer.token))])
        })
    }
}
fn connection(user_home: &Path) -> Result<(Client, Endpoint)> {
    let origin = Origin::parse(HUB).map_err(|_| local_error("The fixed trusted Hub origin is invalid"))?;
    let http = api::hub::http(None).map_err(|_| fail("connection_unavailable", "Could not configure the trusted Hub connection", "Check the local TLS installation"))?;
    let client = Client::signed_in(&origin, &platform::paths::Home::under(user_home), http, || "cozy login <email>".into()).map_err(|_| fail("sign_in_unavailable", "The existing Cozy sign-in is unavailable", "Run cozy login <email> before invoking this operator"))?;
    let policy = SourcePolicy { any_host: true, allow_local: false, max_redirects: 5, ..Default::default() };
    let endpoint = Endpoint::new(&fetch::Client::new(policy), HUB, Some(Arc::new(SignedUser(client.clone())))).map_err(|_| fail("connection_unavailable", "Could not configure the object upload connection", "Inspect the trusted Hub connection"))?;
    Ok((client, endpoint))
}

async fn execute(phase: Phase, proposal: &Proposal, selected: Option<&str>, client: &Client, endpoint: &Endpoint, journal: &mut Journal) -> Result<()> {
    preconditions(client, proposal, selected, phase == Phase::Publish, journal).await?;
    match phase {
        Phase::Check => {}
        Phase::Admit => {
            let assets: BTreeSet<_> = proposal.manifest.processor_assets.iter().map(|asset| asset.object.clone()).collect();
            let headers: BTreeSet<_> = proposal.manifest.mappings.iter().map(|row| row.new_header.clone()).collect();
            let trees: BTreeSet<_> = proposal.manifest.mappings.iter().map(|row| row.new_checkpoint.clone()).collect();
            for (stage, objects) in [("assets", assets), ("headers", headers), ("trees", trees)] {
                for object in objects {
                    let bytes = proposal.objects.get(&object).ok_or_else(|| local_error("An upload payload is missing"))?.clone();
                    let payload = move || Ok(Payload::Bytes(bytes.clone()));
                    let sent = endpoint.upload(&object, &payload).await.map_err(|_| fail("object_upload_failed", format!("Could not confirm object {}", object.sha256), "Inspect the partial receipt and retry admit after resolving the refusal"))?;
                    journal.note(json!({"step":"object_confirmed","stage":stage,"object":object,"payload_sent":sent}))?;
                }
            }
            for row in &proposal.manifest.mappings {
                let (org, name) = row.model.split_once('/').ok_or_else(|| local_error("A model name is invalid"))?;
                let length = i64::try_from(row.new_checkpoint.length).map_err(|_| local_error("A checkpoint length is too large"))?;
                let accepted = client.create_model_checkpoint(org, name, &row.new_checkpoint.id(), length).await.map_err(|error| hub_error(error, "admit the reviewed checkpoint"))?;
                require(accepted.id == row.new_checkpoint.id() && accepted.lane == row.lane && accepted.length == length && matches_ref(&accepted.header, &row.new_header),
                    "admission_answer_changed", "The Hub's admission answer differs from the reviewed checkpoint")?;
                journal.note(json!({"step":"checkpoint_admitted","model":row.model,"lane":row.lane,"checkpoint":row.new_checkpoint,"header":row.new_header}))?;
            }
            preconditions(client, proposal, None, true, journal).await?;
        }
        Phase::Publish => {
            let model = selected.ok_or_else(|| local_error("Publish needs one reviewed model"))?;
            let rows = proposal.models(Some(model)).remove(model).ok_or_else(|| local_error("The selected model is absent from the proposal"))?;
            let (org, name) = model.split_once('/').ok_or_else(|| local_error("A model name is invalid"))?;
            let checkpoints: Vec<_> = rows.iter().map(|row| row.new_checkpoint.id()).collect();
            let result = client.publish_model_release(org, name, &rows[0].proposed_release, &checkpoints).await.map_err(|error| hub_error(error, "publish the reviewed immutable release"))?;
            require(same_version(&result.version, &rows[0].proposed_release), "publish_answer_changed", "The Hub returned a different release version")?;
            verify_release(&result, &rows, true)?;
            journal.note(json!({"step":"release_mapping_confirmed","model":model,"release":rows[0].proposed_release,"checkpoints":checkpoints,"created_claimed":false}))?;
            let card = client.get_model(org, name).await.map_err(|error| hub_error(error, "verify the published model card"))?;
            let old = card.releases.iter().find(|release| same_version(&release.version, &rows[0].old_release)).ok_or_else(|| local_error("The old release is absent after publication"))?;
            verify_release(old, &rows, false)?;
            let new = card.releases.iter().find(|release| same_version(&release.version, &rows[0].proposed_release)).ok_or_else(|| fail("published_release_missing", "The new release is absent from the model card", "Inspect the trusted Hub before retrying"))?;
            verify_release(new, &rows, true)?;
            journal.note(json!({"step":"public_mapping_verified","model":model,"release":rows[0].proposed_release,"old_public_mapping_preserved":true}))?;
        }
    }
    Ok(())
}

async fn run(options: Options) -> Result<()> {
    let mut journal = Journal::new(&options.receipt, options.phase, options.model.clone())?;
    let result = async {
        let proposal = Proposal::read(&options.manifest, &options.inputs, MANIFEST_SHA)?;
        journal.note(json!({"step":"local_proposal_verified","objects":proposal.objects.len(),"mapping_count":proposal.manifest.mappings.len(),"bytes":proposal.manifest.total_new_bytes,"mappings":proposal.manifest.mappings}))?;
        let (client, endpoint) = connection(&options.user_home)?;
        execute(options.phase, &proposal, options.model.as_deref(), &client, &endpoint, &mut journal).await
    }.await;
    match &result { Ok(()) => journal.data.succeeded = true, Err(error) => journal.data.error = Some(error.clone()) }
    journal.save()?;
    result
}

pub async fn entry() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") { println!("{USAGE}"); return ExitCode::SUCCESS; }
    let options = match Options::parse(args) { Ok(options) => options, Err(error) => { eprintln!("error: {}\nnext: {}", error.message, error.next); return ExitCode::from(2); } };
    let phase = options.phase;
    let receipt = options.receipt.clone();
    match run(options).await {
        Ok(()) => { println!("{phase:?} completed. Receipt: {}", receipt.display()); ExitCode::SUCCESS }
        Err(error) => {
            eprintln!("error: {}{}\nnext: {}", error.message, error.status.map(|status| format!(" (HTTP {status})")).unwrap_or_default(), error.next);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
