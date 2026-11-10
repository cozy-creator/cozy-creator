use super::*;
use axum::{Router, http::StatusCode};
use std::{io::Cursor, time::Duration};

struct Hub {
    client: ::api::hub::Client,
    requests: Arc<Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Hub {
    async fn serving(replies: impl FnOnce(&str) -> Vec<(StatusCode, Vec<u8>)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let replies = Arc::new(Mutex::new(VecDeque::from(replies(&origin))));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let router = Router::new().fallback(move |uri: axum::http::Uri| {
            seen.lock().unwrap().push(uri.to_string());
            let reply = replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or((StatusCode::INTERNAL_SERVER_ERROR, Vec::new()));
            async move { reply }
        });
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = ::api::hub::Client::connect(
            &origin,
            tensorfs::fetch::http(&[]).unwrap().build().unwrap(),
            ::api::hub::Credential::Anonymous,
        );
        Self {
            client,
            requests,
            server,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn plan(downloads: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "release": "1.0", "downloads": downloads, "package_interface": {}, "pyproject": {}, "uv_lock": {}
    })).unwrap()
}

fn refusal(code: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"error": {"code": code, "message": "kept message", "remedy": "kept remedy"}})).unwrap()
}

#[tokio::test]
async fn default_archive_plan_is_not_retried() {
    let hub = Hub::serving(|_| vec![(StatusCode::OK, plan(serde_json::json!([])))]).await;
    archive_plan(&hub.client, "author", "example", "1.0")
        .await
        .unwrap();
    assert_eq!(
        hub.requests(),
        ["/v1/packages/author/example/download?release=1.0"]
    );
}

#[tokio::test]
async fn arm_only_ui_uses_the_verified_archive() {
    const UI: &[u8] = b"export default () => 'ARM package UI';";
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "example/ui/main.js",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(UI).unwrap();
    let wheel = zip.finish().unwrap().into_inner();
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&wheel)));
    let hub = Hub::serving(|origin| {
        vec![
            (
                StatusCode::CONFLICT,
                refusal("package_download.project_wheel_unavailable"),
            ),
            (
                StatusCode::OK,
                plan(serde_json::json!([{
                    "path": "example-1.0-cp312-cp312-manylinux_2_28_aarch64.whl", "digest": digest,
                    "url": format!("{origin}/wheel"), "length": wheel.len()
                }])),
            ),
            (StatusCode::OK, wheel.clone()),
            (StatusCode::OK, b"changed bytes".to_vec()),
        ]
    })
    .await;
    let plan = archive_plan(&hub.client, "author", "example", "1.0")
        .await
        .unwrap();
    let file = plan
        .downloads
        .iter()
        .find(|file| wheel_of(&file.path, "example"))
        .unwrap();
    let kept = fetched("example/generate", file).await.unwrap();
    let files = from_wheel("example/generate", kept, "example", "ui/main.js").unwrap();
    assert_eq!(files["ui/main.js"].as_ref(), UI);
    assert_eq!(
        hub.requests(),
        [
            "/v1/packages/author/example/download?release=1.0",
            "/v1/packages/author/example/download?release=1.0&cpu_arch=aarch64",
            "/wheel",
        ]
    );
    let err = fetched("example/generate", file).await.unwrap_err();
    assert_eq!(err.code, "tool_ui_hash_mismatch");
    let oversized = PackageInstallFile {
        length: (MOST_WHEEL + 1) as i64,
        ..PackageInstallFile::default()
    };
    assert!(fetched("example/generate", &oversized).await.is_err());
    assert_eq!(
        hub.requests().len(),
        4,
        "the oversized file must not be requested"
    );
}

#[tokio::test]
async fn unavailable_other_errors_and_network_failures_do_not_loop() {
    let code = "package_download.project_wheel_unavailable";
    let hub = Hub::serving(|_| {
        vec![
            (StatusCode::CONFLICT, refusal(code)),
            (StatusCode::CONFLICT, refusal(code)),
        ]
    })
    .await;
    assert!(
        matches!(archive_plan(&hub.client, "author", "example", "1.0").await, Err(::api::hub::Error::Hub { code: actual, .. }) if actual == code)
    );
    assert_eq!(
        hub.requests().len(),
        2,
        "only the one ARM fallback is allowed"
    );
    for (status, code) in [
        (StatusCode::UNAUTHORIZED, "unauthorized"),
        (StatusCode::FORBIDDEN, "forbidden"),
        (StatusCode::NOT_FOUND, "package.not_found"),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    ] {
        let hub = Hub::serving(|_| vec![(status, refusal(code))]).await;
        match archive_plan(&hub.client, "author", "example", "1.0")
            .await
            .unwrap_err()
        {
            ::api::hub::Error::Hub {
                code: actual,
                message,
                remedy,
                ..
            } => {
                assert_eq!(actual, code);
                assert_eq!(message, "kept message");
                assert_eq!(remedy, "kept remedy");
            }
            err => panic!("unexpected error: {err}"),
        }
        assert_eq!(hub.requests().len(), 1);
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let http = tensorfs::fetch::http(&[]).unwrap()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let client = ::api::hub::Client::connect(&origin, http, ::api::hub::Credential::Anonymous);
    assert!(matches!(
        archive_plan(&client, "author", "example", "1.0").await,
        Err(::api::hub::Error::Call(_))
    ));
}
