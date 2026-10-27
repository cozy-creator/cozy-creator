//! This computer's machine, `local`: Calcifer starts it when something uses it and it exits
//! after 15 minutes idle, so nothing enables it. `cozy machine authorize` lets another
//! computer's controller use it, which keeps it up.
use std::{
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use platform::lock::{Kind as LockKind, LockFile};
use protocol::{
    calcifer::Machine,
    error::{Error, Kind},
};
use serde::Serialize;

use crate::{Context, output};

/// The machine's root as TensorD reads it: the config's `[machine] root` as written now.
fn machine_root(ctx: &Context) -> Result<PathBuf, Error> {
    Ok(machine_config(ctx)?.root(&ctx.home))
}

fn machine_config(ctx: &Context) -> Result<protocol::config::MachineConfig, Error> {
    let path = ctx.home.config();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let (config, _) = protocol::config::Config::parse(&text, &path.display().to_string())?;
    Ok(config.machine)
}

fn lock(root: &Path) -> PathBuf {
    root.join("var/lib/cozy/machine/agent.lock")
}

/// What this computer's machine does with the binary `cozy update` installed: none runs, so the
/// next use starts it; or the one running moves to it once idle.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Adopted {
    NotRunning,
    MovesWhenIdle,
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
/// machine show local` on that computer prints it) use this computer's machine. TensorD reads the
/// file again at its next call: no restart of work. While any controller is authorized the machine
/// listens on every interface (it moves there once idle), stays up, and Calcifer starts it with
/// itself at login; this prints where other computers add it. `--remove` takes a key back: its
/// next call is refused, its open streams end, and runs it started finish.
pub async fn authorize(ctx: &Context, key: &str, remove: bool) -> Result<ExitCode, Error> {
    protocol::calcifer::local_usable("local")?;
    let key = key.trim();
    let changed = match remove {
        false => authorized(ctx, key)?,
        true => unauthorized(ctx, key)?,
    };
    let path = ctx.home.keys().join("authorized");
    let calcifer = ctx.calcifer().await?;
    // Its first call after the change reads the keys again; the machine then listens as they need.
    let mut local: Machine = calcifer.get("/v1/machines/local").await?;
    let image = machine_root(ctx)? == Path::new("/");
    let began = std::time::Instant::now();
    while !remove && local.reach.is_none() && !image && began.elapsed() < std::time::Duration::from_secs(30) {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        local = calcifer.get("/v1/machines/local").await?;
    }
    if ctx.json {
        let verb = if remove { "removed" } else { "authorized" };
        output::json(&serde_json::json!({verb: key, "changed": changed, "machine": local}));
        return Ok(ExitCode::SUCCESS);
    }
    let verb = if remove { "removed" } else { "authorized" };
    let mut record = output::Record::new()
        .field(verb, key)
        .field("in", path.display().to_string());
    if remove {
        record
            .note(match changed {
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
        .note(match (changed, image) {
            (false, _) => "it was already authorized",
            (true, false) => "that controller can now use this machine, which stays up for it",
            // A worker image's machine: its rental still releases itself idle.
            (true, true) => "that controller can now use this machine until the rental ends",
        })
        .print(ctx);
    Ok(ExitCode::SUCCESS)
}

/// Takes `key` out of `keys/authorized`: true when it was there.
fn unauthorized(ctx: &Context, key: &str) -> Result<bool, Error> {
    valid(key)?;
    let path = ctx.home.keys().join("authorized");
    let unwritable = |e: io::Error| Error::new(Kind::Internal, "keys_unwritable", format!("{}: {e}", path.display()));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(unwritable(e)),
    };
    let kept: Vec<&str> = text.lines().filter(|line| line.split('#').next().unwrap_or_default().trim() != key).collect();
    if kept.len() == text.lines().count() {
        return Ok(false);
    }
    let mut left = kept.join("\n");
    if !left.is_empty() {
        left.push('\n');
    }
    write_private(&path, &left).map_err(unwritable)?;
    Ok(true)
}

/// Adds `key` to `keys/authorized` unless it is there: true when it was added.
fn authorized(ctx: &Context, key: &str) -> Result<bool, Error> {
    valid(key)?;
    let path = ctx.home.keys().join("authorized");
    let unwritable = |e: io::Error| {
        Error::new(
            Kind::Internal,
            "keys_unwritable",
            format!("{}: {e}", path.display()),
        )
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(unwritable(e)),
    };
    let held = text
        .lines()
        .any(|line| line.split('#').next().unwrap_or_default().trim() == key);
    if !held {
        let mut text = text;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(key);
        text.push('\n');
        write_private(&path, &text).map_err(unwritable)?;
    }
    Ok(!held)
}

/// Refuses anything but a controller's public key: 32 bytes of unpadded base64url Ed25519.
fn valid(key: &str) -> Result<(), Error> {
    let valid = URL_SAFE_NO_PAD
        .decode(key)
        .ok()
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .is_some_and(|raw| ed25519_dalek::VerifyingKey::from_bytes(&raw).is_ok());
    if !valid {
        return Err(Error::new(
            Kind::Validation,
            "public_key_invalid",
            format!("{key:?} is not a controller's public key (32 bytes of unpadded base64url)"),
        )
        .next("on that computer: cozy machine show local (its `controller key`)"));
    }
    Ok(())
}

/// Writes `text` to `path` whole at 0600, its directory 0700.
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let dir = path.parent().expect("a keys file has a directory");
    platform::durable::private_dir(dir).map_err(io::Error::other)?;
    platform::durable::replace(path, text.as_bytes(), 0o600).map_err(io::Error::other)
}
