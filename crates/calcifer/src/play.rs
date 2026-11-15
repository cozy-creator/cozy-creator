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
    ids::RunId,
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

/// A lifetime in seconds: a positive count of seconds, minutes, hours or days (`90s`, `30m`, `24h`, `7d`).
fn lifetime(text: &str) -> Result<u64, Refusal> {
    let invalid = || Refusal(Error::new(Kind::Usage, "play_expires_invalid", format!("{text:?} is not a duration like 30m, 24h or 7d")));
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
        count => Ok(count * scale),
    }
}

/// The player's link: everything it needs rides the fragment, which never leaves the browser.
fn link(address: &str, fingerprint: &str, token: &str, run: &RunId, output: &str) -> String {
    let fragment = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("v", "1")
        .append_pair("a", address)
        .append_pair("f", fingerprint)
        .append_pair("c", token)
        .append_pair("r", run.as_str())
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
        return Err(Refusal(Error::new(Kind::Usage, "play_output_invalid", "name one output, or name/index")));
    }
    let expires_s = lifetime(&ask.expires)?;
    let row = find_scoped(&api, path.project(), path.key()).await?;
    if row.project_root.is_some() { return Err(Refusal(Error::new(Kind::Conflict,"project_play_unavailable","project playback requires an owned private media transport; read this output through its project route"))); }
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
            format!("run {} has no output {:?}", run.name, ask.output),
        ));
    }
    let number = &run.name;
    let unavailable = |message| Refusal(Error::new(Kind::Unavailable, "machine_unavailable", message).next(format!("cozy run watch {number}")));
    let name = row
        .machine
        .clone()
        .ok_or_else(|| unavailable(format!("run {number} has no machine yet")))?;
    let machine = api.calcifer.maintenance_store().machine(&name)?;
    let (Some(machine), Some(frame)) = (machine, api.machines.frame(&name)) else {
        return Err(unavailable(format!("{name} has not reported its status")));
    };
    let Some(endpoint) = machine.endpoint else {
        return Err(unavailable(format!("{name} has no address yet")));
    };
    let webrtc = frame.webrtc.unwrap_or_default();
    let Some(address) = webrtc.addresses.first() else {
        let reason = match webrtc.unavailable_reason {
            reason if reason.is_empty() => format!("{name} does not listen for players"),
            reason => reason,
        };
        return Err(Refusal(Error::new(Kind::Unavailable, "webrtc_unavailable", reason).next(format!("cozy machine show {name}"))));
    };
    let signer = std::fs::read_to_string(&machine.key)
        .ok()
        .and_then(|pem| SigningKey::from_pkcs8_pem(&pem).ok())
        .ok_or_else(|| {
            let message = format!("cannot read the signing key {}", machine.key.display());
            refuse(Kind::Internal, "machine_key_unreadable", message)
        })?;
    let url_for = |token: &str| link(address, &webrtc.fingerprint, token, &row.id, &ask.output);
    let token = token(&signer, endpoint.worker_id, &row.id, ask.output.clone(), expires_s);
    Ok(Json(Play {
        url: url_for(&token),
        address: address.clone(),
        fingerprint: webrtc.fingerprint.clone(),
        token,
        expires_s,
    }))
}

/// A Cozy-Cap for one output of one run on one machine, for `expires_s` from now.
fn token(
    signer: &SigningKey,
    machine: String,
    run: &RunId,
    output: String,
    expires_s: u64,
) -> String {
    let grant = Grant {
        machine,
        run: run.to_string(),
        outputs: vec![output],
        expires: now_ms() / 1000 + expires_s as i64,
        ..Grant::default()
    };
    capability::mint(signer, grant)
}
