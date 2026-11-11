//! Complete portable checkpoints use the logical layout, without changing any object identity.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::{fail, invalid, Result, Store};
use crate::format::{Header, ObjectRef, Tree};

/// Read only the first CBOR item. Its definite lengths frame the header independently of payload.
pub(crate) fn header(path: &Path) -> Result<(File, Vec<u8>, Header)> {
    let mut file = File::open(path).map_err(fail("open", path))?;
    let size = file.metadata().map_err(fail("stat", path))?.len();
    fn item(file: &mut File, size: u64, depth: usize) -> std::io::Result<()> {
        let bad = |why| std::io::Error::new(std::io::ErrorKind::InvalidData, why);
        if depth > 64 {
            return Err(bad("checkpoint header is nested too deep"));
        }
        let mut first = [0];
        file.read_exact(&mut first)?;
        let (major, info) = (first[0] >> 5, first[0] & 31);
        if depth == 0 && major != 4 {
            return Err(bad("checkpoint header must be a CBOR array"));
        }
        let wide = match info {
            0..=23 => 0,
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return Err(bad("invalid or indefinite checkpoint header")),
        };
        let mut bytes = [0; 8];
        file.read_exact(&mut bytes[8 - wide..])?;
        let n = if wide == 0 {
            info as u64
        } else {
            u64::from_be_bytes(bytes)
        };
        match major {
            2 | 3 => {
                let end = file
                    .stream_position()?
                    .checked_add(n)
                    .filter(|end| *end <= size)
                    .ok_or_else(|| bad("truncated checkpoint header"))?;
                file.seek(SeekFrom::Start(end))?;
            }
            4 | 5 => {
                let count = n
                    .checked_mul(if major == 5 { 2 } else { 1 })
                    .filter(|count| *count <= size)
                    .ok_or_else(|| bad("checkpoint header count exceeds the file"))?;
                for _ in 0..count {
                    item(file, size, depth + 1)?;
                }
            }
            6 => item(file, size, depth + 1)?,
            _ => {}
        }
        Ok(())
    }
    item(&mut file, size, 0)
        .map_err(|e| invalid(path, format!("cannot read checkpoint header: {e}")))?;
    let extent = file.stream_position().map_err(fail("seek", path))?;
    let len = usize::try_from(extent)
        .map_err(|_| invalid(path, "checkpoint header exceeds this platform"))?;
    let mut bytes = vec![0; len];
    file.seek(SeekFrom::Start(0)).map_err(fail("seek", path))?;
    file.read_exact(&mut bytes)
        .map_err(fail("read header", path))?;
    let parsed = Header::from_cbor(&bytes)?;
    Ok((file, bytes, parsed))
}

/// Physical and logical files have exactly the same layout (spec §11).
pub(crate) fn layout(
    header: &Header,
    extent: u64,
) -> crate::format::Result<(Vec<(u64, ObjectRef)>, u64)> {
    let mut objects: Vec<_> = header.chunks.iter().collect();
    for component in header.components() {
        let mut assets: Vec<_> = component.assets.iter().collect();
        assets.sort_by_key(|(name, _)| (name.len(), name.as_bytes()));
        objects.extend(assets.into_iter().map(|(_, object)| object));
    }
    let (mut out, mut len, mut seen) = (Vec::new(), extent, BTreeMap::new());
    for object in objects {
        if seen
            .insert(&object.sha256, object.length)
            .is_some_and(|old| old != object.length)
        {
            return crate::format::invalid(format!(
                "{}: conflicting object lengths",
                object.sha256
            ));
        }
        let start = len
            .checked_add(4095)
            .map(|n| n / 4096 * 4096)
            .ok_or_else(|| {
                crate::format::Error::Invalid("checkpoint file offset overflows".into())
            })?;
        len = start.checked_add(object.length).ok_or_else(|| {
            crate::format::Error::Invalid("checkpoint file length overflows".into())
        })?;
        out.push((start, object.clone()));
    }
    Ok((out, len))
}

