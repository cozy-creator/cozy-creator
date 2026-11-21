use automerge::{
    transaction::Transactable, ActorId, AutoCommit, AutoSerde, ObjId, ObjType, ReadDoc, ROOT,
};
use loro::{
    Container, ContainerTrait, ExportMode, LoroDoc, LoroMap, ValueOrContainer, VersionVector,
};
use serde_json::{json, Value};
use std::{fs, time::Instant};

const ORDERS: [[usize; 7]; 4] = [
    [4, 3, 2, 1, 0, 2, 0],
    [2, 4, 0, 3, 1, 4, 2],
    [0, 1, 2, 3, 4, 0, 4],
    [3, 2, 1, 4, 0, 1, 3],
];
fn write_json(path: &str, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap() + "\n").unwrap();
}
fn actor(n: u64) -> ActorId {
    ActorId::from(n.to_be_bytes())
}
fn am_obj(doc: &AutoCommit, parent: &ObjId, key: &str) -> ObjId {
    doc.get(parent, key).unwrap().unwrap().1
}
fn am_json(doc: &AutoCommit) -> Value {
    serde_json::to_value(AutoSerde::from(doc)).unwrap()
}
fn am_versions(doc: &AutoCommit, asset_id: &str) -> ObjId {
    let assets = am_obj(doc, &ROOT, "assets");
    let asset = am_obj(doc, &assets, asset_id);
    am_obj(doc, &asset, "versions")
}
fn am_add_version(doc: &mut AutoCommit, asset_id: &str, version_id: &str, prompt: &str) {
    let versions = am_versions(doc, asset_id);
    let version = doc.put_object(&versions, version_id, ObjType::Map).unwrap();
    doc.put(&version, "version_id", version_id).unwrap();
    doc.put(
        &version,
        "blob_hash",
        format!("blake3:synthetic/{asset_id}/{version_id}"),
    )
    .unwrap();
    doc.put(&version, "prompt", prompt).unwrap();
}
fn am_seed(asset_count: usize, version_count: usize) -> AutoCommit {
    let mut doc = AutoCommit::new().with_actor(actor(100));
    for (k, v) in [
        ("project_id", "project-xs160"),
        ("title", "Cozy project"),
        ("description", "seed description"),
        ("prompt", "seed prompt"),
        ("approval", "pending"),
    ] {
        doc.put(ROOT, k, v).unwrap();
    }
    let assets = doc.put_object(ROOT, "assets", ObjType::Map).unwrap();
    for i in 1..=asset_count {
        let id = format!("asset-{i:04}");
        let asset = doc.put_object(&assets, id.as_str(), ObjType::Map).unwrap();
        doc.put(&asset, "asset_id", id.as_str()).unwrap();
        doc.put(&asset, "name", format!("Asset {i}")).unwrap();
        doc.put_object(&asset, "versions", ObjType::Map).unwrap();
        for j in 1..=version_count {
            am_add_version(&mut doc, &id, &format!("version-{j:04}"), "seed");
        }
    }
    doc.commit();
    doc
}
fn am_replicas(seed: &mut AutoCommit) -> Vec<AutoCommit> {
    let bytes = seed.save();
    (101..=104)
        .map(|id| AutoCommit::load(&bytes).unwrap().with_actor(actor(id)))
        .collect()
}
fn am_sync(docs: &mut [AutoCommit]) -> Value {
    let updates: Vec<_> = docs.iter_mut().map(|d| d.get_changes(&[])).collect();
    for (i, doc) in docs.iter_mut().enumerate() {
        for j in 0..updates.len() {
            doc.apply_changes(updates[(i + j) % updates.len()].clone())
                .unwrap();
        }
        doc.apply_changes(updates[0].clone()).unwrap();
    }
    let value = am_json(&docs[0]);
    for doc in docs.iter() {
        assert_eq!(am_json(doc), value);
    }
    value
}
fn am_alternatives(doc: &AutoCommit, key: &str) -> Value {
    let mut values: Vec<_> = doc
        .get_all(ROOT, key)
        .unwrap()
        .into_iter()
        .map(|(v, id)| json!({"value": v.as_str().unwrap(), "operation_id": id.to_string()}))
        .collect();
    values.sort_by_key(|v| v["value"].as_str().unwrap().to_owned());
    json!(values)
}
fn automerge_cases() -> Value {
    let mut seed = am_seed(1, 1);
    let mut docs = am_replicas(&mut seed);
    docs[0].put(ROOT, "title", "offline title").unwrap();
    docs[1]
        .put(ROOT, "description", "offline description")
        .unwrap();
    let assets = am_obj(&docs[2], &ROOT, "assets");
    let asset = am_obj(&docs[2], &assets, "asset-0001");
    docs[2].put(&asset, "name", "offline asset name").unwrap();
    let disjoint = am_sync(&mut docs);
    assert_eq!(disjoint["title"], "offline title");
    assert_eq!(disjoint["description"], "offline description");
    assert_eq!(
        disjoint["assets"]["asset-0001"]["name"],
        "offline asset name"
    );

    let mut docs = am_replicas(&mut seed);
    am_add_version(&mut docs[0], "asset-0001", "version-0101", "dawn");
    am_add_version(&mut docs[1], "asset-0001", "version-0102", "dusk");
    let versions = am_sync(&mut docs)["assets"]["asset-0001"]["versions"].clone();
    assert_eq!(versions.as_object().unwrap().len(), 3);

    let mut docs = am_replicas(&mut seed);
    for (i, (p, a)) in [
        ("dawn", "approved"),
        ("dusk", "rejected"),
        ("noon", "needs-review"),
    ]
    .iter()
    .enumerate()
    {
        docs[i].put(ROOT, "prompt", *p).unwrap();
        docs[i].put(ROOT, "approval", *a).unwrap();
        docs[i].commit();
    }
    let conflict_visible = am_sync(&mut docs);
    let prompts = am_alternatives(&docs[0], "prompt");
    let approvals = am_alternatives(&docs[0], "approval");
    assert_eq!(prompts.as_array().unwrap().len(), 3);
    assert_eq!(approvals.as_array().unwrap().len(), 3);
    for doc in &docs {
        assert_eq!(am_alternatives(doc, "prompt"), prompts);
    }
    let bytes = docs[0].save();
    fs::write("automerge-conflict.snapshot", &bytes).unwrap();
    let mut restored = AutoCommit::load(&bytes).unwrap().with_actor(actor(201));
    assert_eq!(am_alternatives(&restored, "prompt"), prompts);
    restored
        .put(ROOT, "prompt", "human resolved prompt")
        .unwrap();
    assert_eq!(restored.get_all(ROOT, "prompt").unwrap().len(), 1);
    let conflicts = json!({"visible_projection": conflict_visible, "prompt_get_all": prompts,
        "approval_get_all": approvals, "alternatives_survive_snapshot_reload": true,
        "prompt_after_causally_later_resolution": am_alternatives(&restored, "prompt"),
        "current_alternatives_queryable": true});

    let mut delete_field = Vec::new();
    for (delete_index, update_index) in [(0, 1), (2, 0)] {
        let mut docs = am_replicas(&mut seed);
        docs[delete_index].delete(ROOT, "title").unwrap();
        docs[update_index]
            .put(ROOT, "title", "concurrent update")
            .unwrap();
        let merged = am_sync(&mut docs);
        delete_field.push(
            json!({"deleting_actor": 101 + delete_index, "updating_actor": 101 + update_index,
            "visible_title": merged.get("title"), "get_all": am_alternatives(&docs[0], "title")}),
        );
    }
    let mut docs = am_replicas(&mut seed);
    let hidden_versions = am_versions(&docs[0], "asset-0001");
    let assets = am_obj(&docs[0], &ROOT, "assets");
    docs[0].delete(&assets, "asset-0001").unwrap();
    am_add_version(
        &mut docs[1],
        "asset-0001",
        "version-0102",
        "concurrent child update",
    );
    let deleted_asset = am_sync(&mut docs);
    let hidden_keys: Vec<_> = docs[0].keys(&hidden_versions).collect();
    let deletion = json!({"same_slot_delete_vs_write": delete_field,
        "asset_visible_after_parent_delete_vs_child_update": deleted_asset["assets"].get("asset-0001").is_some(),
        "versions_readable_by_retained_object_id": hidden_keys});

    let mut docs = am_replicas(&mut seed);
    docs[0].put(ROOT, "title", "saved offline title").unwrap();
    let saved = docs[0].save();
    docs[0] = AutoCommit::load(&saved).unwrap().with_actor(actor(201));
    docs[0]
        .put(ROOT, "prompt", "post-reload offline prompt")
        .unwrap();
    am_add_version(
        &mut docs[1],
        "asset-0001",
        "version-0102",
        "other offline replica",
    );
    let reload = am_sync(&mut docs);
    assert_eq!(reload["title"], "saved offline title");
    assert_eq!(reload["prompt"], "post-reload offline prompt");
    assert!(reload["assets"]["asset-0001"]["versions"]
        .get("version-0102")
        .is_some());

    let mut docs = am_replicas(&mut seed);
    let base_heads = seed.get_heads();
    docs[0].put(ROOT, "title", "first dependent write").unwrap();
    docs[0].commit();
    let first = docs[0].get_changes(&base_heads);
    let first_heads = docs[0].get_heads();
    docs[0]
        .put(ROOT, "title", "second dependent write")
        .unwrap();
    docs[0].commit();
    let second = docs[0].get_changes(&first_heads);
    am_add_version(
        &mut docs[1],
        "asset-0001",
        "version-0102",
        "offline version",
    );
    docs[2]
        .put(ROOT, "description", "offline description")
        .unwrap();
    let packets = [
        seed.get_changes(&[]),
        first,
        second,
        docs[1].get_changes(&base_heads),
        docs[2].get_changes(&base_heads),
    ];
    let expected = am_sync(&mut docs);
    let mut heads = Vec::new();
    for (i, order) in ORDERS.iter().enumerate() {
        let mut doc = AutoCommit::new().with_actor(actor(301 + i as u64));
        for &n in order {
            doc.apply_changes(packets[n].clone()).unwrap();
        }
        assert_eq!(am_json(&doc), expected);
        heads.push(format!("{:?}", doc.get_heads()));
    }
    assert!(heads.windows(2).all(|w| w[0] == w[1]));
    let mut pending = AutoCommit::new().with_actor(actor(401));
    pending.apply_changes(packets[2].clone()).unwrap();
    let pending_save = pending.save();
    let mut pending_reload = AutoCommit::load(&pending_save)
        .unwrap()
        .with_actor(actor(402));
    for n in [0, 1, 3, 4] {
        pending_reload.apply_changes(packets[n].clone()).unwrap();
    }
    let pending_persisted = am_json(&pending_reload) == expected;
    json!({"library": "automerge", "version": "0.12.0", "replicas": 4,
        "disjoint_fields": {"converged": true, "value": disjoint},
        "concurrent_version_additions": {"preserved_all_three": true, "versions": versions},
        "semantic_conflicts": conflicts, "delete_vs_update": deletion,
        "save_reload_further_offline_edits": {"converged": true, "snapshot_bytes": saved.len()},
        "duplicate_reordered_delivery": {"converged": true, "orders": ORDERS, "identical_heads": true,
            "dependent_packet_before_seed_survived_save_reload": pending_persisted,
            "snapshot_before_missing_dependencies_bytes": pending_save.len()}})
}

