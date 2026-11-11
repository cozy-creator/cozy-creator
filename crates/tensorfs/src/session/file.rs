//! A checkpoint as one read-only file, assembled from the store on demand: its header, then its
//! chunks and assets in header order, each at the next 4 KiB boundary, with zero gaps.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use crate::failure::{Failure, Kind, Result};
use crate::format::ObjectRef;
use crate::io::{Copy, Dst, ReadOpts, Src};

use super::{Checkpoint, Download};

/// The checkpoint as one read-only, seekable file: its header, then its chunks and assets in
/// header order at 4 KiB-aligned offsets, assembled from the store on demand; nothing is
/// written. It holds what it may read until dropped. A read fetches the objects it lacks when
/// the checkpoint may fetch, so read from outside an async context.
pub struct CheckpointFile {
    checkpoint: Checkpoint,
    header: Arc<[u8]>,
    /// Each object's offset, in file order.
    layout: Vec<(u64, ObjectRef)>,
    /// The objects the last read used.
    opened: Vec<(ObjectRef, Opened)>,
    len: u64,
    pos: u64,
}

impl Checkpoint {
    /// This checkpoint as a [`CheckpointFile`].
    pub fn open_file(&self) -> Result<CheckpointFile> {
        let bytes = self.header_bytes();
        let (layout, len) = crate::store::file::layout(self.header(), bytes.len() as u64)?;
        Ok(CheckpointFile {
            checkpoint: self.clone(),
            header: bytes,
            layout,
            opened: Vec::new(),
            len,
            pos: 0,
        })
    }

    /// Atomically write the complete portable checkpoint. Only the header and a bounded
    /// copy buffer are held in memory. Read outside an async context, as for `open_file`.
    pub fn write_file(&self, dest: &std::path::Path) -> Result<()> {
        crate::store::copy_checked(&mut self.open_file()?, dest, None)?;
        Ok(())
    }
}

impl CheckpointFile {

    /// Its length in bytes: the last object's end.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Up to `buf.len()` bytes from `at`, through the read engine; 0 at or past the end.
    #[allow(unsafe_code)]
    fn read_at(&mut self, at: u64, buf: &mut [u8]) -> Result<usize> {
        let end = self.len.min(at.saturating_add(buf.len() as u64));
        if at >= end {
            return Ok(0);
        }
        let first = self
            .layout
            .partition_point(|(start, o)| start + o.length <= at);
        let last = self.layout.partition_point(|(start, _)| *start < end);
        let wanted: Vec<(u64, ObjectRef)> = self.layout[first..last]
            .iter()
            .filter(|(_, o)| o.length > 0)
            .cloned()
            .collect();
        self.open(wanted.iter().map(|(_, o)| o))?;
        let mut dst =
            |from: u64, to: u64| Dst::host(&mut buf[(from - at) as usize..(to - at) as usize]);
        let mut copies = Vec::new();
        let mut pos = at;
        if pos < self.header.len() as u64 {
            let to = end.min(self.header.len() as u64);
            copies.push(Copy {
                src: Src::Host(self.header[pos as usize..].as_ptr()),
                dst: dst(pos, to),
            });
            pos = to;
        }
        for (start, object) in &wanted {
            if pos < *start {
                copies.push(Copy {
                    src: Src::Zero,
                    dst: dst(pos, *start),
                });
                pos = *start;
            }
            let to = end.min(start + object.length);
            let opened = &self
                .opened
                .iter()
                .find(|(o, _)| o == object)
                .expect("opened above")
                .1;
            copies.push(Copy {
                src: opened.at(pos - start),
                dst: dst(pos, to),
            });
            pos = to;
        }
        if pos < end {
            copies.push(Copy {
                src: Src::Zero,
                dst: dst(pos, end),
            });
        }
        let opts = ReadOpts {
            direct: false,
            ..ReadOpts::default()
        };
        // SAFETY: the destinations are disjoint ranges of `buf`; header bytes, mappings and files
        // stay alive in `self` until the read returns.
        unsafe { crate::io::read(&copies, &opts) }.map_err(|e| e.source)?;
        Ok((end - at) as usize)
    }

    /// Opens `objects`, fetching those not here, and keeps them until the next read.
    fn open<'a>(&mut self, objects: impl Iterator<Item = &'a ObjectRef>) -> Result<()> {
        let mut kept = std::mem::take(&mut self.opened);
        let (mut opened, mut new) = (Vec::new(), Vec::new());
        for object in objects {
            match kept.iter().position(|(o, _)| o == object) {
                Some(i) => opened.push(kept.swap_remove(i)),
                None if !new.contains(object) && !opened.iter().any(|(o, _)| o == object) => {
                    new.push(object.clone())
                }
                None => {}
            }
        }
        let mut absent = Vec::new();
        for object in &new {
            if !self.checkpoint.stored().holds(object)? {
                absent.push(object.clone());
            }
        }
        if !absent.is_empty() {
            let want = Download::default();
            crate::raw::runtime()?.block_on(self.checkpoint.fetch_objects_exact(&absent, &want))?;
        }
        for object in new {
            let file = self.checkpoint.stored().object(&object)?;
            let length = object.length;
            opened.push((object, Opened::new(file, length)));
        }
        self.opened = opened;
        Ok(())
    }
}

