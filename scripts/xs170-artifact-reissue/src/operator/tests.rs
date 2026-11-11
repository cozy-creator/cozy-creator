use super::*;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, head, post, put},
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tempfile::TempDir;
use tensorfs::format::{
    Body as TensorBody, Component, Dtype, Location, Tensor, Value as ModelValue,
};

fn fixture_bundle(root: &Path) -> (PathBuf, PathBuf, String) {
    let inputs = root.join("inputs");
    let prepared = root.join("prepared");
    fs::create_dir_all(&inputs).unwrap();
    fs::create_dir_all(prepared.join("objects")).unwrap();
    let chunk = ObjectRef::of(&[0, 0, 0, 0]);
    let mut objects = BTreeMap::new();
    let mut assets = vec![];
    for index in 0..5 {
        let bytes = format!("processor-fixture-{index}").into_bytes();
        let object = ObjectRef::of(&bytes);
        objects.insert(object.clone(), bytes);
        assets.push(Asset {
            name: format!("processor/file-{index}"),
            object,
        });
    }
    let mut mappings = vec![];
    let mut originals = vec![];
    for (model, lane) in [
        (H3, "bf16"),
        (H3, "bf16-full"),
        (H3, "fp8"),
        (H3, "int8"),
        (H3, "nvfp4"),
        (UPSCALER, "bf16"),
    ] {
        let names: &[&str] = if model == H3 {
            &[
                "transformer",
                "transformer_ref",
                "text_encoder",
                "vae",
                "audio_vae",
            ]
        } else {
            &["upscaler", "model"]
        };
        let mut components: Vec<_> = names
            .iter()
            .map(|name| Component {
                name: (*name).into(),
                ..Default::default()
            })
            .collect();
        components[0].tensors.push(Tensor {
            key: "weight".into(),
            dtype: Dtype::F32,
            shape: vec![1],
            body: TensorBody::Data(Location {
                chunk: 0,
                offset: 0,
            }),
        });
        if model == UPSCALER {
            components[1].config.insert(
                "architecture".into(),
                json!("minimax-h3-latent-upscaler-3d-conv-v1"),
            );
        }
        let metadata = BTreeMap::from([(
            "fixture_lane".into(),
            ModelValue::Str(format!("{model}/{lane}")),
        )]);
        let old = Header::new(
            vec![chunk.clone()],
            None,
            components.clone(),
            metadata.clone(),
        )
        .unwrap();
        let old_bytes = old.to_cbor().unwrap();
        let old_header = ObjectRef::of(&old_bytes);
        let old_tree = Tree::of(
            lane,
            old_header.clone(),
            &BTreeMap::from([("README.md".into(), ObjectRef::of(b"readme"))]),
        )
        .unwrap();
        let old_tree_bytes = old_tree.to_json().unwrap();
        let old_checkpoint = ObjectRef::of(&old_tree_bytes);
        let stem = format!("{}-{lane}", model.replace('/', '-'));
        let header_path = inputs.join(format!("{stem}.header"));
        fs::write(&header_path, old_bytes).unwrap();
        let tree_path = inputs.join(format!("{stem}.tree"));
        fs::write(&tree_path, old_tree_bytes).unwrap();
        let (old_release, proposed_release) = if model == H3 {
            let encoder = components
                .iter_mut()
                .find(|component| component.name == "text_encoder")
                .unwrap();
            for asset in &assets {
                encoder
                    .assets
                    .insert(asset.name.clone(), asset.object.clone());
            }
            ("1.0.0-rc.7", "1.0.0-rc.8")
        } else {
            components[1].config.insert(
                "latent_normalization".into(),
                json!({"enabled":true,"mean":vec![0.0;24],"std":vec![1.0;24]}),
            );
            ("0.1.0", "0.1.1")
        };
        let new = Header::new(vec![chunk.clone()], None, components, metadata).unwrap();
        let bytes = new.to_cbor().unwrap();
        let new_header = ObjectRef::of(&bytes);
        objects.insert(new_header.clone(), bytes);
        let tree = Tree::of(
            lane,
            new_header.clone(),
            &BTreeMap::from([("README.md".into(), ObjectRef::of(b"readme"))]),
        )
        .unwrap();
        let bytes = tree.to_json().unwrap();
        let new_checkpoint = ObjectRef::of(&bytes);
        objects.insert(new_checkpoint.clone(), bytes);
        mappings.push(Mapping {
            model: model.into(),
            lane: lane.into(),
            old_release: old_release.into(),
            proposed_release: proposed_release.into(),
            old_checkpoint: old_checkpoint.clone(),
            new_checkpoint,
            old_header: old_header.clone(),
            new_header,
        });
        originals.push(Original {
            model: model.into(),
            lane: lane.into(),
            release: old_release.into(),
            old_checkpoint: old_checkpoint.id(),
            old_tree: tree_path,
            old_tree_length: old_checkpoint.length,
            old_header: header_path,
            old_header_ref: old_header,
        });
    }
    let total_new_bytes = objects.values().map(|bytes| bytes.len() as u64).sum();
    let new_unique_objects = objects
        .iter()
        .map(|(object, bytes)| {
            let path = PathBuf::from("objects").join(&object.sha256);
            fs::write(prepared.join(&path), bytes).unwrap();
            ObjectAt {
                object: object.clone(),
                path,
            }
        })
        .collect();
    let manifest = Manifest {
        format: "xs170.required-data-reissue/1".into(),
        prepared_only: true,
        mappings,
        processor_assets: assets,
        new_unique_objects,
        new_unique_object_count: 17,
        total_new_bytes,
    };
    let manifest_path = prepared.join("manifest.json");
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(&manifest_path, &bytes).unwrap();
    let inventory_path = inputs.join("inventory.json");
    fs::write(
        &inventory_path,
        serde_json::to_vec(&Inventory { inputs: originals }).unwrap(),
    )
    .unwrap();
    (manifest_path, inventory_path, ObjectRef::of(&bytes).sha256)
}

