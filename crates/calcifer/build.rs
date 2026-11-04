//! Embeds the page `web/app` builds: writes a table of `web/app/dist`'s files for `page.rs` to include. With no
//! `dist` the table is empty, this warns and Calcifer answers `503 page_not_built`: compiling needs no Node.
use std::{env, fmt::Write as _, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dist = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it")).join("../../web/app/dist");
    // A watched path that's missing reruns this on every build, and so rebuilds Calcifer. An empty folder can be
    // watched, so the build that fills it is seen. A read-only checkout refuses this and keeps what it has.
    let _ = fs::create_dir_all(&dist);
    if dist.is_dir() {
        println!("cargo:rerun-if-changed={}", dist.display());
    }
    let mut files = vec![];
    if dist.join("index.html").is_file() {
        files.push(("index.html".to_owned(), dist.join("index.html")));
        for entry in fs::read_dir(dist.join("assets")).into_iter().flatten().flatten() {
            if entry.path().is_file() {
                files.push((format!("assets/{}", entry.file_name().to_string_lossy()), entry.path()));
            }
        }
        files.sort();
    } else {
        println!("cargo:warning=the page isn't built: cd web/app && pnpm install --frozen-lockfile && pnpm build");
    }
    let mut table = String::from("const FILES: &[(&str, &[u8], &str)] = &[\n");
    for (name, path) in &files {
        writeln!(table, "    ({name:?}, include_bytes!({path:?}), {:?}),", media_type(name)).expect("a string takes it");
    }
    table.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets it"));
    fs::write(out.join("page_files.rs"), table).expect("OUT_DIR is writable");
}

fn media_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("json" | "map") => "application/json",
        _ => "application/octet-stream",
    }
}
