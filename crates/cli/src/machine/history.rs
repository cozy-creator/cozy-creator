//! `cozy machine ls --history`: every rental the Hub holds for this account, ended ones too and
//! rented from any computer, newest first, with what each cost, a total per UTC day and in all.
//! An operator also reads what the providers billed, the money that drains their balances.
use std::{collections::BTreeMap, process::ExitCode};

use jiff::{SignedDuration, Timestamp};
use protocol::{
    calcifer::clock,
    error::{Error, Kind},
};
use serde::Serialize;

use super::usd;
use crate::{Context, context::hub_answer, output};

#[derive(Serialize)]
struct History {
    /// Rentals started at or after this instant; none: every one.
    since: Option<Timestamp>,
    rentals: Vec<Row>,
    days: Vec<Sum>,
    total: Sum,
}

#[derive(Serialize)]
struct Row {
    rental_id: String,
    name: String,
    sku: String,
    gpus: i64,
    state: String,
    started_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<Timestamp>,
    /// From its order to its end, or to now while it lives.
    seconds: i64,
    /// What it cost its renter: `charged`, `unmetered` (priced, not charged) or `estimate`.
    spend_usd_micros: i64,
    spend_basis: String,
    /// What its provider billed Tensorhub (an operator's read): `provider_billed` or `estimate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_cost_usd_micros: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_cost_basis: Option<String>,
}

/// Rentals' costs summed, each on its weakest basis.
#[derive(Default, Serialize)]
struct Sum {
    /// A UTC day, `2026-10-30`; none for the whole total.
    #[serde(skip_serializing_if = "Option::is_none")]
    day: Option<String>,
    rentals: usize,
    spend_usd_micros: i64,
    spend_basis: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_cost_usd_micros: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_cost_basis: Option<String>,
}

impl Sum {
    fn add(&mut self, row: &Row) {
        self.rentals += 1;
        self.spend_usd_micros += row.spend_usd_micros;
        self.spend_basis = weakest(&self.spend_basis, &row.spend_basis, "charged");
        if let (Some(micros), Some(basis)) = (row.provider_cost_usd_micros, &row.provider_cost_basis) {
            *self.provider_cost_usd_micros.get_or_insert(0) += micros;
            let held = self.provider_cost_basis.take().unwrap_or_default();
            self.provider_cost_basis = Some(weakest(&held, basis, "provider_billed"));
        }
    }

    fn spend(&self) -> String {
        cost(self.spend_usd_micros, &self.spend_basis)
    }

    fn provider(&self) -> Option<String> {
        Some(cost(self.provider_cost_usd_micros?, self.provider_cost_basis.as_deref()?))
    }
}

/// The weaker of two bases: an estimate is weaker than anything known, unmetered than charged.
fn weakest(held: &str, next: &str, settled: &str) -> String {
    let rank = |basis: &str| match basis {
        "" => 0,
        b if b == settled => 1,
        "unmetered" => 2,
        _ => 3,
    };
    if rank(next) > rank(held) { next } else { held }.to_owned()
}

/// A cost as `cozy machine ls` shows spend: a settled one plain, else marked.
fn cost(micros: i64, basis: &str) -> String {
    match basis {
        "charged" | "provider_billed" => usd(micros),
        "unmetered" => format!("{} unmetered", usd(micros)),
        _ => format!("{} est.", usd(micros)),
    }
}

/// `48h`, `7d`, `2w` or `90m`.
pub(super) fn since(text: &str) -> Result<SignedDuration, String> {
    let refused = || format!("{text:?} is not a duration such as 90m, 48h, 7d or 2w");
    let split = text.find(|c: char| !c.is_ascii_digit()).ok_or_else(refused)?;
    let count: i64 = text[..split].parse().map_err(|_| refused())?;
    let unit = match &text[split..] {
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return Err(refused()),
    };
    Ok(SignedDuration::from_secs(count.saturating_mul(unit)))
}

