//! `cozy login|logout|whoami`: this computer's device key at each Hub, through Calcifer
//! (CALCIFER-API.md "Account"). Login enrolls a key by an emailed code; every later call signs in
//! with it silently. Codes and names are read from stdin, never from argv. `--export` enrolls a
//! key for another computer (CI) into a file, which that computer installs with `--key`.
use std::{io::Write as _, path::PathBuf, process::ExitCode};

use protocol::{
    config::Origin,
    error::{Error, Kind},
    hub::{Account, At, Code, CodeAnswer, CodeRequest, KeyInstall, Naming, Proven, Purpose, SignedOut},
};
use serde_json::json;

use crate::{Context, calcifer_client::{Client, path}, machine::usd, output};

#[derive(clap::Args, Debug)]
pub struct Login {
    /// Your Tensorhub account's email.
    #[arg(required_unless_present = "key")]
    email: Option<String>,
    /// Enroll a new key for another computer, such as CI, and write it to FILE (owner-only);
    /// this computer's own key is untouched
    #[arg(long, value_name = "FILE", conflicts_with = "key")]
    export: Option<PathBuf>,
    /// Sign this computer in with a key `--export` wrote
    #[arg(long, value_name = "FILE", conflicts_with = "email")]
    key: Option<PathBuf>,
}

fn at(hub: &Origin) -> At {
    At { hub: Some(hub.to_string()) }
}

/// Enrolls this computer's key by the code the Hub emails, then names the account if it has
/// no name yet. A key already accepted for this email is kept.
pub async fn login(ctx: &Context, args: Login) -> Result<ExitCode, Error> {
    let hub = ctx.hub();
    let calcifer = ctx.calcifer().await?;
    if let Some(file) = &args.key {
        let key = std::fs::read_to_string(file).map_err(|e| {
            Error::credential("hub_credential", format!("{}: {e}", file.display())).next(ctx.config.login(hub))
        })?;
        let ask = KeyInstall { hub: Some(hub.to_string()), key };
        let account = calcifer.put("/v1/account/key", &ask).await?;
        return signed_in(ctx, &calcifer, hub, account, "signed in").await;
    }
    let email = args.email.as_deref().unwrap_or_default().trim();
    if let Some(file) = &args.export {
        let Proven::Export(key) = prove(ctx, &calcifer, hub, Purpose::Export, Some(email)).await? else {
            return Err(unexpected());
        };
        platform::durable::replace(file, key.key.as_bytes(), 0o600)
            .map_err(|e| Error::new(Kind::Internal, "key_unwritable", e.to_string()))?;
        if ctx.json {
            output::json(&json!({
                "status": "exported", "hub": hub, "email": key.email, "device_key": key.device_key, "file": file,
            }));
        } else {
            output::Record::new()
                .field("status", "exported")
                .field("hub", hub)
                .field("email", &key.email)
                .field("device key", &key.device_key)
                .field("file", file.display())
                .print(ctx);
        }
        return Ok(ExitCode::SUCCESS);
    }
    let now: Account = calcifer.get_query("/v1/account", &at(hub)).await?;
    if now.logged_in && now.email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(email)) {
        return signed_in(ctx, &calcifer, hub, now, "already signed in").await;
    }
    let Proven::SignIn(account) = prove(ctx, &calcifer, hub, Purpose::SignIn, Some(email)).await? else {
        return Err(unexpected());
    };
    signed_in(ctx, &calcifer, hub, account, "signed in").await
}

/// Has the Hub email a code for `purpose`, and answers it with what the user types.
async fn prove(
    ctx: &Context,
    calcifer: &Client,
    hub: &Origin,
    purpose: Purpose,
    email: Option<&str>,
) -> Result<Proven, Error> {
    let ask = CodeRequest { hub: Some(hub.to_string()), purpose, email: email.map(str::to_owned) };
    let sent: Code = calcifer.post("/v1/account/codes", &ask, None).await?;
    let code = ask_line(ctx, &format!("A code was sent to {}. Code: ", sent.email), "code")?;
    calcifer.post(&path(&["v1", "account", "codes", &sent.id]), &CodeAnswer { code }, None).await
}

fn unexpected() -> Error {
    Error::new(Kind::Internal, "untyped_answer", "Calcifer proved another code than the one asked")
}