impl Read for CheckpointFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.read_at(self.pos, buf).map_err(|f| {
            let kind = match f.kind {
                Kind::NotFound => io::ErrorKind::NotFound,
                _ => io::ErrorKind::Other,
            };
            io::Error::new(kind, f)
        })?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for CheckpointFile {
    /// Past the end is allowed, and reads there return nothing; before the start is not.
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let (base, by) = match to {
            SeekFrom::Start(at) => (at, 0),
            SeekFrom::End(by) => (self.len, by),
            SeekFrom::Current(by) => (self.pos, by),
        };
        let before = Failure {
            kind: Kind::Invalid,
            detail: "seek before the start".into(),
        };
        self.pos = base
            .checked_add_signed(by)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, before))?;
        Ok(self.pos)
    }
}

/// An object's bytes: mapped where the platform maps, else its file for positioned reads.
enum Opened {
    #[cfg(unix)]
    Mapped(Map),
    File(File),
}

impl Opened {
    fn new(file: File, length: u64) -> Opened {
        #[cfg(unix)]
        if let Some(map) = Map::new(&file, length) {
            return Opened::Mapped(map);
        }
        let _ = length;
        Opened::File(file)
    }

    fn at(&self, offset: u64) -> Src<'_> {
        match self {
            #[cfg(unix)]
            Opened::Mapped(map) => Src::Host(map.ptr.cast::<u8>().wrapping_add(offset as usize)),
            Opened::File(file) => Src::File(file, offset),
        }
    }
}

/// A read-only shared mapping of a whole object file.
#[cfg(unix)]
struct Map {
    ptr: *mut libc::c_void,
    len: usize,
}

// SAFETY: a read-only mapping, only ever read.
#[cfg(unix)]
#[allow(unsafe_code)]
unsafe impl Send for Map {}
// SAFETY: as above.
#[cfg(unix)]
#[allow(unsafe_code)]
unsafe impl Sync for Map {}

#[cfg(unix)]
#[allow(unsafe_code)]
impl Map {
    fn new(file: &File, length: u64) -> Option<Map> {
        use std::os::fd::AsRawFd;
        let len = usize::try_from(length).ok().filter(|&n| n > 0)?;
        // SAFETY: maps `len` bytes of an object file of that length (checked when it opened),
        // read-only; unmapped on drop.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        (ptr != libc::MAP_FAILED).then_some(Map { ptr, len })
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: this mapping, unmapped once.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Client, Config, Open, Selection};

    fn client(cache_dir: std::path::PathBuf) -> Client {
        Client::new(Config { cache_dir, endpoint: "http://127.0.0.1:1".into(), mirror: None, token: None, ca_der: None }).unwrap()
    }

    #[test]
    fn portable_checkpoint_write_open_and_seek() {
        let dir = tempfile::tempdir().unwrap();
        let source = client(dir.path().join("source-store"));
        let (header, weights, tokenizer) = crate::store::file::tests::fixture();
        source.store.put_bytes(&weights).unwrap();
        source.store.put_bytes(&tokenizer).unwrap();
        let id = source.store.put_bytes(&header.to_cbor().unwrap()).unwrap();
        let raw = source.store.checkpoint(&id).unwrap();
        let checkpoint = Checkpoint::managed(raw, None, None, None, Vec::new(), source.store.clone()).unwrap();
        let path = dir.path().join("only.cozytensors");
        checkpoint.write_file(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut logical = checkpoint.open_file().unwrap();
        assert_eq!(bytes.len() as u64, logical.len());
        for at in [0, 10, id.length - 1, 4095, 4096, 4100, logical.len() - 1, logical.len(), logical.len() + 1] {
            logical.seek(SeekFrom::Start(at)).unwrap();
            let mut actual = [0; 37];
            let n = logical.read(&mut actual).unwrap();
            let start = bytes.len().min(at as usize);
            assert_eq!(&actual[..n], &bytes[start..bytes.len().min(start + actual.len())]);
        }
        drop(logical);
        drop(checkpoint);
        drop(source);
        std::fs::remove_dir_all(dir.path().join("source-store")).unwrap();
        let fresh = client(dir.path().join("fresh-store"));
        let checkpoint = crate::raw::runtime().unwrap().block_on(fresh.open(path.to_str().unwrap(), &Open { local_files_only: true, ..Open::default() })).unwrap();
        assert_eq!(checkpoint.id(), &id);
        assert!(checkpoint.missing(&Selection::All).unwrap().is_empty());
        assert_eq!(checkpoint.component("model").unwrap().read_decoded("weight").unwrap(), weights);
        assert_eq!(checkpoint.component("model").unwrap().asset("tokenizer.json").unwrap(), tokenizer);
        assert!(checkpoint.component("model").unwrap().asset("missing-tokenizer.model").unwrap_err().to_string().contains("missing-tokenizer.model"));
        let second = dir.path().join("again.cozytensors");
        checkpoint.write_file(&second).unwrap();
        assert_eq!(std::fs::read(second).unwrap(), bytes);
    }

    #[test]
    fn portable_checkpoint_thin_is_explicit_and_failed_export_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path().join("store"));
        let (header, _, _) = crate::store::file::tests::fixture();
        let thin = dir.path().join("thin.cozytensors");
        std::fs::write(&thin, header.to_cbor().unwrap()).unwrap();
        let options = Open { local_files_only: true, lazy: true, ..Open::default() };
        let checkpoint = crate::raw::runtime().unwrap().block_on(client.open(thin.to_str().unwrap(), &options)).unwrap();
        assert!(!checkpoint.missing(&Selection::All).unwrap().is_empty());
        let dest = dir.path().join("preserved.cozytensors");
        std::fs::write(&dest, b"previous complete export").unwrap();
        assert!(checkpoint.write_file(&dest).is_err());
        assert_eq!(std::fs::read(dest).unwrap(), b"previous complete export");
    }
}
