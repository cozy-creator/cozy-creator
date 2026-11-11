//! `/v1/account`: this computer's device key at each Hub. A key is enrolled by a code the Hub
//! emails (`codes`), kept in `~/.cozy/keys`, and signs every later call silently. An exported key
//! is answered to the client and kept nowhere here: it is another computer's own key, which that
//! computer installs (`PUT /v1/account/key`). Signing out revokes a key this computer enrolled; an
//! installed one may sign in elsewhere too, so it is only forgotten here.
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
use ::api::hub::{Bearer, Client, Credential, DeviceKey, Enrollment, UserSession};
use api::{
    config::Origin,
    error::{Error, Kind},
    calcifer::hub::{Account, At, Code, CodeAnswer, CodeRequest, CodeStep, ExportedKey, KeyInstall, Naming, Proven, Purpose, SignedOut},
};

use super::{key, origin, reader, refusal, signed_in};
use crate::api::{Api, Refusal, parse, refuse};

/// The label a Hub shows for this computer's key, and for an exported one.
const LABEL: &str = "cozy";
const EXPORT_LABEL: &str = "cozy-export";
/// How long a code waits for what arrived: the Hub's own last this long.
const WAITS: Duration = Duration::from_secs(600);
/// How many of a mailbox's codes an answer is tried on. Each miss spends the Hub's budget for guesses.
const TRIED: usize = 3;

/// The codes sent and not yet answered, by id.
#[derive(Default)]
pub(crate) struct Codes(Mutex<HashMap<String, Arc<Pending>>>);

struct Pending {
    hub: Origin,
    purpose: Purpose,
    enrollment: Enrollment,
    sent: Instant,
    /// The code the Hub answered with a step of its own (a refusal carrying `metadata`): the
    /// enrollment is still live there, and finishes with this code once the step is taken.
    stepped: Mutex<Option<String>>,
}

impl Pending {
    fn to(&self, hub: &Origin, purpose: Purpose, email: &str) -> bool {
        self.hub == *hub && self.purpose == purpose && self.enrollment.email().eq_ignore_ascii_case(email)
    }
}

impl Codes {
    fn keep(&self, pending: Pending) -> String {
        let id = format!("code-{}", hex::encode(crate::random()));
        let mut codes = self.0.lock().expect("codes");
        codes.retain(|_, waiting| waiting.sent.elapsed() < WAITS);
        codes.insert(id.clone(), Arc::new(pending));
        id
    }

    /// What's live for this mailbox and reason, newest first.
    fn sent_to(&self, hub: &Origin, purpose: Purpose, email: &str) -> Vec<(String, Arc<Pending>)> {
        let codes = self.0.lock().expect("codes");
        let live = codes.iter().filter(|(_, waiting)| waiting.sent.elapsed() < WAITS && waiting.to(hub, purpose, email));
        let mut found: Vec<_> = live.map(|(id, waiting)| (id.clone(), waiting.clone())).collect();
        found.sort_by_key(|(_, waiting)| waiting.sent.elapsed());
        found
    }

    /// `id`'s code, then the newest others to its mailbox: each email has its own, and someone holding two may type either.
    fn answers(&self, id: &str) -> Option<Vec<Arc<Pending>>> {
        let named = self.0.lock().expect("codes").get(id).filter(|waiting| waiting.sent.elapsed() < WAITS)?.clone();
        let others = self.sent_to(&named.hub, named.purpose, named.enrollment.email());
        let others = others.into_iter().filter(|(other, _)| other != id).map(|(_, waiting)| waiting);
        Some(std::iter::once(named).chain(others).take(TRIED).collect())
    }

    /// `pending`'s code reached a step: `id`, the one the client proved, now names it.
    fn step(&self, id: &str, pending: &Arc<Pending>, code: &str) {
        *pending.stepped.lock().expect("stepped") = Some(code.to_owned());
        self.0.lock().expect("codes").insert(id.to_owned(), pending.clone());
    }

    /// `id`'s code waiting on a step, and that code.
    fn stepping(&self, id: &str) -> Option<(Arc<Pending>, String)> {
        let pending = self.0.lock().expect("codes").get(id).filter(|waiting| waiting.sent.elapsed() < WAITS)?.clone();
        let code = pending.stepped.lock().expect("stepped").clone()?;
        Some((pending, code))
    }

