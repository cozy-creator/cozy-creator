//! `tfs conform endpoint`: a black-box check of an endpoint against `spec/distribution.md`, over
//! a fixture commit of [`FIXTURE`] (lane `int8`) built the same way every time. With `write` it
//! uploads the fixture and checks the write side first; without, the fixture's objects must be
//! there (a `write` run over the same storage seeds them). A case is a MUST or a SHOULD; buckets
//! and static servers fail some SHOULDs (no closure check, no Range) by design.

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;

use base64::engine::{general_purpose::STANDARD, Engine as _};
use super::endpoint::{upload_commit, Endpoint, Hardware, Payload, UploadControl};
use super::{Client, Config, Fetcher, ObjectStore, Origin, Want};
use crate::err::{Code, Refusal, Result};
use crate::format::{Config as HeaderConfig, Dtype, ObjectRef, Tree};
use crate::sha256::hex_digest;
use crate::store::Store;
use crate::writer::{Data, Source};

pub const FIXTURE: &str = "conformance/endpoint";

/// One case's outcome.
#[derive(Debug, Clone)]
pub struct Case {
    pub name: &'static str,
    pub must: bool,
    /// `None` when the case does not apply (a directory has no HTTP statuses).
    pub ok: Option<bool>,
    pub detail: String,
}

struct Cases(Vec<Case>);

impl Cases {
    fn record(&mut self, name: &'static str, must: bool, outcome: Option<std::result::Result<(), String>>) -> bool {
        let (ok, detail) = match outcome {
            None => (None, "n/a".to_owned()),
            Some(Ok(())) => (Some(true), String::new()),
            Some(Err(why)) => (Some(false), why),
        };
        self.0.push(Case { name, must, ok, detail });
        ok != Some(false)
    }
}

fn expect(cond: bool, why: impl FnOnce() -> String) -> std::result::Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(why())
    }
}

/// The fixture in a store under `scratch`: a one-component cozytensors as a commit in lane
/// `int8`, with a README. The tree.
fn fixture(scratch: &Path) -> Result<(Store, ObjectRef)> {
    let failed = crate::store::Error::refusal;
    let store = Store::open(scratch.join("fixture")).map_err(failed)?;
    let mut w = store.writer(None, None).map_err(failed)?;
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8).collect();
    let config: HeaderConfig = serde_json::from_str(r#"{"conformance":1}"#).unwrap();
    let mut c = w.component("model", Default::default(), config, None).map_err(failed)?;
    c.put("w", &Dtype::U8, &[bytes.len() as u64], Data::Plain(Source::Read(&mut &bytes[..]))).map_err(failed)?;
    let header = w.commit().map_err(failed)?;
    let readme = store.put_bytes(b"# cozytensors distribution conformance\n").map_err(failed)?;
    let tree = Tree::of("int8", header, &[("README.md".to_owned(), readme)].into())
        .map_err(|e| Refusal { code: Code::UNKNOWN_FORMAT, detail: e.to_string() })?;
    let tree = store.put_tree(&tree).map_err(failed)?;
    Ok((store, tree))
}

/// Runs the suite at `endpoint`, building the fixture under `scratch`.
pub async fn endpoint(endpoint: Arc<Endpoint>, client: &Client, write: bool, scratch: &Path) -> Result<Vec<Case>> {
    let (store, tree) = fixture(scratch)?;
    let mut cases = Cases(Vec::new());
    let nonce = hex_digest(format!("{:?}", SystemTime::now()).as_bytes());
    if write && !write_side(&endpoint, &store, &tree, &nonce, &mut cases).await? {
        return Ok(cases.0);
    }
    read_side(&endpoint, client, &store, &tree, &nonce, scratch, &mut cases).await?;
    Ok(cases.0)
}

