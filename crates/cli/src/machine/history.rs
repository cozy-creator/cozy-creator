//! `cozy machine ls --history`: every rental the Hub holds for this account, ended ones too and
//! rented from any computer, newest first, with what each cost, a total per UTC day and in all
//! (`GET /v1/rentals`). An operator also reads what the providers billed.
use std::process::ExitCode;

use jiff::{SignedDuration, Timestamp};
use protocol::{
    calcifer::{RentalHistory, RentalSum, clock},
    error::Error,
};

use super::usd;
use crate::{Context, output};

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
    let mut query = vec![("hub", ctx.hub().to_string())];
    if let Some(window) = since {
        let at = Timestamp::now().saturating_sub(window).unwrap_or_default();
        query.push(("since_ms", at.as_millisecond().to_string()));
    }
    let history: RentalHistory = ctx.calcifer().await?.get_query("/v1/rentals", &query).await?;
    if ctx.json {
        output::json(&history);
        return Ok(ExitCode::SUCCESS);
    }
    print(ctx, &history);
    Ok(ExitCode::SUCCESS)
}

fn print(ctx: &Context, h: &RentalHistory) {
    let operator = h.total.provider_cost_usd_micros.is_some();
    let mut header = vec!["RENTAL", "GPUS", "STARTED (UTC)", "RAN", "STATE", "COST"];
    if operator {
        header.push("PROVIDER COST");
    }
    let utc = |ms: i64| Timestamp::from_millisecond(ms).unwrap_or_default().strftime("%Y-%m-%d %H:%M").to_string();
    let mut table = output::Table::new(ctx, header);
    for row in &h.rentals {
        let mut cells = vec![
            row.name.clone(),
            format!("{}× {}", row.gpus, row.sku),
            utc(row.started_at_ms),
            match row.ended_at_ms {
                Some(_) => clock(row.ran_s * 1000),
                None => format!("{} so far", clock(row.ran_s * 1000)),
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
    let line = |sum: &RentalSum| {
        let spend = cost(sum.spend_usd_micros, &sum.spend_basis);
        match sum.provider_cost_usd_micros.zip(sum.provider_cost_basis.as_deref()) {
            Some((micros, basis)) => format!("{} rentals, {spend}; providers billed {}", sum.rentals, cost(micros, basis)),
            None => format!("{} rentals, {spend}", sum.rentals),
        }
    };
    if h.days.len() > 1 {
        for day in &h.days {
            println!("{}: {}", day.day.as_deref().unwrap_or_default(), line(day));
        }
    }
    let since = h.since_ms.map_or(String::new(), |ms| format!(" since {} UTC", utc(ms)));
    println!("Total{since}: {}", line(&h.total));
    let bases = [Some(h.total.spend_basis.as_str()), h.total.provider_cost_basis.as_deref()];
    if bases.iter().flatten().any(|b| !matches!(*b, "charged" | "provider_billed")) {
        println!("est.: a provider window not yet settled; unmetered: priced at the rental's rate, not charged");
    }
}
