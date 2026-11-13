//! CI-only typed native baselines for the large browser fixtures. This is not a project loader.
use std::{collections::BTreeMap, fs, io::Write as _, path::PathBuf, sync::Arc};

use api::calcifer::project::Card;
use serde::Deserialize;

use super::{
    crdt, db,
    folder::{Asset, File, Lock, Measured},
    graph::name_key,
    written::{Boxed, Written},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
    directory: PathBuf,
    receipt: PathBuf,
    /// Each box in order, its fields as stored.
    boxes: Vec<Boxed>,
    assets: BTreeMap<String, Asset>,
    makes: BTreeMap<String, String>,
    files: BTreeMap<String, File>,
    measured: BTreeMap<String, Measured>,
    cards: BTreeMap<String, Card>,
    expected_assets: usize,
    expected_versions: usize,
}

#[test]
#[ignore = "CI-only producer, explicitly invoked by the browser fixture"]
fn seed_current_web_project() {
    assert!(
        std::env::var("GITHUB_ACTIONS").is_ok_and(|value| value == "true"),
        "native web seeds run only in ephemeral CI"
    );
    let input = PathBuf::from(std::env::var_os("COZY_WEB_SEED").expect("seed manifest"));
    let seed: Seed = serde_json::from_slice(&fs::read(input).unwrap()).unwrap();
    let directory = fs::canonicalize(&seed.directory).unwrap();
    for name in [db::DB, "project.db.new", "project.vault.keys.json"] {
        assert!(
            !directory.join(name).exists(),
            "seed requires fresh native storage, found {name}"
        );
    }
    let written = Written {
        boxes: seed.boxes,
        ..Written::default()
    };
    assert_eq!(written.boxes.len(), seed.expected_assets);
    assert_eq!(seed.cards.len(), seed.expected_assets);
    assert_eq!(
        seed.assets
            .values()
            .map(|asset| asset.versions.len())
            .sum::<usize>(),
        seed.expected_versions
    );
    assert!(
        seed.assets
            .values()
            .all(|asset| asset.versions.len() == 2 && asset.numbered == 2)
    );
    // The old workloads intentionally used unique word-derived references for
    // hardlinked PNGs. Do not hash/deduplicate them through media upload APIs.
    for hash in seed.measured.keys() {
        let hex = hash.strip_prefix("sha256:").expect("file reference");
        assert!(
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert!(directory.join("files").join(format!("{hex}.png")).is_file());
    }
    // Local file rows were empty in the baseline: normal Folder::fill discovers
    // these PNGs; only their already-measured dimensions are persisted here.
    assert!(seed.files.is_empty());
    let measured = seed.measured.len();
    let canvas = seed
        .cards
        .into_iter()
        .map(|(name, card)| {
            (
                name_key(&name),
                db::CardRow {
                    name,
                    card,
                    seq: 0,
                    by: String::new(),
                },
            )
        })
        .collect();
    let stored = db::Stored {
        written: Arc::new(written),
        canvas: Arc::new(canvas),
        lock: Arc::new(Lock {
            assets: seed.assets,
            makes: seed.makes,
            files: seed.files,
            ..Default::default()
        }),
        measured: Arc::new(seed.measured),
        ..Default::default()
    };
    let path = directory.join(db::DB);
    db::create(&path, &stored, &[], &[]).unwrap();
    let mut conn = db::open(&path).unwrap();
    db::set_identity(
        &mut conn,
        &path,
        &super::fresh_id(),
        directory.file_name().unwrap().to_str().unwrap(),
        None,
    )
    .unwrap();
    let loaded = db::load(&conn, &path).unwrap();
    assert_eq!(loaded.written.boxes.len(), seed.expected_assets);
    assert_eq!(loaded.canvas.len(), seed.expected_assets);
    assert_eq!(
        loaded
            .lock
            .assets
            .values()
            .map(|asset| asset.versions.len())
            .sum::<usize>(),
        seed.expected_versions
    );
    assert_eq!(loaded.measured.len(), measured);
    assert!(loaded.crdt.is_some());
    let schema: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let format: i64 = conn
        .query_row("SELECT format FROM crdt_document WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .unwrap();
    drop(conn);
    // What this build writes, beside what it wrote: the fixture holds the seed to the product's current schema, not a number of its own.
    let receipt = serde_json::json!({ "schema": schema, "crdt_format": format, "current_schema": db::VERSION,
        "current_crdt_format": crdt::FORMAT, "assets": seed.expected_assets, "versions": seed.expected_versions, "measured": measured,
        "canvas": seed.expected_assets, "local_file_rows": 0 });
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(seed.receipt)
        .unwrap();
    output
        .write_all(serde_json::to_string(&receipt).unwrap().as_bytes())
        .unwrap();
    output.sync_all().unwrap();
}
