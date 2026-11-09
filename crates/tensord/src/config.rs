//! TensorD's configuration: the binary's one load ([`api::config`]: the `[machine]` table
//! of `~/.cozy/config.toml`, then a pod's environment), the command line's lifetime, and the
//! grant judged from them. Nothing else in TensorD reads the environment; the processes it starts
//! inherit [`api::config::environment`], filtered.
use crate::machine::{
    endpoint::Endpoint,
    grant::{Grant, Lifetime},
};
use executor_api::DisplayPace;
use api::config::{self as shared, Loaded, MachineConfig, MachineMode, ENV};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

/// `cozy machine serve` (and the `tensord` binary's run modes): this computer's machine, which
/// Calcifer starts when something uses it, or with `--rental` a provider's pod.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::Args)]
pub struct Flags {
    /// A provider's pod: Hub lease, readiness receipt, idle release.
    #[arg(long)]
    pub rental: bool,
}

impl Flags {
    pub fn lifetime(&self) -> Lifetime {
        match self.rental {
            true => Lifetime::Rental,
            false => Lifetime::Local,
        }
    }
}

/// The readiness key's names, taken out of the environment once read.
const RECEIPT_KEYS: [&str; 2] = ["COZY_BOOTSTRAP_RECEIPT_HMAC_KEY_B64URL", "COZY_BOOTSTRAP_RECEIPT_HMAC_KEY_FILE"];
/// The provider's own names: where it maps this pod's ports, and its credential.
const PROVIDER: &[&str] = &[
    "COZY_WEBRTC_PUBLIC_ADDRESS",
    "COZY_PROVIDER_API_ORIGIN",
    "RUNPOD_PUBLIC_IP",
    "RUNPOD_POD_ID",
    "RUNPOD_API_KEY",
    "PUBLIC_IPADDR",
    "CONTAINER_ID",
    "CONTAINER_API_KEY",
];
const PROVIDER_PREFIXES: &[&str] = &["RUNPOD_TCP_PORT_", "VAST_TCP_PORT_"];

/// One machine's configuration.
pub(crate) struct Config {
    pub grant: Grant,
    /// CUDA_VISIBLE_DEVICES as the launcher set it: indices or UUIDs; empty is no GPU.
    pub visible_devices: Option<String>,
    /// The most memory each GPU offers (`machine.gpu_memory_limit`).
    pub gpu_memory_limit: Option<u64>,
    /// How it shares its computer: `machine.mode`, else a rental dedicated and anything else shared.
    pub mode: MachineMode,
    /// How a call shares a GPU that drives a display: `machine.display_pace` on a shared machine
    /// (absent: the default), none on a dedicated one or when it never pauses.
    pub display_pace: Option<DisplayPace>,
    /// The provider's public mapping of this pod's ports, told to the Hub at boot.
    pub endpoint: Option<Endpoint>,
    /// The provider's names, for [`crate::machine::provider::ProviderSelf::take`].
    pub provider: BTreeMap<String, String>,
    /// Where this machine's own log rolls: `<root>/var/lib/cozy/machine/logs`.
    pub logs: PathBuf,
    /// What the load ignored, said once the log is up.
    pub warnings: Vec<String>,
}

/// The one load.
pub(crate) fn load(lifetime: Lifetime) -> io::Result<Config> {
    let loaded = shared::load().map_err(|e| invalid(&named(&e.message)))?;
    from_loaded(lifetime, loaded, shared::environment())
}

/// `machine.gpu_memory_limit` and `machine.mode` as config.toml says them now. The service reads
/// them at each start: it is forked from a parent that loaded the config before an update wrote
/// the limit or a hand edit the mode.
pub(crate) fn reread(config: &mut Config) -> io::Result<()> {
    let loaded = shared::load().map_err(|e| invalid(&named(&e.message)))?;
    config.gpu_memory_limit = loaded.config.machine.gpu_memory_limit;
    config.mode = mode(config.grant.lifetime, &loaded.config.machine);
    config.display_pace = display_pace(config.mode, &loaded.config.machine);
    Ok(())
}

/// `machine.display_pace` (absent: the default) on a shared machine whose pace pauses at all.
fn display_pace(mode: MachineMode, machine: &MachineConfig) -> Option<DisplayPace> {
    let pace = machine.display_pace.unwrap_or_default();
    (mode == MachineMode::Shared && pace.pause_ms > 0).then_some(pace)
}

/// `machine.mode`, else a rental dedicated and anything else shared.
fn mode(lifetime: Lifetime, machine: &MachineConfig) -> MachineMode {
    machine.mode.unwrap_or(match lifetime {
        Lifetime::Rental => MachineMode::Dedicated,
        Lifetime::Local => MachineMode::Shared,
    })
}

/// Takes the readiness key out of the environment, so nothing this machine starts inherits it.
/// Call before any thread starts, after a provider's re-execution (which still needs it).
pub(crate) fn forget_receipt_key() {
    for name in RECEIPT_KEYS {
        std::env::remove_var(name);
    }
}

