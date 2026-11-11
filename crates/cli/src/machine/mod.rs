//! `cozy machine`: this computer's machine (`local`, started when used), machines rented on any
//! Hub, and other computers' machines added by address. Every machine is reached through Calcifer.
use std::process::ExitCode;

use clap::Subcommand;
use api::{
    calcifer::{Machine, NewMachine, clock},
    error::{Error, Kind},
    ids::Digest,
};

use crate::{Context, output};

mod end;
mod history;
pub(crate) mod list;
pub(crate) mod live;
mod local;
pub(crate) use end::activate;
pub(crate) use local::{Adopted, adopt};
mod rent;

pub(crate) use list::status_fields;
pub(crate) use live::Following;
pub(crate) use rent::{Followed, Notices, agree, follow as until_ready};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    verb: Verb,
}

// This computer's machine role reads its arguments once it lands.
#[allow(dead_code)]
#[derive(Subcommand, Debug)]
enum Verb {
    /// List local, rented and added machines.
    #[command(alias = "list")]
    Ls {
        /// Show rental history and daily spending totals.
        #[arg(long)]
        history: bool,
        /// Show rental history from the last 48h, 7d, or another duration.
        #[arg(long, value_name = "DURATION", value_parser = history::since)]
        since: Option<jiff::SignedDuration>,
    },
    /// Show a machine's status and activity.
    Show {
        /// Machine name or rental ID.
        name: String,
    },
    /// Show a machine's logs.
    Logs {
        /// Machine name or rental ID.
        name: String,
        #[command(flatten)]
        query: crate::daemon::LogQuery,
    },
    // Follows the order to its end. Exit 0 means ready, 1 means the order ended
    // without a machine, and 3 means the caller stopped following a live order.
    // JSON emits the quote, rental.accepted, and the ready or unfollowed record.
    /// Rent a machine, or list available GPUs when no SKU is given.
    Rent(rent::Rent),
    // Billing stops when the Hub confirms termination. Ctrl-C stops following
    // the request while Calcifer continues to carry it out.
    /// End a rental or remove an added machine after its outputs arrive.
    End {
        /// Machine name or rental ID.
        name: String,
        /// End immediately, discarding outputs that have not arrived.
        #[arg(long)]
        force: bool,
    },
    /// Reset a rental's 15-minute idle timer.
    Keepalive {
        /// Rental name or ID.
        name: String
    },
    /// Open a root SSH session on a rental, or run a command.
    Ssh {
        /// Rental name or ID.
        name: String,
        /// Remote command to run instead of opening a shell.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Update a machine's Runtime, TensorFS and Cozy binary.
    Update {
        /// Machine name or rental ID.
        name: String,
        // Releases come from PyPI or GitHub prereleases. Updating only Runtime
        // keeps compatible TensorFS, otherwise it resolves a compatible release.
        /// Runtime version or path to a wheel or source checkout.
        #[arg(long, value_name = "VERSION|PATH")]
        runtime: Option<String>,
        /// TensorFS version or path to a wheel or source checkout.
        #[arg(long, value_name = "VERSION|PATH")]
        tensorfs: Option<String>,
        // A dev binary on local also replaces the installed CLI and Calcifer.
        /// Cozy binary version, local binary or source checkout.
        #[arg(long, value_name = "VERSION|PATH")]
        tensord: Option<String>,
        // This development limit persists across machine updates and restarts.
        /// Limit memory per GPU, e.g. 8GiB. Use off to remove the limit.
        #[arg(long, value_name = "SIZE|off", value_parser = gpu_memory_limit)]
        gpu_memory_limit: Option<u64>,
        /// Use shared or dedicated GPU mode.
        #[arg(long, value_name = "shared|dedicated", value_parser = machine_mode)]
        mode: Option<api::config::MachineMode>,
    },
    /// Add a machine by address and TLS fingerprint.
    Add {
        /// Name to use for this machine.
        name: String,
        /// Machine address as host:port.
        address: String,
        /// SHA-256 TLS fingerprint shown by cozy machine show local.
        #[arg(long)]
        fingerprint: Digest,
    },
    /// Pin this project's runs to these machines, each run to whichever finishes first; with none, any machine.
    Use {
        /// Machine names, by space or comma (`local,ryuk`).
        #[arg(value_name = "MACHINE", value_delimiter = ',')]
        machines: Vec<String>,
    },
    // Authorizing a controller makes the machine listen on every interface.
    /// Allow remote access from another controller's public key.
    Authorize {
        // Base64url keys may begin with a dash, so hyphenated values are accepted.
        /// Controller public key from cozy machine show local.
        #[arg(allow_hyphen_values = true)]
        public_key: String,
        /// Remove this controller's access.
        #[arg(long)]
        remove: bool,
    },
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::Ls { history, since } if history || since.is_some() => history::list(ctx, since).await,
        Verb::Ls { .. } => list::list(ctx).await,
        Verb::Show { name } => list::show(ctx, &name).await,
        Verb::Logs { name, query } => crate::daemon::machine_log(ctx, &name, &query).await,
        Verb::Rent(rent) => rent::exec(ctx, rent).await,
        Verb::End { name, force } => end::end(ctx, &name, force).await,
        Verb::Keepalive { name } => end::keepalive(ctx, &name).await,
        Verb::Ssh { name, command } => ssh(ctx, &name, &command).await,
        Verb::Add {
            name,
            address,
            fingerprint,
        } => add(ctx, name, address, fingerprint).await,
        Verb::Update {
            name,
            runtime,
            tensorfs,
            tensord,
            gpu_memory_limit,
            mode,
        } => end::update(ctx, &name, runtime, tensorfs, tensord, gpu_memory_limit, mode).await,
        Verb::Authorize { public_key, remove } => local::authorize(ctx, &public_key, remove).await,
        Verb::Use { machines } => use_machines(ctx, machines).await,
    }
}

