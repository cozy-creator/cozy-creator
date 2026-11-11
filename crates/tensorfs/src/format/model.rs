//! Model documents: `cozytensors.tree/1`, a checkpoint (one lane, one cozytensors file and the model
//! files beside it), and `cozytensors.repo/1`, a model's releases and checkpoints. Both parse through a
//! `Wire` twin that carries the `format` tag. The legacy repo/1 tag and commits field remain
//! byte-compatible until a coordinated format migration; Rust uses Model and checkpoints.

use std::collections::BTreeMap;
use std::str::FromStr;

use pep440_rs::Version;
use serde::{Deserialize, Serialize};

use super::doc::{path, segment};
use super::{check_format, invalid, is_header, json, Error, ObjectRef, Result};
use super::{HEADER_PATH, LEGACY_MODEL_FORMAT, MODEL_FORMAT, TREE_FORMAT};

/// The folder that holds a plain folder's chunks and assets: no model file lives under it.
pub const CHUNKS_DIR: &str = "chunks";

/// `cozytensors.tree/1`: a checkpoint, one immutable state of a model. Its `lane` is the label its
/// author gave it (`bf16`, `fp8`, `int8-convrot`, …), read only by a consumer asking for that
/// label. Its one `.cozytensors` entry is the CAS header. Required model data belongs to that
/// header's closure; other entries are ancillary files such as README and attribution.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "TreeWire", into = "TreeWire")]
pub struct Tree {
    pub lane: String,
    pub files: BTreeMap<String, ObjectRef>,
}

#[derive(Serialize, Deserialize)]
struct TreeWire {
    format: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    lane: String,
    files: BTreeMap<String, ObjectRef>,
}

impl TryFrom<TreeWire> for Tree {
    type Error = Error;
    fn try_from(w: TreeWire) -> Result<Self> {
        check_format(&w.format, TREE_FORMAT)?;
        for p in w.files.keys() {
            check_file(p)?;
        }
        Ok(Tree {
            lane: w.lane,
            files: w.files,
        })
    }
}

impl From<Tree> for TreeWire {
    fn from(t: Tree) -> Self {
        TreeWire {
            format: TREE_FORMAT.into(),
            lane: t.lane,
            files: t.files,
        }
    }
}

/// A model file path: plain segments, not under [`CHUNKS_DIR`].
pub fn check_file(p: &str) -> Result<()> {
    if !path(p) || p.split('/').next() == Some(CHUNKS_DIR) {
        return invalid(format!("file path {p:?} is not a model file path"));
    }
    Ok(())
}

/// A lane or release label: alphanumeric first, then at most 63 of `A-Za-z0-9._+!-`.
pub fn label(text: &str) -> bool {
    let b = text.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._+!-".contains(c))
}

impl Tree {
    /// A checkpoint of `header` at [`HEADER_PATH`] in lane `lane` (empty: unlabelled, a machine's
    /// own), beside `files` (their own `.cozytensors` files dropped: a checkpoint holds one).
    pub fn of(lane: &str, header: ObjectRef, files: &BTreeMap<String, ObjectRef>) -> Result<Tree> {
        if !lane.is_empty() && !label(lane) {
            return invalid(format!(
                "lane {lane:?}: start alphanumeric, then at most 63 of letters, digits, . _ + ! -"
            ));
        }
        let mut files: BTreeMap<String, ObjectRef> = files
            .iter()
            .filter(|(p, _)| !is_header(p))
            .map(|(p, r)| (p.clone(), r.clone()))
            .collect();
        files.insert(HEADER_PATH.into(), header);
        Ok(Tree {
            lane: lane.into(),
            files,
        })
    }

    pub fn from_json(bytes: &[u8]) -> Result<Tree> {
        json::read(bytes, TREE_FORMAT)
    }
    pub fn to_json(&self) -> Result<Vec<u8>> {
        json::write(self)
    }

    /// The checkpoint's one cozytensors file: its path and header.
    pub fn header(&self) -> Result<(&str, &ObjectRef)> {
        let mut headers = self.files.iter().filter(|(p, _)| is_header(p));
        match (headers.next(), headers.next()) {
            (Some((path, header)), None) => Ok((path, header)),
            (None, _) => invalid("the checkpoint holds no .cozytensors file"),
            (Some(_), Some(_)) => {
                invalid("the checkpoint holds several .cozytensors files; a checkpoint holds one")
            }
        }
    }

