use std::env;

fn main() {
    match env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "linux" => println!("cargo:rustc-link-arg-bin=tfs=-Wl,-rpath,$ORIGIN/../lib/varena-v1:$ORIGIN/../data/lib/varena-v1"),
        "macos" => println!("cargo:rustc-link-arg-bin=tfs=-Wl,-rpath,@loader_path/../lib/varena-v1"),
        _ => {},
    }
}
