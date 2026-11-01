//! `/v1/account`: this computer's device key at each Hub. A key is enrolled by a code the Hub
//! emails (`codes`), kept in `~/.cozy/keys`, and signs every later call silently. An exported key
//! is answered to the client and kept nowhere here.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path, Query, State,
        rejection::{BytesRejection, QueryRejection},
    },
};
use hub_client::{Bearer, Client, Credential, DeviceKey, Enrollment, UserSession};
use protocol::{
    config::Origin,
    error::{Error, Kind},
    hub::{Account, At, Code, CodeAnswer, CodeRequest, ExportedKey, KeyInstall, Naming, Proven, Purpose, SignedOut},
};

use super::{key, origin, reader, refusal, signed_in};
use crate::api::{Api, Refusal, parse, refuse};

/// The label a Hub shows for this computer's key, and for an exported one.
const LABEL: &str = "cozy";
const EXPORT_LABEL: &str = "cozy-export";
/// How long a code waits for what arrived; the Hub's own expire well before.
const WAITS: Duration = Duration::from_secs(3600);

/// The codes sent and not yet answered, by id.
#[derive(Default)]
pub(crate) struct Codes(Mutex<HashMap<String, Pending>>);

struct Pending {
    hub: Origin,
    purpose: Purpose,
    enrollment: Enrollment,
    sent: Instant,
}

impl Codes {
    fn keep(&self, pending: Pending) -> String {
        let id = format!("code-{}", hex::encode(crate::random()));
        let mut codes = self.0.lock().expect("codes");
        codes.retain(|_, waiting| waiting.sent.elapsed() < WAITS);
        codes.insert(id.clone(), pending);
        id
    }

    fn take(&self, id: &str) -> Option<Pending> {
        self.0.lock().expect("codes").remove(id)
    }
}

