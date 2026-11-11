use api::{code::source_files, calcifer::output_file_name as file_name};

#[test]
fn output_slots_have_distinct_bounded_portable_names() {
    let cases = [
        ("é".to_owned(), 0),
        ("ô".to_owned(), 0),
        (format!("{}a", "x".repeat(128)), 0),
        (format!("{}b", "x".repeat(128)), 0),
        ("clip".to_owned(), 1),
        ("clip-1".to_owned(), 0),
        ("../../escape".to_owned(), u32::MAX),
    ];
    let mut names = std::collections::HashSet::new();
    for (name, index) in cases {
        let file = file_name(u64::MAX, &name, index, "video/mp4");
        assert_eq!(file, file_name(u64::MAX, &name, index, "video/mp4"));
        assert!(file.ends_with(".mp4"));
        assert!(file.len() <= 255);
        assert!(!file.contains(['/', '\\', ':', '\0']));
        assert!(names.insert(file));
    }
}

#[test]
fn source_bundle_excludes_direnv_credentials_and_keeps_project_resources() {
    let project = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        (".envrc", "export TEST_TOKEN=benign-marker\n"),
        (".env", "TEST_TOKEN=benign-marker\n"),
        ("pyproject.toml", "[project]\nname='fixture'\n"),
        (".gitignore", "resource.txt\n"),
        ("resource.txt", "runtime input\n"),
    ] {
        std::fs::write(project.path().join(name), bytes).unwrap();
    }
    std::fs::create_dir(project.path().join(".direnv")).unwrap();
    std::fs::write(
        project.path().join(".direnv/state"),
        "benign-credential-state",
    )
    .unwrap();
    let mut left = Vec::new();
    let files = source_files(project.path(), &mut left).unwrap();
    assert!(files.contains(&"pyproject.toml".into()));
    assert!(files.contains(&"resource.txt".into()));
    assert!(!files.contains(&".envrc".into()));
    assert!(!files.contains(&".env".into()));
    assert!(!files.iter().any(|file| file.starts_with(".direnv")));
    assert!(left.iter().any(|name| name == ".envrc"));
    assert!(left.iter().any(|name| name.starts_with(".direnv")));
}

#[test]
fn retired_run_options_remain_readable_without_being_serialized() {
    let request: api::calcifer::RunRequest = serde_json::from_value(serde_json::json!({
        "target": "example/generate", "no_memo": true,
    })).unwrap();
    assert_eq!(request.target, "example/generate");
    assert!(serde_json::to_value(request).unwrap().get("no_memo").is_none());

    let spec: api::machine::RunSpec = serde_json::from_value(serde_json::json!({
        "entrypoint": "generate", "no_memo": true,
    })).unwrap();
    assert_eq!(spec.entrypoint, "generate");
    assert!(serde_json::to_value(spec).unwrap().get("no_memo").is_none());
}
