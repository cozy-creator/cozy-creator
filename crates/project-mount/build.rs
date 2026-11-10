use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=COZY_SECUREFS_ARCHIVE");
    println!("cargo:rerun-if-env-changed=COZY_SECUREFS_REQUIRED");
    println!("cargo:rerun-if-changed=helper/manifest.json");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    let Some(archive) = env::var_os("COZY_SECUREFS_ARCHIVE") else {
        assert!(
            env::var_os("COZY_SECUREFS_REQUIRED").is_none(),
            "release builds require scripts/prepare-securefs.py and COZY_SECUREFS_ARCHIVE"
        );
        fs::write(output.join("securefs.zip"), []).expect("write empty helper marker");
        return;
    };
    let archive = PathBuf::from(archive);
    println!("cargo:rerun-if-changed={}", archive.display());
    let target = env::var("TARGET").expect("Cargo supplies TARGET");
    let manifest: serde_json::Value = serde_json::from_str(include_str!("helper/manifest.json"))
        .expect("valid pinned helper manifest");
    let pin = &manifest["targets"][&target];
    let expected = pin["sha256"]
        .as_str()
        .expect("securefs is pinned for this release target");
    let bytes = fs::read(&archive).expect("read the prepared securefs archive");
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        expected,
        "securefs archive checksum mismatch"
    );
    fs::write(output.join("securefs.zip"), bytes).expect("embed the verified helper archive");
}