/// `GET /v1/account`: who this computer signs in as, its credit, and where to buy more.
pub(super) async fn show(
    State(api): State<Arc<Api>>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<Account>, Refusal> {
    let hub = origin(&api, query?.0.hub.as_deref())?;
    if !hub.device_key(api.machines.hubs.home()).exists() {
        return Ok(Json(Account::default()));
    }
    let user = signed_in(&api, &hub)?;
    let named = match user.get_current_account().await.map_err(|e| refusal(&hub, e).0) {
        Ok(account) => Some(account),
        Err(e) if e.code == "account.name_required" => None,
        // Revoked from another computer.
        Err(e) if e.kind == Kind::Credential => {
            return Ok(Json(Account { key_refused: true, ..Account::default() }));
        }
        Err(e) => return Err(Refusal(e)),
    };
    let mut answer = accepted(&user);
    if let Some(account) = named {
        let credit = user.get_credit().await.map_err(|e| refusal(&hub, e))?.credit;
        answer.billing = Some(if credit.is_some() { "metered" } else { "unmetered" }.into());
        answer.balance_usd_micros = credit.map(|c| c.available_usd_micros);
        answer.buy_credits = Some(format!("{hub}/account"));
        answer.account = Some(account.name);
        answer.orgs = account.orgs;
    }
    Ok(Json(answer))
}

/// An accepted key's account, before its name and credit are read.
fn accepted(user: &Client) -> Account {
    let key = key(user);
    Account {
        logged_in: true,
        email: Some(key.email.clone()),
        device_key: Some(key.id.clone()),
        ..Account::default()
    }
}

/// The account a key just enrolled or installed signs in as: unnamed until `PUT
/// /v1/account/name`. No credit is read.
async fn enrolled(api: &Api, hub: &Origin) -> Result<Account, Refusal> {
    let user = signed_in(api, hub)?;
    let mut answer = accepted(&user);
    match user.get_current_account().await.map_err(|e| refusal(hub, e).0) {
        Ok(account) => {
            answer.account = Some(account.name);
            answer.orgs = account.orgs;
        }
        Err(e) if e.code == "account.name_required" => {}
        Err(e) => return Err(Refusal(e)),
    }
    Ok(answer)
}

/// `POST /v1/account/codes`: the Hub emails a code proving the mailbox; its id finishes it.
pub(super) async fn send_code(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Code>, Refusal> {
    let ask: CodeRequest = parse(&body?)?;
    let hub = origin(&api, ask.hub.as_deref())?;
    let anonymous = reader(&api, &hub)?.anonymous();
    let email = ask.email.as_deref().map(str::trim).filter(|e| !e.is_empty());
    let enrollment = match (ask.purpose, email) {
        (Purpose::SignIn, Some(email)) => Enrollment::begin(&anonymous, email, LABEL, None).await,
        (Purpose::Export, Some(email)) => Enrollment::begin(&anonymous, email, EXPORT_LABEL, None).await,
        (Purpose::SignIn | Purpose::Export, None) => {
            return Err(refuse(Kind::Usage, "email_required", "a code is sent to the account's email: name it"));
        }
        // Proves the mailbox of the key this computer holds.
        (Purpose::SignOutEverywhere, _) => {
            let session = UserSession::open(&hub, api.machines.hubs.home(), || crate::rentals::login(&hub))
                .map_err(Refusal)?;
            Enrollment::begin(&anonymous, &session.key().email, LABEL, Some(session.key())).await
        }
    };
    let enrollment = enrollment.map_err(|e| refusal(&hub, e))?;
    let email = enrollment.email().to_owned();
    let id = api.codes.keep(Pending { hub, purpose: ask.purpose, enrollment, sent: Instant::now() });
    Ok(Json(Code { id, email }))
}

/// `POST /v1/account/codes/{id}`: what arrived, which does what the code was sent for. A code is
/// answered once: a wrong one is sent again.
pub(super) async fn prove(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Proven>, Refusal> {
    let CodeAnswer { code } = parse(&body?)?;
    let Pending { hub, purpose, enrollment, .. } = api.codes.take(&id).ok_or_else(|| {
        Refusal(Error::not_found("code_not_found", format!("no code {id} is waiting"))
            .remedy("ask for another: it is answered once"))
    })?;
    let anonymous = reader(&api, &hub)?.anonymous();
    let (key, bearer) = enrollment.finish(&anonymous, &code).await.map_err(|e| refusal(&hub, e))?;
    Ok(Json(match purpose {
        Purpose::SignIn => {
            keep(&api, &hub, &key, Some(&bearer))?;
            Proven::SignIn(enrolled(&api, &hub).await?)
        }
        Purpose::Export => Proven::Export(ExportedKey {
            email: key.email.clone(),
            device_key: key.id.clone(),
            key: String::from_utf8(key.document()).expect("a key is JSON"),
        }),
        Purpose::SignOutEverywhere => {
            let proven = Client::connect(&hub, reqwest::Client::new(), Credential::Bearer(bearer.token));
            proven.revoke_other_device_keys().await.map_err(|e| refusal(&hub, e))?;
            Proven::SignOutEverywhere(erase(&api, &hub, true).await?)
        }
    }))
}

/// `PUT /v1/account/name`: names the account a new key signs in as.
pub(super) async fn name(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Account>, Refusal> {
    let Naming { hub, name } = parse(&body?)?;
    let hub = origin(&api, hub.as_deref())?;
    let user = signed_in(&api, &hub)?;
    let account = user.register_account(name.trim()).await.map_err(|e| refusal(&hub, e))?;
    let mut answer = accepted(&user);
    answer.account = Some(account.name);
    answer.orgs = account.orgs;
    Ok(Json(answer))
}

/// `PUT /v1/account/key`: an exported key becomes this computer's at its Hub, proven by naming
/// its account.
pub(super) async fn install(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Account>, Refusal> {
    let KeyInstall { hub, key } = parse(&body?)?;
    let hub = origin(&api, hub.as_deref())?;
    let key = DeviceKey::read(key.as_bytes(), "the key").map_err(|e| refusal(&hub, e))?;
    if key.hub.trim_end_matches('/') != hub.as_str().trim_end_matches('/') {
        return Err(refuse(Kind::Usage, "key_other_hub", format!("the key is for {}, not {hub}", key.hub)));
    }
    keep(&api, &hub, &key, None)?;
    Ok(Json(enrolled(&api, &hub).await?))
}

/// `DELETE /v1/account`: this computer's key, revoked at the Hub and erased here.
pub(super) async fn sign_out(
    State(api): State<Arc<Api>>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<SignedOut>, Refusal> {
    let hub = origin(&api, query?.0.hub.as_deref())?;
    Ok(Json(erase(&api, &hub, false).await?))
}

/// Asks the Hub to revoke this computer's key, then erases it whatever the Hub answers.
async fn erase(api: &Api, hub: &Origin, everywhere: bool) -> Result<SignedOut, Refusal> {
    let home = api.machines.hubs.home();
    let mut out = SignedOut {
        email: None,
        everywhere: false,
        hub: hub.to_string(),
        notes: vec![],
        status: "not signed in".into(),
    };
    if !hub.device_key(home).exists() {
        return Ok(out);
    }
    let user = signed_in(api, hub)?;
    let key = key(&user);
    let revoked = user.revoke_sign_in_key(&key.id).await;
    for path in [hub.device_key(home), hub.bearer(home)] {
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(refuse(Kind::Internal, "key_unremovable", format!("{}: {e}", path.display())));
        }
    }
    if let Err(e) = revoked {
        let code = refusal(hub, e).0.code;
        out.notes.push(format!("{hub} did not confirm revoking this computer's key ({code}); it is erased here"));
    }
    out.email = Some(key.email.clone());
    out.everywhere = everywhere;
    out.status = "signed out".into();
    Ok(out)
}

/// Keeps `key` as this computer's at `hub`, with the Bearer its enrollment minted (none for an
/// installed key: its first call mints one).
fn keep(api: &Api, hub: &Origin, key: &DeviceKey, bearer: Option<&Bearer>) -> Result<(), Refusal> {
    let home = api.machines.hubs.home();
    let unwritable = |e: String| Refusal(Error::new(Kind::Internal, "key_unwritable", e));
    platform::durable::private_dir(&home.keys()).map_err(|e| unwritable(e.to_string()))?;
    key.save(&hub.device_key(home)).map_err(|e| refusal(hub, e))?;
    let kept = match bearer {
        Some(bearer) => {
            let bytes = serde_json::to_vec(bearer).expect("a bearer serializes");
            platform::durable::replace(&hub.bearer(home), &bytes, 0o600).map_err(|e| e.to_string())
        }
        None => match std::fs::remove_file(hub.bearer(home)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
    };
    kept.map_err(unwritable)
}
