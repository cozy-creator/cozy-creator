//! `~/.cozy/config.toml`, loaded once per process and shared by the CLI,
//! Calcifer and the machine (TensorD). Sources, each over the last: the file, the environment
//! (credentials and locations only: two model-source tokens, the Hub and where releases come
//! from, and a pod's machine contract). The home is always `<user home>/.cozy`. A key this
//! build does not read is named once and ignored, so a file another version wrote never disables
//! a command.
use std::{collections::BTreeMap, fmt, path::PathBuf, sync::OnceLock};

use platform::paths::Home;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::error::{Error, Kind};

pub const DEFAULT_HUB_URL: &str = "https://tensorhub.com";

/// Every environment variable the load reads, with its key: credentials for model sources, by the
/// names their own tools use; the Hub (dev and CI point one command elsewhere with it); where
/// releases are downloaded from (dist's installers read the same one); and the machine's settings
/// a pod's launcher gives (Tensorhub `podenv`, plus the devices the container may use).
pub const ENV: [(&str, &str); 19] = [
    ("HF_TOKEN", "huggingface_token"),
    ("CIVITAI_TOKEN", "civitai_token"),
    ("COZY_HUB_URL", "hub_url"),
    ("COZY_DOWNLOAD_URL", "download_url"),
    ("COZY_MACHINE_ROOT", "machine.root"),
    ("COZY_TENSORFS_ROOT", "machine.store"),
    ("COZY_TENSORFS_CACHE", "machine.cache"),
    ("COZY_LISTEN_HOST", "machine.listen_host"),
    ("COZY_WORKER_ID", "machine.worker_id"),
    ("COZY_WORKER_AUTH_TOKEN", "machine.worker_token"),
    ("COZY_WORKER_INTERNAL_PORT", "machine.port"),
    ("COZY_WEBRTC_INTERNAL_PORT", "machine.webrtc_port"),
    ("COZY_SSH_PUBLIC_KEY", "machine.ssh_public_key"),
    ("COZY_BOOTSTRAP_RECEIPT_HMAC_KEY_B64URL", "machine.receipt_key"),
    ("COZY_BOOTSTRAP_RECEIPT_HMAC_KEY_FILE", "machine.receipt_key_file"),
    ("TENSORHUB_ORIGIN", "machine.hub_origin"),
    ("TENSORHUB_CA_DER_B64URL", "machine.hub_ca_der"),
    ("TENSORHUB_OBJECT_STORAGE_HOSTS", "machine.object_storage_hosts"),
    ("CUDA_VISIBLE_DEVICES", "machine.visible_devices"),
];

/// The variables whose empty value is a value: `CUDA_VISIBLE_DEVICES=` is CUDA's "no GPUs". Every
/// other one empty is unset.
const EMPTY_IS_A_VALUE: [&str; 1] = ["CUDA_VISIBLE_DEVICES"];

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The Hub: [`DEFAULT_HUB_URL`] unless set.
    pub hub_url: Origin,
    pub calcifer: CalciferConfig,
    pub machine: MachineConfig,
    pub huggingface_token: Option<Secret>,
    pub civitai_token: Option<Secret>,
    /// The directory holding one release's files (dist-manifest.json, the archives); absent: GitHub
    /// Releases.
    pub download_url: Option<Url>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CalciferConfig {
    /// Calcifer's loopback port; 0 takes any free one (its lock record says which).
    pub port: u16,
}

/// `[machine]`: this computer's machine (TensorD, `cozy machine serve`), which judges these. A
/// pod's launcher gives the same settings through the environment ([`ENV`]).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MachineConfig {
    /// The machine API's port. On this computer, absent or 0: a free port, kept across restarts.
    #[serde(deserialize_with = "port")]
    pub port: Option<u16>,
    /// The WebRTC player's port, kept across restarts the same way.
    #[serde(deserialize_with = "port")]
    pub webrtc_port: Option<u16>,
    /// The TensorFS store it fills; absent: a pod's `var/lib/tensorfs` under its root, this
    /// computer's shared TensorFS cache (`TENSORFS_HOME`, else `~/.cache/tensorfs`).
    pub store: Option<PathBuf>,
    /// Tests only: a shorter idle window than the fixed 15 minutes after which a rental releases
    /// itself and this computer's machine exits (its executors at a third of it).
    pub idle_seconds: Option<u32>,
    /// The image layout's root: `/` on a pod, `~/.cozy/machine` on this computer ([`Self::root`]).
    pub root: Option<PathBuf>,
    /// The datacenter cache volume's mount.
    pub cache: Option<PathBuf>,
    /// `0.0.0.0` or `127.0.0.1`.
    pub listen_host: Option<String>,
    pub worker_id: Option<String>,
    pub worker_token: Option<Secret>,
    /// A pod's boot keys (`COZY_AUTHORIZED_KEYS=a,b`), from the environment only: this
    /// computer's controllers are `keys/owner.pem`'s and `keys/authorized`'s.
    #[serde(skip)]
    pub authorized_keys: Vec<String>,
    /// A development rental's SSH key.
    pub ssh_public_key: Option<String>,
    pub receipt_key: Option<Secret>,
    pub receipt_key_file: Option<PathBuf>,
    pub hub_origin: Option<String>,
    pub hub_ca_der: Option<String>,
    /// Exact hosts, or `.suffix` for subdomains (`a,b` in the environment).
    #[serde(deserialize_with = "list")]
    pub object_storage_hosts: Vec<String>,
    /// CUDA_VISIBLE_DEVICES: indices or UUIDs; empty is no GPU.
    pub visible_devices: Option<String>,
    /// The most memory each GPU offers, below its own (`8GiB` or bytes): a larger card stands in
    /// for a smaller one (`cozy machine update --gpu-memory-limit`).
    #[serde(deserialize_with = "size")]
    pub gpu_memory_limit: Option<u64>,
}

