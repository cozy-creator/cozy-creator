fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target == "linux" {
        println!("cargo:rustc-link-arg=-Wl,--build-id=sha1");
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    } else if target == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-install_name,@rpath/libvarena.dylib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
    } else if target == "windows" {
        // A CodeView GUID is the private image identity; no PDB is needed at runtime.
        println!("cargo:rustc-link-arg=/DEBUG");
    }
}
