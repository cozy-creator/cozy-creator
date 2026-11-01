//! Empty v2 → v3 transition. Never reinterpret a live table or credit its old holdings.
use super::*;
use std::sync::atomic::AtomicU64;

pub(super) const OLD_WORDS: usize = HEADER + 2 * (17 + ROWS * (20 + 6));
const RETIRED: u64 = u64::from_le_bytes(*b"VARENA!\0");
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Prepared under the caller's old publication lock. Keeping this separate from commit
/// makes the actual lock/tombstone protocol observable in the real-process release gate.
pub(super) struct Prepared {
    pid: u32,
    old: os::Mapping,
    _claims: Vec<os::Locked>,
    _new_file: Arc<os::Descriptor>,
    _new_publication: os::Locked,
    _new_map: os::Mapping,
    temporary: PathBuf,
    temporary_identity: (u64, u64),
    path: PathBuf,
}

impl Prepared {
    pub(super) fn prepare(file: &Arc<os::Descriptor>, path: &Path, expected: &[u64; 15]) -> Result<Self> {
        let (dev, ino, size) = file.identity()?;
        let named = fs::symlink_metadata(path)?;
        if named.file_type().is_symlink() || (named.dev(), named.ino()) != (dev, ino) {
            return Err(Error::Busy);
        }
        let actual = file.header(HEADER)?;
        let mut old_expected = *expected;
        old_expected[1] = 2;
        old_expected[2] = OLD_WORDS as u64;
        if size != (OLD_WORDS * 8) as u64
            || !matches!(actual[0], MAGIC | RETIRED)
            || actual[1..15] != old_expected[1..]
        {
            return Err(Error::Incompatible("older ledger identity or format"));
        }
        let mut claims = Vec::with_capacity(ROWS);
        for row in 0..ROWS {
            claims.push(file.lock((row + 1) as i64, false)?
                .ok_or(Error::Incompatible("older ledger participant is still active"))?);
        }
        // Row locks, not age or a zero byte count, establish that no old participant owns
        // this table. Stale rows may remain after process death; their bytes give no credit.
        let old = os::Mapping::new(file, HEADER)?;
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_file_name(format!(".{}.upgrade-{}-{}-{serial}",
            path.file_name().expect("ledger filename").to_string_lossy(), std::process::id(), os::monotonic_ns()?));
        let new_file = os::Descriptor::create_new(&temporary)?;
        let (new_dev, new_ino, _) = new_file.identity()?;
        let prepared = (|| {
            let publication = new_file.lock(0, false)?.ok_or(Error::Busy)?;
            new_file.resize(WORDS * 8)?;
            new_file.allocate(WORDS * 8)?;
            let map = os::Mapping::new(&new_file, WORDS)?;
            for word in HEADER..WORDS { map.store(word, 0, Ordering::Relaxed); }
            for (at, value) in Data::default().encode().into_iter().enumerate() {
                map.store(HEADER + at, value, Ordering::Relaxed);
            }
            for (at, value) in expected.iter().copied().enumerate().skip(1) {
                map.store(at, value, Ordering::Relaxed);
            }
            map.store(SELECTOR, 2, Ordering::Release);
            map.store(0, MAGIC, Ordering::Release);
            Ok((publication, map))
        })();
        let (publication, map) = match prepared {
            Ok(value) => value,
            Err(error) => {
                remove_owned(&temporary, (new_dev, new_ino));
                return Err(error);
            }
        };
        Ok(Self { pid: std::process::id(), old, _claims: claims, _new_file: new_file, _new_publication: publication,
            _new_map: map, temporary, temporary_identity: (new_dev, new_ino), path: path.into() })
    }

    pub(super) fn tombstone(&self) -> Result<(u64, u64)> {
        if self.pid != std::process::id() { return Err(Error::Inherited); }
        let magic = self.old.load(0, Ordering::Acquire);
        let selected = self.old.load(SELECTOR, Ordering::Acquire);
        // v2 checked its inode BEFORE taking publication. Its already-waiting transaction
        // must see generation0 after it acquires the old lock. A waiting old open must see
        // nonzero incompatible magic, never magic0 that it could initialize again.
        self.old.store(SELECTOR, 0, Ordering::Release);
        self.old.store(0, RETIRED, Ordering::Release);
        Ok((magic, selected))
    }

    pub(super) fn commit(self) -> Result<()> {
        let (magic, selected) = self.tombstone()?;
        if let Err(error) = fs::rename(&self.temporary, &self.path) {
            // Every old waiter is still excluded by the caller's publication lock.
            self.old.store(SELECTOR, selected, Ordering::Release);
            self.old.store(0, magic, Ordering::Release);
            return Err(error.into());
        }
        Ok(())
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if self.pid == std::process::id() { remove_owned(&self.temporary, self.temporary_identity); }
    }
}

fn remove_owned(path: &Path, identity: (u64, u64)) {
    if fs::symlink_metadata(path).is_ok_and(|meta| !meta.file_type().is_symlink()
        && (meta.dev(), meta.ino()) == identity) {
        let _ = fs::remove_file(path);
    }
}