/// `cozy machine use` (cl-873): the current project's machines, which its runs and renders go to unless a run names its own.
async fn use_machines(ctx: &Context, machines: Vec<String>) -> Result<ExitCode, Error> {
    let project = crate::project::identity(ctx).await?;
    let path = crate::calcifer_client::path(&["v1", "projects", &project.id, "machines"]);
    let kept: api::calcifer::project::Machines = ctx.calcifer().await?.put(&path, &api::calcifer::project::Machines { machines }).await?;
    if ctx.json {
        output::json(&kept);
        return Ok(ExitCode::SUCCESS);
    }
    let said = match kept.machines.as_slice() {
        [] => "This project's runs go to any machine.".to_owned(),
        [one] => format!("This project's runs go to {one}."),
        names => format!("This project's runs go to {}, each to whichever finishes first.", names.join(", ")),
    };
    output::say(ctx, &said);
    output::next(ctx, &["cozy run <[org/]package/function>", "cozy status"]);
    Ok(ExitCode::SUCCESS)
}

fn machine_mode(text: &str) -> Result<api::config::MachineMode, String> {
    match text {
        "shared" => Ok(api::config::MachineMode::Shared),
        "dedicated" => Ok(api::config::MachineMode::Dedicated),
        _ => Err("mode must be shared or dedicated".into()),
    }
}

/// `--gpu-memory-limit`: a size, or `off` (0, which the machine reads as no limit).
fn gpu_memory_limit(text: &str) -> Result<u64, String> {
    match text {
        "off" => Ok(0),
        size => match api::config::parse_size(size)? {
            0 => Err("a limit of 0 bytes: write `off` to lift it".into()),
            bytes => Ok(bytes),
        },
    }
}

/// The route of one machine, by name, alias or rental id: `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`.
pub(crate) fn route(name: &str) -> Result<String, Error> {
    let mut bytes = name.bytes();
    let first = bytes.next().is_some_and(|b| b.is_ascii_alphanumeric());
    if !first
        || name.len() > 128
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(Error::new(
            Kind::Usage,
            "machine_name_invalid",
            format!("{name:?} is not a machine name or rental id"),
        )
        .next("cozy machine ls"));
    }
    Ok(format!("/v1/machines/{name}"))
}

/// The machine `text` names, by Calcifer's one resolver: a live machine's name or the name it had
/// before, or any machine's rental id. Every `--on` that reads a machine resolves it here.
pub(crate) async fn machine(ctx: &Context, text: &str) -> Result<Machine, Error> {
    ctx.calcifer().await?.get(&route(text)?).await
}

async fn add(
    ctx: &Context,
    name: String,
    address: String,
    fingerprint: Digest,
) -> Result<ExitCode, Error> {
    route(&name)?;
    let ask = NewMachine::Add {
        name,
        address,
        fingerprint,
    };
    let added: Machine = ctx
        .calcifer()
        .await?
        .post("/v1/machines", &ask, None)
        .await?;
    if ctx.json {
        output::json(&added);
        return Ok(ExitCode::SUCCESS);
    }
    output::Record::new()
        .field("machine", &added.name)
        .field("address", added.address.as_deref().unwrap_or("—"))
        .field(
            "fingerprint",
            added.fingerprint.map_or("—".into(), |f| f.to_string()),
        )
        .field("state", list::state_cell(&added))
        .print(ctx);
    output::next(
        ctx,
        &[
            &format!("cozy run <[org/]package/function> --on {}", added.name),
            &format!("cozy machine show {}", added.name),
        ],
    );
    Ok(ExitCode::SUCCESS)
}

/// `cozy machine ssh`: runs `Machine.ssh`, for a rental rented with `--ssh`.
async fn ssh(ctx: &Context, name: &str, command: &[String]) -> Result<ExitCode, Error> {
    let m = machine(ctx, name).await?;
    let Some(login) = &m.ssh else {
        return Err(match &m.rental {
            Some(rental) => Error::new(Kind::Usage, "machine_ssh_none", format!("{} has no SSH key; keys must be added when renting", m.name))
                .next(format!("cozy machine rent {} --ssh", rental.sku)),
            None => Error::new(Kind::Usage, "machine_ssh_none", format!("{} is not a rental; connect using its owner's SSH details", m.name)),
        });
    };
    let mut ssh = std::process::Command::new(&login.args[0]);
    ssh.args(&login.args[1..]);
    if !command.is_empty() {
        ssh.arg("--").args(command);
    }
    let unstarted = |e: std::io::Error| {
        Error::new(Kind::Unavailable, "ssh_unavailable", format!("ssh: {e}"))
            .remedy("install an OpenSSH client")
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        Err(unstarted(ssh.exec()))
    }
    #[cfg(not(unix))]
    {
        let status = ssh.status().map_err(unstarted)?;
        Ok(ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8))
    }
}

/// The line `machine ls` and `status` show where this OS runs no models itself (no `local`).
pub(crate) fn local_note() -> Option<String> {
    let off = api::calcifer::local_usable("local").err()?;
    Some(format!("{}: {}", off.message, off.remedy.unwrap_or_default()))
}

/// Money as people read it: `$3.19`.
pub(crate) fn usd(micros: i64) -> String {
    format!("${:.2}", micros as f64 / 1e6)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