impl MachineConfig {
    /// This computer's machine root: `root`, else a worker image's `/` once a machine booted
    /// there (on a pod, a shell's `cozy` reaches the machine the image started, whatever its
    /// environment), else `~/.cozy/machine`. Its API's endpoint is
    /// `<root>/var/lib/cozy/tensord/api-ready.json`.
    pub fn root(&self, home: &Home) -> PathBuf {
        self.root.clone().unwrap_or_else(|| match image_machine() {
            true => PathBuf::from("/"),
            false => home.machine(),
        })
    }
}

/// Whether a worker image's machine (rooted at `/`, a pod's) booted on this computer: its boot id.
pub fn image_machine() -> bool {
    cfg!(target_os = "linux") && std::path::Path::new("/var/lib/cozy/machine/boot-id").is_file()
}

/// The environment name of a pod's boot keys.
pub const AUTHORIZED_KEYS: &str = "COZY_AUTHORIZED_KEYS";

/// A list as a TOML array, or as one comma-separated string (how the environment spells it).
fn list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum List {
        Spelled(String),
        Items(Vec<String>),
    }
    Ok(match List::deserialize(d)? {
        List::Spelled(text) => split(&text),
        List::Items(items) => items,
    })
}

/// A port as TOML writes it, or as the environment spells it.
fn port<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Port {
        Number(u16),
        Spelled(String),
    }
    match Option::<Port>::deserialize(d)? {
        None => Ok(None),
        Some(Port::Number(n)) => Ok(Some(n)),
        Some(Port::Spelled(text)) => {
            text.trim().parse().map(Some).map_err(|_| serde::de::Error::custom(format!("{text:?} is not a port")))
        }
    }
}

fn size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Size {
        Bytes(u64),
        Spelled(String),
    }
    match Option::<Size>::deserialize(d)? {
        None => Ok(None),
        Some(Size::Bytes(n)) => Ok(Some(n)),
        Some(Size::Spelled(text)) => parse_size(&text).map(Some).map_err(serde::de::Error::custom),
    }
}

/// A size as people write it: bytes, or a number of `KiB`, `MiB`, `GiB` or `TiB` (any case,
/// with or without a space): `8GiB`, `1.5 GiB`, `512MiB`.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let digits = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let (number, unit) = text.split_at(digits);
    let shift = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "kib" => 10,
        "mib" => 20,
        "gib" => 30,
        "tib" => 40,
        _ => return Err(format!("{text:?} is not a size: write bytes or KiB, MiB, GiB, TiB (8GiB)")),
    };
    let value: f64 = number.parse().map_err(|_| format!("{text:?} is not a size: no number"))?;
    Ok((value * (1u64 << shift) as f64) as u64)
}

fn split(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// A credential: never printed.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// A Tensorhub base URL in its one spelling: lowercase scheme and host, no default port, no
/// trailing slash. Every record and credential is keyed by it, so `https://tensorhub.com:443/`
/// and `https://tensorhub.com` are one Hub.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Origin(String);

impl Default for Origin {
    fn default() -> Self {
        Self(DEFAULT_HUB_URL.into())
    }
}

impl<'de> Deserialize<'de> for Origin {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(d)?).map_err(|e| serde::de::Error::custom(e.message))
    }
}