impl Store {
    /// Admit a complete `.cozytensors` file as a checkpoint in `lane`, returning its tree id.
    /// Every embedded occurrence is checked even when the CAS already holds that object.
    /// Header-only inputs with external references are thin and are refused here.
    pub fn admit_file(&self, path: &Path, lane: &str) -> Result<ObjectRef> {
        let (mut file, bytes, header) = header(path)?;
        let (layout, length) = layout(&header, bytes.len() as u64)?;
        let tree = Tree::of(lane, ObjectRef::of(&bytes), &BTreeMap::new())?;
        let actual = file.metadata().map_err(fail("stat", path))?.len();
        if actual != length {
            return Err(invalid(path, format!("complete checkpoint needs {length} bytes, has {actual} (thin, truncated or trailing data)")));
        }
        let header_id = ObjectRef::of(&bytes);
        let _hold = self.hold(
            &std::iter::once(header_id.clone())
                .chain(header.objects().cloned())
                .collect::<Vec<_>>(),
        )?;
        let mut cursor = bytes.len() as u64;
        let mut buffer = vec![0; 1 << 20];
        for (start, object) in layout {
            let gap = (start - cursor) as usize;
            file.read_exact(&mut buffer[..gap])
                .map_err(fail("read checkpoint padding", path))?;
            if buffer[..gap].iter().any(|&b| b != 0) {
                return Err(invalid(
                    path,
                    format!("nonzero checkpoint padding at {cursor}"),
                ));
            }
            // Do not use put(expect): it skips reading a source when its object is cached.
            let mut admission = self.admit(Some(&object))?;
            let mut left = object.length;
            while left > 0 {
                let n = left.min(buffer.len() as u64) as usize;
                file.read_exact(&mut buffer[..n])
                    .map_err(fail("read checkpoint object", path))?;
                std::io::Write::write_all(&mut admission, &buffer[..n])
                    .map_err(|e| admission.refusal(e))?;
                left -= n as u64;
            }
            admission.finish()?;
            cursor = start + object.length;
        }
        // Publish metadata only after all embedded bytes have passed verification.
        self.put_bytes(&bytes)?;
        let tree_id = self.put_tree(&tree)?;
        self.used(
            &tree_id.sha256,
            [&tree_id, &header_id].into_iter().chain(header.objects()),
        )?;
        Ok(tree_id)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format::{Body, ClassRef, Component, Dtype, Location, Tensor};
    use std::fs;
    use std::io::Write;

    pub(crate) fn fixture() -> (Header, Vec<u8>, Vec<u8>) {
        let weights = [1f32, 2., 3., 4.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let tokenizer = br#"{"model":{"vocab":{"hello":0}}}"#.to_vec();
        let component = Component {
            name: "model".into(),
            class: ClassRef::new("fixture", "1", "Linear"),
            config: serde_json::from_str(r#"{"in_features":2,"out_features":2}"#).unwrap(),
            tensors: vec![Tensor {
                key: "weight".into(),
                dtype: Dtype::F32,
                shape: vec![2, 2],
                body: Body::Data(Location {
                    chunk: 0,
                    offset: 0,
                    length: weights.len() as u64,
                }),
            }],
            // Repeated refs must appear physically twice and be checked twice.
            assets: BTreeMap::from([
                ("tokenizer.json".into(), ObjectRef::of(&tokenizer)),
                ("same.json".into(), ObjectRef::of(&tokenizer)),
            ]),
            ..Default::default()
        };
        (
            Header::new(
                vec![ObjectRef::of(&weights)],
                None,
                vec![component],
                BTreeMap::new(),
            )
            .unwrap(),
            weights,
            tokenizer,
        )
    }

    pub(crate) fn complete(path: &Path) -> (Header, Vec<u8>, Vec<u8>, Vec<(u64, ObjectRef)>) {
        let (header, weights, tokenizer) = fixture();
        let bytes = header.to_cbor().unwrap();
        let (objects, _) = layout(&header, bytes.len() as u64).unwrap();
        let mut file = File::create(path).unwrap();
        file.write_all(&bytes).unwrap();
        for (offset, object) in &objects {
            let cursor = file.stream_position().unwrap();
            file.write_all(&vec![0; (*offset - cursor) as usize])
                .unwrap();
            file.write_all(if *object == ObjectRef::of(&weights) {
                &weights
            } else {
                &tokenizer
            })
            .unwrap();
        }
        (header, weights, tokenizer, objects)
    }

    #[test]
    fn portable_checkpoint_fresh_store_offline_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("only.cozytensors");
        let (header, weights, tokenizer, _) = complete(&path);
        let store = Store::open(dir.path().join("fresh")).unwrap();
        let tree = store.admit_file(&path, "f32").unwrap();
        // The admitted checkpoint remains usable when its sole input file is removed.
        fs::remove_file(path).unwrap();
        let checkpoint = store.at_checkpoint(&tree).unwrap();
        assert_eq!(checkpoint.id(), &ObjectRef::of(&header.to_cbor().unwrap()));
        assert_eq!(
            checkpoint.asset("model", "tokenizer.json").unwrap(),
            tokenizer
        );
        assert_eq!(
            checkpoint.component("model").unwrap().config,
            header.components()[0].config
        );
        let mut actual = Vec::new();
        checkpoint
            .chunk(0)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, weights);
        // Tiny CPU inference from the admitted tensor, independent of source/provider caches.
        let values: Vec<_> = actual
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(
            [
                values[0] * 2. + values[1] * 3.,
                values[2] * 2. + values[3] * 3.
            ],
            [8., 18.]
        );
    }

    #[test]
    fn portable_checkpoint_rejects_damage_even_in_warm_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("only.cozytensors");
        let (header, weights, _, objects) = complete(&path);
        let store = Store::open(dir.path().join("store")).unwrap();
        store.admit_file(&path, "f32").unwrap();
        let good = fs::read(&path).unwrap();
        for (label, index, expected) in [
            ("weight hash", objects[0].0, "expected "),
            ("last duplicate hash", objects[2].0, "expected "),
            (
                "padding",
                header.to_cbor().unwrap().len() as u64,
                "nonzero checkpoint padding",
            ),
        ] {
            let mut damaged = good.clone();
            damaged[index as usize] ^= 1;
            fs::write(&path, damaged).unwrap();
            let error = store.admit_file(&path, "f32").unwrap_err().to_string();
            assert!(error.contains(expected), "{label}: {error}");
        }
        for bytes in [
            &good[..good.len() - 1],
            &[good.as_slice(), &[0]].concat(),
            &header.to_cbor().unwrap(),
        ] {
            fs::write(&path, bytes).unwrap();
            let error = store.admit_file(&path, "f32").unwrap_err().to_string();
            assert!(error.contains("complete checkpoint needs"), "{error}");
        }
        fs::write(&path, &good[..5]).unwrap();
        assert!(store
            .admit_file(&path, "f32")
            .unwrap_err()
            .to_string()
            .contains("header"));
        // Existing verified CAS bytes remain intact throughout failed admissions.
        assert_eq!(store.read(&header.chunks[0]).unwrap(), weights);
    }

    #[test]
    fn portable_checkpoint_empty_closure_and_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.cozytensors");
        fs::write(&path, Header::default().to_cbor().unwrap()).unwrap();
        let store = Store::open(dir.path().join("store")).unwrap();
        store.admit_file(&path, "empty").unwrap();
        let (mut header, _, _) = fixture();
        header.chunks[0].length = (1u64 << 33) + 17;
        let (objects, length) = layout(&header, 32).unwrap();
        assert!(objects[1].0 > u32::MAX as u64);
        assert!(length > 1u64 << 33);
        header.chunks[0].length = u64::MAX;
        assert!(layout(&header, 32)
            .unwrap_err()
            .to_string()
            .contains("overflows"));
    }
}
