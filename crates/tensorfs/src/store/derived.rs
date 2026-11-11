//! Derived pages: bytes a reader made from a checkpoint's (a LoRA baked into its weights), kept so
//! a later process reads them instead of making them again.
//!
//! `derived/<slot>/<page>` names the object of page `page` of a slot (one block of a component
//! under one fill identity; each overlay its pages were made under is a page). Its mtime is the
//! page's last use. The object is cache in its checkpoint's lane, kept by the use named by the
//! checkpoint's header (which a commit's use names too), so it goes when that lane goes; a record
//! whose object went is removed when it is next read.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use super::{fail, invalid, is_digest, touch, Result, Store};
use crate::format::ObjectRef;

/// A derived page: its key (a digest of what made its bytes) and its object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    pub page: String,
    pub object: ObjectRef,
}

impl Store {
    /// Page `page` of `slot`, or with none its page used last, when its object is here whole; the
    /// object and the record are then young.
    pub fn derived(&self, slot: &str, page: Option<&str>) -> Result<Option<Derived>> {
        let dir = self.root.join("derived").join(slot);
        if !is_digest(slot) || page.is_some_and(|page| !is_digest(page)) {
            return Err(invalid(&dir, "a derived page and its slot are named by digests"));
        }
        let pages: Vec<(String, PathBuf)> = match page {
            Some(page) => vec![(page.to_owned(), dir.join(page))],
            None => {
                let mut found: Vec<(SystemTime, String, PathBuf)> = Vec::new();
                for path in super::retention::list(&dir)? {
                    let Some(name) = path.file_name().and_then(|name| name.to_str()).filter(|name| is_digest(name)) else {
                        continue;
                    };
                    let used = fs::metadata(&path).and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    found.push((used, name.to_owned(), path));
                }
                found.sort_by(|a, b| b.0.cmp(&a.0));
                found.into_iter().map(|(_, name, path)| (name, path)).collect()
            }
        };
        for (page, path) in pages {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(fail("read", &path)(error)),
            };
            if let Ok(object) = serde_json::from_slice::<ObjectRef>(&bytes) {
                if self.reuse(&object)? {
                    touch(&path)?;
                    return Ok(Some(Derived { page, object }));
                }
            }
            // Its object went, or the record is not one: the next keep writes it again.
            let _ = fs::remove_file(&path);
        }
        Ok(None)
    }

    /// Keeps `bytes` as page `page` of `slot`, made from the checkpoint whose header is `of`:
    /// cached in that checkpoint's lane. None: the disk has no room for it, and nothing is kept.
    pub fn keep_derived(&self, slot: &str, page: &str, of: &ObjectRef, bytes: &[u8]) -> Result<Option<Derived>> {
        let path = self.root.join("derived").join(slot).join(page);
        if !is_digest(slot) || !is_digest(page) {
            return Err(invalid(&path, "a derived page and its slot are named by digests"));
        }
        if !self.maintain(bytes.len() as u64)? {
            return Ok(None);
        }
        let object = self.put_bytes(bytes)?;
        self.used(&of.sha256, [&object])?;
        // A record never names an object a crash could tear (an ephemeral store syncs once here).
        self.sync()?;
        let record = serde_json::to_vec(&object).map_err(|error| invalid(&path, error))?;
        self.commit(&path, &record, false)?;
        Ok(Some(Derived { page: page.to_owned(), object }))
    }
}
