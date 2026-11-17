//! `GET /v1/runs/{id}/play`: how a browser plays one of a run's outputs straight from its
//! machine over WebRTC, with no relay: the address to dial, the certificate its DTLS presents,
//! and a Cozy-Cap for that output alone.
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
};
use api::{
    calcifer::Play,
    machine::capability::{self, Grant},
    error::{Error, Kind},
};
use ed25519_dalek::{SigningKey, pkcs8::DecodePrivateKey as _};
use serde::Deserialize;

use crate::{
    api::{Api, Refusal, RunPath, find_scoped},
    store::{State as RunState, now_ms},
};

/// The static player page (GitHub Pages' build of `web/player`).
const PLAYER: &str = "https://cozy-creator.github.io/cozy-creator/play/";

#[derive(Deserialize)]
pub(crate) struct Ask {
    /// `name`, or `name/i` for a list's item `i`.
    output: String,
    /// How long the link works, such as `30m`, `24h` or `7d`.
    #[serde(default = "an_hour")]
    expires: String,
}

fn an_hour() -> String {
    "1h".into()
}

/// The longest a link plays: what's asked for longer is this.
const LONGEST_S: u64 = 30 * 86_400;

/// A lifetime in seconds: a positive count of seconds, minutes, hours or days (`90s`, `30m`, `24h`, `7d`), at most 30 days.
fn lifetime(text: &str) -> Result<u64, Refusal> {
    let invalid = || Refusal(Error::new(Kind::Usage, "play_expires_invalid", format!("{text:?} isn't a duration like 30m, 24h or 7d.")));
    let unit = text.chars().last().ok_or_else(invalid)?;
    let count: u64 = text[..text.len() - unit.len_utf8()].parse().map_err(|_| invalid())?;
    let scale = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return Err(invalid()),
    };
    match count {
        0 => Err(invalid()),
        count => Ok(count.checked_mul(scale).map_or(LONGEST_S, |s| s.min(LONGEST_S))),
    }
}

/// The player's link: everything it needs rides the fragment, which never leaves the browser.
fn link(address: &str, fingerprint: &str, token: &str, run: &str, output: &str) -> String {
    let fragment = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("v", "1")
        .append_pair("a", address)
        .append_pair("f", fingerprint)
        .append_pair("c", token)
        .append_pair("r", run)
        .append_pair("o", output)
        .finish();
    format!("{PLAYER}#{fragment}")
}

pub(crate) async fn play(
    State(api): State<Arc<Api>>,
    Path(path): Path<RunPath>,
    query: Result<Query<Ask>, QueryRejection>,
) -> Result<Json<Play>, Refusal> {
    let Query(ask) = query?;
    if ask.output.is_empty() || ask.output.contains(['#', '&', '=', ' ', '\t', '\n']) {
        return Err(Refusal(Error::new(Kind::Usage, "play_output_invalid", "Name one result, as name or name/index.")));
    }
    let expires_s = lifetime(&ask.expires)?;
    let row = find_scoped(&api, path.project(), path.key()).await?;
    // A link plays from the machine to whoever holds it: never an encrypted project's output, which only its owner reads (cl-1066).
    if row.project_root.as_deref().is_some_and(crate::project::vault::encrypted) {
        return Err(Refusal(Error::new(Kind::Forbidden, "project_play_encrypted", "An encrypted project's results play only in the project, not by a link.")));
    }
    let refuse = |kind, code: &str, message: String| Refusal(Error::new(kind, code, message));
    // A run still going may yet make the output; an ended one never will.
    let (name, index) = match ask.output.rsplit_once('/') {
        Some((name, i)) if i.parse::<u32>().is_ok() => (name, i.parse::<u32>().ok()),
        _ => (ask.output.as_str(), None),
    };
    let run = api.calcifer.record(&row)?;
    let made = run.output.iter().any(|item| item.name == name && index.is_none_or(|i| item.index == i));
    if row.state == RunState::Settled && !made {
        return Err(refuse(
            Kind::NotFound,
            "output_not_found",
            format!("Run {} has no result {:?}.", run.name, ask.output),
        ));
    }
    let number = &run.name;
    let unavailable = |message| Refusal(Error::new(Kind::Unavailable, "machine_unavailable", message).next(format!("cozy run watch {number}")));
    let name = row
        .machine
        .clone()
        .ok_or_else(|| unavailable(format!("Run {number} isn't on a machine yet.")))?;
    let machine = api.calcifer.maintenance_store().machine(&name)?;
    let (Some(machine), Some(frame)) = (machine, api.machines.frame(&name)) else {
        return Err(unavailable(format!("{name} hasn't reported its status yet.")));
    };
    let Some(endpoint) = machine.endpoint else {
        return Err(unavailable(format!("{name} is still starting up.")));
    };
    let webrtc = frame.webrtc.unwrap_or_default();
    let Some(address) = webrtc.addresses.first() else {
        let reason = match webrtc.unavailable_reason {
            reason if reason.is_empty() => format!("{name} can't stream video to players."),
            reason => reason,
        };
        return Err(Refusal(Error::new(Kind::Unavailable, "webrtc_unavailable", reason).next(format!("cozy machine show {name}"))));
    };
    let signer = std::fs::read_to_string(&machine.key)
        .ok()
        .and_then(|pem| SigningKey::from_pkcs8_pem(&pem).ok())
        .ok_or_else(|| {
            let message = format!("Cozy can't read the signing key {}.", machine.key.display());
            refuse(Kind::Internal, "machine_key_unreadable", message)
        })?;
    // A project's run on this computer's machine keeps its outputs in the project's workspace there (cl-1066): the link names it, and
    // the machine reads there while this Calcifer has it open, as it does again here. A rental keeps them in its own store.
    let (scope, generation) = match api.calcifer.row_owner(&row).map_err(Refusal::from)? {
        Some(owner) if name == "local" => {
            let scope = crate::private_io::scope_for_project(owner.project_id(), owner.root()).map_err(Refusal)?;
            let held = (scope.storage.project_id.clone(), scope.storage.generation);
            let link = api.calcifer.link(&name).ok().flatten().ok_or_else(|| unavailable(format!("{name} isn't reachable.")))?;
            link.with_private(scope).probe_private().await
                .map_err(|_| unavailable(format!("{name} couldn't open this project's files to play them.")))?;
            held
        }
        _ => (String::new(), 0),
    };
    let expires = i64::try_from(expires_s).ok().and_then(|s| (now_ms() / 1000).checked_add(s))
        .ok_or_else(|| refuse(Kind::Internal, "play_expires_invalid", "This computer's clock is past what a link can say.".into()))?;
    // The run as its machine knows it: sent again, `<id>~<attempt>`.
    let sent = row.sent_id();
    let url_for = |token: &str| link(address, &webrtc.fingerprint, token, &sent, &ask.output);
    let token = capability::mint(&signer, Grant {
        machine: endpoint.worker_id,
        run: sent.clone(),
        outputs: vec![ask.output.clone()],
        scope,
        generation,
        expires,
        ..Grant::default()
    });
    Ok(Json(Play {
        url: url_for(&token),
        address: address.clone(),
        fingerprint: webrtc.fingerprint.clone(),
        token,
        expires_s,
    }))
}

