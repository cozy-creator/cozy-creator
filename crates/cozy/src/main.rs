//! The `cozy` binary dispatches to its roles: CLI verbs, `calcifer` and `machine serve`.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

/// mimalloc gives back what any thread freed, on every OS. glibc's malloc kept each thread's frees in that thread's arena: in one
/// Calcifer, web-scale's 5,000-asset project three times left 2,389 MB resident after its last delete, 62 MB of it in use (cl-822).
/// Counted, so Calcifer says what its code holds, which its resident memory doesn't (cl-826).
#[global_allocator]
static ALLOCATOR: platform::heap::Counted<mimalloc::MiMalloc> = platform::heap::Counted(mimalloc::MiMalloc);

fn main() -> std::process::ExitCode {
    if let Some(code) = project_mount::run_supervisor() { std::process::exit(code); }
    // Where this cozy started from, before an update can rename another build over it (cl-981); `machine serve` says a failure.
    let _ = platform::exe::record();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let words: Vec<&std::ffi::OsString> = args.iter().filter(|arg| *arg != "--json").collect();
    match words.as_slice() {
        [role] if *role == "calcifer" => calcifer::serve::main(),
        // It takes nothing, so `--help` or a typo starts no daemon.
        [role, rest @ ..] if *role == "calcifer" => {
            println!("Run the local controller. Cozy starts it automatically; use `cozy doctor` for repairs.");
            match rest.iter().all(|arg| *arg == "--help" || *arg == "-h") {
                true => std::process::ExitCode::SUCCESS,
                false => std::process::ExitCode::FAILURE,
            }
        }
        [noun, role, rest @ ..] if *noun == "machine" && *role == "serve" => {
            machine(std::iter::once(*role).chain(rest.iter().copied()).cloned())
        }
        _ => cli::main(),
    }
}

/// `machine serve`'s arguments, from `serve` on.
fn machine(args: impl Iterator<Item = std::ffi::OsString>) -> std::process::ExitCode {
    tensord::serve::main(args)
}
