//! The hardware ceilings copies are measured against (design §13): a volume's O_DIRECT
//! sequential read rate here, a GPU's pinned H2D rate in `metrics`. Each is kept per boot in
//! the TensorFS store ([`keep_ceilings_in`]'s, else [`crate::home::resolve`]'s) at `tmp/ceilings/table`,
//! tab-separated `key value…` lines; a key's highest first value wins, so a probe taken at a
//! contended moment, or a copy that ran faster than it, never leaves a ceiling stuck low.

use std::collections::BTreeMap;
use std::cmp::Reverse;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use platform::durable;

use super::host::{read_at, Bounce};

/// Bytes a probe reads at most.
const PROBE: u64 = 1 << 30;
/// Bytes a probe must read for its rate to stand for the volume (and be kept).
pub(super) const KEEP: u64 = 256 << 20;
/// 4 MiB requests on 8 threads: on four RTX 4090 pods' volumes within 5% of the fastest O_DIRECT
/// case of a thread x request grid (3.3-6.2 GB/s), where 1 MiB requests read 3-20% under it, and a
/// ceiling taken so made every bound that lenient (cl-1318).
const BLOCK: usize = 4 << 20;
const THREADS: usize = 8;

/// The measured ceilings for a volume and, optionally, a GPU.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ceiling {
    /// Sequential read GB/s of the volume.
    pub disk_gbps: f64,
    /// Measured past the page cache (O_DIRECT); else the filesystem refused it.
    pub disk_direct: bool,
    /// Pinned host→device GB/s, when a CUDA device was requested.
    pub h2d_gbps: Option<f64>,
    /// Streams used by the fastest measured H2D probe.
    pub streams: Option<u32>,
}

/// What reads from `path`'s volume (and, with `device`, copies to that GPU) can reach.
pub fn ceiling(device: Option<i32>, path: &Path) -> io::Result<Ceiling> {
    #[cfg(all(unix, feature = "cuda"))]
    let gpu = device.map(|device| super::metrics::h2d(device).ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, format!("CUDA device {device} has no measurable H2D ceiling")))).transpose()?;
    #[cfg(not(all(unix, feature = "cuda")))]
    let gpu: Option<(f64, u32)> = if device.is_some() { return Err(io::Error::new(io::ErrorKind::Unsupported, "H2D ceiling is unavailable on this platform")); } else { None };
    let (disk_gbps, disk_direct) = disk(path)?;
    Ok(Ceiling {
        disk_gbps,
        disk_direct,
        h2d_gbps: gpu.map(|g| g.0),
        streams: gpu.map(|g| g.1),
    })
}

/// GB/s reading the volume holding `path` (a file, or a folder of them): its largest files in
/// whole blocks, up to 1 GiB, on 8 threads, O_DIRECT where the filesystem allows (else through
/// the page cache, and `false`). Where they hold under 256 MiB, a scratch file of `SCRATCH` bytes
/// written beside them is read instead: a probe of a few small files measured their opening, not the
/// disk (6e-06 GB/s on RTX 4090 pods whose TensorFS home was empty, against `dd`'s 3.7-5.5, cl-1318).
pub fn disk(path: &Path) -> io::Result<(f64, bool)> {
    let volume = durable::device(path).map_err(io::Error::other)?;
    if let Some(seen) = disk_known(volume) {
        return Ok(seen);
    }
    let mut found = Vec::new();
    files(path, volume, &mut found)?;
    found.sort_by_key(|f| Reverse(f.0));
    let held: u64 = found.iter().map(|f| f.0).sum();
    let scratch = (held < KEEP).then(|| written(path)).transpose()?;
    if let Some(scratch) = &scratch {
        found = vec![(SCRATCH, scratch.clone())];
    }
    let seen = read_blocks(&found);
    if let Some(scratch) = scratch {
        let _ = fs::remove_file(scratch);
    }
    let (read, seen) = seen?;
    if read >= KEEP {
        disk_keep(volume, seen);
    }
    Ok(seen)
}

/// Bytes of the scratch file a probe writes where the volume's files are too few to read.
const SCRATCH: u64 = 512 << 20;

/// A scratch file of `SCRATCH` bytes beside `path`, written, synced and out of the page cache: what a
/// probe reads where the volume holds too few bytes of its own. Its bytes vary, so no filesystem
/// compresses them away.
fn written(path: &Path) -> io::Result<PathBuf> {
    let dir = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")) };
    let at = dir.join(format!(".tensorfs-disk-probe-{}", std::process::id()));
    let made = (|| {
        let mut f = File::create(&at)?;
        let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ u64::from(std::process::id());
        let mut chunk = vec![0u8; 8 << 20];
        for _ in 0..SCRATCH / chunk.len() as u64 {
            for word in chunk.chunks_exact_mut(8) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                word.copy_from_slice(&state.to_le_bytes());
            }
            f.write_all(&chunk)?;
        }
        f.sync_all()?;
        #[cfg(target_os = "linux")]
        let _ = rustix::fs::fadvise(&f, 0, None, rustix::fs::Advice::DontNeed);
        Ok(())
    })();
    if let Err(error) = made {
        let _ = fs::remove_file(&at);
        return Err(error);
    }
    Ok(at)
}

