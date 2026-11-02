use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn link(source: &Path, destination: &Path) {
    if let Ok(previous) = fs::read_link(destination) {
        if previous == source {
            return;
        }
    }
    if fs::symlink_metadata(destination).is_ok() {
        // Only this build script's generated dependency slots are passed here.
        // An older hard-link fallback must not retain a replaced compiler artifact.
        fs::remove_file(destination).expect("replace generated native dependency slot");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(source, destination).expect("stage native dependency link");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(source, destination)
        .expect("native wheel builds require Windows file-symlink capability (Developer Mode or build privilege); hard links can retain a stale core");
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = env::var("TARGET").unwrap();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile = out.ancestors().nth(3).expect("Cargo profile directory");
    let native = out.join(".native");
    fs::create_dir_all(&native).unwrap();
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=../varena/include/varena.h");
    let include = package.join("wheel-data/data/include/varena/v1");
    fs::create_dir_all(&include).unwrap();
    fs::copy(
        package.join("../varena/include/varena.h"),
        include.join("varena.h"),
    )
    .unwrap();
    let cli_native = if os == "windows" {
        package.join("wheel-data/scripts")
    } else {
        package.join("wheel-data/data/lib/varena-v1")
    };
    fs::create_dir_all(&cli_native).unwrap();
    let name = match os.as_str() {
        "linux" => "libvarena.so",
        "macos" => "libvarena.dylib",
        "windows" => "varena.dll",
        _ => panic!("unsupported native target"),
    };
    let core = profile.join("deps").join(name);
    // Cargo can run this script before the dependency link finishes. Links resolve
    // only when maturin packages the completed cohort; no stale primary core copy.
    link(&core, &native.join(name));
    link(&core, &cli_native.join(name));
    let rustc = env::var_os("RUSTC").unwrap();
    let output = Command::new(rustc)
        .args(["--print", "target-libdir", "--target", &target])
        .output()
        .unwrap();
    assert!(output.status.success(), "rustc target-libdir failed");
    let libdir = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    for entry in fs::read_dir(libdir).unwrap() {
        let path = entry.unwrap().path();
        let filename = path.file_name().unwrap().to_string_lossy();
        if (filename.starts_with("libstd-") || filename.starts_with("std-"))
            && matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("so" | "dylib" | "dll")
            )
        {
            fs::copy(&path, native.join(path.file_name().unwrap())).unwrap();
            fs::copy(&path, cli_native.join(path.file_name().unwrap())).unwrap();
        }
    }
    if os == "linux" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/.native");
    }
    if os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/.native");
    }
}
