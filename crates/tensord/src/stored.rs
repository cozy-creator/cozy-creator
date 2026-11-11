//! What the journal stores as JSON, bytes in base64: a model choice, a run's products and a
//! finished run's log. Measured on these, JSON is 3–5× protobuf's bytes, a few hundred bytes a record.
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub(crate) fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("a stored value serializes")
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AttemptOutcome {
    pub worker_boot_id: String,
    pub request_id: String,
    pub attempt_ordinal: u64,
    #[serde(with = "b64")]
    pub invocation_spec_digest: Vec<u8>,
    pub outcome_id: String,
    #[serde(with = "b64")]
    pub outcome_digest: Vec<u8>,
    #[serde(with = "b64")]
    pub outcome_canonical_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DownloadAdapterRef {
    pub component: String,
    pub model: String,
    pub release: String,
    pub lane: String,
    pub manifest: String,
    pub source_component: String,
    pub scale: String,
    pub source: String,
    pub profiles: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LogEvent {
    pub sequence: u64,
    pub attempt_ordinal: u64,
    pub at_ms: u64,
    pub kind: String,
    #[serde(with = "b64")]
    pub body_canonical_bytes: Vec<u8>,
    pub product: Option<RunProduct>,
    pub outcome: Option<AttemptOutcome>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LogPage {
    pub events: Vec<LogEvent>,
    pub next_after: u64,
    pub head_sequence: u64,
    pub compacted_through: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ModelChoice {
    pub parameter: String,
    // The journal and supported machine frame use their historical spelling.
    #[serde(rename = "repository")]
    pub model: String,
    pub release: String,
    pub lane: String,
    pub manifest: Option<Ref>,
    pub source: String,
    pub profiles: Vec<String>,
    /// The base component a LoRA source was trained on, as its uploader says (`--trained-on`).
    pub trained_on: String,
    pub adapters: Vec<DownloadAdapterRef>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct NativeByteRetentionRequest {
    pub source: Option<NativeByteTreeRef>,
    pub retention_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct NativeByteTreeRef {
    pub producer_root_id: String,
    #[serde(with = "b64")]
    pub receipt_digest: Vec<u8>,
    pub manifest: Option<Ref>,
    pub content_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Ref {
    #[serde(with = "b64")]
    pub digest: Vec<u8>,
    pub length: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RunProduct {
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<api::machine::CheckpointRef>,
    pub op: i32,
    pub index: u32,
    pub content: Option<Ref>,
    pub media_type: String,
    pub label: String,
    pub source: Option<NativeByteRetentionRequest>,
    pub parts: Vec<RunProductPart>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub(crate) enum RunProductOp {
    Set = 1,
    Append = 2,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RunProductPart {
    pub content: Option<Ref>,
    pub source: Option<NativeByteRetentionRequest>,
    pub duration_us: u64,
}

/// Stored bytes as base64 text.
mod b64 {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serializer};
    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = std::borrow::Cow::<str>::deserialize(deserializer)?;
        STANDARD.decode(text.as_bytes()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod model_name_tests {
    use super::*;

    #[test]
    fn journal_model_identity_survives_the_source_rename() {
        let historic = br#"{"parameter":"weights","repository":"author/model","release":"1.0","lane":"bf16"}"#;
        let choice: ModelChoice = decode(historic).unwrap();
        assert_eq!(choice.model, "author/model");
        assert_eq!(choice.release, "1.0");
        let stored: serde_json::Value = serde_json::from_slice(&encode(&choice)).unwrap();
        assert_eq!(stored["repository"], "author/model");
        assert!(stored.get("model").is_none());
    }
}
