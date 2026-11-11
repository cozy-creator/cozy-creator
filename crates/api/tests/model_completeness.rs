#[test]
fn older_inventory_without_completeness_remains_unknown() {
    let machine: api::machine::Checkpoint = serde_json::from_value(serde_json::json!({
        "release":"1", "lane":"bf16", "manifest":"sha256:header"
    })).unwrap();
    assert_eq!(machine.complete, None);
    let controller: api::calcifer::Checkpoint = serde_json::from_value(serde_json::json!({
        "release":"1", "lane":"bf16", "manifest":"sha256:header"
    })).unwrap();
    assert_eq!(controller.complete, None);
    let held: api::calcifer::HeldModel = serde_json::from_value(serde_json::json!({
        "machine":"local", "model":"test/model", "release":"1", "lane":"bf16",
        "precision":"bf16", "checkpoint":"sha256:header", "bytes":12, "shared_bytes":0
    })).unwrap();
    assert_eq!(held.complete, None);
}

#[test]
fn completeness_keeps_false_distinct_from_unknown() {
    for complete in [None, Some(false), Some(true)] {
        let held = api::calcifer::HeldModel { complete, ..Default::default() };
        let json = serde_json::to_value(&held).unwrap();
        assert_eq!(json.get("complete").is_some(), complete.is_some());
        let back: api::calcifer::HeldModel = serde_json::from_value(json).unwrap();
        assert_eq!(back.complete, complete);
    }
}
