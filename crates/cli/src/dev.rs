//! Dev-only options and subcommands (DEV.md): in no `--help`, and refused unless the config says
//! `dev = true`. They are ours, not the official interface; anyone may turn them on.
use protocol::error::{Error, Kind};

/// Every dev-only option or subcommand: the command it belongs to below `cozy`, then its id (a
/// subcommand's name, or an option's field name).
const DEV_ONLY: &[(&[&str], &str)] = &[(&["machine", "rent"], "provider"), (&["run"], "no_memo")];

/// `cozy`'s grammar with every dev-only entry hidden.
pub fn hide(command: clap::Command) -> clap::Command {
    DEV_ONLY.iter().fold(command, |command, (path, id)| at(command, path, id))
}

fn at(command: clap::Command, path: &[&str], id: &str) -> clap::Command {
    match path.split_first() {
        Some((name, rest)) => command.mut_subcommand(name, |sub| at(sub, rest, id)),
        None if command.get_arguments().any(|arg| arg.get_id() == id) => command.mut_arg(id, |arg| arg.hide(true)),
        None => command.mut_subcommand(id, |sub| sub.hide(true)),
    }
}

/// Refuses a dev-only entry the command line used, unless `dev`.
pub fn admit(matches: &clap::ArgMatches, dev: bool) -> Result<(), Error> {
    if dev {
        return Ok(());
    }
    for (path, id) in DEV_ONLY {
        let Some(matches) = path.iter().try_fold(matches, |m, name| m.subcommand_matches(name)) else {
            continue;
        };
        let spelled = if matches.subcommand_name() == Some(*id) {
            id.to_string()
        } else if matches.ids().any(|given| given.as_str() == *id)
            && matches.value_source(id) == Some(clap::parser::ValueSource::CommandLine)
        {
            format!("--{}", id.replace('_', "-"))
        } else {
            continue;
        };
        return Err(Error::new(Kind::Usage, "dev_only", format!("`cozy {} {spelled}` is dev-only", path.join(" ")))
            .remedy("put `dev = true` at the top of ~/.cozy/config.toml"));
    }
    Ok(())
}
