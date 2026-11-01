//! `/v1/orgs`: an author claims the org the Hub parked under their name; a platform admin edits an
//! org's name, links and claim handles (xs-131).
use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
};
use protocol::hub::{At, Org, OrgClaim, OrgEdit, OrgHandle, OrgProof};

use super::{body, origin, refusal, signed_in};
use crate::api::{Api, Refusal, parse};

/// `POST /v1/orgs/{org}/claim`: the org is the caller's once a handle it lists shows the code;
/// until then, the code and where to post it.
pub(super) async fn claim(
    State(api): State<Arc<Api>>,
    Path(org): Path<String>,
    ask: Result<Bytes, BytesRejection>,
) -> Result<Json<OrgClaim>, Refusal> {
    let At { hub } = body(ask)?;
    let hub = origin(&api, hub.as_deref())?;
    let answer = signed_in(&api, &hub)?.claim_org(&org).await.map_err(|e| refusal(&hub, e))?;
    Ok(Json(OrgClaim {
        claimed: answer.claimed,
        code: answer.code,
        org: answer.org,
        proofs: (answer.proofs.into_iter())
            .map(|p| OrgProof { checked: p.checked, handle: p.handle, r#where: p.r#where })
            .collect(),
        proven_by: answer.proven_by,
    }))
}

/// `PATCH /v1/orgs/{org}` (platform admins): each field given replaces the org's own.
pub(super) async fn edit(
    State(api): State<Arc<Api>>,
    Path(org): Path<String>,
    ask: Result<Bytes, BytesRejection>,
) -> Result<Json<Org>, Refusal> {
    let OrgEdit { hub, name, links, handles } = parse(&ask?)?;
    let hub = origin(&api, hub.as_deref())?;
    let client = signed_in(&api, &hub)?;
    let page = (client.edit_org(&org, name.as_deref(), links.as_deref(), handles.as_deref()).await)
        .map_err(|e| refusal(&hub, e))?;
    Ok(Json(Org {
        name: page.name,
        display_name: page.display_name,
        links: page.links,
        handles: (page.handles.into_iter()).map(|h| OrgHandle { site: h.site, handle: h.handle }).collect(),
        parked: page.parked,
        reserved: page.reserved,
        models: page.models,
        mirrors: page.mirrors,
        admin_uploads: page.admin_uploads,
    }))
}
