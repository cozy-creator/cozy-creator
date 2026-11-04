//! The `cozy` binary dispatches to its roles: CLI verbs, `calcifer` and `machine serve`.
#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let words: Vec<&std::ffi::OsString> = args.iter().filter(|arg| *arg != "--json").collect();
    match words.as_slice() {
        [role] if *role == "calcifer" => calcifer::serve::main(),
        // It takes nothing, so `--help` or a typo starts no daemon.
        [role, rest @ ..] if *role == "calcifer" => {
            println!("cozy calcifer is this computer's daemon, which cozy starts when it needs it. It takes no arguments: `cozy status` shows it, and `cozy doctor` restarts a stuck one.");
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