/// What a process this service starts inherits: never the worker credential. Called in the service
/// alone, before any thread: its stable parent keeps the credential for the machine it may exec again.
pub(crate) fn for_what_it_starts() {
    std::env::remove_var("COZY_WORKER_AUTH_TOKEN");
}

fn from_loaded(lifetime: Lifetime, loaded: Loaded, environment: &BTreeMap<String, String>) -> io::Result<Config> {
    let machine = &loaded.config.machine;
    let provider: BTreeMap<String, String> = environment
        .iter()
        .filter(|(n, _)| PROVIDER.contains(&n.as_str()) || PROVIDER_PREFIXES.iter().any(|p| n.starts_with(p)))
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();
    let reach = crate::machine::player::reach(machine.webrtc_port, &provider);
    let grant = Grant::new(lifetime, machine, reach, &loaded.home)?;
    let mut warnings = loaded.warnings;
    warnings.extend(
        environment
            .keys()
            .filter(|n| (n.starts_with("COZY_") || n.starts_with("TENSORHUB_")) && !known(n))
            .map(|n| format!("ignoring {n}, which this machine does not read")),
    );
    let rental = lifetime == Lifetime::Rental;
    Ok(Config {
        logs: grant.layout.state.join("logs"),
        visible_devices: machine.visible_devices.clone(),
        gpu_memory_limit: machine.gpu_memory_limit,
        mode: mode(lifetime, machine),
        display_pace: display_pace(mode(lifetime, machine), machine),
        endpoint: rental.then(|| crate::machine::endpoint::from_env(provider.clone())).flatten(),
        provider,
        grant,
        warnings,
    })
}

fn known(name: &str) -> bool {
    ENV.iter().any(|(n, _)| *n == name) || PROVIDER.contains(&name) || name == shared::AUTHORIZED_KEYS
}

/// A load refusal naming a `machine.*` key also names the variable a pod sets it with.
fn named(message: &str) -> String {
    let mut message = message.to_owned();
    for (name, key) in ENV {
        message = message.replace(&format!("`{key}`"), &format!("`{key}` ({name})"));
    }
    message
}

/// Logs to stderr, and to `<dir>/tensord.<date>.log` when a directory is given: rolled daily,
/// the newest seven kept.
pub(crate) fn init_logs(dir: Option<&Path>) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let builder = tracing_subscriber::fmt().with_ansi(false);
    let file = dir.and_then(|dir| {
        std::fs::create_dir_all(dir)
            .map_err(|e| eprintln!("tensord: no log directory {}: {e}", dir.display()))
            .ok()?;
        DailyLog::open(dir)
            .map_err(|e| eprintln!("tensord: no log file in {}: {e}", dir.display()))
            .ok()
    });
    let _ = match file {
        Some(file) => builder.with_writer(file.and(std::io::stderr)).try_init(),
        None => builder.with_writer(std::io::stderr).try_init(),
    };
}

/// `<dir>/tensord.<UTC date>.log`, appended to: a new file at each UTC midnight, and the oldest
/// removed so seven remain.
struct DailyLog {
    dir: PathBuf,
    today: std::sync::Mutex<(jiff::civil::Date, std::sync::Arc<std::fs::File>)>,
}

impl DailyLog {
    const KEPT: usize = 7;

    fn open(dir: &Path) -> io::Result<Self> {
        let day = Self::day();
        let file = Self::start(dir, day)?;
        Ok(Self { dir: dir.to_owned(), today: std::sync::Mutex::new((day, file.into())) })
    }

    fn day() -> jiff::civil::Date {
        jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date()
    }

    /// `day`'s file, after the oldest others go.
    fn start(dir: &Path, day: jiff::civil::Date) -> io::Result<std::fs::File> {
        let name = format!("tensord.{day}.log");
        let mut older: Vec<String> = std::fs::read_dir(dir)?
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|other| other.starts_with("tensord.") && other.ends_with(".log") && *other != name)
            .collect();
        older.sort();
        for old in &older[..older.len().saturating_sub(Self::KEPT - 1)] {
            if let Err(e) = std::fs::remove_file(dir.join(old)) {
                eprintln!("tensord: cannot remove the old log {old}: {e}");
            }
        }
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join(name))
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DailyLog {
    type Writer = DayFile;

    fn make_writer(&'a self) -> DayFile {
        let mut today = self.today.lock().unwrap_or_else(|e| e.into_inner());
        let day = Self::day();
        if day != today.0 {
            match Self::start(&self.dir, day) {
                Ok(file) => *today = (day, file.into()),
                Err(e) => eprintln!("tensord: no log file for {day} in {}: {e}", self.dir.display()),
            }
        }
        DayFile(today.1.clone())
    }
}

struct DayFile(std::sync::Arc<std::fs::File>);

impl io::Write for DayFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        (&*self.0).write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, detail.to_owned())
}
