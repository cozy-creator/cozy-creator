//! The two listener observations the Hub's readiness reader requires, made against the real
//! listener over loopback: it presents exactly this machine's leaf, and it refuses a foreign
//! credential.
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use api::machine::{capability, path, StatusFrame, SERVER_NAME};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty, Limited};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::{
    client::TlsStream,
    rustls::{
        self,
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider},
        pki_types::{CertificateDer, ServerName, UnixTime},
        DigitallySignedStruct, SignatureScheme,
    },
    TlsConnector,
};

/// True when a TLS handshake on the listener presents exactly `leaf`.
pub async fn presents_leaf(port: u16, leaf: &[u8]) -> bool {
    connect(port, leaf).await.is_some()
}

/// True when the listener refuses a call whose Cozy-Cap is well formed, names this machine and
/// is unexpired, but is signed by a key made for this one probe.
pub async fn refuses_foreign_capability(port: u16, leaf: &[u8], worker_id: &str) -> bool {
    match super::identity::random::<32>() {
        Ok(seed) => {
            let foreign = ed25519_dalek::SigningKey::from_bytes(&seed);
            refuses_capability_of(port, leaf, worker_id, &foreign).await
        }
        Err(_) => false,
    }
}

/// True when `GET /v1/status` carrying a machine-scope Cozy-Cap signed by `key` is refused as a
/// credential while the same call with no capability answers: the refusal is the credential's,
/// not a closed door.
pub async fn refuses_capability_of(
    port: u16,
    leaf: &[u8],
    worker_id: &str,
    key: &ed25519_dalek::SigningKey,
) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let grant = capability::Grant {
        machine: worker_id.into(),
        action: capability::MACHINE.into(),
        expires: now + 300,
        ..Default::default()
    };
    let signed = format!("{}{}", capability::SCHEME, capability::mint(key, grant));
    let open = status(port, leaf, None).await;
    let refused = status(port, leaf, Some(&signed)).await;
    open.is_some_and(|(status, body)| status.is_success() && serde_json::from_slice::<StatusFrame>(&body).is_ok())
        && refused.is_some_and(|(status, _)| status == StatusCode::UNAUTHORIZED)
}

/// `GET /v1/status` on the listener, with `authorization` when given: its status and body.
async fn status(port: u16, leaf: &[u8], authorization: Option<&str>) -> Option<(StatusCode, Bytes)> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(connect(port, leaf).await?)).await.ok()?;
    tokio::spawn(connection);
    let mut request = Request::get(path::STATUS)
        .header("host", format!("127.0.0.1:{port}"))
        .header("accept", "application/json");
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    let response = sender.send_request(request.body(Empty::<Bytes>::new()).ok()?).await.ok()?;
    let status = response.status();
    let body = Limited::new(response.into_body(), 1 << 20).collect().await.ok()?.to_bytes();
    Some((status, body))
}

/// A TLS session with the listener, which it gets only by presenting exactly `leaf`.
async fn connect(port: u16, leaf: &[u8]) -> Option<TlsStream<TcpStream>> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .ok()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Leaf { leaf: leaf.to_vec(), provider }))
        .with_no_client_auth();
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    let name = ServerName::try_from(SERVER_NAME).ok()?;
    TlsConnector::from(Arc::new(config)).connect(name, tcp).await.ok()
}

/// Admits exactly `leaf`, whatever its name or dates say.
#[derive(Debug)]
struct Leaf {
    leaf: Vec<u8>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Leaf {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match end_entity.as_ref() == self.leaf {
            true => Ok(ServerCertVerified::assertion()),
            false => Err(rustls::Error::General("the listener presented another TLS leaf".into())),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