/// Reports the account a key signs in as, naming it first when it has no name yet.
async fn signed_in(
    ctx: &Context,
    calcifer: &Client,
    hub: &Origin,
    mut account: Account,
    status: &str,
) -> Result<ExitCode, Error> {
    // A taken or invalid name asks again; an empty line ends the login.
    while account.account.is_none() {
        let name = ask_line(ctx, "Tensorhub account name: ", "account name")?;
        let ask = Naming { hub: Some(hub.to_string()), name };
        match calcifer.put("/v1/account/name", &ask).await {
            Err(e) if matches!(e.code.as_str(), "account.name_unavailable" | "account.name_invalid") => {
                output::warn(&format!("{} ({})", e.message, e.code));
            }
            named => account = named?,
        }
    }
    let (email, name) = (account.email.unwrap_or_default(), account.account.unwrap_or_default());
    if ctx.json {
        output::json(&json!({
            "status": status, "hub": hub, "email": email, "account": name, "device_key": account.device_key,
        }));
    } else {
        output::Record::new()
            .field("status", status)
            .field("hub", hub)
            .field("email", &email)
            .field("account", &name)
            .print(ctx);
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(clap::Args, Debug)]
pub struct Logout {
    /// Also sign out every other computer of this account (asks for an emailed code).
    #[arg(long)]
    everywhere: bool,
}

/// Erases this computer's key at the Hub, asking the Hub to revoke it first. The key is erased
/// whatever the Hub answers.
pub async fn logout(ctx: &Context, args: Logout) -> Result<ExitCode, Error> {
    let hub = ctx.hub();
    let calcifer = ctx.calcifer().await?;
    let mut out = None;
    if args.everywhere {
        match prove(ctx, &calcifer, hub, Purpose::SignOutEverywhere, None).await {
            Ok(Proven::SignOutEverywhere(signed_out)) => out = Some(signed_out),
            Ok(_) => return Err(unexpected()),
            // Signed out already: there is no key to prove a mailbox with.
            Err(e) if e.code == "not_logged_in" => {}
            Err(e) => return Err(e),
        }
    }
    let out: SignedOut = match out {
        Some(out) => out,
        None => calcifer.delete_query("/v1/account", &at(hub)).await?,
    };
    if ctx.json {
        output::json(&out);
        return Ok(ExitCode::SUCCESS);
    }
    let mut record = output::Record::new().field("status", &out.status).field("hub", &out.hub);
    if let Some(email) = &out.email {
        record = record.field("email", email);
    }
    if out.everywhere {
        record = record.field("other computers", "signed out");
    }
    for note in out.notes {
        record = record.note(note);
    }
    record.print(ctx);
    Ok(ExitCode::SUCCESS)
}

#[derive(clap::Args, Debug)]
pub struct Whoami {}

/// Who this computer signs in as at the command's Hub, as that Hub says: the account, its credit
/// (`unmetered` where the Hub does not meter it) and the Hub's page to buy more.
pub async fn whoami(ctx: &Context, _args: Whoami) -> Result<ExitCode, Error> {
    let hub = ctx.hub();
    let account: Account = ctx.calcifer().await?.get_query("/v1/account", &at(hub)).await?;
    if ctx.json {
        output::json(&account);
        return Ok(ExitCode::SUCCESS);
    }
    let mut record = output::Record::new();
    match (account.logged_in, account.key_refused) {
        (true, _) => {
            record = record
                .field("account", account.account.as_deref().unwrap_or("none yet"))
                .field("email", account.email.as_deref().unwrap_or_default());
            if !account.orgs.is_empty() {
                record = record.field("orgs", account.orgs.join(", "));
            }
            if account.billing.is_some() {
                record = record.field("balance", account.balance_usd_micros.map_or("unmetered".into(), usd));
            }
            record = record.field("device key", account.device_key.as_deref().unwrap_or_default());
            if let Some(buy) = &account.buy_credits {
                record = record.note(format!("Buy credits at {buy}"));
            }
        }
        (false, true) => record = record.field("login", "this computer's key is no longer accepted"),
        (false, false) => record = record.field("login", "not signed in"),
    }
    record.print(ctx);
    if !account.logged_in || account.account.is_none() {
        output::next(ctx, &[&ctx.config.login(hub)]);
    }
    Ok(ExitCode::SUCCESS)
}

/// One line from stdin after a prompt on stderr.
fn ask_line(_ctx: &Context, prompt: &str, what: &str) -> Result<String, Error> {
    anstream::eprint!("{prompt}");
    let _ = anstream::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::new(Kind::Usage, "input_unreadable", format!("the {what}: {e}")))?;
    let line = line.trim();
    if line.is_empty() {
        return Err(Error::new(
            Kind::Usage,
            "input_missing",
            format!("no {what} was given"),
        ));
    }
    Ok(line.to_owned())
}
