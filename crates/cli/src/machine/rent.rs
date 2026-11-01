//! `cozy machine rent`: the catalog, and one paid order Calcifer places and keeps under its key.
use std::{
    io::{IsTerminal as _, Write as _},
    process::ExitCode,
};

use protocol::{
    calcifer::{Machine, MachineState, NewMachine, Quote, RentalSkus, Rented, RunQuote},
    config::Origin,
    error::{Error, Kind},
};

use super::{Following, list::state_cell, usd};
use crate::{Context, output};

#[derive(clap::Args, Debug)]
pub struct Rent {
    /// A product from the catalog.
    sku: Option<String>,
    /// GPUs on the machine: any count the catalog lists.
    #[arg(long, requires = "sku", value_parser = clap::value_parser!(u32).range(1..))]
    gpus: Option<u32>,
    /// Disk in GB, at least (the Hub's default is 100).
    #[arg(long, requires = "sku", value_parser = clap::value_parser!(u32).range(1..))]
    disk_gb: Option<u32>,
    /// Log in as root over SSH with this public key (`cozy machine ssh`); alone, your
    /// `~/.ssh/id_ed25519.pub` (or id_ecdsa, id_rsa).
    #[arg(long, requires = "sku", value_name = "PUBLIC_KEY_FILE", num_args = 0..=1)]
    ssh: Option<Option<std::path::PathBuf>>,
    /// A Hub admin's worker image by digest (sha256:<hex>), for a gated image bake.
    #[arg(long, requires = "sku", hide = true, value_parser = image_digest)]
    image_digest: Option<String>,
    /// Dev builds only: the provider to buy from (runpod or vast), for a release suite that rents on
    /// each. The Hub decides who may.
    #[cfg(feature = "dev")]
    #[arg(long, requires = "sku", hide = true, value_parser = ["runpod", "vast"])]
    provider: Option<String>,
}

fn image_digest(digest: &str) -> Result<String, String> {
    match digest.strip_prefix("sha256:") {
        Some(hex)
            if hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
        {
            Ok(digest.to_owned())
        }
        _ => Err("name a worker image as sha256:<64 lowercase hex>".to_owned()),
    }
}

impl Rent {
    #[cfg(feature = "dev")]
    fn provider(&self) -> Option<String> {
        self.provider.clone()
    }
    #[cfg(not(feature = "dev"))]
    fn provider(&self) -> Option<String> {
        None
    }
}