    /// The model files: everything but the header.
    pub fn repo_files(&self) -> impl Iterator<Item = (&str, &ObjectRef)> {
        (self.files.iter())
            .filter(|(p, _)| !is_header(p))
            .map(|(p, r)| (p.as_str(), r))
    }
}

/// A model's releases and checkpoints. New model/1 documents require explicit release
/// visibility; legacy repo/1 bytes keep their historical public-release interpretation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "ModelWire", into = "ModelWire")]
pub struct Model {
    pub name: String,
    pub releases: Vec<Release>,
    pub checkpoints: Vec<ObjectRef>,
    pub deleted: Vec<String>,
    modern: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseVisibility {
    #[default]
    Public,
    Private,
}

/// A release selecting one checkpoint per lane, and its catalog visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub lanes: BTreeMap<String, ObjectRef>,
    pub visibility: ReleaseVisibility,
}

#[derive(Serialize, Deserialize)]
struct ModelWire {
    format: String,
    name: String,
    #[serde(default)]
    releases: Vec<ReleaseWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commits: Option<Vec<ObjectRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoints: Option<Vec<ObjectRef>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deleted: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct ReleaseWire {
    version: String,
    lanes: BTreeMap<String, ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<ReleaseVisibility>,
}

impl TryFrom<ModelWire> for Model {
    type Error = Error;
    fn try_from(w: ModelWire) -> Result<Self> {
        let modern = check_format(&w.format, MODEL_FORMAT).is_ok();
        if !modern {
            check_format(&w.format, LEGACY_MODEL_FORMAT)?;
        }
        if (modern && w.commits.is_some()) || (!modern && w.checkpoints.is_some()) {
            return invalid("model document uses the other format's checkpoint field");
        }
        let mut releases = Vec::new();
        for release in w.releases {
            if modern && release.visibility.is_none() {
                return invalid(format!(
                    "{}@{}: model/1 release requires visibility public or private",
                    w.name, release.version
                ));
            }
            if !label(&release.version) || !release.lanes.keys().all(|lane| label(lane)) {
                tracing::warn!(
                    "{}: release {:?} or one of its lanes is not a label: skipped",
                    w.name,
                    release.version
                );
                continue;
            }
            releases.push(Release {
                version: release.version,
                lanes: release.lanes,
                visibility: release.visibility.unwrap_or_default(),
            });
        }
        let model = Model {
            name: w.name,
            releases,
            checkpoints: if modern { w.checkpoints } else { w.commits }.unwrap_or_default(),
            deleted: w.deleted,
            modern,
        };
        if !model_name(&model.name) {
            return invalid(format!("model name {:?} is not <org>/<name>", model.name));
        }
        Ok(model)
    }
}

impl From<Model> for ModelWire {
    fn from(model: Model) -> Self {
        let modern = model.modern
            || model
                .releases
                .iter()
                .any(|r| r.visibility == ReleaseVisibility::Private);
        ModelWire {
            format: if modern {
                MODEL_FORMAT
            } else {
                LEGACY_MODEL_FORMAT
            }
            .into(),
            name: model.name,
            releases: model
                .releases
                .into_iter()
                .map(|r| ReleaseWire {
                    version: r.version,
                    lanes: r.lanes,
                    visibility: modern.then_some(r.visibility),
                })
                .collect(),
            commits: (!modern).then(|| model.checkpoints.clone()),
            checkpoints: modern.then_some(model.checkpoints),
            deleted: model.deleted,
        }
    }
}

/// The root namespace's org (Tensorhub's first-party models): a name with no org, `sdxl`, is
/// `_/sdxl` in a reference, a model key and a URL.
pub const ROOT_ORG: &str = "_";

/// A model name: `<org>/<name>`, two segments.
pub fn model_name(name: &str) -> bool {
    name.split_once('/')
        .is_some_and(|(org, name)| segment(org) && segment(name))
}

impl Model {
    pub fn new(name: &str) -> Model {
        Model {
            name: name.into(),
            ..Model::default()
        }
    }
    pub(crate) fn from_json(bytes: &[u8]) -> Result<Model> {
        match json::read(bytes, MODEL_FORMAT) {
            Err(Error::Format { .. }) => json::read(bytes, LEGACY_MODEL_FORMAT),
            parsed => parsed,
        }
    }
    /// The canonical bytes; refuses a document a reader would refuse.
    pub(crate) fn to_json(&self) -> Result<Vec<u8>> {
        self.check()?;
        json::write(self)
    }
    fn check(&self) -> Result<()> {
        if !model_name(&self.name) {
            return invalid(format!("model name {:?} is not <org>/<name>", self.name));
        }
        for release in &self.releases {
            if !label(&release.version) || release.lanes.keys().any(|lane| !label(lane)) {
                return invalid(format!(
                    "release {:?} or one of its lanes is not a label",
                    release.version
                ));
            }
        }
        Ok(())
    }

