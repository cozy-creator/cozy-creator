//! Actual Writer/Checkpoint/Source clients. GPU modes are operator-owned and UUID-bound.
use serde_json::{json, Value};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};
use tensorfs::io::source::{IoTally, Item, Mode, Placement, Source};
use tensorfs::io::{self, Dst, ItemSource, ReadControl};
use tensorfs::{Checkpoint, Client, Config, WriterOptions};

#[path = "raw_source/mod.rs"]
mod proof;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const PAYLOAD: usize = 1 << 20;
const FILE_AT: usize = 4096;
const INLINE_AT: usize = FILE_AT + PAYLOAD + 4096;
const LENGTH: usize = INLINE_AT + 17 + 4096;

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
struct Recipe {
    checkpoint: Checkpoint,
    source: Arc<Source>,
    items: Vec<Item>,
    expected: Vec<u8>,
    payload: Vec<u8>,
    chunk_path: PathBuf,
}
fn checkpoint(root: &Path) -> Result<Recipe> {
    std::fs::create_dir(root)?; // Preserve previous fixtures/receipts; never overwrite a run.
    let cache = root.join("cache");
    let client = Client::new(Config {
        endpoint: "https://unused.invalid".into(),
        cache_dir: cache.clone(),
        mirror: None,
        token: None,
        ca_der: None,
    })?;
    let path = root.join("source.cozytensors");
    let mut writer = client.writer(WriterOptions {
        dest: Some(&path),
        ..Default::default()
    })?;
    let payload = (0..PAYLOAD)
        .map(|index| ((index * 37 + 11) % 251) as u8)
        .collect::<Vec<_>>();
    writer
        .component(
            "fixture",
            tensorfs::format::ClassRef::new("tensorfs", "0", "RawSourceFixture"),
            Default::default(),
            None,
        )?
        .put(
            "weight",
            tensorfs::format::Dtype::U8,
            &[PAYLOAD as u64],
            &mut Cursor::new(&payload),
        )?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let checkpoint = runtime.block_on(writer.commit())?;
    drop(writer);
    drop(client);
    let spans = checkpoint.spans(&[vec!["fixture/weight".into()]])?;
    require(
        spans.len() == 1 && spans[0].len == PAYLOAD as u64,
        "fixture checkpoint unexpectedly split the one stored part",
    )?;
    require(
        matches!(&spans[0].source, ItemSource::Object(_)),
        "checkpoint has no real file span",
    )?;
    let inline: Arc<[u8]> = (0..17).map(|n| 180 + n).collect::<Vec<u8>>().into();
    let items = vec![
        Item {
            offset: FILE_AT as u64,
            len: PAYLOAD as u64,
            source: spans[0].source.clone(),
            part_end: true,
        },
        Item {
            offset: INLINE_AT as u64,
            len: inline.len() as u64,
            source: ItemSource::Inline(inline.clone()),
            part_end: true,
        },
    ];
    let mut expected = vec![0; LENGTH];
    expected[FILE_AT..FILE_AT + PAYLOAD].copy_from_slice(&payload);
    expected[INLINE_AT..INLINE_AT + inline.len()].copy_from_slice(&inline);
    let source = Arc::new(checkpoint.local_source(false));
    let store = tensorfs::store::Store::open(&cache)?;
    let chunk_path = store.path(&checkpoint.chunks()[0])?;
    Ok(Recipe {
        checkpoint,
        source,
        items,
        expected,
        payload,
        chunk_path,
    })
}
fn host(root: &Path) -> Result<Value> {
    let recipe = checkpoint(root)?;
    let mut output = vec![99; LENGTH];
    let tally = IoTally::default();
    let stats = unsafe {
        recipe.source.read_placements(
            &[Placement {
                items: &recipe.items,
                origin: 0,
                dst: Dst::host(&mut output),
            }],
            2,
            Mode::Buffered,
            &tally,
            None,
        )?
    };
    require(
        output == recipe.expected,
        "real checkpoint/inline/padding host bytes differ",
    )?;
    require(
        stats.bytes == (PAYLOAD + 17) as u64,
        "host logical byte accounting differs",
    )?;
    let mut overlapping = recipe.items.clone();
    overlapping[1].offset = FILE_AT as u64;
    let mut refused = vec![123; LENGTH];
    let result = unsafe {
        recipe.source.read_placements(
            &[Placement {
                items: &overlapping,
                origin: 0,
                dst: Dst::host(&mut refused),
            }],
            2,
            Mode::Buffered,
            &IoTally::default(),
            None,
        )
    };
    require(
        result.is_err() && refused.iter().all(|&byte| byte == 123),
        "overlap validation wrote a destination",
    )?;
    let cancel = ReadControl::default();
    cancel.cancel();
    let result = unsafe {
        recipe.source.read_placements_cancelable(
            &[Placement {
                items: &recipe.items,
                origin: 0,
                dst: Dst::host(&mut refused),
            }],
            2,
            Mode::Buffered,
            &IoTally::default(),
            &cancel,
        )
    };
    // The existing joined host reader initializes padding before its piece-level
    // cancellation checks. Preserve that exact contract; the new raw GPU operation's
    // cancellation-before-submission proof is a separate actual-GPU mode.
    let padding = (LENGTH - PAYLOAD - 17) as u64;
    require(
        result.is_err() && cancel.issued_bytes() == padding,
        "joined host cancellation changed its exact padding progress",
    )?;
    require(
        refused[FILE_AT..FILE_AT + PAYLOAD]
            .iter()
            .all(|&byte| byte == 123)
            && refused[INLINE_AT..INLINE_AT + 17]
                .iter()
                .all(|&byte| byte == 123),
        "joined host cancellation copied file or inline data",
    )?;
    let maps = std::fs::read_to_string("/proc/self/maps")?;
    require(
        !maps.contains("libcuda.so"),
        "host Source proof initialized CUDA",
    )?;
    Ok(
        json!({"passed":true,"mode":"host","checkpoint":recipe.checkpoint.id(),
        "logical_bytes":stats.bytes,"full_bytes":output.len(),"file_inline_padding_exact":true,
        "overlap_refused_before_write":true,"joined_host_cancel_padding_bytes":padding,"cuda_loaded":false}),
    )
}
fn run() -> Result<Value> {
    let args = std::env::args().collect::<Vec<_>>();
    let mode = args.get(1).ok_or("mode required")?;
    let root = PathBuf::from(args.get(2).ok_or("new fixture output directory required")?);
    if mode == "host" {
        return host(&root);
    }
    let uuid = args.get(3).ok_or("actual GPU UUID required")?;
    let device = args.get(4).map_or(Ok(0), |value| value.parse::<i32>())?;
    proof::run(mode, &root, uuid, device)
}
fn main() {
    match run() {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("{}", json!({"passed":false,"error":error.to_string()}));
            std::process::exit(1);
        }
    }
}