pub async fn exec(ctx: &Context, rent: Rent) -> Result<ExitCode, Error> {
    let hub = ctx.hub();
    let provider = rent.provider();
    let Some(sku) = rent.sku else {
        return catalog(ctx, hub).await;
    };
    let gpus = rent.gpus.unwrap_or(1);
    let ssh_key = rent.ssh.as_ref().map(|file| ssh_key(ctx, file.as_deref())).transpose()?;
    // One key names this order: Calcifer keeps the ask under it, and a retry never orders twice.
    let key = mint_key();
    let ask = |agreed| NewMachine::Rent {
        sku: sku.clone(),
        hub: hub.to_string(),
        gpus: rent.gpus,
        disk_gb: rent.disk_gb,
        image: rent.image_digest.clone(),
        provider: provider.clone(),
        agreed,
        ssh_key: ssh_key.clone(),
    };
    let calcifer = ctx.calcifer().await?;
    // The Hub's price for exactly this order, agreed before it is placed or resumed. Calcifer
    // admits the product first: an explicit ask is honoured or refused, never widened.
    let quote = calcifer.post("/v1/rental-quotes", &ask(None), None).await?;
    let quoted = RunQuote {
        sku: sku.clone(),
        gpus: Some(gpus),
        quote,
        why: None,
        alternatives: vec![],
    };
    if !agree(ctx, &quoted)? {
        output::say(ctx, "nothing was rented");
        return Ok(ExitCode::SUCCESS);
    }
    let agreed = Some(quoted.quote.agreed());
    let rented: Rented = match calcifer.post("/v1/machines", &ask(agreed), Some(&key)).await {
        // An answer lost on the way back may still have placed the order, which Calcifer keeps.
        Err(e) if matches!(e.kind, Kind::Unavailable | Kind::Internal) => {
            let e = e
                .remedy("the order may stand: `cozy machine ls` shows it before you rent again")
                .next("cozy machine ls");
            output::refusal(ctx, &e);
            return Ok(ExitCode::from(UNFOLLOWED));
        }
        answer => answer?,
    };
    if ctx.json {
        // The order stands from here, so a script that stops watching mid-boot can name and end it.
        output::ndjson(&serde_json::json!({
            "type": "rental.accepted",
            "rental_id": rented.machine.rental_id,
            "name": rented.machine.name,
            "sku": sku,
            "gpus": gpus,
            "replay": rented.replay,
        }));
    }
    let mut notices = Notices::default();
    for note in notices.fresh(&rented.machine) {
        output::say(ctx, note);
    }
    if let Some(asked) = rent.disk_gb.filter(|asked| rented.quote.disk_gb < *asked) {
        output::warn(&format!(
            "the Hub plans a {} GB disk, below the {asked} GB asked",
            rented.quote.disk_gb
        ));
    }
    if rented.replay {
        output::say(ctx, "this order was placed already; following it");
    }
    let machine = match follow(ctx, rented.machine.clone(), notices).await? {
        Followed::Ready(machine) => machine,
        Followed::Unfollowed => return Ok(ExitCode::from(UNFOLLOWED)),
    };
    if ctx.json {
        output::json(&Rented { machine, ..rented });
        return Ok(ExitCode::SUCCESS);
    }
    let name = &machine.name;
    let mut record = output::Record::new()
        .field("machine", name)
        .field("gpus", gpus)
        .field("$/hour", usd(rented.quote.gpu_usd_micros_per_hour));
    if let Some(ssh) = machine.rental.as_ref().and_then(|r| r.ssh.as_deref()) {
        record = record.field("ssh", ssh);
    }
    record = record
        .note(format!("billing continues until `cozy machine end {name}` confirms its release"))
        .note("it ends itself after 15 minutes idle (no queued or running work)");
    if gpus > 1 && gpus % 2 == 1 {
        record = record.note(
            "an odd GPU count leaves one GPU out of parallel runs, which shard at even counts",
        );
    }
    record.print(ctx);
    output::next(
        ctx,
        &[
            &format!("cozy run <[org/]package/function> --on {name}"),
            &format!("cozy machine end {name}"),
        ],
    );
    // The last line is the order's end, for a person and a script alike.
    anstream::println!("{name}: {}", state_cell(&machine));
    Ok(ExitCode::SUCCESS)
}

/// The OpenSSH public key `--ssh` names (none: the user's own), as one line.
fn ssh_key(ctx: &Context, file: Option<&std::path::Path>) -> Result<String, Error> {
    let file = match file {
        Some(file) => file.to_path_buf(),
        None => ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"]
            .iter()
            .map(|name| ctx.user_home.join(".ssh").join(name))
            .find(|path| path.is_file())
            .ok_or_else(|| {
                Error::new(Kind::Usage, "ssh_key_missing", "no SSH public key in ~/.ssh")
                    .remedy("make one (`ssh-keygen -t ed25519`), or name its .pub file: --ssh <file>")
            })?,
    };
    let text = std::fs::read_to_string(&file).map_err(|e| {
        Error::new(Kind::Usage, "ssh_key_unreadable", format!("{}: {e}", file.display()))
    })?;
    let key = text.trim();
    let one_line = !key.is_empty() && !key.contains(['\n', '\r', '\0']) && key.len() <= 8192;
    if !one_line || key.contains("PRIVATE KEY") {
        return Err(Error::new(
            Kind::Usage,
            "ssh_key_invalid",
            format!("{} is not one OpenSSH public key line", file.display()),
        )
        .remedy("name the .pub file: --ssh ~/.ssh/id_ed25519.pub"));
    }
    Ok(key.to_owned())
}

/// What the Hub says of a rental for a person to know: why it runs off the requester's continent.
/// The Hub places a rental after it accepts the order, so a follower says it once it appears.
#[derive(Default)]
pub(crate) struct Notices(Vec<String>);

impl Notices {
    /// The rental's notices not said yet, now counted as said.
    pub(crate) fn fresh<'a>(&mut self, machine: &'a Machine) -> Vec<&'a str> {
        let rental = machine.rental.as_ref();
        let fresh: Vec<&str> = rental
            .into_iter()
            .filter_map(|r| r.placement_note.as_deref())
            .filter(|note| !self.0.iter().any(|said| said == note))
            .collect();
        self.0.extend(fresh.iter().map(|note| note.to_string()));
        fresh
    }
}

