//! Authorized disposable diagnostics only. The caller must first prove the old writer and
//! every attributed execution quiescent and retain canonical history in the project journal.
//! No execution rows, results, outputs, staging, model data or retention records are touched.
use std::{io, path::Path};

pub(super) fn retire_disposable(
    machine_logs: &Path,
    engine_root: &Path,
) -> io::Result<Vec<String>> {
    crate::config::with_legacy_diagnostics_quiet(|| {
        #[cfg(target_os = "linux")]
        {
            linux::retire(machine_logs, engine_root)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (machine_logs, engine_root);
            Ok(vec!["diagnostic_retirement_platform_unqualified".into()])
        }
    })
}

pub(super) fn retire_store_diagnostics(store: &Path) -> io::Result<Vec<String>> {
    crate::config::with_legacy_diagnostics_quiet(|| {
        #[cfg(target_os = "linux")]
        {
            linux::store_logs(store)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = store;
            Ok(vec![
                "store_diagnostic_retirement_platform_unqualified".into()
            ])
        }
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
    use serde::Deserialize;
    use std::{
        ffi::{OsStr, OsString},
        fs::File,
        io::Read as _,
        os::{
            fd::OwnedFd,
            unix::{
                ffi::{OsStrExt as _, OsStringExt as _},
                fs::MetadataExt as _,
            },
        },
        path::{Component, PathBuf},
    };

    const MAX_ENTRIES: usize = 4096;
    const MAX_PEAK_BYTES: u64 = 1 << 20;
    const MAX_TOTAL_PEAK_BYTES: u64 = 16 << 20;
    struct Directory {
        fd: OwnedFd,
        path: PathBuf,
    }
    struct Retirement {
        pending: Vec<String>,
        entries: usize,
        peak_bytes: u64,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Peak {
        key: String,
        bytes: u64,
    }

    impl Retirement {
        fn pending(&mut self, code: &str, path: &Path) {
            self.pending.push(format!("{code}:{}", path.display()));
        }
        fn root(&mut self, path: &Path) -> Option<Directory> {
            if !path.is_absolute() {
                self.pending("relative_diagnostic_root", path);
                return None;
            }
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let mut fd = match rustix::fs::open("/", flags, Mode::empty()) {
                Ok(fd) => fd,
                Err(_) => {
                    self.pending("unreadable_diagnostic_root", path);
                    return None;
                }
            };
            for component in path.components() {
                match component {
                    Component::RootDir => (),
                    Component::Normal(name) => {
                        match rustix::fs::openat(&fd, name, flags, Mode::empty()) {
                            Ok(next) => fd = next,
                            Err(rustix::io::Errno::NOENT) => return None,
                            Err(_) => {
                                self.pending("unsafe_or_unreadable_diagnostic_root", path);
                                return None;
                            }
                        }
                    }
                    _ => {
                        self.pending("untrusted_diagnostic_root_component", path);
                        return None;
                    }
                }
            }
            Some(Directory {
                fd,
                path: path.to_owned(),
            })
        }
        fn list(&mut self, root: &Directory) -> Vec<(OsString, FileType)> {
            let read = match Dir::read_from(&root.fd) {
                Ok(read) => read,
                Err(_) => {
                    self.pending("unreadable_diagnostic_directory", &root.path);
                    return vec![];
                }
            };
            let mut found = vec![];
            for entry in read {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        self.pending("unreadable_diagnostic_entry", &root.path);
                        break;
                    }
                };
                let name = entry.file_name().to_bytes();
                if name == b"." || name == b".." {
                    continue;
                }
                if self.entries >= MAX_ENTRIES {
                    self.pending("diagnostic_retirement_entry_limit", &root.path);
                    break;
                }
                self.entries += 1;
                let name = OsString::from_vec(name.to_vec());
                match rustix::fs::statat(&root.fd, &name, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(found_stat) => {
                        found.push((name, FileType::from_raw_mode(found_stat.st_mode)))
                    }
                    Err(rustix::io::Errno::NOENT) => (),
                    Err(_) => self.pending("unreadable_diagnostic_entry", &root.path.join(name)),
                }
            }
            found.sort_by(|a, b| a.0.cmp(&b.0));
            found
        }
        fn open(
            &mut self,
            root: &Directory,
            name: &OsStr,
            write: bool,
        ) -> Option<(File, std::fs::Metadata)> {
            let path = root.path.join(name);
            let flags = (if write { OFlags::RDWR } else { OFlags::RDONLY })
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC;
            let fd = match rustix::fs::openat(&root.fd, name, flags, Mode::empty()) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => return None,
                Err(_) => {
                    self.pending("unsafe_or_unwritable_diagnostic", &path);
                    return None;
                }
            };
            let file = File::from(fd);
            let metadata = match file.metadata() {
                Ok(metadata)
                    if metadata.is_file()
                        && metadata.nlink() == 1
                        && metadata.uid() == rustix::process::geteuid().as_raw() =>
                {
                    metadata
                }
                _ => {
                    self.pending("unproven_diagnostic_file_identity", &path);
                    return None;
                }
            };
            if !Self::same_name(root, name, &metadata) {
                self.pending("diagnostic_file_replaced", &path);
                return None;
            }
            Some((file, metadata))
        }
        fn same_name(root: &Directory, name: &OsStr, metadata: &std::fs::Metadata) -> bool {
            rustix::fs::statat(&root.fd, name, AtFlags::SYMLINK_NOFOLLOW).is_ok_and(|found| {
                FileType::from_raw_mode(found.st_mode) == FileType::RegularFile
                    && found.st_dev == metadata.dev()
                    && found.st_ino == metadata.ino()
                    && found.st_nlink == 1
            })
        }
        fn truncate_log(&mut self, root: &Directory, name: &OsStr) {
            let path = root.path.join(name);
            let Some((file, before)) = self.open(root, name, true) else {
                return;
            };
            let steady = file.metadata().is_ok_and(|now| {
                now.len() == before.len() && now.modified().ok() == before.modified().ok()
            });
            if !steady || !Self::same_name(root, name, &before) {
                self.pending("diagnostic_writer_not_quiescent", &path);
                return;
            }
            // Preserve the current append writer's inode. No rename, backup or outside copy.
            if file.set_len(0).and_then(|()| file.sync_all()).is_err() {
                self.pending("diagnostic_truncation_pending", &path);
                return;
            }
            if !file.metadata().is_ok_and(|now| now.len() == 0)
                || !Self::same_name(root, name, &before)
            {
                self.pending("diagnostic_changed_during_retirement", &path);
            }
        }
        fn remove_peak(&mut self, root: &Directory, name: &OsStr) {
            let path = root.path.join(name);
            let Some((mut file, before)) = self.open(root, name, false) else {
                return;
            };
            if before.len() > MAX_PEAK_BYTES
                || self.peak_bytes.saturating_add(before.len()) > MAX_TOTAL_PEAK_BYTES
            {
                self.pending("diagnostic_peak_byte_limit", &path);
                return;
            }
            let mut bytes = Vec::new();
            if file
                .by_ref()
                .take(MAX_PEAK_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
            {
                self.pending("unreadable_diagnostic_peak", &path);
                return;
            }
            self.peak_bytes += bytes.len() as u64;
            let known = serde_json::from_slice::<Vec<Peak>>(&bytes).is_ok_and(|peaks| {
                peaks.iter().all(|peak| {
                    let _ = peak.bytes; // The exact documented high-water field, not an arbitrary JSON file.
                    peak.key
                        .split_once('|')
                        .is_some_and(|(construction, shape)| {
                            !construction.is_empty() && !shape.is_empty() && !shape.contains('|')
                        })
                })
            });
            if !known {
                self.pending("unknown_measurement_cache_format", &path);
                return;
            }
            let steady = bytes.len() as u64 == before.len()
                && file.metadata().is_ok_and(|now| {
                    now.len() == before.len() && now.modified().ok() == before.modified().ok()
                });
            if !steady || !Self::same_name(root, name, &before) {
                self.pending("measurement_cache_changed", &path);
                return;
            }
            if rustix::fs::unlinkat(&root.fd, name, AtFlags::empty()).is_err() {
                self.pending("measurement_cache_retirement_pending", &path);
                return;
            }
            if rustix::fs::fsync(&root.fd).is_err() {
                self.pending("measurement_cache_directory_sync_pending", &root.path);
            }
        }
    }

    fn daily_name(name: &OsStr) -> bool {
        let Some(name) = name.to_str() else {
            return false;
        };
        if name == "tensord.log" {
            return true;
        }
        name.strip_prefix("tensord.")
            .and_then(|name| name.strip_suffix(".log"))
            .is_some_and(|day| day.len() == 10 && day.parse::<jiff::civil::Date>().is_ok())
    }
    fn peak_name(name: &OsStr) -> bool {
        name.to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .is_some_and(|model| {
                !model.is_empty()
                    && model
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            })
    }
    pub(super) fn store_logs(store: &Path) -> io::Result<Vec<String>> {
        let mut retirement = Retirement {
            pending: vec![],
            entries: 0,
            peak_bytes: 0,
        };
        if let Some(logs) = retirement.root(&store.join("logs")) {
            for name in [
                "memory.log",
                "memory.log.1",
                "transport.log",
                "transport.log.1",
            ] {
                let name = OsStr::new(name);
                match rustix::fs::statat(&logs.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(found)
                        if FileType::from_raw_mode(found.st_mode) == FileType::RegularFile =>
                    {
                        retirement.truncate_log(&logs, name)
                    }
                    Ok(_) => retirement.pending("unsafe_store_diagnostic", &logs.path.join(name)),
                    Err(rustix::io::Errno::NOENT) => (),
                    Err(_) => {
                        retirement.pending("unreadable_store_diagnostic", &logs.path.join(name))
                    }
                }
            }
        }
        Ok(retirement.pending)
    }
    pub(super) fn retire(machine_logs: &Path, engine_root: &Path) -> io::Result<Vec<String>> {
        let mut retirement = Retirement {
            pending: vec![],
            entries: 0,
            peak_bytes: 0,
        };
        if let Some(logs) = retirement.root(machine_logs) {
            for (name, kind) in retirement.list(&logs) {
                if !daily_name(&name) {
                    // Do not retire stdout/stderr external sinks or infer ownership of user files.
                    retirement.pending("unrecognized_machine_log_entry", &logs.path.join(name));
                    continue;
                }
                if kind != FileType::RegularFile {
                    retirement.pending("unsafe_machine_log_entry", &logs.path.join(name));
                    continue;
                }
                retirement.truncate_log(&logs, &name);
            }
        }
        if let Some(peaks) = retirement.root(&engine_root.join("gpu/peaks")) {
            for (name, kind) in retirement.list(&peaks) {
                if !peak_name(&name) || kind != FileType::RegularFile {
                    retirement.pending(
                        "unrecognized_measurement_cache_entry",
                        &peaks.path.join(name),
                    );
                    continue;
                }
                retirement.remove_peak(&peaks, &name);
            }
        }
        retirement.pending.sort();
        retirement.pending.dedup();
        Ok(retirement.pending)
    }
}
