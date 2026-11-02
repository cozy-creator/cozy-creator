//! Real native files, SQLite journal and blocking stager; no machine/network mock.
use super::*;
use std::{fs, path::PathBuf, time::Duration};

fn binary() -> PathBuf {
    std::env::var_os("COZY_NATIVE_PROOF_BINARY").map(PathBuf::from)
        .expect("set COZY_NATIVE_PROOF_BINARY to a real private-linked cozy bundle binary")
}
fn submit(input: StagedInput, version: String) -> SubmitRun {
    SubmitRun {
        agreed: None, on: On::Machine("isolated-unconnected-proof".into()), hub: String::new(),
        kind: wire::Kind::Update, target: format!("tensord {version} (dev)"),
        spec: v1::RunSpec {
            kind: v1::RunKind::Update,
            payload: Some(v1::Json::parse(json!({"tensord":version,"mode":"dedicated"}).to_string()).unwrap()),
            inputs: vec![v1::InputFile { field:"tensord".into(), digest:input.digest.to_string(),
                length:input.length, media_type:"application/octet-stream".into(), order:0 }],
            ..Default::default()
        }, inputs:vec![input], out:None, warnings:vec![],
    }
}
fn isolated(home: &Path, outputs: &Path) -> Arc<crate::Calcifer> {
    let calcifer = crate::Calcifer::open(home, outputs).unwrap();
    calcifer.put_machine(&crate::store::MachineRow {
        name:"isolated-unconnected-proof".into(), kind:MachineKind::Added, hub:String::new(),
        rental_id:None, endpoint:None, key:home.join("unused-key"),
        state:wire::MachineState::Booting, served:false, reason:Some("host proof has no endpoint".into()),
    }).unwrap();
    calcifer
}

async fn until(mut done: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !done() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await.expect("actual stager did not reach the expected boundary");
}

#[test]
fn mode_payload_and_wanted_readback_are_typed() {
    let request: MachineUpdate = serde_json::from_value(json!({"mode":"dedicated"})).unwrap();
    assert_eq!(request.mode, Some(protocol::config::MachineMode::Dedicated));
    assert_eq!(serde_json::to_value(request).unwrap(), json!({"mode":"dedicated"}));
    assert!(serde_json::from_value::<MachineUpdate>(json!({"mode":"sometimes"})).is_err());
    let spec = v1::RunSpec { payload:Some(v1::Json::parse(json!({"runtime":"0.37.1","mode":"dedicated"}).to_string()).unwrap()), ..Default::default() };
    let wanted = wanted(&spec);
    let mut status = MachineStatus { runtime:"0.37.1".into(), ..Default::default() };
    assert!(!runs(&status, &wanted));
    status.mode = "shared".into();
    assert!(!runs(&status, &wanted));
    status.mode = "dedicated".into();
    assert!(runs(&status, &wanted));
}

#[test]
#[ignore = "requires an actual built private native binary and sidecars"]
fn real_binary_measurement_keeps_version_and_complete_bundle() {
    let binary = binary();
    let expected = says(&binary).expect("real binary version");
    let Measured { input, version, bundle, .. } = measured(&binary, "").unwrap();
    let bundle = bundle.expect("actual immutable native dependencies must be carried");
    assert_eq!(version.as_deref(), Some(expected.as_str()));
    assert!(input.path.starts_with(bundle.path()));
    assert_eq!(Digest::of(&fs::read(&input.path).unwrap()), input.digest);
    let executable = platform::native_bundle::staged(&input.path).unwrap();
    assert_eq!(says(&executable).as_deref(), Some(expected.as_str()));
    let manifest: Value = serde_json::from_slice(&fs::read(executable.parent().unwrap().join(platform::native_bundle::MANIFEST)).unwrap()).unwrap();
    assert!(manifest["libraries"].as_array().unwrap().len() >= 2);
    for library in manifest["libraries"].as_array().unwrap() {
        let bytes = fs::read(executable.parent().unwrap().join(library["name"].as_str().unwrap())).unwrap();
        assert_eq!(hex::encode(sha2::Sha256::digest(bytes)), library["sha256"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an actual built private native binary and sidecars"]
async fn canceled_caller_keeps_bundle_through_actual_journal_custody() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("calcifer");fs::create_dir_all(&home).unwrap();
    let calcifer = isolated(&home, &root.path().join("outputs"));
    let source = binary();
    let measured = measured(&source, "").unwrap();
    let temp_path = measured.bundle.as_ref().unwrap().path().to_path_buf();
    let cas = home.join("inputs").join(hex::encode(measured.input.digest.bytes()));
    let submitted = submit(measured.input, measured.version.unwrap());
    let identity = submission_identity(&submitted, &[source]);
    // The real first stage copies/hardlinks before this real journal/staging mutex.
    let guard = calcifer.staging.lock().unwrap();
    let owner = calcifer.clone();
    let caller = tokio::spawn(async move {
        owner.submit_with_input_owners("canceled-native-update", &identity, submitted,
            vec![measured.bundle.unwrap()]).await
    });
    until(|| cas.is_file()).await;
    assert!(calcifer.store.keyed("canceled-native-update").unwrap().is_none());
    caller.abort();assert!(caller.await.unwrap_err().is_cancelled());
    assert!(temp_path.exists(), "detached blocking stager lost its archive owner");
    drop(guard);
    until(|| calcifer.store.keyed("canceled-native-update").unwrap().is_some()).await;
    until(|| !temp_path.exists()).await;
    assert!(cas.is_file(), "durable CAS lost the packed archive after owner retirement");
    calcifer.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an actual built private native binary and sidecars"]
async fn same_key_generated_bundles_keep_content_identity_through_journal_race() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("calcifer");fs::create_dir_all(&home).unwrap();
    let calcifer = isolated(&home, &root.path().join("outputs"));
    let source = binary();
    let first = measured(&source, "").unwrap();let second = measured(&source, "").unwrap();
    assert_ne!(first.input.path, second.input.path);
    assert_eq!(first.input.digest, second.input.digest);
    let paths = [first.bundle.as_ref().unwrap().path().to_path_buf(), second.bundle.as_ref().unwrap().path().to_path_buf()];
    let a = submit(first.input, first.version.unwrap());let b = submit(second.input, second.version.unwrap());
    let a_id = submission_identity(&a, &[source.clone()]);let b_id = submission_identity(&b, &[source]);
    assert_eq!(a_id,b_id);
    let (a,b) = tokio::join!(
        calcifer.submit_with_input_owners("same-native-update", &a_id, a, vec![first.bundle.unwrap()]),
        calcifer.submit_with_input_owners("same-native-update", &b_id, b, vec![second.bundle.unwrap()])
    );
    let (a,b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.run.id,b.run.id);assert_ne!(a.replay,b.replay);
    assert!(paths.iter().all(|path| !path.exists()));
    assert!(calcifer.store.keyed("same-native-update").unwrap().is_some());
    calcifer.close();
}
