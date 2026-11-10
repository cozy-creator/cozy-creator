#![cfg(feature = "store")]

use std::io::Write as _;

use tensorfs::{
    err::{Code, Result},
    format::{decode::gguf::Type, Dtype, Encoding, Metadata},
    import::{self, gguf, source, Layout, Part, Sink},
};

fn safetensors(begin: u64, end: u64, payload: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new()
        .suffix(".safetensors")
        .tempfile()
        .unwrap();
    let head =
        serde_json::json!({"w": {"dtype": "U8", "shape": [1], "data_offsets": [begin, end]}})
            .to_string();
    file.write_all(&(head.len() as u64).to_le_bytes()).unwrap();
    file.write_all(head.as_bytes()).unwrap();
    file.write_all(payload).unwrap();
    file
}

#[test]
fn safetensors_offsets_stay_in_the_payload() {
    let valid = safetensors(0, 1, &[7]);
    let length = valid.as_file().metadata().unwrap().len();
    let (tensors, _) = source::read_header(valid.path(), 0, length).unwrap();
    assert_eq!(tensors["w"].start, length - 1);
    assert_eq!(tensors["w"].len, 1);
    for (begin, end) in [(u64::MAX - 32, u64::MAX - 31), (0, 1), (2, 1)] {
        let file = safetensors(begin, end, &[]);
        let length = file.as_file().metadata().unwrap().len();
        assert_eq!(
            source::read_header(file.path(), 0, length)
                .unwrap_err()
                .code,
            Code::CARRIER_GEOMETRY
        );
    }
}

fn gguf_file(tensors: &[(u64, u64)], payload: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let mut bytes = b"GGUF".to_vec();
    bytes.extend(3u32.to_le_bytes());
    bytes.extend((tensors.len() as u64).to_le_bytes());
    bytes.extend(0u64.to_le_bytes());
    for (i, (size, offset)) in tensors.iter().enumerate() {
        let name = format!("w{i}");
        bytes.extend((name.len() as u64).to_le_bytes());
        bytes.extend(name.as_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(size.to_le_bytes());
        bytes.extend(24u32.to_le_bytes()); // ggml I8
        bytes.extend(offset.to_le_bytes());
    }
    bytes.resize(bytes.len().next_multiple_of(32), 0);
    bytes.extend(payload);
    file.write_all(&bytes).unwrap();
    file
}

#[test]
fn gguf_refuses_overflowing_tensor_ends_and_alignment() {
    for tensors in [
        vec![(u64::MAX - 31, 0)],                      // data section + tensor size
        vec![(u64::MAX - 30, 0)],                      // rounding up to alignment
        vec![(u64::MAX - 63, 0), (64, u64::MAX - 63)], // offset + size
    ] {
        let file = gguf_file(&tensors, &[]);
        let error = gguf::read(file.as_file()).unwrap_err();
        assert!(
            matches!(error.code, Code::CARRIER_GEOMETRY | Code::CARRIER_TRUNCATED),
            "{error}"
        );
    }
    let valid = gguf_file(&[(1, 0), (1, 32)], &[7; 33]);
    let read = gguf::read(valid.as_file()).unwrap();
    assert_eq!(read.tensors[1].offset, 32);
    assert_eq!(read.tensors[1].nbytes, 1);
    let header = gguf::header(3, &[], &[("w".into(), vec![1], Type::by_id(24).unwrap())]).unwrap();
    assert_eq!(header.len() % 32, 0);
    assert!(gguf::header(
        3,
        &[],
        &[("w".into(), vec![u64::MAX - 30], Type::by_id(24).unwrap())]
    )
    .is_err());
}

#[derive(Default)]
struct Collected {
    bytes: Vec<u8>,
    committed: bool,
}

impl Sink for Collected {
    fn begin(&mut self, _: &Layout) -> Result<()> {
        Ok(())
    }
    fn put(
        &mut self,
        _: &str,
        _: &str,
        _: &Dtype,
        _: &[u64],
        _: Option<&Encoding>,
        parts: Vec<Part>,
    ) -> Result<()> {
        for part in parts {
            part.data.read_to_end(&mut self.bytes).unwrap();
        }
        Ok(())
    }
    fn commit(&mut self, _: Metadata, _: &dyn Fn(u64, u64)) -> Result<()> {
        self.committed = true;
        Ok(())
    }
}

#[test]
fn converter_refuses_wrapped_header_bytes_and_keeps_valid_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let access = import::Access::default();
    for (file, valid) in [
        (safetensors(u64::MAX - 32, u64::MAX - 31, &[]), false),
        (safetensors(0, 1, &[7]), true),
    ] {
        let sources = [file.path().display().to_string()];
        let request = import::Request {
            sources: &sources,
            converter: Some(import::AS_IS),
            source_variant: None,
            trained_on: None,
            access: &access,
            tmp: tmp.path(),
            progress: &|_, _, _| {},
            stop: None,
        };
        let mut sink = Collected::default();
        let result = import::convert(&request, &mut sink);
        if valid {
            result.unwrap();
            assert!(sink.committed);
            assert_eq!(sink.bytes, [7]);
        } else {
            assert_eq!(result.unwrap_err().code, Code::CARRIER_GEOMETRY);
            assert!(!sink.committed);
            assert!(sink.bytes.is_empty());
        }
    }
}
