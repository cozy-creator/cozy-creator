//! `cozy org claim`: an author takes the org Tensorhub parked under their name; `cozy org edit`: a
//! platform admin sets an org's name, links and claim handles (xs-131). Through Calcifer
//! (CALCIFER-API.md "Orgs").
use std::process::ExitCode;

use protocol::error::Error;
use protocol::hub::{At, Org, OrgClaim, OrgEdit};

use crate::Context;
use crate::calcifer_client::path;
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
    /// Set an org's display name, links or claim handles (platform admins); each one given replaces
    /// what it has. An org that does not exist yet is parked under the handles given.
    Edit(Edit),
}

#[derive(clap::Args, Debug)]
pub struct Edit {
    /// The org, e.g. stabilityai.
    org: String,
    /// Its author's display name.
    #[arg(long)]
    name: Option<String>,
    /// A link to the author (their site, Hugging Face or GitHub page); repeat for several.
    #[arg(long = "link", value_name = "URL")]
    links: Vec<String>,
    /// A handle the author may claim the org by, site:handle (hf, github or civitai); repeat.
    #[arg(long = "handle", value_name = "SITE:HANDLE")]
    handles: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct Claim {
    /// The org, e.g. black-forest-labs.
    org: String,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::Claim(args) => claim(ctx, args).await,
        Verb::Edit(args) => edit(ctx, args).await,
    }
}

async fn edit(ctx: &Context, args: Edit) -> Result<ExitCode, Error> {
    let ask = OrgEdit {
        hub: Some(ctx.hub().to_string()),
        name: args.name,
        links: (!args.links.is_empty()).then_some(args.links),
        handles: (!args.handles.is_empty()).then_some(args.handles),
    };
    let org: Org = ctx.calcifer().await?.patch(&path(&["v1", "orgs", &args.org]), &ask).await?;
    if ctx.json {
        output::json(&org);
        return Ok(ExitCode::SUCCESS);
    }
    let handles: Vec<String> = org.handles.iter().map(|h| format!("{}:{}", h.site, h.handle)).collect();
    let mut record = Record::new().field("org", &org.name).field("name", &org.display_name)
        .field("handles", &handles.join(", ")).field("links", &org.links.join(", "));
    record = record.field("state", if org.parked { "parked" } else { "claimed" });
    if let Some(reserved) = &org.reserved {
        record = record.note(reserved.clone());
    }
    record.print(ctx);
    Ok(ExitCode::SUCCESS)
}

async fn claim(ctx: &Context, args: Claim) -> Result<ExitCode, Error> {
    let at = At { hub: Some(ctx.hub().to_string()) };
    let answer: OrgClaim = ctx.calcifer().await?.post(&path(&["v1", "orgs", &args.org, "claim"]), &at, None).await?;
    if ctx.json {
        output::json(&answer);
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