/// What a renter is charged: the GPUs and our price an hour (`1× L40S at $1.64/h`). Disk and
/// traffic are never the renter's, whatever a provider charges us.
fn price(sku: &str, gpus: u32, q: &Quote) -> String {
    format!("{gpus}× {} at {}/h", sku.to_ascii_uppercase(), usd(q.gpu_usd_micros_per_hour))
}

/// A quote as `price` says it, then why the Hub placed it there and the machines it would rent
/// instead, each against the pick: its time and its price an hour.
fn placed(q: &RunQuote) -> String {
    let mut said = price(&q.sku, q.gpus.unwrap_or(1), &q.quote);
    if let Some(why) = &q.why {
        said += &format!("\n  {why}");
    }
    for a in &q.alternatives {
        let label = match a.label.is_empty() {
            true => String::new(),
            false => format!(" ({})", a.label),
        };
        said += &format!(
            "\n  or {}× {}{label}: {:.2}× the time, {}/h",
            a.gpus,
            a.sku.to_ascii_uppercase(),
            a.time,
            usd(a.usd_micros_per_hour),
        );
    }
    if !q.alternatives.is_empty() {
        said += "\n  rent another with --on new:<sku> [--gpus N]";
    }
    said
}

/// The renter's yes to `quoted` (said as `placed`: its first line the offer, the rest why):
/// `--yes`, else a person's on a terminal (a no is `false`). With neither nothing is rented, and
/// the refusal carries the offer, in text and JSON. `--json` first prints the quote as one line,
/// for tools to check: the machine, our price an hour, and the disk the Hub sized for it. Once the
/// Hub accepts the order, `cozy machine rent` prints `{"type":"rental.accepted", rental_id, name}`
/// at once, and the machine's document when it is ready.
pub(crate) fn agree(ctx: &Context, quoted: &RunQuote) -> Result<bool, Error> {
    if ctx.json {
        output::ndjson(&serde_json::json!({"quote": {
            "sku": quoted.sku,
            "gpus": quoted.gpus.unwrap_or(1),
            "usd_micros_per_hour": quoted.quote.gpu_usd_micros_per_hour,
            "container_disk_gb": quoted.quote.disk_gb,
            "why": quoted.why,
            "alternatives": quoted.alternatives,
        }}));
    }
    let said = placed(quoted);
    let (offer, why) = said.split_once('\n').unwrap_or((&said, ""));
    if ctx.yes {
        output::say(ctx, &format!("Renting {offer}"));
        if !why.is_empty() {
            output::say(ctx, why);
        }
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(Error::new(Kind::Usage, "quote_unagreed", format!("{offer}: nothing was rented"))
            .remedy("to rent at this price, run the same command with --yes"));
    }
    if !why.is_empty() {
        anstream::eprintln!("{why}");
    }
    anstream::eprint!("Rent {offer}? [Y/n] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return Ok(false);
    }
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes"))
}

/// What following an order came to: its machine ready, or the follower stopped (and said so)
/// while the order goes on.
pub(crate) enum Followed {
    Ready(Machine),
    Unfollowed,
}

/// `machine rent`'s exit once it stops following an order that goes on (Ctrl-C, SIGTERM, Calcifer
/// lost): 0 is a ready machine, and 1 an order that ended without one.
const UNFOLLOWED: u8 = 3;

