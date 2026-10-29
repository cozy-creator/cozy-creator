//! An environment's import pages (xs-132): the page-cache pages its import-only parent's import
//! read, recorded once beside the environment, and asked of the kernel again (`WILLNEED`) at
//! machine start, so a later import after a reboot or a page-cache eviction reads no more from
//! disk than a warm one. SDXL on a laptop: 4.4 s cached, 11.1 s with its files evicted (518 MB
//! read in 7,200 faults); its pages read back first, 4.1 s. Asked as the import starts, they
//! competed with its own reads and bought nothing, so nothing races an import.
//! The parent's shared objects count the pages its own page tables hold, never the whole file
//! (SDXL touches 541 MB of the 3.7 GB its 71 objects hold); its other files (bytecode, sources,
//! data) count what the page cache holds of them. Only reclaimable cache: nothing is locked or
//! held, and the kernel drops it under pressure.
use crate::catalog::HeldGeneration;
use std::{
    io,
    path::{Path, PathBuf},
};

/// Kept in the generation's directory, so it goes with the environment; one per App (a callee's
/// parent imports its own package).
fn file(held: &HeldGeneration) -> Option<PathBuf> {
    let app: String = held.record.application.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    Some(directory(held)?.join(format!("import-pages-{app}.json")))
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Pages {
    page: u64,
    bytes: u64,
    /// Each file's cached pages as `[first page, count]` runs.
    files: Vec<(PathBuf, Vec<(u64, u64)>)>,
}

/// The generation's directory: its interpreter is `<directory>/env/bin/python` (`Catalog::resolve`).
fn directory(held: &HeldGeneration) -> Option<&Path> {
    held.record.python.parent()?.parent()?.parent()
}

/// What `held`'s record asks: its bytes; 0 when none is recorded.
pub fn recorded(held: &HeldGeneration) -> u64 {
    read(held).map_or(0, |pages| pages.bytes)
}

fn read(held: &HeldGeneration) -> Option<Pages> {
    let raw = std::fs::read(file(held)?).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Right after `held`'s parent (`parent`) imported: what of its environment and interpreter it
/// read, unless an earlier import recorded it. Off the caller's thread.
pub fn record_later(held: &HeldGeneration, parent: platform::process::Birth) {
    let Some(path) = file(held) else { return };
    if path.exists() {
        return;
    }
    let held = held.clone();
    let started = std::thread::Builder::new().name("import-pages".into()).spawn(move || {
        platform::process::background_thread();
        if let Err(error) = record(&held, &parent, &path) {
            tracing::warn!("recording {}'s import pages: {error}", held.record.package);
        }
    });
    if let Err(error) = started {
        tracing::warn!("recording import pages: {error}");
    }
}

fn record(held: &HeldGeneration, parent: &platform::process::Birth, path: &Path) -> io::Result<()> {
    let page = platform::durable::page_size();
    let mut mapped = mapped(parent.pid, page);
    if parent.ended().unwrap_or(true) {
        return Err(io::Error::other("its import-only parent ended before its pages were read"));
    }
    let env = held.record.python.parent().and_then(Path::parent).ok_or_else(|| io::Error::other("no environment"))?;
    let interpreter = std::fs::canonicalize(&held.record.python)?;
    let mut roots = vec![env.to_path_buf()];
    roots.extend(interpreter.parent().and_then(Path::parent).map(|base| base.join("lib")));
    let mut pages = Pages { page, ..Default::default() };
    let mut add = |file: PathBuf, runs: Vec<(u64, u64)>| {
        if !runs.is_empty() {
            pages.bytes += runs.iter().map(|(_, count)| count * page).sum::<u64>();
            pages.files.push((file, runs));
        }
    };
    for file in files(&roots) {
        if let Some(touched) = mapped.remove(&file) {
            add(file, runs(touched));
        } else if !shared_object(&file) {
            let Ok(open) = std::fs::File::open(&file) else { continue };
            add(file, platform::durable::cached_runs(&open));
        }
    }
    for (file, touched) in mapped {
        add(file, runs(touched));
    }
    platform::durable::swap(path, &serde_json::to_vec(&pages).map_err(io::Error::other)?, 0o600)?;
    tracing::info!("{}: {} MB of import pages in {} files", held.record.package, pages.bytes >> 20, pages.files.len());
    Ok(())
}

/// `held`'s recorded pages, asked of the kernel at the lowest CPU and I/O priority, off the
/// caller's thread.
pub fn read_ahead_later(held: Vec<HeldGeneration>) {
    if held.is_empty() {
        return;
    }
    let started = std::thread::Builder::new().name("read-ahead".into()).spawn(move || {
        platform::process::background_thread();
        for pages in held.iter().filter_map(read) {
            for (path, runs) in &pages.files {
                let Ok(file) = std::fs::File::open(path) else { continue };
                for (first, count) in runs {
                    platform::durable::will_need(&file, first * pages.page, count * pages.page);
                }
            }
        }
    });
    if let Err(error) = started {
        tracing::warn!("reading import pages ahead: {error}");
    }
}

/// A shared object (`libtorch.so`, `_C.cpython-312-x86_64-linux-gnu.so.1`): an import maps it,
/// so what it read is what its page tables hold.
fn shared_object(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(".so") || name.contains(".so."))
}

fn runs(pages: std::collections::BTreeSet<u64>) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = vec![];
    for page in pages {
        match out.last_mut() {
            Some((first, count)) if *first + *count == page => *count += 1,
            _ => out.push((page, 1)),
        }
    }
    out
}

