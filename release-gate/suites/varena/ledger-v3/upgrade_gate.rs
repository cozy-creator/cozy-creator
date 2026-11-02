// Appended only to an output copy of the exact ledger module. The prepared/commit/tombstone
// functions and OS locks are actual product code. No driver/participant is faked.
pub fn gate_migrate(path: &Path) -> Result<()> {
    use std::io::{BufRead, Write};
    let file = os::Descriptor::open(path)?;
    let _publication = file.lock(0, true)?.ok_or(Error::Busy)?;
    let mut expected: [u64; 15] = file.header(15)?.try_into().unwrap();
    expected[0] = MAGIC;
    expected[1] = VERSION;
    expected[2] = WORDS as u64;
    let prepared = upgrade::Prepared::prepare(&file, path, &expected)?;
    println!("{{\"prepared\":true,\"pid\":{}}}", std::process::id());
    std::io::stdout().flush()?;
    let command = std::io::stdin().lock().lines().next().unwrap()?;
    if command == "tombstone" {
        prepared.tombstone()?;
        println!("{{\"tombstoned\":true}}");
        std::io::stdout().flush()?;
        // The controller kills this owned child at the real post-tombstone crash point.
        let _ = std::io::stdin().lock().lines().next();
        return Err(Error::Invalid("crash gate unexpectedly resumed"));
    }
    if command == "rename-only-control" { return prepared.gate_rename_only(); }
    if command != "commit" { return Err(Error::Invalid("migration gate command")); }
    prepared.commit()
}