#[derive(Clone)]
struct FixtureState {
    calls: Arc<Mutex<Vec<String>>>,
    objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    payloads: Arc<BTreeMap<String, Vec<u8>>>,
    cards: Arc<Mutex<BTreeMap<String, Value>>>,
    mappings: Arc<Vec<Mapping>>,
    store: String,
    collision: Arc<AtomicBool>,
}
fn model_card(model: &str, rows: &[&Mapping]) -> Value {
    let (org, name) = model.split_once('/').unwrap();
    let lanes: BTreeMap<_,_>=rows.iter().map(|row|(row.lane.clone(),json!({"checkpoint":row.old_checkpoint.id(),"length":row.old_checkpoint.length,"header":row.old_header}))).collect();
    json!({"model":{"org":org,"name":name,"kind":"base"},"releases":[{"version":rows[0].old_release,"visibility":"public","lanes":lanes}],"checkpoints":[]})
}
fn auth(headers: &HeaderMap) {
    assert_eq!(headers.get("authorization").unwrap(), "Bearer fixture-only");
}
async fn card(
    State(state): State<FixtureState>,
    RoutePath((org, name)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Json<Value> {
    auth(&headers);
    let model = format!("{org}/{name}");
    state
        .calls
        .lock()
        .unwrap()
        .push(format!("GET /v1/models/{model}"));
    Json(state.cards.lock().unwrap()[&model].clone())
}
async fn blob(
    State(state): State<FixtureState>,
    RoutePath((_aa, _bb, sha)): RoutePath<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    auth(&headers);
    state
        .calls
        .lock()
        .unwrap()
        .push(format!("HEAD /blobs/{sha}"));
    if let Some(bytes) = state.objects.lock().unwrap().get(&sha) {
        return (
            StatusCode::OK,
            [
                ("content-length", bytes.len().to_string()),
                ("etag", format!("\"sha256:{sha}\"")),
            ],
            Body::empty(),
        )
            .into_response();
    }
    assert!(
        state.payloads.contains_key(&sha),
        "operator asked for an old weight or supporting object"
    );
    (
        StatusCode::NOT_FOUND,
        [(
            "location",
            format!("{}/put/{sha}?capability=fixture-secret", state.store),
        )],
        Json(json!({"error":{"code":"not_found"}})),
    )
        .into_response()
}
async fn store(
    State(state): State<FixtureState>,
    RoutePath(sha): RoutePath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    assert!(
        !headers.contains_key("authorization"),
        "Hub bearer leaked to a storage origin"
    );
    assert_eq!(headers.get("if-none-match").unwrap(), "*");
    assert!(headers.contains_key("x-amz-checksum-sha256"));
    assert_eq!(ObjectRef::of(&body).sha256, sha);
    assert_eq!(body.as_ref(), state.payloads[&sha].as_slice());
    state.calls.lock().unwrap().push(format!("PUT /put/{sha}"));
    state.objects.lock().unwrap().insert(sha, body.to_vec());
    StatusCode::OK
}
async fn admission(
    State(state): State<FixtureState>,
    RoutePath((org, name)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    auth(&headers);
    let model = format!("{org}/{name}");
    state
        .calls
        .lock()
        .unwrap()
        .push(format!("POST /v1/models/{model}/checkpoints"));
    assert!(body.get("commit").is_none());
    let object: ObjectRef = serde_json::from_value(body["checkpoint"].clone()).unwrap();
    assert!(state.objects.lock().unwrap().contains_key(&object.sha256));
    let row = state
        .mappings
        .iter()
        .find(|row| row.model == model && row.new_checkpoint == object)
        .unwrap();
    let result =
        json!({"id":object.id(),"length":object.length,"lane":row.lane,"header":row.new_header});
    state.cards.lock().unwrap().get_mut(&model).unwrap()["checkpoints"]
        .as_array_mut()
        .unwrap()
        .push(result.clone());
    (StatusCode::CREATED, Json(result))
}
async fn publish(
    State(state): State<FixtureState>,
    RoutePath((org, name, version)): RoutePath<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    auth(&headers);
    let model = format!("{org}/{name}");
    state
        .calls
        .lock()
        .unwrap()
        .push(format!("PUT /v1/models/{model}/releases/{version}"));
    let mut cards = state.cards.lock().unwrap();
    let card = cards.get_mut(&model).unwrap();
    if state.collision.swap(false, Ordering::SeqCst)
        || card["releases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|release| same_version(release["version"].as_str().unwrap(), &version))
    {
        return (
            StatusCode::CONFLICT,
            Json(
                json!({"error":{"code":"release.conflict","message":"https://invalid.test/?capability=fixture-secret","remedy":"do not leak this body"}}),
            ),
        );
    }
    let selected: Vec<_> = state
        .mappings
        .iter()
        .filter(|row| row.model == model)
        .collect();
    assert_eq!(
        body["checkpoints"],
        json!(
            selected
                .iter()
                .map(|row| row.new_checkpoint.id())
                .collect::<Vec<_>>()
        )
    );
    assert!(body.get("commits").is_none());
    let lanes:BTreeMap<_,_>=selected.iter().map(|row|(row.lane.clone(),json!({"checkpoint":row.new_checkpoint.id(),"length":row.new_checkpoint.length,"header":row.new_header}))).collect();
    let result = json!({"version":version,"visibility":"public","lanes":lanes});
    card["releases"]
        .as_array_mut()
        .unwrap()
        .push(result.clone());
    (StatusCode::CREATED, Json(result))
}
struct Fixture {
    root: TempDir,
    proposal: Proposal,
    state: FixtureState,
    client: Client,
    endpoint: Endpoint,
    servers: Vec<tokio::task::JoinHandle<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let (manifest, inputs, sha) = fixture_bundle(root.path());
        let proposal = Proposal::read(&manifest, &inputs, &sha).unwrap();
        let store_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let store_url = format!("http://{}", store_listener.local_addr().unwrap());
        let hub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hub_url = format!("http://{}", hub_listener.local_addr().unwrap());
        let cards = proposal
            .models(None)
            .iter()
            .map(|(model, rows)| (model.clone(), model_card(model, rows)))
            .collect();
        let state = FixtureState {
            calls: Arc::new(Mutex::new(vec![])),
            objects: Arc::new(Mutex::new(BTreeMap::new())),
            payloads: Arc::new(
                proposal
                    .objects
                    .iter()
                    .map(|(object, bytes)| (object.sha256.clone(), bytes.to_vec()))
                    .collect(),
            ),
            cards: Arc::new(Mutex::new(cards)),
            mappings: Arc::new(proposal.manifest.mappings.clone()),
            store: store_url,
            collision: Arc::new(AtomicBool::new(false)),
        };
        let store_router = Router::new()
            .route("/put/{sha}", put(store))
            .with_state(state.clone());
        let hub_router = Router::new()
            .route("/v1/models/{org}/{name}", get(card))
            .route("/blobs/{aa}/{bb}/{sha}", head(blob))
            .route("/v1/models/{org}/{name}/checkpoints", post(admission))
            .route("/v1/models/{org}/{name}/releases/{version}", put(publish))
            .with_state(state.clone());
        let servers = vec![
            tokio::spawn(async move { axum::serve(store_listener, store_router).await.unwrap() }),
            tokio::spawn(async move { axum::serve(hub_listener, hub_router).await.unwrap() }),
        ];
        let client = Client::connect(
            &hub_url,
            api::hub::http(None).unwrap(),
            Credential::Bearer("fixture-only".into()),
        );
        let policy = SourcePolicy {
            any_host: true,
            allow_local: true,
            ..Default::default()
        };
        let endpoint = Endpoint::new(
            &fetch::Client::new(policy),
            &hub_url,
            Some(fetch::bearer("fixture-only")),
        )
        .unwrap();
        Self {
            root,
            proposal,
            state,
            client,
            endpoint,
            servers,
        }
    }
    fn journal(&self, name: &str, phase: Phase, model: Option<&str>) -> Journal {
        Journal::new(
            &self.root.path().join(format!("{name}.json")),
            phase,
            model.map(str::to_owned),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
    }
}

#[test]
fn default_check_requires_the_explicit_trusted_origin() {
    let mut args = vec![
        "--manifest",
        "proposal.json",
        "--inputs",
        "inputs.json",
        "--hub",
        HUB,
        "--user-home",
        "/fixture",
        "--receipt",
        "new.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    assert_eq!(Options::parse(args.clone()).unwrap().phase, Phase::Check);
    args[5] = "https://untrusted.invalid".into();
    assert_eq!(Options::parse(args).unwrap_err().code, "hub_not_trusted");
}
#[test]
fn altered_manifest_or_payload_refuses_before_credentials() {
    let root = tempfile::tempdir().unwrap();
    let (manifest, inputs, sha) = fixture_bundle(root.path());
    assert_eq!(
        Proposal::read(&manifest, &inputs, MANIFEST_SHA)
            .err()
            .unwrap()
            .code,
        "manifest_checksum_changed"
    );
    let value: Value = read_json(&manifest).unwrap();
    let object = &value["new_unique_objects"][0];
    fs::write(
        manifest
            .parent()
            .unwrap()
            .join(object["path"].as_str().unwrap()),
        b"changed",
    )
    .unwrap();
    assert_eq!(
        Proposal::read(&manifest, &inputs, &sha).err().unwrap().code,
        "object_checksum_changed"
    );
}
#[tokio::test]
async fn check_has_no_upload_or_admission_side_effects() {
    let fixture = Fixture::new().await;
    let mut journal = fixture.journal("check", Phase::Check, None);
    execute(
        Phase::Check,
        &fixture.proposal,
        None,
        &fixture.client,
        &fixture.endpoint,
        &mut journal,
    )
    .await
    .unwrap();
    assert!(
        fixture
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.starts_with("GET /v1/models/"))
    );
    assert!(fixture.state.objects.lock().unwrap().is_empty());
}
#[tokio::test]
async fn seventeen_objects_six_admissions_and_explicit_publications_use_only_reviewed_refs() {
    let fixture = Fixture::new().await;
    let mut admit = fixture.journal("admit", Phase::Admit, None);
    execute(
        Phase::Admit,
        &fixture.proposal,
        None,
        &fixture.client,
        &fixture.endpoint,
        &mut admit,
    )
    .await
    .unwrap();
    assert_eq!(fixture.state.objects.lock().unwrap().len(), 17);
    assert_eq!(
        admit
            .data
            .events
            .iter()
            .filter(|event| event["step"] == "checkpoint_admitted")
            .count(),
        6
    );
    let calls = fixture.state.calls.lock().unwrap().clone();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("PUT /put/"))
            .count(),
        17
    );
    assert!(!calls.iter().any(|call| call.starts_with("GET /blobs/")));
    for model in [H3, UPSCALER] {
        let mut journal = fixture.journal(
            &format!("publish-{}", model.replace('/', '-')),
            Phase::Publish,
            Some(model),
        );
        execute(
            Phase::Publish,
            &fixture.proposal,
            Some(model),
            &fixture.client,
            &fixture.endpoint,
            &mut journal,
        )
        .await
        .unwrap();
    }
    for card in fixture.state.cards.lock().unwrap().values() {
        assert_eq!(card["releases"].as_array().unwrap().len(), 2);
        assert_eq!(card["releases"][0]["visibility"], "public");
    }
}
#[tokio::test]
async fn existing_version_or_changed_old_mapping_stops_before_writes() {
    let fixture = Fixture::new().await;
    fixture.state.cards.lock().unwrap().get_mut(H3).unwrap()["releases"]
        .as_array_mut()
        .unwrap()
        .push(json!({"version":"1.0.0rc8","visibility":"public","lanes":{}}));
    let mut journal = fixture.journal("existing", Phase::Admit, None);
    let error = execute(
        Phase::Admit,
        &fixture.proposal,
        None,
        &fixture.client,
        &fixture.endpoint,
        &mut journal,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "proposed_version_exists");
    assert!(fixture.state.objects.lock().unwrap().is_empty());
    fixture.state.cards.lock().unwrap().get_mut(H3).unwrap()["releases"]
        .as_array_mut()
        .unwrap()
        .pop();
    fixture.state.cards.lock().unwrap().get_mut(H3).unwrap()["releases"][0]["lanes"]["bf16"]["checkpoint"] =
        json!("sha256:changed");
    let mut journal = fixture.journal("changed", Phase::Admit, None);
    assert_eq!(
        execute(
            Phase::Admit,
            &fixture.proposal,
            None,
            &fixture.client,
            &fixture.endpoint,
            &mut journal
        )
        .await
        .unwrap_err()
        .code,
        "release_mapping_changed"
    );
    assert!(fixture.state.objects.lock().unwrap().is_empty());
}
#[tokio::test]
async fn publication_requires_admission_and_conflicts_are_safe_to_print() {
    let fixture = Fixture::new().await;
    let mut early = fixture.journal("early", Phase::Publish, Some(H3));
    assert_eq!(
        execute(
            Phase::Publish,
            &fixture.proposal,
            Some(H3),
            &fixture.client,
            &fixture.endpoint,
            &mut early
        )
        .await
        .unwrap_err()
        .code,
        "checkpoint_not_admitted"
    );
    let mut admit = fixture.journal("admit", Phase::Admit, None);
    execute(
        Phase::Admit,
        &fixture.proposal,
        None,
        &fixture.client,
        &fixture.endpoint,
        &mut admit,
    )
    .await
    .unwrap();
    fixture.state.collision.store(true, Ordering::SeqCst);
    let mut publish = fixture.journal("collision", Phase::Publish, Some(H3));
    let error = execute(
        Phase::Publish,
        &fixture.proposal,
        Some(H3),
        &fixture.client,
        &fixture.endpoint,
        &mut publish,
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, Some(409));
    assert_eq!(error.code, "immutable_conflict");
    let safe = serde_json::to_string(&error).unwrap();
    assert!(
        !safe.contains("fixture-secret")
            && !safe.contains("https://invalid")
            && !safe.contains("do not leak")
    );
    assert_eq!(
        fixture.state.cards.lock().unwrap()[H3]["releases"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
