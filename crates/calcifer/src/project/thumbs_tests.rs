use super::*;
use std::{fs, future::Future, process::Command, task::{Context, Waker}};

use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};

#[test]
#[ignore = "requires ffmpeg and ffprobe on PATH"]
fn restored_thumbnail_input_recovers() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "project::thumbs::tests::restored_thumbnail_input_fixture", "--nocapture"])
        .env("COZY_THUMB_TEST_ROOT", root.path())
        .env("TENSORFS_HOME", root.path().join("tensorfs"))
        .output().unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "thumbnail fixture: {}", output.status);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restored_thumbnail_input_fixture() {
    let Some(root) = std::env::var_os("COZY_THUMB_TEST_ROOT").map(PathBuf::from) else { return };
    let home = platform::paths::Home::under(&root);
    let (ffmpeg, _) = platform::tools::ffmpeg(&home.tools(), &crate::tools::path()).expect("ffmpeg and ffprobe on PATH");
    assert!(platform::tools::ffmpeg_encodes(&ffmpeg.path, "libx264"));
    home.open().unwrap();
    fs::create_dir_all(home.calcifer()).unwrap();
    super::super::bind_calcifer(&home.calcifer()).unwrap();
    let calcifer = crate::Calcifer::open(&home.calcifer(), &home.outputs()).unwrap();
    let config = api::config::Config { hub_url: api::config::Origin::parse("http://127.0.0.1:9").unwrap(), ..Default::default() };
    let machines = crate::machines::Machines::start(calcifer.clone(), home.clone(), &root, &config);
    let api = Api::new(calcifer.clone(), machines, "thumbnail-fixture");

    let project_dir = root.join("project");
    let files = project_dir.join("files");
    fs::create_dir_all(&files).unwrap();
    let still = root.join("source.png");
    let made = Command::new(&ffmpeg.path)
        .args(["-v", "error", "-f", "lavfi", "-i", "color=red:s=32x24", "-frames:v", "1", "-threads", "1", "-update", "1"])
        .arg(&still).output().unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let digest = folder::hash(&still).unwrap();
    let hex = folder::hex_of(&digest);
    let source = files.join(format!("{hex}.png"));
    fs::rename(&still, &source).unwrap();
    let malformed = root.join("malformed.png");
    fs::write(&malformed, b"invalid PNG bytes").unwrap();
    let malformed_digest = folder::hash(&malformed).unwrap();
    fs::rename(&malformed, files.join(format!("{}.png", folder::hex_of(&malformed_digest)))).unwrap();
    let project_dir = super::super::made_new(&api, &project_dir, "thumbnail", false).unwrap();
    let project = api.projects.open(&api, &project_dir).unwrap();
    let ask = |digest: &str| thumb(State(api.clone()), Path((project.id.clone(), digest.to_owned())), Query(Asked { w: Some(256), strip: None, preview: false }), HeaderMap::new());

    let permit = FFMPEGS.acquire_many(2).await.unwrap();
    let mut first = std::pin::pin!(ask(&digest));
    // The handler checks the source before its first suspension. FFmpeg still awaits its turn.
    assert!(first.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    fs::rename(&source, &still).unwrap();
    drop(permit);
    let missing = first.await.unwrap_err();
    assert_eq!(missing.0.code, "thumb_failed");
    println!("queued draw: {}", missing.0.message);
    assert_eq!(missing.into_response().status(), StatusCode::BAD_REQUEST);

    fs::rename(&still, &source).unwrap();
    assert_eq!(folder::hash(&source).unwrap(), digest);
    let restored = ask(&digest).await.expect("restoring the same bytes must allow the thumbnail to be drawn");
    assert_eq!(restored.status(), StatusCode::OK);
    let bytes = to_bytes(restored.into_body(), 1 << 20).await.unwrap();
    assert!(!bytes.is_empty());
    println!("restored draw: {} thumbnail bytes", bytes.len());
    let name = format!("{hex}-256-0");
    let cache = home.thumbs();
    let marker = cache.join(format!("{name}.failed"));
    assert!(!marker.exists());
    let cached = ask(&digest).await.unwrap();
    assert_eq!(to_bytes(cached.into_body(), 1 << 20).await.unwrap(), bytes);

    // Upgrading also recovers when a prior process already poisoned this cache key.
    fs::remove_file(kept(&cache, &name).unwrap()).unwrap();
    fs::write(marker, "Error opening input: No such file or directory").unwrap();
    assert_eq!(ask(&digest).await.unwrap().status(), StatusCode::OK);

    let malformed = ask(&malformed_digest).await.unwrap_err();
    assert_eq!(malformed.0.code, "thumb_failed");
    assert_eq!(malformed.into_response().status(), StatusCode::BAD_REQUEST);
    println!("malformed input: HTTP 400 thumb_failed");
    calcifer.close();
}