/// The largest of `found`, up to `PROBE` bytes, in whole blocks on `THREADS` threads, O_DIRECT where
/// every file allows: the bytes read and (GB/s, whether O_DIRECT).
fn read_blocks(found: &[(u64, PathBuf)]) -> io::Result<(u64, (f64, bool))> {
    let (mut opened, mut blocks, mut total) = (Vec::new(), Vec::new(), 0);
    for (len, p) in found.iter().take_while(|(len, _)| *len > 0) {
        if total >= PROBE {
            break;
        }
        let f = File::open(p)?;
        opened.push((durable::open_direct(&f).ok().flatten(), f));
        for at in (0..*len).step_by(BLOCK) {
            if total >= PROBE {
                break;
            }
            let n = (len - at).min(BLOCK as u64).min(PROBE - total);
            blocks.push((opened.len() - 1, at, n as usize));
            total += n;
        }
    }
    if blocks.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no bytes to read"));
    }
    let direct = opened.iter().all(|(d, _)| d.is_some());
    let (next, bytes, error) = (AtomicUsize::new(0), AtomicU64::new(0), Mutex::new(None));
    let start = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..THREADS.min(blocks.len()) {
            s.spawn(|| {
                let mut bounce = Bounce::default();
                while let Some(&(i, at, need)) = blocks.get(next.fetch_add(1, Relaxed)) {
                    let (d, f) = &opened[i];
                    match read_at(
                        if direct { d.as_ref().unwrap() } else { f },
                        bounce.get(BLOCK),
                        at,
                    ) {
                        Ok(n) if n >= need => bytes.fetch_add(need as u64, Relaxed),
                        Ok(_) => {
                            return *error.lock().unwrap() = Some(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "disk probe file changed while reading",
                            ))
                        }
                        Err(e) => return *error.lock().unwrap() = Some(e),
                    };
                }
            });
        }
    });
    if let Some(e) = error.into_inner().unwrap() {
        return Err(e);
    }
    let read = bytes.into_inner();
    Ok((read, (read as f64 / start.elapsed().as_nanos().max(1) as f64, direct)))
}

/// The kept ceiling of volume `volume` this boot: GB/s and whether it was O_DIRECT.
pub(crate) fn disk_known(volume: u64) -> Option<(f64, bool)> {
    match known(&format!("disk {volume}"))?.as_slice() {
        &[rate, direct] if direct == 0.0 || direct == 1.0 => Some((rate, direct == 1.0)),
        _ => None,
    }
}

/// Keeps `seen` for volume `volume`, unless a higher rate is kept already.
pub(crate) fn disk_keep(volume: u64, (rate, direct): (f64, bool)) {
    keep(&format!("disk {volume}"), &[rate, f64::from(u8::from(direct))]);
}

/// The kept values of `key` this boot: the line with the highest first value.
pub(crate) fn known(key: &str) -> Option<Vec<f64>> {
    let key = format!("{} {key}", boot()?);
    if let Some(seen) = MEMO.lock().unwrap().get(&key) {
        return Some(seen.clone());
    }
    let seen = saved(&fs::read_to_string(table()?).ok()?, &key)?;
    MEMO.lock().unwrap().insert(key, seen.clone());
    Some(seen)
}

/// Keeps `values` for `key` this boot, when its first value beats the one kept.
pub(crate) fn keep(key: &str, values: &[f64]) {
    let first = values.first().copied().unwrap_or(0.0);
    if !(first.is_finite() && first > 0.0) || known(key).is_some_and(|k| k[0] >= first) {
        return;
    }
    let Some((boot, path)) = boot().zip(table()) else { return };
    let key = format!("{boot} {key}");
    MEMO.lock().unwrap().insert(key.clone(), values.to_vec());
    let line: String = values.iter().map(|v| format!("\t{v}")).collect();
    let _ = fs::create_dir_all(path.parent().expect("the table has a directory"));
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        if f.lock().is_ok() {
            let _ = f.write_all(format!("{key}{line}\n").as_bytes());
        }
    }
}

static MEMO: Mutex<BTreeMap<String, Vec<f64>>> = Mutex::new(BTreeMap::new());

/// This boot's id, read once a process: every read's ceiling lookup asks it.
fn boot() -> Option<String> {
    static BOOT: OnceLock<Option<String>> = OnceLock::new();
    BOOT.get_or_init(|| platform::process::boot_id().ok().filter(|s| !s.is_empty())).clone()
}

static STORE: OnceLock<PathBuf> = OnceLock::new();

/// Keeps this process's ceilings in the store at `root` (the one `tfs --store` names), not the
/// home one; the first root named holds.
pub fn keep_ceilings_in(root: &Path) {
    let _ = STORE.set(root.to_path_buf());
}

/// `tmp/ceilings/table` in the TensorFS store. A directory in `tmp/` is its maker's: GC leaves it.
fn table() -> Option<PathBuf> {
    let store = match STORE.get() {
        Some(root) => root.clone(),
        None => crate::home::resolve(None).ok()?,
    };
    Some(store.join("tmp").join("ceilings").join("table"))
}

/// The values of `key`'s line with the highest positive, finite first value.
fn saved(text: &str, key: &str) -> Option<Vec<f64>> {
    let lines = text.lines().filter_map(|l| l.strip_prefix(key)?.strip_prefix('\t'));
    let values = lines.filter_map(|l| l.split('\t').map(|v| v.parse::<f64>().ok()).collect::<Option<Vec<f64>>>());
    let valid = values.filter(|v| v.first().is_some_and(|f| f.is_finite() && *f > 0.0));
    valid.max_by(|a, b| a[0].total_cmp(&b[0]))
}

fn files(path: &Path, volume: u64, out: &mut Vec<(u64, PathBuf)>) -> io::Result<()> {
    if durable::device(path).map_err(io::Error::other)? != volume {
        return Ok(());
    }
    let meta = fs::metadata(path)?;
    if meta.is_file() {
        out.push((meta.len(), path.to_path_buf()));
    }
    if !meta.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_symlink() {
            files(&entry.path(), volume, out)?;
        }
    }
    Ok(())
}