impl Origin {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.trim();
        let url = Url::parse(text).ok().filter(|u| {
            matches!(u.scheme(), "http" | "https")
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none()
        });
        match url {
            Some(url) => Ok(Self(url.as_str().trim_end_matches('/').to_owned())),
            None => Err(Error::new(
                Kind::Usage,
                "hub_invalid",
                format!(
                    "{text:?} is not an http or https URL without credentials, query or fragment"
                ),
            )),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// This computer's device key at this Hub (hub-client's `DeviceKey`), mode 0600.
    pub fn device_key(&self, home: &Home) -> PathBuf {
        home.keys().join(format!("device-{}.json", self.key_name()))
    }

    /// The short Bearer that device key last minted, kept for the next process.
    pub fn bearer(&self, home: &Home) -> PathBuf {
        home.keys().join(format!("device-{}.bearer.json", self.key_name()))
    }

    fn key_name(&self) -> String {
        hex::encode(Sha256::digest(self.0.as_bytes()))
    }
}

impl std::ops::Deref for Origin {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Origin> for String {
    fn from(origin: Origin) -> String {
        origin.0
    }
}

impl PartialEq<str> for Origin {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for Origin {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for Origin {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

/// One process's configuration and where it came from.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub config: Config,
    pub home: Home,
    /// The user's home directory, which services and `~` are relative to.
    pub user_home: PathBuf,
    /// What the load ignored, for the caller to show once.
    pub warnings: Vec<String>,
}

/// The one read of the environment and the config file.
pub fn load() -> Result<Loaded, Error> {
    let user_home = std::env::home_dir()
        .ok_or_else(|| Error::new(Kind::Internal, "no_home", "this user has no home directory"))?;
    load_from(environment(), user_home)
}

/// The load over a given environment and user home (what tests drive).
pub fn load_from(environment: &BTreeMap<String, String>, user_home: PathBuf) -> Result<Loaded, Error> {
    let home = Home::under(&user_home);
    let path = home.config();
    let file = path.display().to_string();
    let mut table = match std::fs::read_to_string(&path) {
        Ok(text) => parse_table(&text, &file)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => return Err(invalid(&file, "", &error.to_string())),
    };
    // As spelled: a value is a string until its setting's type reads it (`"0"` stays a device).
    let mut set = vec![];
    for (var, key) in ENV {
        let given = |v: &&String| !v.is_empty() || EMPTY_IS_A_VALUE.contains(&var);
        if let Some(value) = environment.get(var).filter(given) {
            set.push((var, key));
            let (tables, name) = key.rsplit_once('.').map_or((None, key), |(t, n)| (Some(t), n));
            let mut at = &mut table;
            for part in tables.into_iter().flat_map(|t| t.split('.')) {
                let entry = at.entry(part).or_insert_with(|| toml::Table::new().into());
                if !entry.is_table() {
                    *entry = toml::Table::new().into();
                }
                at = entry.as_table_mut().expect("a table");
            }
            at.insert(name.into(), value.clone().into());
        }
    }
    let (mut config, warnings) = extract(table, &file, &set)?;
    config.machine.authorized_keys = split(environment.get(AUTHORIZED_KEYS).map_or("", String::as_str));
    Ok(Loaded {
        config,
        home,
        user_home,
        warnings,
    })
}

impl Config {
    /// A config file's text as the load would read it: how an edit proves it still loads.
    pub fn parse(text: &str, name: &str) -> Result<(Config, Vec<String>), Error> {
        extract(parse_table(text, name)?, name, &[])
    }

    /// `cozy login`, spelled for `hub`: a Hub other than the configured one is named by the
    /// environment for that one command.
    pub fn login(&self, hub: &str) -> String {
        match self.hub_url == hub {
            true => "cozy login <email>".into(),
            false => format!("COZY_HUB_URL={hub} cozy login <email>"),
        }
    }

    /// Drops blank secrets.
    fn normalize(&mut self) {
        for secret in [
            &mut self.huggingface_token,
            &mut self.civitai_token,
            &mut self.machine.worker_token,
            &mut self.machine.receipt_key,
        ] {
            if secret.as_ref().is_some_and(|s| s.0.trim().is_empty()) {
                *secret = None;
            }
        }
    }
}

/// This process's environment, read once: the load's source, and what the processes it starts
/// inherit (each filters it further).
pub fn environment() -> &'static BTreeMap<String, String> {
    static ENVIRONMENT: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    ENVIRONMENT.get_or_init(|| std::env::vars().collect())
}

fn invalid(file: &str, key: &str, detail: &str) -> Error {
    let at = if key.is_empty() || key == "." { String::new() } else { format!(" `{key}`") };
    Error::new(Kind::Usage, "config_invalid", format!("{file}{at}: {detail}"))
        .remedy(format!("correct or remove it in {file}"))
}

fn parse_table(text: &str, file: &str) -> Result<toml::Table, Error> {
    text.parse().map_err(|e: toml::de::Error| invalid(file, "", e.message()))
}

/// The table as a Config, naming each key this build does not read. A value `env` set is refused
/// by its variable's name.
fn extract(table: toml::Table, file: &str, env: &[(&str, &str)]) -> Result<(Config, Vec<String>), Error> {
    let mut warnings = vec![];
    let mut ignored = |path: serde_ignored::Path<'_>| {
        warnings.push(format!("{file}: `{path}` is not a setting this cozy reads; ignored"))
    };
    let value = toml::Value::Table(table);
    let mut config: Config = serde_path_to_error::deserialize(serde_ignored::Deserializer::new(value, &mut ignored))
        .map_err(|e| {
            let key = e.path().to_string();
            match env.iter().find(|(_, k)| *k == key) {
                Some((var, _)) => Error::new(Kind::Usage, "config_invalid", format!("{var}: {}", e.inner().message()))
                    .remedy(format!("correct or unset {var}")),
                None => invalid(file, &key, &e.inner().to_string()),
            }
        })?;
    config.normalize();
    Ok((config, warnings))
}
