//! One Hub as one credential: a method per route cozy calls, each named for its operation in
//! `api/openapi.yaml` and given its route as written there.
use std::sync::Arc;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use reqwest::header::{ACCEPT, ETAG, HeaderName, HeaderValue, IF_NONE_MATCH};
use reqwest::{Method, Request, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;

use platform::paths::Home;
use protocol::{config::Origin, error::Error as Refusal};

use crate::Error;
use crate::device::UserSession;
use crate::types::*;

/// Who a request speaks as.
#[derive(Clone)]
pub enum Credential {
    Anonymous,
    /// A signed-in user: a Bearer from device-key login, minted again when stale or refused.
    User(Arc<UserSession>),
    /// A fixed Bearer: an enrollment's email-proven token, for the calls only it may make.
    Bearer(String),
    /// A rental pod's worker capability, on the worker routes.
    Worker {
        id: String,
        token: String,
    },
}

#[derive(Clone)]
pub struct Client {
    origin: String,
    http: reqwest::Client,
    credential: Credential,
}

/// What a path parameter escapes: the `url` crate's path-segment set, so a value stays one segment.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'%');

impl Client {
    /// A client of the Hub at `origin` speaking as `credential`, over `http` (which carries the
    /// TLS roots and timeouts the caller chose).
    pub fn connect(origin: &str, http: reqwest::Client, credential: Credential) -> Self {
        let origin = origin.trim_end_matches('/').to_owned();
        Self {
            origin,
            http,
            credential,
        }
    }

    /// `hub` as this computer's signed-in user, over `http`; refused when it holds no device key
    /// there, `login` (asked only then) signing it in.
    pub fn signed_in(
        hub: &Origin,
        home: &Home,
        http: reqwest::Client,
        login: impl FnOnce() -> String,
    ) -> Result<Self, Refusal> {
        let session = UserSession::open(hub, home, login)?;
        Ok(Self::connect(
            hub,
            http,
            Credential::User(Arc::new(session)),
        ))
    }

    /// The same Hub, anonymously: for the exchanges that mint a credential.
    pub fn anonymous(&self) -> Self {
        Self::connect(&self.origin, self.http.clone(), Credential::Anonymous)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn credential(&self) -> &Credential {
        &self.credential
    }

    pub async fn get_protected_resource_metadata(&self) -> Result<ProtectedResource, Error> {
        self.json(self.at(Method::GET, "/.well-known/oauth-protected-resource", &[]))
            .await
    }

    pub async fn get_current_account(&self) -> Result<Account, Error> {
        self.json(self.at(Method::GET, "/v1/accounts/current", &[]))
            .await
    }

    pub async fn register_account(&self, account: &str) -> Result<Account, Error> {
        let request = self.at(Method::PUT, "/v1/accounts/{account}", &[account]);
        self.json(request.json(&json!({}))).await
    }

    /// Revokes every device key of the account but the caller's: an email-proven Bearer's call.
    pub async fn revoke_other_device_keys(&self) -> Result<(), Error> {
        self.send(self.at(Method::DELETE, "/v1/auth/v1/device-keys", &[]))
            .await
            .map(drop)
    }

    pub(crate) async fn begin_device_key_enrollment(
        &self,
        email: &str,
        public_key: &str,
        label: &str,
    ) -> Result<DeviceKeyEnrollment, Error> {
        let body = json!({"email": email, "public_key": public_key, "label": label});
        let request = self.at(Method::POST, "/v1/auth/v1/device-keys/enroll/begin", &[]);
        self.json(request.json(&body)).await
    }

    pub(crate) async fn finish_device_key_enrollment(
        &self,
        enrollment_id: &str,
        code: &str,
        signature: &str,
    ) -> Result<AuthResult, Error> {
        let body = json!({"enrollment_id": enrollment_id, "code": code, "signature": signature});
        let request = self.at(Method::POST, "/v1/auth/v1/device-keys/enroll/finish", &[]);
        self.json(request.json(&body)).await
    }

    pub(crate) async fn begin_device_key_login(
        &self,
        device_key_id: &str,
    ) -> Result<DeviceKeyChallenge, Error> {
        let request = self.at(Method::POST, "/v1/auth/v1/device-keys/login/begin", &[]);
        self.json(request.json(&json!({"device_key_id": device_key_id})))
            .await
    }

    pub(crate) async fn finish_device_key_login(
        &self,
        challenge_id: &str,
        signature: &str,
    ) -> Result<AuthResult, Error> {
        let request = self.at(Method::POST, "/v1/auth/v1/device-keys/login/finish", &[]);
        self.json(request.json(&json!({"challenge_id": challenge_id, "signature": signature})))
            .await
    }

    pub async fn revoke_sign_in_key(&self, id: &str) -> Result<(), Error> {
        self.send(self.at(Method::DELETE, "/v1/auth/v1/me/sign-in-keys/{id}", &[id]))
            .await
            .map(drop)
    }

    pub async fn create_capability(&self, ask: &CapabilityRequest) -> Result<Capability, Error> {
        self.json(self.at(Method::POST, "/v1/capabilities", &[]).json(ask))
            .await
    }

    pub async fn get_credit(&self) -> Result<CreditResponse, Error> {
        self.json(self.at(Method::GET, "/v1/credit", &[])).await
    }

    pub async fn get_execution_environment(&self) -> Result<ExecutionEnvironment, Error> {
        self.json(self.at(Method::GET, "/v1/execution-environment", &[]))
            .await
    }

    pub async fn search_models(
        &self,
        q: Option<&str>,
        family: Option<&str>,
        limit: u32,
    ) -> Result<ModelSearch, Error> {
        let request = self.at(Method::GET, "/v1/models", &[]);
        self.json(
            request
                .query(&[("q", q), ("family", family)])
                .query(&[("limit", limit)]),
        )
        .await
    }

    pub async fn get_model(&self, org: &str, name: &str) -> Result<ModelCard, Error> {
        self.json(self.at(Method::GET, "/v1/models/{org}/{name}", &[org, name]))
            .await
    }

    /// Sets a model's discovery family; empty clears it.
    pub async fn set_model_family(
        &self,
        org: &str,
        name: &str,
        family: &str,
    ) -> Result<ModelAnswer, Error> {
        let request = self.at(Method::PATCH, "/v1/models/{org}/{name}", &[org, name]);
        self.json(request.json(&json!({"family": family}))).await
    }

    pub async fn delete_model(&self, org: &str, name: &str) -> Result<(), Error> {
        self.send(self.at(Method::DELETE, "/v1/models/{org}/{name}", &[org, name]))
            .await
            .map(drop)
    }

    pub async fn delete_model_commit(
        &self,
        org: &str,
        name: &str,
        commit: &str,
    ) -> Result<(), Error> {
        let request = self.at(
            Method::DELETE,
            "/v1/models/{org}/{name}/commits/{commit}",
            &[org, name, commit],
        );
        self.send(request).await.map(drop)
    }

    /// Releases `commits` (one per lane) as `release`, public for good.
    pub async fn publish_model_release(
        &self,
        org: &str,
        name: &str,
        release: &str,
        commits: &[String],
    ) -> Result<ModelRelease, Error> {
        let request = self.at(
            Method::PUT,
            "/v1/models/{org}/{name}/releases/{release}",
            &[org, name, release],
        );
        self.json(request.json(&json!({"commits": commits}))).await
    }

    pub async fn search_packages(
        &self,
        q: Option<&str>,
        limit: u32,
    ) -> Result<PackageSearch, Error> {
        let request = self.at(Method::GET, "/v1/packages", &[]);
        self.json(request.query(&[("q", q)]).query(&[("limit", limit)]))
            .await
    }

    pub async fn get_package(&self, org: &str, name: &str) -> Result<PackageCard, Error> {
        self.json(self.at(Method::GET, "/v1/packages/{org}/{name}", &[org, name]))
            .await
    }

    /// `get_package` unless the card of `etag` is still current: None for unchanged (304), else
    /// the card and its ETag.
    pub async fn get_package_if_changed(
        &self,
        org: &str,
        name: &str,
        etag: Option<&str>,
    ) -> Result<Option<(PackageCard, Option<String>)>, Error> {
        let mut request = self.at(Method::GET, "/v1/packages/{org}/{name}", &[org, name]);
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let response = self.send(request).await?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(Some((read(response).await?, etag)))
    }

    pub async fn delete_package(&self, org: &str, name: &str) -> Result<PackageDeletion, Error> {
        self.json(self.at(Method::DELETE, "/v1/packages/{org}/{name}", &[org, name]))
            .await
    }

    /// The exact files of `release` (`python_version` picks its project wheel), behind presigned
    /// URLs.
    pub async fn get_package_download_plan(
        &self,
        org: &str,
        name: &str,
        release: &str,
        python_version: Option<&str>,
    ) -> Result<PackageInstallPlan, Error> {
        let request = self.at(
            Method::POST,
            "/v1/packages/{org}/{name}/download",
            &[org, name],
        );
        let query = [
            ("release", Some(release)),
            ("python_version", python_version),
        ];
        self.json(request.query(&query).json(&json!({}))).await
    }

    /// Declares a release's files; an upload grant for each the Hub lacks.
    pub async fn begin_package_release(
        &self,
        org: &str,
        name: &str,
        release: &str,
        files: &[PackageDeclaredFile],
    ) -> Result<PackageReleaseGrants, Error> {
        let request = self.at(
            Method::POST,
            "/v1/packages/{org}/{name}/publish/{release}",
            &[org, name, release],
        );
        self.json(request.json(&json!({"files": files}))).await
    }

    pub async fn finalize_package_release(
        &self,
        org: &str,
        name: &str,
        release: &str,
        publication_id: &str,
        python_version: &str,
    ) -> Result<PackageFinalization, Error> {
        let body = json!({"publication_id": publication_id, "python_version": python_version});
        let request = self.at(
            Method::POST,
            "/v1/packages/{org}/{name}/publish/{release}/finalize",
            &[org, name, release],
        );
        self.json(request.json(&body)).await
    }

    pub async fn get_package_finalization(
        &self,
        org: &str,
        name: &str,
        release: &str,
    ) -> Result<PackageFinalization, Error> {
        let request = self.at(
            Method::GET,
            "/v1/packages/{org}/{name}/publish/{release}/status",
            &[org, name, release],
        );
        self.json(request).await
    }

    pub async fn get_package_release(
        &self,
        org: &str,
        name: &str,
        release: &str,
    ) -> Result<PackageRelease, Error> {
        let request = self.at(
            Method::GET,
            "/v1/packages/{org}/{name}/releases/{release}",
            &[org, name, release],
        );
        self.json(request).await
    }

    pub async fn yank_package_release(
        &self,
        org: &str,
        name: &str,
        release: &str,
    ) -> Result<PackageYank, Error> {
        let request = self.at(
            Method::DELETE,
            "/v1/packages/{org}/{name}/releases/{release}",
            &[org, name, release],
        );
        self.json(request.json(&json!({}))).await
    }

    pub async fn unyank_package_release(
        &self,
        org: &str,
        name: &str,
        release: &str,
    ) -> Result<PackageYank, Error> {
        let request = self.at(
            Method::POST,
            "/v1/packages/{org}/{name}/releases/{release}/unyank",
            &[org, name, release],
        );
        self.json(request.json(&json!({}))).await
    }

    /// The machines that hold one run of a package function, the pick first. Nothing is bought.
    pub async fn place_rental(
        &self,
        ask: &RentalPlacementRequest,
    ) -> Result<RentalPlacement, Error> {
        self.json(
            self.at(Method::POST, "/v1/rental-placements", &[])
                .json(ask),
        )
        .await
    }

    /// Prices `request` at the offer a rental of it would buy.
    pub async fn quote_rental(&self, request: &RentalRequest) -> Result<RentalQuote, Error> {
        self.json(
            self.at(Method::POST, "/v1/rental-quotes", &[])
                .json(request),
        )
        .await
    }

    /// The products buyable now, with their GPU counts and prices.
    pub async fn list_rental_skus(&self) -> Result<Vec<RentalSku>, Error> {
        self.json(self.at(Method::GET, "/v1/rental-skus", &[]))
            .await
    }

    pub async fn get_rental_sku_status(
        &self,
        name: &str,
        accelerator_count: u32,
    ) -> Result<RentalSkuStatus, Error> {
        let request = self.at(Method::GET, "/v1/rental-skus/{name}", &[name]);
        self.json(request.query(&[("accelerator_count", accelerator_count)]))
            .await
    }

    /// Rents `request`; the same `key` again answers the same rental.
    pub async fn create_rental(&self, key: &str, request: &RentalRequest) -> Result<Rental, Error> {
        let request = self
            .at(Method::POST, "/v1/rentals", &[])
            .header("idempotency-key", key)
            .json(request);
        self.json(request).await
    }

    /// The caller's rentals in `state` (`live`, or `all` with the ended ones).
    pub async fn list_rentals(&self, state: &str) -> Result<RentalListing, Error> {
        self.json(
            self.at(Method::GET, "/v1/rentals", &[])
                .query(&[("state", state)]),
        )
        .await
    }

    pub async fn get_rental(&self, id: &str) -> Result<Rental, Error> {
        self.json(self.at(Method::GET, "/v1/rentals/{id}", &[id]))
            .await
    }

    pub async fn release_rental(&self, id: &str) -> Result<(), Error> {
        self.send(self.at(Method::DELETE, "/v1/rentals/{id}", &[id]))
            .await
            .map(drop)
    }

    pub async fn get_software(&self) -> Result<Software, Error> {
        self.json(self.at(Method::GET, "/v1/software", &[])).await
    }

    pub async fn get_worker_authorized_keys(&self) -> Result<WorkerAuthorizedKeys, Error> {
        self.json(self.at(Method::GET, "/v1/worker/rental/authorized-keys", &[]))
            .await
    }

    pub async fn report_worker_endpoint(&self, report: &WorkerEndpointReport) -> Result<(), Error> {
        let request = self
            .at(Method::POST, "/v1/worker/rental/endpoint", &[])
            .json(report);
        self.send(request).await.map(drop)
    }

    pub async fn release_worker_rental(&self, worker_id: &str) -> Result<(), Error> {
        let request = self.at(Method::POST, "/v1/worker/rental/release", &[]);
        self.send(request.json(&json!({"worker_id": worker_id})))
            .await
            .map(drop)
    }

    /// `method` at `route`, its `{parameters}` filled from `args` in order.
    fn at(&self, method: Method, route: &str, args: &[&str]) -> RequestBuilder {
        let mut args = args
            .iter()
            .map(|arg| utf8_percent_encode(arg, SEGMENT).to_string());
        let path: Vec<String> = route
            .split('/')
            .map(|part| match part.starts_with('{') {
                true => args.next().expect("an argument per parameter"),
                false => part.to_owned(),
            })
            .collect();
        let url = format!("{}{}", self.origin, path.join("/"));
        self.http
            .request(method, url)
            .header(ACCEPT, "application/json")
    }

    async fn json<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, Error> {
        read(self.send(request).await?).await
    }

    /// Sends `request` as the credential. A refused Bearer logs in again, once; the first answer
    /// stands when the second Bearer cannot be had. A 4xx or 5xx is the Hub's refusal.
    async fn send(&self, request: RequestBuilder) -> Result<Response, Error> {
        let request = request.build().map_err(|e| Error::Call(e.to_string()))?;
        let again = request.try_clone();
        let (request, bearer) = self.present(request).await?;
        let mut response = self.execute(request).await?;
        if let (Credential::User(session), Some(again), Some(bearer)) =
            (&self.credential, again, bearer)
            && response.status() == StatusCode::UNAUTHORIZED
        {
            session.refused(&bearer).await;
            match self.present(again).await {
                Ok((again, _)) => response = self.execute(again).await?,
                Err(error) => tracing::warn!(%error, "presenting the credential again"),
            }
        }
        match response.status().is_client_error() || response.status().is_server_error() {
            true => Err(Error::refused(response).await),
            false => Ok(response),
        }
    }

    async fn execute(&self, request: Request) -> Result<Response, Error> {
        self.http
            .execute(request)
            .await
            .map_err(|e| Error::unanswered(&e))
    }

    /// `request` with the credential's headers, and the user's Bearer among them.
    async fn present(&self, mut request: Request) -> Result<(Request, Option<String>), Error> {
        let (headers, bearer) = match &self.credential {
            Credential::Anonymous => (vec![], None),
            Credential::User(session) => {
                // Login is itself a call through this client: boxed, as the future recurses.
                let token = Box::pin(session.bearer(&self.anonymous())).await?.token;
                (
                    vec![("authorization", format!("Bearer {token}"))],
                    Some(token),
                )
            }
            Credential::Bearer(token) => (vec![("authorization", format!("Bearer {token}"))], None),
            Credential::Worker { id, token } => (
                vec![
                    ("x-cozy-worker-id", id.clone()),
                    ("x-cozy-worker-token", token.clone()),
                ],
                None,
            ),
        };
        for (name, value) in headers {
            let value = HeaderValue::from_str(&value)
                .map_err(|_| Error::Credential(format!("an unusable {name} value")))?;
            request
                .headers_mut()
                .insert(HeaderName::from_static(name), value);
        }
        Ok((request, bearer))
    }
}

/// An answer's body as `T`; a shape it cannot read names its field (`lanes.bf16.bytes: …`).
async fn read<T: DeserializeOwned>(response: Response) -> Result<T, Error> {
    let body = response.bytes().await.map_err(|e| Error::unanswered(&e))?;
    let mut json = serde_json::Deserializer::from_slice(&body);
    let read = serde_path_to_error::deserialize(&mut json).map_err(|e| {
        match e.path().to_string().as_str() {
            "." => e.into_inner().to_string(),
            path => format!("{path}: {}", e.into_inner()),
        }
    });
    read.and_then(|answer| json.end().map(|()| answer).map_err(|e| e.to_string()))
        .map_err(Error::Answer)
}