    /// Release `version`.
    pub(crate) fn release(&self, version: &str) -> Option<&Release> {
        self.releases.iter().find(|r| r.version == version)
    }

    /// Records `checkpoint` as `version`'s lane `lane`, the release kept in version order.
    pub fn set_lane(&mut self, version: &str, lane: &str, checkpoint: ObjectRef) {
        match self.releases.iter_mut().find(|r| r.version == version) {
            Some(release) => drop(release.lanes.insert(lane.into(), checkpoint)),
            None => {
                let release = Release {
                    version: version.into(),
                    lanes: BTreeMap::from([(lane.into(), checkpoint)]),
                    visibility: ReleaseVisibility::Public,
                };
                let after = |r: &Release| order(&r.version) > order(version);
                let at = self
                    .releases
                    .iter()
                    .position(after)
                    .unwrap_or(self.releases.len());
                self.releases.insert(at, release);
            }
        }
    }

    /// Records `checkpoint` as one of the model's, the newest.
    pub fn add_checkpoint(&mut self, checkpoint: ObjectRef) {
        self.checkpoints.retain(|c| *c != checkpoint);
        self.checkpoints.push(checkpoint);
    }

    /// The checkpoint `release` (none: the newest) names in `lane` (none: its only lane), as
    /// (release, lane, checkpoint). A model without releases (a machine's own model) answers its newest
    /// checkpoint, in no release. Refused naming what the model has.
    pub fn resolve(
        &self,
        release: Option<&str>,
        lane: Option<&str>,
    ) -> Result<(String, Option<String>, ObjectRef)> {
        if self.releases.is_empty() && release.is_none() {
            return match self.checkpoints.last() {
                Some(checkpoint) => {
                    Ok((String::new(), lane.map(str::to_owned), checkpoint.clone()))
                }
                None => invalid(format!("{} has no checkpoint", self.name)),
            };
        }
        let found = match release {
            Some(version) => self.release(version),
            None => self
                .releases
                .iter()
                .rev()
                .find(|r| r.visibility == ReleaseVisibility::Public),
        };
        let Some(found) = found else {
            let has: Vec<&str> = self.releases.iter().map(|r| r.version.as_str()).collect();
            return invalid(format!(
                "{} has no release {} (has {has:?})",
                self.name,
                release.unwrap_or("at all")
            ));
        };
        let picked = match lane {
            Some(lane) => found.lanes.get_key_value(lane),
            None if found.lanes.len() == 1 => found.lanes.iter().next(),
            None => None,
        };
        match picked {
            Some((lane, checkpoint)) => Ok((
                found.version.clone(),
                Some(lane.clone()),
                checkpoint.clone(),
            )),
            None => {
                let has: Vec<&str> = found.lanes.keys().map(String::as_str).collect();
                let wanted = lane.map_or_else(
                    || "name a lane".to_owned(),
                    |lane| format!("has no lane {lane:?}"),
                );
                invalid(format!(
                    "{}@{}: {wanted} (lanes {has:?})",
                    self.name, found.version
                ))
            }
        }
    }

    /// Every checkpoint the model keeps alive, each once.
    pub(crate) fn roots(&self) -> Vec<&ObjectRef> {
        let mut roots: Vec<&ObjectRef> = self
            .releases
            .iter()
            .flat_map(|r| r.lanes.values())
            .chain(&self.checkpoints)
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }
}

#[cfg(test)]
mod compatibility_tests {
    #[test]
    fn model_names_preserve_legacy_persisted_bytes() {
        let legacy =
            br#"{"commits":[],"format":"cozytensors.repo/1","name":"fixture/model","releases":[]}"#;
        let mut model = super::Model::from_json(legacy).unwrap();
        assert!(model.checkpoints.is_empty());
        assert_eq!(model.to_json().unwrap(), legacy);
        let checkpoint = crate::format::ObjectRef::of(b"checkpoint");
        model.add_checkpoint(checkpoint.clone());
        let bytes = model.to_json().unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["format"], "cozytensors.repo/1");
        assert!(wire.get("checkpoints").is_none());
        assert_eq!(
            super::Model::from_json(&bytes).unwrap().checkpoints,
            [checkpoint]
        );
    }