fn lm(doc: &LoroDoc) -> LoroMap {
    doc.get_map("project")
}
fn lsub(map: &LoroMap, key: &str) -> LoroMap {
    match map.get(key).unwrap() {
        ValueOrContainer::Container(Container::Map(m)) => m,
        _ => panic!("expected map"),
    }
}
fn ljson(doc: &LoroDoc) -> Value {
    serde_json::to_value(lm(doc).get_deep_value()).unwrap()
}
fn lversions(doc: &LoroDoc, asset_id: &str) -> LoroMap {
    lsub(&lsub(&lsub(&lm(doc), "assets"), asset_id), "versions")
}
fn l_add_version(doc: &LoroDoc, asset_id: &str, version_id: &str, prompt: &str) {
    let version = lversions(doc, asset_id)
        .insert_container(version_id, LoroMap::new())
        .unwrap();
    version.insert("version_id", version_id).unwrap();
    version
        .insert(
            "blob_hash",
            format!("blake3:synthetic/{asset_id}/{version_id}"),
        )
        .unwrap();
    version.insert("prompt", prompt).unwrap();
}
fn lseed(asset_count: usize, version_count: usize) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(100).unwrap();
    let project = lm(&doc);
    for (k, v) in [
        ("project_id", "project-xs160"),
        ("title", "Cozy project"),
        ("description", "seed description"),
        ("prompt", "seed prompt"),
        ("approval", "pending"),
    ] {
        project.insert(k, v).unwrap();
    }
    let assets = project.insert_container("assets", LoroMap::new()).unwrap();
    for i in 1..=asset_count {
        let id = format!("asset-{i:04}");
        let asset = assets.insert_container(&id, LoroMap::new()).unwrap();
        asset.insert("asset_id", id.as_str()).unwrap();
        asset.insert("name", format!("Asset {i}")).unwrap();
        asset.insert_container("versions", LoroMap::new()).unwrap();
        for j in 1..=version_count {
            l_add_version(&doc, &id, &format!("version-{j:04}"), "seed");
        }
    }
    doc.commit();
    doc
}
fn lreplicas(seed: &LoroDoc) -> Vec<LoroDoc> {
    let bytes = seed.export(ExportMode::Snapshot).unwrap();
    (101..=104)
        .map(|id| {
            let doc = LoroDoc::new();
            doc.import(&bytes).unwrap();
            doc.set_peer_id(id).unwrap();
            doc
        })
        .collect()
}
fn lsync(docs: &[LoroDoc]) -> Value {
    let updates: Vec<_> = docs
        .iter()
        .map(|d| d.export(ExportMode::all_updates()).unwrap())
        .collect();
    for (i, doc) in docs.iter().enumerate() {
        for j in 0..updates.len() {
            doc.import(&updates[(i + j) % updates.len()]).unwrap();
        }
        doc.import(&updates[0]).unwrap();
    }
    let value = ljson(&docs[0]);
    for doc in docs {
        assert_eq!(ljson(doc), value);
        assert_eq!(doc.oplog_vv(), docs[0].oplog_vv());
    }
    value
}
fn lhistory(doc: &LoroDoc) -> Value {
    serde_json::to_value(
        doc.export_json_updates_without_peer_compression(
            &VersionVector::default(),
            &doc.oplog_vv(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn loro_cases() -> Value {
    let seed = lseed(1, 1);
    let docs = lreplicas(&seed);
    lm(&docs[0]).insert("title", "offline title").unwrap();
    lm(&docs[1])
        .insert("description", "offline description")
        .unwrap();
    lsub(&lsub(&lm(&docs[2]), "assets"), "asset-0001")
        .insert("name", "offline asset name")
        .unwrap();
    let disjoint = lsync(&docs);
    assert_eq!(disjoint["title"], "offline title");
    assert_eq!(disjoint["description"], "offline description");
    assert_eq!(
        disjoint["assets"]["asset-0001"]["name"],
        "offline asset name"
    );

    let docs = lreplicas(&seed);
    l_add_version(&docs[0], "asset-0001", "version-0101", "dawn");
    l_add_version(&docs[1], "asset-0001", "version-0102", "dusk");
    let versions = lsync(&docs)["assets"]["asset-0001"]["versions"].clone();
    assert_eq!(versions.as_object().unwrap().len(), 3);

    let docs = lreplicas(&seed);
    let mut branch_frontiers = Vec::new();
    for (i, (p, a)) in [
        ("dawn", "approved"),
        ("dusk", "rejected"),
        ("noon", "needs-review"),
    ]
    .iter()
    .enumerate()
    {
        lm(&docs[i]).insert("prompt", *p).unwrap();
        lm(&docs[i]).insert("approval", *a).unwrap();
        docs[i].commit();
        branch_frontiers.push(docs[i].state_frontiers());
    }
    let conflict_visible = lsync(&docs);
    let history = lhistory(&docs[0]);
    write_json("loro-conflict-history.json", &history);
    let bytes = docs[0].export(ExportMode::Snapshot).unwrap();
    fs::write("loro-conflict.snapshot", &bytes).unwrap();
    let restored = LoroDoc::new();
    restored.import(&bytes).unwrap();
    restored.set_peer_id(201).unwrap();
    let mut checkouts = Vec::new();
    for frontiers in &branch_frontiers {
        restored.checkout(frontiers).unwrap();
        let value = ljson(&restored);
        checkouts.push(json!({"frontiers": format!("{frontiers:?}"), "prompt": value["prompt"], "approval": value["approval"]}));
    }
    assert_eq!(
        checkouts
            .iter()
            .map(|x| x["prompt"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["dawn", "dusk", "noon"]
    );
    restored.checkout_to_latest();
    let winner_peer = lm(&restored).get_last_editor("prompt");
    lm(&restored)
        .insert("prompt", "human resolved prompt")
        .unwrap();
    restored.commit();
    write_json("loro-after-resolution-history.json", &lhistory(&restored));
    let conflicts = json!({"visible_projection": conflict_visible,
        "prompt_get_last_editor": winner_peer, "branch_values_recovered_by_checkout_after_reload": checkouts,
        "history_file": "loro-conflict-history.json", "history_after_resolution_file": "loro-after-resolution-history.json",
        "current_alternatives_queryable": false, "prompt_after_causally_later_resolution": ljson(&restored)["prompt"],
        "note": "Map read exposes one LWW winner. Exported operations retain alternatives and dependencies, but history alone does not label unresolved semantic conflicts."});

    let mut delete_field = Vec::new();
    for (delete_index, update_index) in [(0, 1), (2, 0)] {
        let docs = lreplicas(&seed);
        lm(&docs[delete_index]).delete("title").unwrap();
        lm(&docs[update_index])
            .insert("title", "concurrent update")
            .unwrap();
        let merged = lsync(&docs);
        delete_field.push(json!({"deleting_peer": 101 + delete_index, "updating_peer": 101 + update_index,
            "visible_title": merged.get("title"), "last_editor": lm(&docs[0]).get_last_editor("title")}));
    }
    let docs = lreplicas(&seed);
    let hidden_versions_id = lversions(&docs[0], "asset-0001").id();
    lsub(&lm(&docs[0]), "assets").delete("asset-0001").unwrap();
    l_add_version(
        &docs[1],
        "asset-0001",
        "version-0102",
        "concurrent child update",
    );
    let deleted_asset = lsync(&docs);
    let hidden_versions =
        serde_json::to_value(docs[0].get_map(hidden_versions_id).get_deep_value()).unwrap();
    let deletion = json!({"same_slot_delete_vs_write": delete_field,
        "asset_visible_after_parent_delete_vs_child_update": deleted_asset["assets"].get("asset-0001").is_some(),
        "versions_readable_by_retained_container_id": hidden_versions});

    let mut docs = lreplicas(&seed);
    lm(&docs[0]).insert("title", "saved offline title").unwrap();
    let saved = docs[0].export(ExportMode::Snapshot).unwrap();
    let restored = LoroDoc::new();
    restored.import(&saved).unwrap();
    restored.set_peer_id(201).unwrap();
    docs[0] = restored;
    lm(&docs[0])
        .insert("prompt", "post-reload offline prompt")
        .unwrap();
    l_add_version(
        &docs[1],
        "asset-0001",
        "version-0102",
        "other offline replica",
    );
    let reload = lsync(&docs);
    assert_eq!(reload["title"], "saved offline title");
    assert_eq!(reload["prompt"], "post-reload offline prompt");
    assert!(reload["assets"]["asset-0001"]["versions"]
        .get("version-0102")
        .is_some());

    let docs = lreplicas(&seed);
    let base_vv = seed.oplog_vv();
    lm(&docs[0])
        .insert("title", "first dependent write")
        .unwrap();
    docs[0].commit();
    let first = docs[0].export(ExportMode::updates(&base_vv)).unwrap();
    let first_vv = docs[0].oplog_vv();
    lm(&docs[0])
        .insert("title", "second dependent write")
        .unwrap();
    docs[0].commit();
    let second = docs[0].export(ExportMode::updates(&first_vv)).unwrap();
    l_add_version(&docs[1], "asset-0001", "version-0102", "offline version");
    lm(&docs[2])
        .insert("description", "offline description")
        .unwrap();
    let packets = [
        seed.export(ExportMode::all_updates()).unwrap(),
        first,
        second,
        docs[1].export(ExportMode::updates(&base_vv)).unwrap(),
        docs[2].export(ExportMode::updates(&base_vv)).unwrap(),
    ];
    let expected = lsync(&docs);
    let mut status_logs = Vec::new();
    let mut vvs = Vec::new();
    for (i, order) in ORDERS.iter().enumerate() {
        let doc = LoroDoc::new();
        doc.set_peer_id(301 + i as u64).unwrap();
        let mut statuses = Vec::new();
        for &n in order {
            statuses.push(format!(
                "packet {n}: {:?}",
                doc.import(&packets[n]).unwrap()
            ));
        }
        assert_eq!(ljson(&doc), expected);
        vvs.push(doc.oplog_vv());
        status_logs.push(statuses);
    }
    assert!(vvs.windows(2).all(|w| w[0] == w[1]));
    let pending = LoroDoc::new();
    pending.import(&packets[2]).unwrap();
    let pending_save = pending.export(ExportMode::Snapshot).unwrap();
    let pending_bytes = pending.export_pending_updates().unwrap().unwrap();
    let snapshot_only = LoroDoc::new();
    snapshot_only.import(&pending_save).unwrap();
    let snapshot_and_pending = LoroDoc::new();
    snapshot_and_pending.import(&pending_save).unwrap();
    snapshot_and_pending.import(&pending_bytes).unwrap();
    for n in [0, 1, 3, 4] {
        snapshot_only.import(&packets[n]).unwrap();
        snapshot_and_pending.import(&packets[n]).unwrap();
    }
    let pending_persisted = ljson(&snapshot_and_pending) == expected;
    assert!(pending_persisted);
    json!({"library": "loro", "version": "1.17.1", "replicas": 4,
        "disjoint_fields": {"converged": true, "value": disjoint},
        "concurrent_version_additions": {"preserved_all_three": true, "versions": versions},
        "semantic_conflicts": conflicts, "delete_vs_update": deletion,
        "save_reload_further_offline_edits": {"converged": true, "snapshot_bytes": saved.len()},
        "duplicate_reordered_delivery": {"converged": true, "orders": ORDERS, "identical_version_vectors": true,
            "import_statuses": status_logs,
            "dependent_packet_before_seed_survived_snapshot_only": ljson(&snapshot_only) == expected,
            "dependent_packet_before_seed_survived_snapshot_and_pending_export": pending_persisted,
            "snapshot_bytes": pending_save.len(), "pending_export_bytes": pending_bytes.len()}})
}
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn version_identity_cases() -> Value {
    let mut am = am_seed(1, 1);
    let mut replicas = am_replicas(&mut am);
    am_add_version(&mut replicas[0], "asset-0001", "version-collision", "dawn");
    am_add_version(&mut replicas[1], "asset-0001", "version-collision", "dusk");
    am_sync(&mut replicas);
    let am_versions = am_versions(&replicas[0], "asset-0001");
    let mut am_collision_prompts: Vec<_> = replicas[0]
        .get_all(&am_versions, "version-collision")
        .unwrap()
        .iter()
        .map(|(_, id)| {
            replicas[0]
                .get(id, "prompt")
                .unwrap()
                .unwrap()
                .0
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    am_collision_prompts.sort();
    assert_eq!(am_collision_prompts, ["dawn", "dusk"]);
    let original = am_obj(&am, &crate::am_versions(&am, "asset-0001"), "version-0001");
    am.put(&original, "prompt", "mutation of existing version")
        .unwrap();
    assert_eq!(
        am_json(&am)["assets"]["asset-0001"]["versions"]["version-0001"]["prompt"],
        "mutation of existing version"
    );

    let loro = lseed(1, 1);
    let replicas = lreplicas(&loro);
    l_add_version(&replicas[0], "asset-0001", "version-collision", "dawn");
    l_add_version(&replicas[1], "asset-0001", "version-collision", "dusk");
    let visible = lsync(&replicas)["assets"]["asset-0001"]["versions"]["version-collision"].clone();
    lsub(&lversions(&loro, "asset-0001"), "version-0001")
        .insert("prompt", "mutation of existing version")
        .unwrap();
    assert_eq!(
        ljson(&loro)["assets"]["asset-0001"]["versions"]["version-0001"]["prompt"],
        "mutation of existing version"
    );
    json!({"same_version_id_concurrent_creation": {"automerge_get_all_child_prompts": am_collision_prompts,
        "loro_visible_child": visible}, "existing_version_records_are_mutable_in_both": true,
        "requirement": "Use distributed-unique version IDs and enforce immutable additions in the application; a mutable CRDT map does not enforce this policy."})
}
fn small_shape_measurements() -> Value {
    let mut am_runs = Vec::new();
    let mut loro_runs = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let mut doc = am_seed(1000, 2);
        let create_ms = ms(start);
        let start = Instant::now();
        let bytes = doc.save();
        let save_ms = ms(start);
        let start = Instant::now();
        let loaded = AutoCommit::load(&bytes).unwrap();
        let load_ms = ms(start);
        assert_eq!(am_json(&doc), am_json(&loaded));
        let start = Instant::now();
        let mut replica = AutoCommit::load(&bytes).unwrap().with_actor(actor(101));
        let heads = doc.get_heads();
        replica.put(ROOT, "title", "one offline edit").unwrap();
        let delta = replica.get_changes(&heads);
        let delta_bytes: usize = delta.iter().map(|c| c.raw_bytes().len()).sum();
        let prepare_delta_ms = ms(start);
        let start = Instant::now();
        doc.apply_changes(delta).unwrap();
        let import_delta_ms = ms(start);
        assert_eq!(am_json(&doc), am_json(&replica));
        am_runs.push(
            json!({"create_ms":create_ms,"save_ms":save_ms,"load_ms":load_ms,
            "snapshot_bytes":bytes.len(),"prepare_replica_and_delta_ms":prepare_delta_ms,
            "import_one_field_delta_ms":import_delta_ms,"raw_change_delta_bytes":delta_bytes}),
        );

        let start = Instant::now();
        let doc = lseed(1000, 2);
        let create_ms = ms(start);
        let start = Instant::now();
        let bytes = doc.export(ExportMode::Snapshot).unwrap();
        let save_ms = ms(start);
        let loaded = LoroDoc::new();
        let start = Instant::now();
        loaded.import(&bytes).unwrap();
        let load_ms = ms(start);
        assert_eq!(ljson(&doc), ljson(&loaded));
        assert_eq!(ljson(&doc), am_json(&am_seed(1000, 2)));
        let start = Instant::now();
        let replica = LoroDoc::new();
        replica.import(&bytes).unwrap();
        replica.set_peer_id(101).unwrap();
        let vv = doc.oplog_vv();
        lm(&replica).insert("title", "one offline edit").unwrap();
        let delta = replica.export(ExportMode::updates(&vv)).unwrap();
        let prepare_delta_ms = ms(start);
        let start = Instant::now();
        doc.import(&delta).unwrap();
        let import_delta_ms = ms(start);
        assert_eq!(ljson(&doc), ljson(&replica));
        loro_runs.push(
            json!({"create_ms":create_ms,"save_ms":save_ms,"load_ms":load_ms,
            "snapshot_bytes":bytes.len(),"prepare_replica_and_delta_ms":prepare_delta_ms,
            "import_one_field_delta_ms":import_delta_ms,"encoded_delta_bytes":delta.len()}),
        );
    }
    json!({"scope":"synthetic local micro-measurement, not production benchmark", "repetitions":3,
        "assets":1000,"versions_per_asset":2,"seed_commit_count":1,"same_normalized_model_asserted":true,
        "timing_boundary":"in-process operations; excludes compile, disk I/O, JSON assertion/normalization; sequential single process; no warmup",
        "automerge":am_runs,"loro":loro_runs})
}
fn main() {
    let am = automerge_cases();
    println!("Automerge behavioral scenarios executed");
    let loro = loro_cases();
    println!("Loro behavioral scenarios executed");
    assert_eq!(
        am["disjoint_fields"]["value"],
        loro["disjoint_fields"]["value"]
    );
    assert_eq!(
        am["concurrent_version_additions"]["versions"],
        loro["concurrent_version_additions"]["versions"]
    );
    let identity = version_identity_cases();
    let shape = small_shape_measurements();
    let results = json!({"automerge":am,"loro":loro,"version_identity_and_immutability":identity,"small_shape_measurements":shape});
    write_json("results.json", &results);
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}