    /// One answered settles every code to that mailbox for that reason.
    fn answered(&self, pending: &Pending) {
        let email = pending.enrollment.email();
        self.0.lock().expect("codes").retain(|_, waiting| !waiting.to(&pending.hub, pending.purpose, email));
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
    // Proves the mailbox of the key this computer holds.
    let session = match ask.purpose {
        Purpose::SignOutEverywhere => {
            Some(UserSession::open(&hub, api.machines.hubs.home(), || crate::rentals::login(&hub)).map_err(Refusal)?)
        }
        Purpose::SignIn | Purpose::Export => None,
    };
    let email = match (&session, email) {
        (Some(session), _) => session.key().email.clone(),
        (None, Some(email)) => email.to_owned(),
        (None, None) => return Err(refuse(Kind::Usage, "email_required", "a code is sent to the account's email: name it")),
    };
    let label = if ask.purpose == Purpose::Export { EXPORT_LABEL } else { LABEL };
    let enrollment = match Enrollment::begin(&anonymous, &email, label, session.as_ref().map(UserSession::key)).await {
        Ok(enrollment) => enrollment,
        Err(e) => {
            // Asked again too soon: the code already out still works, so that's the answer.
            let out = matches!(e, ::api::hub::Error::Hub { status: 429, .. }).then(|| api.codes.sent_to(&hub, ask.purpose, &email));
            let Some((id, _)) = out.and_then(|mut codes| (!codes.is_empty()).then(|| codes.remove(0))) else {
                return Err(refusal(&hub, e));
            };
            return Ok(Json(Code { id, email, retry_after_s: e.stated_wait().map(|wait| wait.as_secs()) }));
        }
    };
    let id = api.codes.keep(Pending { hub, purpose: ask.purpose, enrollment, sent: Instant::now(), stepped: Mutex::default() });
    Ok(Json(Code { id, email, retry_after_s: None }))
}

/// The Hub's word that this isn't the code: 401, which isn't this computer's sign-in failing.
fn wrong_code(error: &::api::hub::Error) -> bool {
    matches!(error, ::api::hub::Error::Hub { status: 401, code, .. } if code == "invalid_code" || code == "code_expired")
}

/// `POST /v1/account/codes/{id}`: what arrived, which does what the code was sent for. Either of
/// two emails' codes works, and a wrong one leaves them waiting.
pub(super) async fn prove(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Proven>, Refusal> {
    let CodeAnswer { code } = parse(&body?)?;
    let waiting = api.codes.answers(&id).ok_or_else(|| {
        Refusal(Error::not_found("code_not_found", format!("no code {id} is waiting")).remedy("ask for another"))
    })?;
    let (hub, purpose) = (waiting[0].hub.clone(), waiting[0].purpose);
    let anonymous = reader(&api, &hub)?.anonymous();
    for pending in &waiting {
        match pending.enrollment.finish(&anonymous, &code).await {
            Ok((key, bearer)) => {
                api.codes.answered(pending);
                return Ok(Json(finished(&api, &hub, purpose, key, bearer).await?));
            }
            Err(e) if wrong_code(&e) => {}
            Err(e) => return Err(stepped(&api, &id, pending, &code, e)),
        }
    }
    Err(Refusal(
        Error::new(Kind::Validation, "code_wrong", "that code isn't right").remedy("use the code from your latest email, or ask for another"),
    ))
}

/// `POST /v1/account/codes/{id}/step`: the step a Hub named, refusing `id`'s code with its details, sent
/// under AuthKit's mount as asked; then that code again, which finishes as `prove` does.
pub(super) async fn step(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Proven>, Refusal> {
    let CodeStep { path, body } = parse(&body?)?;
    let plain = |segment: &str| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !path.split('/').all(plain) {
        return Err(refuse(Kind::Validation, "step_path_invalid", format!("{path:?} isn't a path under AuthKit's mount")));
    }
    let (pending, code) = api.codes.stepping(&id).ok_or_else(|| {
        Refusal(Error::not_found("code_not_found", format!("no code {id} waits on a step")).remedy("ask for another"))
    })?;
    let anonymous = reader(&api, &pending.hub)?.anonymous();
    anonymous.authkit_step(&path, &body).await.map_err(|e| refusal(&pending.hub, e))?;
    match pending.enrollment.finish(&anonymous, &code).await {
        Ok((key, bearer)) => {
            api.codes.answered(&pending);
            Ok(Json(finished(&api, &pending.hub, pending.purpose, key, bearer).await?))
        }
        Err(e) => Err(stepped(&api, &id, &pending, &code, e)),
    }
}

/// A refusal of a code that names a step of the Hub's own (one with `metadata`) leaves the
/// enrollment waiting on it, under `id`.
fn stepped(api: &Api, id: &str, pending: &Arc<Pending>, code: &str, error: ::api::hub::Error) -> Refusal {
    if matches!(&error, ::api::hub::Error::Hub { metadata: Some(_), .. }) {
        api.codes.step(id, pending, code);
    }
    refusal(&pending.hub, error)
}

/// What a finished code does: keeps this computer's key, answers an exported one, or signs out
/// every other key.
async fn finished(api: &Api, hub: &Origin, purpose: Purpose, key: DeviceKey, bearer: Bearer) -> Result<Proven, Refusal> {
    Ok(match purpose {
        Purpose::SignIn => {
            keep(api, hub, &key, Some(&bearer))?;
            Proven::SignIn(enrolled(api, hub).await?)
        }
        Purpose::Export => Proven::Export(ExportedKey {
            email: key.email.clone(),
            device_key: key.id.clone(),
            key: String::from_utf8(key.document()).expect("a key is JSON"),
        }),
        Purpose::SignOutEverywhere => {
            let proven = api.machines.hubs.anonymous(hub).map_err(Refusal)?.with(Credential::Bearer(bearer.token));
            proven.revoke_other_device_keys().await.map_err(|e| refusal(hub, e))?;
            Proven::SignOutEverywhere(erase(api, hub, true).await?)
        }
    })
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
/// its account. It is kept as installed: signing out here never revokes it.
pub(super) async fn install(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Account>, Refusal> {
    let KeyInstall { hub, key } = parse(&body?)?;
    let hub = origin(&api, hub.as_deref())?;
    let mut key = DeviceKey::read(key.as_bytes(), "the key").map_err(|e| refusal(&hub, e))?;
    key.installed = true;
    if key.hub.trim_end_matches('/') != hub.as_str().trim_end_matches('/') {
        return Err(refuse(Kind::Usage, "key_other_hub", format!("the key is for {}, not {hub}", key.hub)));
    }
    keep(&api, &hub, &key, None)?;
    Ok(Json(enrolled(&api, &hub).await?))
}

/// `DELETE /v1/account`: this computer's key, erased here, and revoked at the Hub when this
/// computer enrolled it.
pub(super) async fn sign_out(
    State(api): State<Arc<Api>>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<SignedOut>, Refusal> {
    let hub = origin(&api, query?.0.hub.as_deref())?;
    Ok(Json(erase(&api, &hub, false).await?))
}

/// Erases this computer's key, whatever the Hub answers. A key this computer enrolled is revoked at
/// the Hub first: no other computer holds it. One installed from a file (`cozy login --key`) may
/// sign in elsewhere (CI jobs share one exported key; a copied key is another computer's), so it
/// is revoked only when signing out everywhere.
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
    let revoked = match everywhere || !key.installed {
        true => Some(user.revoke_sign_in_key(&key.id).await),
        false => None,
    };
    for path in [hub.device_key(home), hub.bearer(home)] {
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(refuse(Kind::Internal, "key_unremovable", format!("{}: {e}", path.display())));
        }
    }
    match revoked {
        Some(Err(e)) => {
            let code = refusal(hub, e).0.code;
            out.notes.push(format!("{hub} did not confirm revoking this computer's key ({code}); it is erased here"));
        }
        None => out.notes.push(
            "this computer's key came from `cozy login --key`, so it is forgotten here but not revoked: \
             another computer may sign in with it. `cozy logout --everywhere` revokes every key"
                .into(),
        ),
        Some(Ok(())) => {}
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
