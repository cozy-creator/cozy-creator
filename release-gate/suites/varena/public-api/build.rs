fn main() {
    // This integration fixture has an actual Rust dependency on the shared core,
    // so Cargo builds it before linking the additional external C-ABI client.
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!(
        "cargo:rustc-link-search=native={}",
        out.ancestors().nth(3).unwrap().join("deps").display()
    );
}
