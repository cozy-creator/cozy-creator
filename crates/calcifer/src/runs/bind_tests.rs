use super::*;
use serde_json::json;

async fn shape_term(raw: &str, nullable: bool) -> Result<Bound, Error> {
    let shape = json!({"input": "aspect_ratio"});
    let ty = if nullable { json!({"union": ["null", shape]}) } else { shape };
    let callable: Callable = serde_json::from_value(json!({
        "name": "canvas", "accelerator": false,
        "request": {"fields": [{"name": "aspect_ratio", "type": ty}]}
    })).unwrap();
    let interface = Interface { callables: vec![callable], ..Default::default() };
    let dir = tempfile::tempdir().unwrap();
    let terms = Terms {
        input: None,
        inputs: vec![format!("aspect_ratio={raw}")],
        input_text: None,
        assets: vec![],
        fidelities: vec![],
        loras: vec![],
        key: "shape-scalar".into(),
        paths: BTreeMap::new(),
        digests: None,
        clips: Clips { dir: dir.path().join("clips"), home: platform::paths::Home::at(dir.path()) },
        uploads: dir.path().join("uploads"),
        project: None,
    };
    bind("fixture/canvas", None, &interface, &interface.callables[0], &terms).await
}

#[tokio::test]
async fn xs159_nullable_shape_keeps_the_direct_shapes_scalar_spelling() {
    // The six real CLI failures, plus a value whose interpretation belongs to Runtime.
    for raw in ["16:1", "1:16", "32000:2000", "17:1", "1:17", "32768:32", "unrecognized-ratio"] {
        let direct = shape_term(raw, false).await.unwrap();
        let nullable = shape_term(raw, true).await.unwrap();
        assert_eq!(nullable.payload, direct.payload, "{raw}");
        assert_eq!(nullable.payload["aspect_ratio"], raw);
        assert!(nullable.ignored.is_empty() && nullable.notes.is_empty());
    }
    // Optionality still accepts null and does not make an empty shape valid.
    for raw in ["null", "None"] {
        assert_eq!(shape_term(raw, true).await.unwrap().payload["aspect_ratio"], Value::Null);
    }
    assert_eq!(shape_term("", true).await.unwrap_err().code, shape_term("", false).await.unwrap_err().code);
}

#[test]
fn xs159_shape_union_keeps_null_and_primitive_spellings() {
    let ty = Type::Union { branches: vec![Type::Shape, Type::Null, Type::Bool, Type::Int], tag_field: None };
    assert_eq!(scalar(&ty, "None"), Some(Value::Null));
    assert_eq!(scalar(&ty, "null"), Some(Value::Null));
    assert_eq!(scalar(&ty, "True"), Some(json!(true)));
    assert_eq!(scalar(&ty, "20.0"), Some(json!(20)));
}
