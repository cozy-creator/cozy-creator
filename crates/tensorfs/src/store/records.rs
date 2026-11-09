//! Refs record models: each model's releases and commits this store holds. Each is one JSON file, changed under its directory's lock and replaced by a
//! temp and a rename: a reader sees the old record or the new.
//!
//! The lock is `flock` on an `O_CLOEXEC` descriptor: held by its open file description, waited
//! for, never inspected. A child forked meanwhile holds it only until it execs. A short shared
//! acquisition fence also spans root publication so GC cannot delete bytes between reading
//! metadata and naming them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use platform::lock;

use super::{fail, invalid, platform, Error, Result, Store};
use crate::format::{repo_name, ObjectRef, Repo};

/// ASCII letters, digits, `.`, `_` and `-`, not leading `.`.
pub(crate) fn is_name(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && !text.starts_with('.')
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn ref_segment(segment: &str) -> String {
    let folded = segment.to_ascii_lowercase();
    let stem = folded.split('.').next().unwrap_or("");
    let device = matches!(stem, "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if is_name(&folded, 128) && !folded.ends_with('.') && !device {
        folded
    } else {
        // Hashing bounds Unicode and long names, without changing the protocol's raw repo_key
        // or its case folding.
        format!("%{}", ObjectRef::of(folded.as_bytes()).sha256)
    }
}

impl Store {
    pub(super) fn lock(&self, dir: &str) -> Result<lock::DirLock> {
        let path = self.root.join(dir);
        lock::dir(&path).map_err(platform("lock", path))
    }

    /// Makes every object the new `roots` reach young, then everything this handle admitted
    /// durable, so a record never names what GC or a crash could take: each root and what it
    /// reaches. A root must be here; what it reaches need not be (a pull may take some
    /// components only).
    fn retain<'a>(&self, roots: impl IntoIterator<Item = &'a ObjectRef>) -> Result<Vec<(String, Vec<ObjectRef>)>> {
        let mut reached = Vec::new();
        for root in roots {
            if !self.reuse(root)? {
                return Err(Error::Absent(root.sha256.clone()));
            }
            let closure = self.closure(root)?;
            for object in &closure {
                self.reuse(object)?;
            }
            reached.push((root.sha256.clone(), closure));
        }
        self.sync()?;
        Ok(reached)
    }

    /// Case-folded local filenames; a nonportable segment is its hash, prefixed `%`.
    fn ref_path(&self, name: &str) -> Result<PathBuf> {
        // The format's repository name, as the public Ref takes it; is_name remains the
        // stricter grammar for writer keys.
        match name.split_once('/') {
            Some((org, model)) if repo_name(name) => Ok(self
                .root
                .join("refs")
                .join(ref_segment(org))
                .join(format!("{}.json", ref_segment(model)))),
            _ => Err(invalid(
                self.root.join("refs"),
                format!("{name:?} is not org/name"),
            )),
        }
    }

    fn canonical_ref(path: &Path, name: &str) -> Result<Option<Repo>> {
        let repo = Self::parse_ref(path)?;
        if repo
            .as_ref()
            .is_some_and(|repo| !repo.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid(path, format!("ref does not name {name:?}")));
        }
        Ok(repo)
    }

    fn parse_ref(path: &Path) -> Result<Option<Repo>> {
        match fs::read(path) {
            Ok(bytes) => Repo::from_json(&bytes)
                .map(Some)
                .map_err(|error| invalid(path, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(fail("read", path)(error)),
        }
    }

    pub fn get_ref(&self, name: &str) -> Result<Option<Repo>> {
        Self::canonical_ref(&self.ref_path(name)?, name)
    }

    /// Changes ref `name` to what `change` makes of it; `None` removes it. Naming a commit is a
    /// use of it, and a `local/` ref roots it.
    pub fn update_ref(
        &self,
        name: &str,
        change: impl FnOnce(Option<Repo>) -> Option<Repo>,
    ) -> Result<Option<Repo>> {
        let path = self.ref_path(name)?;
        let _acquisition = super::retention::acquire(self)?;
        let _lock = self.lock("refs")?;
        let old = Self::canonical_ref(&path, name)?;
        let commits = |repo: &Option<Repo>| -> Vec<ObjectRef> {
            repo.iter().flat_map(Repo::roots).cloned().collect()
        };
        let before = commits(&old);
        let Some(new) = change(old) else {
            self.remove_record(&path)?;
            return Ok(None);
        };
        if !new.name.eq_ignore_ascii_case(name) {
            return Err(invalid(
                &path,
                format!("repo {:?} cannot be ref {name}", new.name),
            ));
        }
        let bytes = new.to_json().map_err(|error| invalid(&path, error))?;
        let after = commits(&Some(new.clone()));
        let reached = self.retain(after.iter().filter(|commit| !before.contains(commit)))?;
        self.commit(&path, &bytes, true)?;
        for (root, closure) in &reached {
            self.used(root, closure)?;
        }
        Ok(Some(new))
    }

    /// Every model record, by name.
    pub fn refs(&self) -> Result<Vec<Repo>> {
        let dir = self.root.join("refs");
        let mut repos = Vec::new();
        for org in fs::read_dir(&dir).map_err(fail("list", &dir))? {
            let org = org.map_err(fail("list", &dir))?.path();
            if !org.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&org).map_err(fail("list", &org))? {
                let path = entry.map_err(fail("list", &org))?.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    repos.extend(Self::parse_ref(&path)?);
                }
            }
        }
        repos.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(repos)
    }
}