/// Follows an order over Calcifer's machine events until it ends: its machine ready, or the order
/// ended, which its Hub says only once nothing of it bills. Everything between is progress, said on
/// stderr as it changes (an attempt that found no capacity, the order placed again elsewhere, its
/// rename) and never a verdict: the exit code and the last line are. Ctrl-C or SIGTERM stops
/// following only, and says the order goes on.
pub(crate) async fn follow(
    ctx: &Context,
    booting: Machine,
    mut notices: Notices,
) -> Result<Followed, Error> {
    let id = booting.rental_id.clone().unwrap_or_else(|| booting.name.clone());
    let mut machine = booting;
    let stop = stopped();
    tokio::pin!(stop);
    let calcifer = match ctx.calcifer().await {
        Ok(calcifer) => calcifer,
        Err(e) => return Ok(unfollowed(ctx, &machine, Some(e))),
    };
    let opened = tokio::select! {
        opened = Following::open(&calcifer, &id) => opened,
        _ = &mut stop => return Ok(unfollowed(ctx, &machine, None)),
    };
    let mut following = match opened {
        Ok((following, now)) => {
            machine = now;
            following
        }
        Err(e) => return Ok(unfollowed(ctx, &machine, Some(e))),
    };
    let mut said = String::new();
    loop {
        for note in notices.fresh(&machine) {
            output::say(ctx, note);
        }
        match machine.state {
            MachineState::Ready | MachineState::Stopped => return Ok(Followed::Ready(machine)),
            MachineState::Ended => return Err(ended(&machine)),
            // Ending lives until its Hub affirms the end; Unknown is a newer Calcifer's word.
            MachineState::Booting | MachineState::Ending | MachineState::Unknown => {
                let line = format!("{}: {}", machine.name, state_cell(&machine));
                if line != said {
                    output::say(ctx, &line);
                    said = line;
                }
            }
        }
        let next = tokio::select! {
            next = following.next(&calcifer) => next,
            _ = &mut stop => return Ok(unfollowed(ctx, &machine, None)),
        };
        machine = match next {
            Ok(Some(next)) => next,
            // It left the list, which shows no ended machine: its record says where it stands.
            Ok(None) => match calcifer.get(&super::route(&id)?).await {
                Ok(now) => now,
                Err(e) => return Ok(unfollowed(ctx, &machine, Some(e))),
            },
            Err(e) => return Ok(unfollowed(ctx, &machine, Some(e))),
        };
    }
}

/// Ctrl-C, or SIGTERM from a supervisor (`timeout`): either stops the follower, never the order.
async fn stopped() {
    #[cfg(unix)]
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut term) => tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        },
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// Says the follower stopped (`lost`: Calcifer could not be followed) while the order goes on,
/// and how to find and end it.
fn unfollowed(ctx: &Context, machine: &Machine, lost: Option<Error>) -> Followed {
    let name = &machine.name;
    let why = lost.map(|e| e.message);
    if ctx.json {
        output::ndjson(&serde_json::json!({
            "type": "rental.unfollowed",
            "rental_id": machine.rental_id,
            "name": name,
            "lost": why,
        }));
        return Followed::Unfollowed;
    }
    let stopped = why.map_or_else(|| "stopped following".to_owned(), |why| format!("lost Calcifer: {why}"));
    output::next(ctx, &["cozy machine ls", &format!("cozy machine end {name}")]);
    // The last line is the verdict, as a ready machine's is.
    anstream::println!("{name}: still live, bills once its machine is ready ({stopped})");
    Followed::Unfollowed
}

/// The order ended without a machine: its Hub says so only once nothing of it bills. One line, the
/// verdict, last.
fn ended(m: &Machine) -> Error {
    let why = m.reason.as_deref().unwrap_or("its Hub ended it");
    Error::new(
        Kind::Unavailable,
        "rental_ended",
        format!("{}: ended without a machine, nothing bills ({why})", m.name),
    )
}

fn mint_key() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the OS has randomness");
    format!("rent-{}", hex::encode(bytes))
}

/// The catalog, cheapest first: each product's GPUs and what we charge for them an hour
/// (`GET /v1/rental-skus`).
async fn catalog(ctx: &Context, hub: &Origin) -> Result<ExitCode, Error> {
    let query = [("hub", hub.to_string())];
    let RentalSkus { rental_skus: skus } = ctx.calcifer().await?.get_query("/v1/rental-skus", &query).await?;
    if ctx.json {
        output::json(&skus);
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = output::Table::new(
        ctx,
        ["NAME", "GPU", "GPUS", "COMPUTE", "VRAM", "$/HOUR"],
    );
    for sku in &skus {
        let width = &sku.widths[0];
        let counts: Vec<String> = sku
            .widths
            .iter()
            .map(|w| w.accelerator_count.to_string())
            .collect();
        let compute = match sku.compute_capability.as_str() {
            "" => "—".into(),
            cc => format!("sm_{}", cc.replace('.', "")),
        };
        table = table.row([
            sku.name.clone(),
            sku.accelerator_model.clone(),
            counts.join(", "),
            compute,
            if sku.vram_gb > 0 {
                format!("{} GB", sku.vram_gb)
            } else {
                "—".into()
            },
            usd(width.price_usd_micros_per_hour),
        ]);
    }
    table.print();
    match skus.is_empty() {
        false => {
            output::say(ctx, "Prices are for the fewest GPUs, an hour.");
            output::next(ctx, &["cozy machine rent <name> [--gpus N]"]);
        }
        true => output::say(ctx, "No machine is for rent on this Hub right now."),
    }
    Ok(ExitCode::SUCCESS)
}
