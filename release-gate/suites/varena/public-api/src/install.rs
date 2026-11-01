//! Actual native archive staging, raw-binary transport, activation layout and rollback.
use std::{fs, path::PathBuf, process::Command};

fn run(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new(path)
        .arg("--version")
        .env_remove("LD_LIBRARY_PATH")
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: proof OLD_ARCHIVE NEW_ARCHIVE NEW_OUTPUT_DIRECTORY".into());
    }
    let output = &args[2];
    fs::create_dir(output)?;
    let first = platform::native_bundle::staged(&args[0])?;
    let second = platform::native_bundle::staged(&args[1])?;
    let installed = output.join("cozy");
    platform::native_bundle::copy_libraries(first.parent().unwrap(), output)?;
    fs::copy(&first, &installed)?;
    let before = run(&installed)?;
    fs::copy(&installed, output.join(".cozy-previous"))?;
    let original_libraries = fs::read_dir(output)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == "so" || e == "dll" || e == "dylib")
        })
        .collect::<Vec<_>>();
    platform::native_bundle::copy_libraries(second.parent().unwrap(), output)?;
    let staged = output.join(".cozy-update");
    fs::copy(&second, &staged)?;
    let candidate = run(&staged)?;
    fs::rename(staged, &installed)?;
    let after = run(&installed)?;
    assert_eq!(candidate, after);
    assert!(original_libraries.iter().all(|p| p.is_file()));
    let repacked = output.join("transport.tar.gz");
    assert!(platform::native_bundle::pack_binary(&installed, &repacked)?.is_some());
    let transported = platform::native_bundle::staged(&repacked)?;
    assert_eq!(run(&transported)?, after);
    fs::copy(output.join(".cozy-previous"), &installed)?;
    assert_eq!(run(&installed)?, before);
    println!(
        "{{\"native_install\":true,\"update_staging\":true,\"raw_binary_bundle\":true,\"rollback\":true,\"preserved_old_libraries\":{},\"live_machine_modified\":false}}",
        original_libraries.len()
    );
    Ok(())
}
