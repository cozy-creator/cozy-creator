fn main() {
    println!("cargo:rerun-if-env-changed=VARENA_LIB_DIR");
    if let Some(directory) = std::env::var_os("VARENA_LIB_DIR") {
        println!(
            "cargo:rustc-link-search=native={}",
            directory.to_string_lossy()
        );
    }
}