async fn write_side(endpoint: &Arc<Endpoint>, store: &Store, tree: &ObjectRef, nonce: &str, cases: &mut Cases) -> Result<bool> {
    let control = UploadControl { cancelled: &|| false, progress: &|_, _| {}, commit: &|| true };
    let up = upload_commit(endpoint, FIXTURE, tree, store, &control).await.map(drop).map_err(|e| e.to_string());
    if !cases.record("upload: HEAD 404 → PUT → HEAD 200, then the commit recorded", true, Some(up)) {
        return Ok(false);
    }
    let again = endpoint.commit(FIXTURE, tree).await;
    cases.record("commits: recording a held commit again says so", true, Some(expect(matches!(again, Ok(true)), || format!("{again:?}"))));

    let claimed = ObjectRef::of(format!("conformance {nonce}").as_bytes());
    let wrong = Bytes::from(vec![b'x'; claimed.length as usize]);
    let sent = endpoint.upload(&claimed, &move || Ok(Payload::Bytes(wrong.clone()))).await;
    let held = endpoint.has(&claimed).await?;
    let outcome = expect(sent.is_err() && !held, || format!("answered {sent:?}, holds it: {held}"));
    cases.record("upload: wrong bytes are refused and not stored", true, Some(outcome));

    let readme = store.tree(tree).map_err(crate::store::Error::refusal)?.files["README.md"].clone();
    let bytes = Bytes::from(store.read(&readme).map_err(crate::store::Error::refusal)?);
    let checksum = STANDARD.encode(crate::sha256::digest(&bytes));
    let headers = vec![
        ("content-length".to_owned(), bytes.len().to_string()),
        ("x-amz-checksum-sha256".to_owned(), checksum),
        ("if-none-match".to_owned(), "*".to_owned()),
    ];
    let again = endpoint.raw("PUT", &readme.key(), headers, Some(bytes)).await?;
    let outcome = again.map(|(status, ..)| expect(matches!(status, 200 | 201 | 204 | 412), || format!("answered {status}")));
    cases.record("upload: a repeated PUT is idempotent", true, outcome);

    // A commit citing a header nobody uploaded.
    let missing = ObjectRef::of(format!("absent header {nonce}").as_bytes());
    let cited = Tree::of("int8", missing.clone(), &Default::default()).and_then(|t| t.to_json())
        .map_err(|e| Refusal { code: Code::UNKNOWN_FORMAT, detail: e.to_string() })?;
    let cited_ref = ObjectRef::of(&cited);
    let cited = Bytes::from(cited);
    endpoint.upload(&cited_ref, &move || Ok(Payload::Bytes(cited.clone()))).await?;
    let refused = endpoint.commit(FIXTURE, &cited_ref).await;
    let directory = endpoint.raw("HEAD", &cited_ref.key(), vec![], None).await?.is_none();
    let outcome = expect(refused.as_ref().is_err_and(|e| e.detail.contains(&missing.sha256)), || format!("{refused:?}"));
    cases.record("commits: an incomplete closure is refused, naming what is missing (servers)", false, (!directory).then_some(outcome));
    Ok(true)
}

async fn read_side(
    endpoint: &Arc<Endpoint>,
    client: &Client,
    store: &Store,
    tree: &ObjectRef,
    nonce: &str,
    scratch: &Path,
    cases: &mut Cases,
) -> Result<()> {
    let absent = endpoint.resolve(&format!("conformance/absent-{}", &nonce[..12]), None, None, &Hardware::default()).await;
    let outcome = expect(absent.as_ref().is_err_and(|e| e.code == Code::REF_NOT_FOUND), || format!("{absent:?}"));
    cases.record("resolve: an absent model is 404", true, Some(outcome));

    // Down through the Fetcher: redirects followed, every object checked.
    let into = Store::open(scratch.join("pulled")).map_err(crate::store::Error::refusal)?;
    let fetcher = Fetcher::new(Arc::new(into.clone()) as Arc<dyn ObjectStore>, client.clone(), None, Config::default());
    let origin: Arc<dyn Origin> = endpoint.clone();
    let pulled = fetcher.pull_tree(&origin, tree, &Want::default()).await;
    fetcher.settled().await;
    let outcome = pulled.map(|p| p.objects).map_err(|e| e.to_string()).and_then(|n| expect(n >= 4, || format!("{n} objects")));
    cases.record("blobs: GET returns each object, verified", true, Some(outcome));

    let header = store.tree(tree).map_err(crate::store::Error::refusal)?.header()
        .map_err(|e| Refusal { code: Code::UNKNOWN_FORMAT, detail: e.to_string() })?.1.clone();
    let head = endpoint.raw("HEAD", &header.key(), vec![], None).await?;
    let outcome = head.map(|(status, headers, _)| {
        let length = headers.get("content-length").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        expect(status == 200 && length == Some(header.length), || format!("answered {status}, length {length:?}"))
    });
    cases.record("blobs: HEAD is 200 with the length", true, outcome);
    let last = header.length - 1;
    let ranged = endpoint.raw("GET", &header.key(), vec![("range".into(), format!("bytes={last}-{last}"))], None).await?;
    let expected = store.read(&header).map_err(crate::store::Error::refusal)?[last as usize];
    let outcome = ranged.map(|(status, _, body)| match status {
        300..=399 => Ok(()),
        _ => expect(status == 206 && body == [expected], || format!("answered {status} with {} bytes", body.len())),
    });
    cases.record("blobs: Range gives 206 with that byte (or redirects)", false, outcome);
    let unknown = ObjectRef { sha256: hex_digest(nonce.as_bytes()), length: 1 };
    let held = endpoint.has(&unknown).await;
    cases.record("blobs: an unknown object is 404", true, Some(expect(matches!(held, Ok(false)), || format!("{held:?}"))));
    Ok(())
}
