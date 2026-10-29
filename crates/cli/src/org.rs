//! `cozy org claim`: an author takes the org Tensorhub parked under their name (xs-131).
use std::process::ExitCode;

use protocol::error::Error;

use crate::Context;
use crate::context::hub_answer;
use crate::output::{self, Record};

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(clap::Subcommand, Debug)]
enum Verb {
    /// Claim an org parked under your name: the first run gives a code and where to post it on a
    /// handle the org lists; once posted, run it again and the org is yours.
    Claim(Claim),
}

#[derive(clap::Args, Debug)]
pub struct Claim {
    /// The org, e.g. black-forest-labs.
    org: String,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::Claim(args) => claim(ctx, args).await,
    }
}

async fn claim(ctx: &Context, args: Claim) -> Result<ExitCode, Error> {
    let answer = hub_answer(ctx.hub_signed_in(ctx.hub())?.claim_org(&args.org).await)?;
    if ctx.json {
        output::json(&serde_json::json!({
            "org": answer.org, "claimed": answer.claimed, "code": answer.code,
            "proofs": answer.proofs, "proven_by": answer.proven_by,
        }));
        return Ok(ExitCode::SUCCESS);
    }
    if answer.claimed {
        let mut record = Record::new().field("org", &answer.org).field("claimed", "yes: you are its owner");
        if let Some(by) = &answer.proven_by {
            record = record.field("proven by", by);
        }
        record.print(ctx);
        output::next(ctx, &[&format!("cozy model search {}", answer.org)]);
        return Ok(ExitCode::SUCCESS);
    }
    let mut record = Record::new().field("org", &answer.org).field("code", answer.code.as_deref().unwrap_or("-"));
    for proof in &answer.proofs {
        record = record.field(&proof.handle, &format!("{} ({})", proof.r#where, proof.checked));
    }
    record = record.note("Post the code in one of these places, then run this again.");
    record.print(ctx);
    output::next(ctx, &[&format!("cozy org claim {}", answer.org)]);
    Ok(ExitCode::SUCCESS)
}
