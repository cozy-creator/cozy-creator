use std::{env, fs, path::PathBuf};

fn main() {
    // TensorFS says its Python distribution's version everywhere (`VERSION`): a build of the open RC's dev line is
    // `0.27.0-dev.3298+g1a2b3c4d` to cargo and `0.27.0.dev3298+g1a2b3c4d` to pip (crates/tensorfs-py/scripts/dev-version.sh).
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let version = match version.split_once("-dev") {
        Some((release, rest)) => {
            let (number, local) = rest.split_once('+').map_or((rest, String::new()), |(number, local)| (number, format!("+{local}")));
            format!("{release}.dev{}{local}", number.trim_start_matches('.').parse::<u64>().unwrap_or(0))
        }
        None => version,
    };
    println!("cargo:rustc-env=TENSORFS_VERSION={version}");
    match env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "linux" => println!("cargo:rustc-link-arg-bin=tfs=-Wl,-rpath,$ORIGIN/../lib/varena-v1:$ORIGIN/../data/lib/varena-v1"),
        "macos" => println!("cargo:rustc-link-arg-bin=tfs=-Wl,-rpath,@loader_path/../lib/varena-v1"),
        _ => {},
    }
    println!("cargo:rustc-check-cfg=cfg(require_aot)");
    println!("cargo:rerun-if-env-changed=TENSORFS_REQUIRE_AOT");
    let catalog =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("kernels/aot/catalog.rs");
    // A missing path reruns this script, and recompiles TensorFS, on every build: until the catalog exists, the
    // crate's own directory stands for it.
    println!("cargo:rerun-if-changed={}", if catalog.exists() { "kernels/aot" } else { "." });
    println!("cargo:rerun-if-changed=../tensorfs-py/kernels");
    if env::var("TENSORFS_REQUIRE_AOT").is_ok_and(|v| v == "1") {
        assert!(catalog.exists(), "CUDA wheels require prebuilt TensorFS kernels; run crates/tensorfs-py/kernels/build.py first");
        println!("cargo:rustc-cfg=require_aot");
    }
    let source = if catalog.exists() {
        for name in ["kernels.py", "build.py", "abi.py"] {
            let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
            assert_eq!(
                fs::read(root.join("../tensorfs-py/kernels").join(name)).unwrap(),
                fs::read(root.join("kernels/aot").join(format!("source-{name}")))
                    .expect("AOT source receipt"),
                "prebuilt TensorFS kernels are stale; rebuild the AOT catalog"
            );
        }
        fs::read_to_string(catalog).expect("read the prebuilt CUDA kernel catalog")
    } else {
        "pub static BINARIES: &[Binary] = &[];\n".into()
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("kernels.rs"),
        source,
    )
    .unwrap();
}