pub async fn list(ctx: &Context, since: Option<SignedDuration>) -> Result<ExitCode, Error> {
    let hub = ctx.hub();
    let listing = hub_answer(ctx.hub_signed_in(hub)?.list_rentals("all").await)?;
    let now = Timestamp::now();
    let since = since.map(|d| now - d);
    let instant = |at: &str| -> Result<Timestamp, Error> {
        at.parse().map_err(|e| {
            Error::new(Kind::Unavailable, "hub.rental_time_invalid", format!("the Hub's rental time {at:?}: {e}"))
        })
    };
    let mut rentals = vec![];
    for r in listing.rentals {
        let started_at = instant(&r.created_at)?;
        if since.is_some_and(|since| started_at < since) {
            continue;
        }
        let ended_at = r.ended_at.as_deref().map(instant).transpose()?;
        rentals.push(Row {
            seconds: ended_at.unwrap_or(now).duration_since(started_at).as_secs(),
            sku: r.sku.unwrap_or(r.requested_accelerator_model),
            gpus: r.accelerator_count,
            rental_id: r.rental_id,
            name: r.name,
            state: r.state,
            started_at,
            ended_at,
            spend_usd_micros: r.spend_usd_micros,
            spend_basis: r.spend_basis,
            provider_cost_basis: r.provider_cost_basis.filter(|b| !b.is_empty()),
            provider_cost_usd_micros: r.provider_cost_usd_micros,
        });
    }
    rentals.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    let (mut days, mut total) = (BTreeMap::<String, Sum>::new(), Sum::default());
    for row in &rentals {
        let day = row.started_at.strftime("%Y-%m-%d").to_string();
        days.entry(day.clone()).or_insert_with(|| Sum { day: Some(day), ..Sum::default() }).add(row);
        total.add(row);
    }
    let history = History { since, days: days.into_values().rev().collect(), total, rentals };
    if ctx.json {
        output::json(&history);
        return Ok(ExitCode::SUCCESS);
    }
    print(ctx, &history);
    Ok(ExitCode::SUCCESS)
}

fn print(ctx: &Context, h: &History) {
    let operator = h.total.provider_cost_usd_micros.is_some();
    let mut header = vec!["RENTAL", "GPUS", "STARTED (UTC)", "RAN", "STATE", "COST"];
    if operator {
        header.push("PROVIDER COST");
    }
    let mut table = output::Table::new(ctx, header);
    for row in &h.rentals {
        let mut cells = vec![
            row.name.clone(),
            format!("{}× {}", row.gpus, row.sku),
            row.started_at.strftime("%Y-%m-%d %H:%M").to_string(),
            match row.ended_at {
                Some(_) => clock(row.seconds * 1000),
                None => format!("{} so far", clock(row.seconds * 1000)),
            },
            row.state.clone(),
            cost(row.spend_usd_micros, &row.spend_basis),
        ];
        if operator {
            let billed = row.provider_cost_usd_micros.zip(row.provider_cost_basis.as_deref());
            cells.push(billed.map_or("—".into(), |(micros, basis)| cost(micros, basis)));
        }
        table = table.row(cells);
    }
    table.print();
    let line = |sum: &Sum| match sum.provider() {
        Some(provider) => format!("{} rentals, {}; providers billed {provider}", sum.rentals, sum.spend()),
        None => format!("{} rentals, {}", sum.rentals, sum.spend()),
    };
    if h.days.len() > 1 {
        for day in &h.days {
            println!("{}: {}", day.day.as_deref().unwrap_or_default(), line(day));
        }
    }
    let since = h.since.map_or(String::new(), |at| format!(" since {}", at.strftime("%Y-%m-%d %H:%M UTC")));
    println!("Total{since}: {}", line(&h.total));
    let bases = [Some(h.total.spend_basis.as_str()), h.total.provider_cost_basis.as_deref()];
    if bases.iter().flatten().any(|b| !matches!(*b, "charged" | "provider_billed")) {
        println!("est.: a provider window not yet settled; unmetered: priced at the rental's rate, not charged");
    }
}
