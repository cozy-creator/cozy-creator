//! Immutable private native dependencies travel with an executable, including updates.
//! This is a binary dependency contract, never a Runtime/Cozy version or commit gate.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
};

pub const MANIFEST: &str = "varena-native.json";
#[derive(Deserialize, Serialize)]
struct Library {
    name: String,
    sha256: String,
}
#[derive(Deserialize, Serialize)]
struct Manifest {
    schema: u32,
    binary: String,
    libraries: Vec<Library>,
}

fn manifest(bytes: &[u8]) -> io::Result<Manifest> {
    let value: Manifest = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    let basename = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            && s != "."
            && s != ".."
    };
    if value.schema != 1
        || !basename(&value.binary)
        || value.libraries.is_empty()
        || value.libraries.iter().any(|f| {
            !basename(&f.name)
                || f.name == value.binary
                || f.name == MANIFEST
                || f.sha256.len() != 64
                || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(io::Error::other("invalid native dependency manifest"));
    }
    Ok(value)
}
fn install(bytes: &[u8], file: &Library, directory: &Path) -> io::Result<()> {
    if hex::encode(Sha256::digest(bytes)) != file.sha256 {
        return Err(io::Error::other("native library digest mismatch"));
    }
    let path = directory.join(&file.name);
    if path.exists() {
        if hex::encode(Sha256::digest(fs::read(&path)?)) == file.sha256 {
            return Ok(());
        }
        return Err(io::Error::other(format!(
            "refusing to replace different immutable native dependency {}",
            path.display()
        )));
    }
    // New content-addressed names never replace a library used by the previous binary.
    // Stage completely before atomic installation; a failed update leaves old owners intact.
    let mut output = tempfile::Builder::new()
        .prefix(".native-")
        .tempfile_in(directory)?;
    output.write_all(bytes)?;
    output.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        output
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    match output.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(error)
            if error.error.kind() == io::ErrorKind::AlreadyExists
                && hex::encode(Sha256::digest(fs::read(&path)?)) == file.sha256 =>
        {
            Ok(())
        }
        Err(error) => Err(error.error),
    }
}

/// Install the signed/digest-checked archive's library members beside its executable.
/// Old archives with no manifest remain valid. Never extract arbitrary archive paths.
pub fn libraries(
    archive: &mut (impl Read + Seek),
    prefix: &str,
    directory: &Path,
) -> io::Result<bool> {
    let mut bytes = Vec::new();
    archive.rewind()?;
    match crate::tools::unpack(&mut *archive, &format!("{prefix}{MANIFEST}"), &mut bytes) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        other => {
            other?;
        }
    }
    let value = manifest(&bytes)?;
    fs::create_dir_all(directory)?;
    for library in &value.libraries {
        archive.rewind()?;
        let mut body = Vec::new();
        crate::tools::unpack(
            &mut *archive,
            &format!("{prefix}{}", library.name),
            &mut body,
        )?;
        install(&body, library, directory)?;
    }
    fs::write(directory.join(MANIFEST), bytes)?;
    Ok(true)
}

/// Carry an already verified staged bundle's libraries to the activation directory.
/// Both the old and new immutable library names survive rollback.
pub fn copy_libraries(source: &Path, destination: &Path) -> io::Result<()> {
    let bytes = match fs::read(source.join(MANIFEST)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        other => other?,
    };
    let value = manifest(&bytes)?;
    fs::create_dir_all(destination)?;
    for file in &value.libraries {
        install(&fs::read(source.join(&file.name))?, file, destination)?;
    }
    Ok(())
}

/// A locally installed private-linked binary can still be supplied as --tensord.
/// The packager emits these immutable loader names on ELF, Mach-O and PE. Locate
/// exactly those names in the binary, then verify their content identities before
/// transporting them. Static older binaries return None and keep their old path.
pub fn pack_binary(binary: &Path, destination: &Path) -> io::Result<Option<PathBuf>> {
    let body = fs::read(binary)?;
    if body.starts_with(b"PK") || body.starts_with(&[0x1f, 0x8b]) {
        return Ok(None);
    }
    let mut names = std::collections::BTreeSet::new();
    for prefix in [
        "libvarena-private-",
        "varena-private-",
        "libstd-private-",
        "std-private-",
    ] {
        for (at, window) in body.windows(prefix.len()).enumerate() {
            if window != prefix.as_bytes() {
                continue;
            }
            if at != 0 && !matches!(body[at - 1], 0 | b'/' | b'\\') {
                continue;
            }
            let tail = &body[at + prefix.len()..];
            if tail.len() < 33 || !tail[..32].iter().all(u8::is_ascii_hexdigit) {
                continue;
            }
            for suffix in [".so", ".dylib", ".dll"] {
                if tail[32..].starts_with(suffix.as_bytes()) {
                    names.insert(format!(
                        "{prefix}{}{suffix}",
                        std::str::from_utf8(&tail[..32]).unwrap()
                    ));
                }
            }
        }
    }
    if names.is_empty() {
        return Ok(None);
    }
    let directory = binary.parent().unwrap_or(Path::new("."));
    let mut libraries = Vec::new();
    let mut contents = Vec::new();
    for name in names {
        let bytes = fs::read(directory.join(&name))?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        if !name.contains(&sha256[..32]) {
            return Err(io::Error::other(
                "private native dependency content changed",
            ));
        }
        libraries.push(Library { name, sha256 });
        contents.push(bytes);
    }
    let name = if body.starts_with(b"MZ") {
        "cozy.exe"
    } else {
        "cozy"
    };
    let value = Manifest {
        schema: 1,
        binary: name.into(),
        libraries,
    };
    let temporary = tempfile::Builder::new()
        .prefix(".bundle-")
        .tempfile_in(destination.parent().unwrap_or(Path::new(".")))?;
    let encoder = flate2::write::GzEncoder::new(temporary, flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    let mut append = |name: &str, bytes: &[u8], mode| -> io::Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, name, bytes)
    };
    append(name, &body, 0o755)?;
    append(
        MANIFEST,
        &serde_json::to_vec(&value).map_err(io::Error::other)?,
        0o644,
    )?;
    for (library, bytes) in value.libraries.iter().zip(&contents) {
        append(&library.name, bytes, 0o755)?;
    }
    let temporary = archive.into_inner()?.finish()?;
    temporary.as_file().sync_all()?;
    temporary.persist(destination).map_err(|e| e.error)?;
    Ok(Some(destination.to_owned()))
}

/// The existing dev-binary upload can contain a root-level bundle archive. Its original
/// bytes/digest remain intact; the runnable payload is staged in a sibling directory.
pub fn staged(path: &Path) -> io::Result<PathBuf> {
    let mut archive = fs::File::open(path)?;
    let mut magic = [0u8; 2];
    archive.read_exact(&mut magic)?;
    if magic != *b"PK" && magic != [0x1f, 0x8b] {
        return Ok(path.to_owned());
    }
    archive.rewind()?;
    let mut bytes = Vec::new();
    crate::tools::unpack(&mut archive, MANIFEST, &mut bytes)?;
    let value = manifest(&bytes)?;
    let directory = path.with_extension("bundle");
    fs::create_dir_all(&directory)?;
    libraries(&mut archive, "", &directory)?;
    archive.rewind()?;
    let binary = directory.join(value.binary);
    let mut output = fs::File::create(&binary)?;
    crate::tools::unpack(
        &mut archive,
        binary.file_name().unwrap().to_str().unwrap(),
        &mut output,
    )?;
    output.sync_all()?;
    drop(output);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    }
    Ok(binary)
}
