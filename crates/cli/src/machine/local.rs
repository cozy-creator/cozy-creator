//! This computer's machine, `local`: Calcifer starts it when something uses it and it exits
//! after 15 minutes idle, so nothing enables it. `cozy machine authorize` lets another
//! computer's controller use it, which keeps it up.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use platform::lock::{Kind as LockKind, LockFile};
use protocol::{calcifer::Authorized, error::Error};
use serde::Serialize;

use crate::{Context, output};

/// The machine's root as TensorD reads it: the config's `[machine] root` as written now.
fn machine_root(ctx: &Context) -> Result<PathBuf, Error> {
    let path = ctx.home.config();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let (config, _) = protocol::config::Config::parse(&text, &path.display().to_string())?;
    Ok(config.machine.root(&ctx.home))
}

fn lock(root: &Path) -> PathBuf {
    root.join("var/lib/cozy/machine/agent.lock")
}

/// What this computer's machine does with the binary `cozy update` installed: none runs, so the
/// next use starts it; or the one running moves to it once its runs end (`run`, the update that
/// moves it), else once idle.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Adopted {
    NotRunning,
    MovesWhenIdle,
    Moves { run: u64 },
}

pub(crate) fn adopt(ctx: &Context) -> Result<Adopted, Error> {
    let lock = lock(&machine_root(ctx)?);
    let free = !lock.exists() || matches!(LockFile::new(lock).try_lock(LockKind::Exclusive), Ok(Some(_)));
    Ok(match free {
        true => Adopted::NotRunning,
        false => Adopted::MovesWhenIdle,
    })
}

/// `cozy machine authorize <public-key>`: lets the controller with this Ed25519 key (`cozy
/// machine show local` on that computer prints it) use this computer's machine
/// (`PUT /v1/machines/local/authorized-keys/{key}`). TensorD reads its keys again at its next call:
/// no restart of work. While any controller is authorized the machine listens on every interface
/// (it moves there once idle), stays up, and Calcifer starts it with itself at login; this prints
/// where other computers add it. `--remove` takes a key back (`DELETE`): its next call is refused,
/// its open streams end, and runs it started finish.
pub async fn authorize(ctx: &Context, key: &str, remove: bool) -> Result<ExitCode, Error> {
    protocol::calcifer::local_usable("local")?;
    let key = key.trim();
    let path = crate::calcifer_client::path(&["v1", "machines", "local", "authorized-keys", key]);
    let calcifer = ctx.calcifer().await?;
    let answer: Authorized = match remove {
        false => calcifer.put(&path, &serde_json::json!({})).await?,
        true => calcifer.delete(&path).await?,
    };
    let verb = if remove { "removed" } else { "authorized" };
    if ctx.json {
        output::json(&serde_json::json!({verb: key, "changed": answer.changed, "machine": answer.machine}));
        return Ok(ExitCode::SUCCESS);
    }
    let local = &answer.machine;
    let image = answer.image;
    let mut record = output::Record::new()
        .field(verb, key)
        .field("in", answer.file.display().to_string());
    if remove {
        record
            .note(match answer.changed {
                false => "it was not authorized",
                true => "its next call is refused; runs it started finish",
            })
            .print(ctx);
        return Ok(ExitCode::SUCCESS);
    }
    match (&local.reach, &local.fingerprint) {
        (Some(reach), Some(fingerprint)) => {
            record = record
                .field("add it at", reach)
                .field("fingerprint", fingerprint.to_string())
                .note(format!("on that computer: cozy machine add <name> {reach} --fingerprint {fingerprint}"));
            if !image {
                let port = reach.rsplit_once(':').map_or("its", |(_, port)| port);
                record = record.note(format!("behind NAT (a home router), forward TCP port {port} to this computer"));
            }
        }
        _ if image => {
            record = record.note("its renter's `cozy machine show <name>` names the address other computers add");
        }
        _ => {
            record = record.note("it listens on every interface once its runs end; `cozy machine show local` says where");
        }
    }
    record
        .note(match (answer.changed, image) {
            (false, _) => "it was already authorized",
            (true, false) => "that controller can now use this machine, which stays up for it",
            // A worker image's machine: its rental still releases itself idle.
            (true, true) => "that controller can now use this machine until the rental ends",
        })
        .print(ctx);
    Ok(ExitCode::SUCCESS)
}