/// The file pages `pid`'s page tables hold for each regular file it maps (`/proc/<pid>/maps`
/// with `pagemap`'s present bit): what that process touched of them. Empty where it cannot be
/// read (another user's process) or the OS has no such files.
fn mapped(pid: u32, page: u64) -> std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<u64>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut out: std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<u64>> = Default::default();
    let (Ok(maps), Ok(mut pagemap)) = (
        std::fs::read_to_string(format!("/proc/{pid}/maps")),
        std::fs::File::open(format!("/proc/{pid}/pagemap")),
    ) else {
        return out;
    };
    for line in maps.lines() {
        // start-end perms offset dev inode path
        let fields: Vec<&str> = line.splitn(6, ' ').collect();
        let [range, _, offset, _, inode, path] = fields[..] else { continue };
        let path = path.trim_start();
        if inode == "0" || !path.starts_with('/') || path.ends_with(" (deleted)") {
            continue;
        }
        let Some((start, end)) = range.split_once('-') else { continue };
        let (Ok(start), Ok(end), Ok(offset)) =
            (u64::from_str_radix(start, 16), u64::from_str_radix(end, 16), u64::from_str_radix(offset, 16))
        else {
            continue;
        };
        let count = (end - start) / page;
        let mut entries = vec![0u8; count as usize * 8];
        if pagemap.seek(SeekFrom::Start(start / page * 8)).is_err() || pagemap.read_exact(&mut entries).is_err() {
            continue;
        }
        let touched = out.entry(PathBuf::from(path)).or_default();
        for (i, entry) in entries.chunks_exact(8).enumerate() {
            // Bit 63: the page is present in this process's page tables.
            if u64::from_le_bytes(entry.try_into().expect("8 bytes")) >> 63 == 1 {
                touched.insert(offset / page + i as u64);
            }
        }
    }
    out.retain(|_, touched| !touched.is_empty());
    out
}

/// Every regular file under `roots`, links followed, each once.
fn files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let (mut seen, mut out, mut todo) = (std::collections::HashSet::new(), vec![], roots.to_vec());
    while let Some(dir) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let (Ok(meta), Ok(id)) = (std::fs::metadata(&path), platform::durable::FileId::at(&path)) else { continue };
            if !seen.insert(id) {
                continue;
            }
            if meta.is_dir() {
                todo.push(path);
            } else if meta.is_file() && meta.len() > 0 {
                out.push(std::fs::canonicalize(&path).unwrap_or(path));
            }
        }
    }
    out
}