    #[test]
    fn modern_model_rewrites_preserve_private_history_and_deleted_versions() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(dir.path()).unwrap();
        let (mut header, weights, tokenizer) = crate::store::file::tests::fixture();
        store.put_bytes(&weights).unwrap();
        store.put_bytes(&tokenizer).unwrap();
        let header_id = store.put_bytes(&header.to_cbor().unwrap()).unwrap();
        let public = store.put_tree(&super::Tree::of("fp16", header_id, &Default::default()).unwrap()).unwrap();
        header.metadata.insert("fixture_state".into(), crate::format::Value::Str("private".into()));
        let header_id = store.put_bytes(&header.to_cbor().unwrap()).unwrap();
        let private = store.put_tree(&super::Tree::of("fp16", header_id, &Default::default()).unwrap()).unwrap();
        // Same fields and visibility semantics as the Hub's ModelDoc model/1 fixture.
        let wire = serde_json::json!({"format":"cozytensors.model/1", "name":"fixture/model",
            "checkpoints":[], "deleted":["burnedVersion"], "releases":[
                {"version":"1.0", "visibility":"public", "lanes":{"fp16":public}},
                {"version":"2.0", "visibility":"private", "lanes":{"fp16":private}}]});
        let mut model = super::Model::from_json(&serde_json::to_vec(&wire).unwrap()).unwrap();
        assert_eq!(model.resolve(None, None).unwrap().2, public);
        assert_eq!(model.resolve(Some("2.0"), None).unwrap().2, private);
        assert!(model.roots().contains(&&private));
        model.add_checkpoint(public.clone());
        let saved: serde_json::Value = serde_json::from_slice(&model.to_json().unwrap()).unwrap();
        assert_eq!(saved["format"], "cozytensors.model/1");
        assert_eq!(saved["releases"], wire["releases"]);
        assert_eq!(saved["deleted"], wire["deleted"]);
        assert!(saved.get("commits").is_none());

        store.update_ref("fixture/model", |_| Some(model)).unwrap();
        let mut model = store.get_ref("fixture/model").unwrap().unwrap();
        model.set_lane("3.0", "fp16", public);
        store.update_ref("fixture/model", |_| Some(model)).unwrap();
        let restored = store.get_ref("fixture/model").unwrap().unwrap();
        assert_eq!(
            restored.releases[1].visibility,
            super::ReleaseVisibility::Private
        );
        assert_eq!(restored.deleted, ["burnedVersion"]);
    }

    #[test]
    fn modern_model_visibility_is_required_and_unknown_formats_refuse() {
        let object = crate::format::ObjectRef::of(b"fixture");
        let mut wire = serde_json::json!({"format":"cozytensors.model/1", "name":"fixture/model", "checkpoints":[],
            "releases":[{"version":"1.0", "lanes":{"fp16":object}}]});
        assert!(super::Model::from_json(&serde_json::to_vec(&wire).unwrap()).is_err());
        wire["releases"][0]["visibility"] = "unknown".into();
        assert!(super::Model::from_json(&serde_json::to_vec(&wire).unwrap()).is_err());
        wire["releases"][0]["visibility"] = "private".into();
        let model = super::Model::from_json(&serde_json::to_vec(&wire).unwrap()).unwrap();
        assert!(model.resolve(None, None).is_err());
        wire["format"] = "cozytensors.model/2".into();
        assert!(super::Model::from_json(&serde_json::to_vec(&wire).unwrap()).is_err());
    }
}

/// The one "newest" ordering: of `items`, the one whose `version` is the highest final PEP 440
/// version (a post-release is final), else the highest pre-release; of equal versions (`1.0`,
/// `1.0.0`), the last given. Items whose version does not parse are skipped.
pub fn newest_by<T>(items: impl IntoIterator<Item = T>, version: impl Fn(&T) -> &str) -> Option<T> {
    let versioned = items
        .into_iter()
        .filter_map(|item| Some((Version::from_str(version(&item)).ok()?, item)));
    let order = |(a, _): &(Version, T), (b, _): &(Version, T)| {
        a.is_stable().cmp(&b.is_stable()).then_with(|| a.cmp(b))
    };
    versioned.max_by(order).map(|(_, item)| item)
}

/// A version's order (spec §7): PEP 440, an unparsable one first.
fn order(version: &str) -> Option<Version> {
    Version::from_str(version).ok()
}
