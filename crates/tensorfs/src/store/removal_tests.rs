use super::*;
use crate::format::Model;

fn model(store: &Store, name: &str, release: &str, chunks: Vec<ObjectRef>) -> (ObjectRef, ObjectRef) {
    let header = Header::new(chunks, None, vec![], Default::default()).unwrap();
    let header = store.put_bytes(&header.to_cbor().unwrap()).unwrap();
    let tree = store.put_tree(&Tree::of("bf16", header.clone(), &Default::default()).unwrap()).unwrap();
    store.update_ref(name, |old| {
        let mut model = old.unwrap_or_else(|| Model::new(name));
        model.set_lane(release, "bf16", tree.clone());
        Some(model)
    }).unwrap();
    (header, tree)
}

#[test]
fn removal_keeps_shared_and_unrelated_cached_models() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let shared = store.put_bytes(b"shared weights").unwrap();
    let first = store.put_bytes(b"first weights").unwrap();
    let second = store.put_bytes(b"second weights").unwrap();
    let unrelated = store.put_bytes(b"unrelated weights").unwrap();
    let (header, tree) = model(&store, "test/first", "1", vec![shared.clone(), first.clone()]);
    let (_, second_tree) = model(&store, "test/second", "1", vec![shared.clone(), second.clone()]);
    model(&store, "test/unrelated", "1", vec![unrelated.clone()]);
    let removed = store.remove_cached_model("test/first").unwrap();
    assert!(removed.removed);
    assert_eq!(removed.cache_bytes_removed, header.length + tree.length + first.length);
    assert_eq!(removed.retained_bytes, shared.length);
    assert!(store.get_ref("test/first").unwrap().is_none());
    assert!(!store.holds(&first));
    assert!(!store.holds(&header));
    for object in [&shared, &second, &second_tree, &unrelated] { assert!(store.holds(object)); }
    assert!(store.get_ref("test/second").unwrap().is_some());
    assert!(store.get_ref("test/unrelated").unwrap().is_some());
    assert_eq!(store.remove_cached_model("test/first").unwrap(), RemovedModel::default());
}

#[test]
fn removal_keeps_live_checkpoints_and_releases_all_model_lanes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let first = store.put_bytes(b"active weights").unwrap();
    let second = store.put_bytes(b"older weights").unwrap();
    let (header, tree) = model(&store, "local/model", "1", vec![first.clone()]);
    let (old_header, old_tree) = model(&store, "local/model", "2", vec![second.clone()]);
    let active = store.at_checkpoint(&tree).unwrap();
    let removed = store.remove_cached_model("local/model").unwrap();
    assert!(removed.removed);
    assert_eq!(removed.retained_bytes, header.length + tree.length + first.length);
    assert_eq!(removed.cache_bytes_removed, old_header.length + old_tree.length + second.length);
    for object in [&first, &header, &tree] { assert!(store.holds(object)); }
    for object in [&second, &old_header, &old_tree] { assert!(!store.holds(object)); }
    assert!(store.get_ref("local/model").unwrap().is_none());
    assert_eq!(store.read(&first).unwrap(), b"active weights");
    drop(active);
}

#[test]
fn another_ref_to_the_same_checkpoint_keeps_its_uses() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let weights = store.put_bytes(b"aliased weights").unwrap();
    let (header, tree) = model(&store, "test/model", "1", vec![weights.clone()]);
    store.update_ref("test/alias", |_| {
        let mut model = Model::new("test/alias");
        model.add_checkpoint(tree.clone());
        Some(model)
    }).unwrap();
    let removed = store.remove_cached_model("test/model").unwrap();
    assert_eq!(removed.cache_bytes_removed, 0);
    assert_eq!(removed.retained_bytes, weights.length + header.length + tree.length);
    assert!(store.root().join("uses").join(&tree.sha256).is_file());
    assert!(store.at_checkpoint(&tree).is_ok());
}

#[test]
fn an_open_object_reader_is_never_unlinked() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let weights = store.put_bytes(b"reading weights").unwrap();
    model(&store, "test/model", "1", vec![weights.clone()]);
    let mut reader = store.file(&weights).unwrap();
    let removed = store.remove_cached_model("test/model").unwrap();
    assert_eq!(removed.retained_bytes, weights.length);
    assert!(store.holds(&weights));
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"reading weights");
}

#[test]
fn origin_store_removal_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap().origin();
    let weights = store.put_bytes(b"origin weights").unwrap();
    model(&store, "test/model", "1", vec![weights.clone()]);
    assert!(store.remove_cached_model("test/model").is_err());
    assert!(store.get_ref("test/model").unwrap().is_some());
    assert!(store.holds(&weights));
}

#[cfg(unix)]
#[test]
fn removed_cache_bytes_do_not_claim_shared_disk_space_was_freed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("cache")).unwrap();
    let weights = store.put_bytes(b"linked weights").unwrap();
    let (header, tree) = model(&store, "test/model", "1", vec![weights.clone()]);
    let external = dir.path().join("kept-weights");
    fs::hard_link(store.path(&weights).unwrap(), &external).unwrap();
    let removed = store.remove_cached_model("test/model").unwrap();
    assert_eq!(removed.cache_bytes_removed, weights.length + header.length + tree.length);
    assert_eq!(removed.retained_bytes, 0);
    assert_eq!(fs::read(&external).unwrap(), b"linked weights");
}
